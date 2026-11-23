//! GPU capture via DXGI Desktop Duplication:
//!  * HDR-correct still capture (scRGB FP16 → tone-mapped sRGB)
//!  * H.264 screen recording through Media Foundation (hardware encoder when available)
//!  * an optional replay buffer that keeps the last N seconds as short MP4 segments on disk
//! Nothing here runs unless a recording / replay buffer is active.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{ERROR_SUCCESS, HMODULE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{GUID, HSTRING, Interface, PCSTR, s};

// ---------------------------------------------------------------- outputs

pub struct Out {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
    pub rect: RECT,
    rotated: bool,
    pub hdr: bool,
    /// brightness of SDR white ("SDR content brightness" in Windows settings)
    pub sdr_nits: f32,
    /// brightest the panel shows (Windows HDR Calibration profile, else the monitor's report)
    pub max_nits: f32,
    /// where the two values came from, for `--hdr-info`
    pub source: String,
}

/// One active display path: its GDI name ("\\.\DISPLAY1"), SDR white level and ids.
struct PathInfo {
    name: Vec<u16>,
    white: Option<f32>,
    adapter: windows::Win32::Foundation::LUID,
    source_id: u32,
}

unsafe fn display_paths() -> Vec<PathInfo> {
    let (mut np, mut nm) = (0u32, 0u32);
    if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![std::mem::zeroed(); np as usize];
    let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![std::mem::zeroed(); nm as usize];
    if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    for p in &paths[..np as usize] {
        let mut src: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
        src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        src.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
        src.header.adapterId = p.sourceInfo.adapterId;
        src.header.id = p.sourceInfo.id;
        if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
            continue;
        }
        let mut wl: DISPLAYCONFIG_SDR_WHITE_LEVEL = std::mem::zeroed();
        wl.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
        wl.header.size = size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32;
        wl.header.adapterId = p.targetInfo.adapterId;
        wl.header.id = p.targetInfo.id;
        // value is in thousandths of 80 nits
        let white = (DisplayConfigGetDeviceInfo(&mut wl.header) == 0 && wl.SDRWhiteLevel > 0).then(|| wl.SDRWhiteLevel as f32 / 1000.0 * 80.0);
        out.push(PathInfo { name: src.viewGdiDeviceName.iter().copied().take_while(|c| *c != 0).collect(), white, adapter: p.sourceInfo.adapterId, source_id: p.sourceInfo.id });
    }
    out
}

/// Peak and minimum luminance from the display's HDR color profile (the MHC2 tag
/// the Windows HDR Calibration app writes). None when there is no such profile.
unsafe fn calibrated_peak(adapter: windows::Win32::Foundation::LUID, source_id: u32) -> Option<(f32, String)> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    // ColorProfileGetDisplayDefault is Windows 11+, so look it up at run time
    type GetDefault = unsafe extern "system" fn(u32, windows::Win32::Foundation::LUID, u32, u32, u32, *mut *mut u16) -> i32;
    let lib = LoadLibraryW(crate::util::wz("mscms.dll").as_ptr());
    if lib.is_null() {
        return None;
    }
    let f = GetProcAddress(lib, c"ColorProfileGetDisplayDefault".as_ptr() as *const u8)?;
    let get: GetDefault = std::mem::transmute(f);
    let mut name: *mut u16 = std::ptr::null_mut();
    // scope: current user (1), then system (0); type ICC (0), subtype extended display (8 = HDR)
    let ok = [1u32, 0].iter().any(|scope| get(*scope, adapter, source_id, 0, 8, &mut name) >= 0 && !name.is_null());
    if !ok {
        return None;
    }
    let file = crate::util::from_wide(name);
    windows_sys::Win32::Foundation::LocalFree(name as *mut _);
    let dir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let path = if file.contains('\\') { file.clone() } else { format!("{dir}\\System32\\spool\\drivers\\color\\{file}") };
    let icc = std::fs::read(&path).ok()?;
    let be32 = |o: usize| icc.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let count = be32(128)? as usize;
    for i in 0..count.min(256) {
        let e = 132 + i * 12;
        if icc.get(e..e + 4)? == b"MHC2" {
            let off = be32(e + 4)? as usize;
            // MHC2: sig, reserved, lut size, min luminance, peak luminance (s15Fixed16)
            let peak = be32(off + 16)? as i32 as f32 / 65536.0;
            return (peak > 1.0).then(|| (peak, file));
        }
    }
    None
}

pub fn outputs() -> Vec<Out> {
    let mut v = Vec::new();
    let mut paths: Option<Vec<PathInfo>> = None;
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return v };
        let mut ai = 0;
        while let Ok(a) = f.EnumAdapters1(ai) {
            ai += 1;
            let mut oi = 0;
            while let Ok(o) = a.EnumOutputs(oi) {
                oi += 1;
                if let Ok(d) = o.cast::<IDXGIOutput6>().and_then(|o6| o6.GetDesc1()) {
                    if !d.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    let hdr = d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
                    let (mut sdr, mut max, mut source) = (80.0, 80.0, String::from("SDR"));
                    if hdr {
                        let paths = paths.get_or_insert_with(|| display_paths());
                        let want: Vec<u16> = d.DeviceName.iter().copied().take_while(|c| *c != 0).collect();
                        // match by name; with a single display there's no doubt either way
                        let p = paths.iter().find(|p| p.name == want).or(if paths.len() == 1 { paths.first() } else { None });
                        let (white, from) = match p.and_then(|p| p.white) {
                            Some(w) => (w, "Windows SDR brightness setting"),
                            None => (200.0, "Windows default (setting not readable)"),
                        };
                        let calib = p.and_then(|p| calibrated_peak(p.adapter, p.source_id));
                        let (peak, pfrom) = match &calib {
                            Some((pk, file)) => (*pk, format!("HDR calibration profile {file}")),
                            None => (d.MaxLuminance, "monitor's reported peak".to_string()),
                        };
                        sdr = white.clamp(80.0, 1000.0);
                        max = peak.max(sdr * 1.25);
                        source = format!("SDR white {sdr:.0} nits ({from}), peak {max:.0} nits ({pfrom})");
                    }
                    v.push(Out {
                        adapter: a.clone(),
                        output: o,
                        rect: d.DesktopCoordinates,
                        rotated: d.Rotation != DXGI_MODE_ROTATION_IDENTITY && d.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED,
                        hdr,
                        sdr_nits: sdr,
                        max_nits: max,
                        source,
                    });
                } else if let Ok(d) = o.GetDesc() {
                    if !d.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    v.push(Out {
                        adapter: a.clone(),
                        output: o,
                        rect: d.DesktopCoordinates,
                        rotated: d.Rotation != DXGI_MODE_ROTATION_IDENTITY && d.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED,
                        hdr: false,
                        sdr_nits: 80.0,
                        max_nits: 80.0,
                        source: "SDR".into(),
                    });
                }
            }
        }
    }
    v
}

unsafe fn device(adapter: &IDXGIAdapter1) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut d, mut c) = (None, None);
    let r = D3D11CreateDevice(
        adapter,
        D3D_DRIVER_TYPE_UNKNOWN,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut d),
        None,
        Some(&mut c),
    );
    if r.is_err() {
        D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut d), None, Some(&mut c))?;
    }
    Ok((d.unwrap(), c.unwrap()))
}

unsafe fn duplicate(out: &IDXGIOutput, dev: &ID3D11Device) -> windows::core::Result<IDXGIOutputDuplication> {
    if let Ok(o5) = out.cast::<IDXGIOutput5>() {
        if let Ok(d) = o5.DuplicateOutput1(dev, 0, &[DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM]) {
            return Ok(d);
        }
    }
    out.cast::<IDXGIOutput1>()?.DuplicateOutput(dev)
}

/// `--hdr-info`: what Aperture detected for each monitor.
pub fn hdr_report() -> String {
    let mut s = String::from("Aperture HDR detection\r\n\r\n");
    for o in outputs() {
        let r = o.rect;
        s += &format!("Monitor at {},{} ({}x{}): {}\r\n  {}\r\n\r\n", r.left, r.top, r.right - r.left, r.bottom - r.top, if o.hdr { "HDR on" } else { "HDR off" }, o.source);
    }
    s += "Screenshots map SDR white to full white and roll HDR highlights off up to the peak.\r\n";
    s
}

// ---------------------------------------------------------------- HDR stills

fn half_lut() -> &'static [f32] {
    static L: OnceLock<Vec<f32>> = OnceLock::new();
    L.get_or_init(|| {
        (0..=u16::MAX)
            .map(|h| {
                let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
                let e = ((h >> 10) & 0x1F) as i32;
                let m = (h & 0x3FF) as f32;
                s * match e {
                    0 => m * 2f32.powi(-24),
                    31 => 65504.0,
                    _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
                }
            })
            .collect()
    })
}

fn srgb_lut() -> &'static [u8] {
    static L: OnceLock<Vec<u8>> = OnceLock::new();
    L.get_or_init(|| {
        (0..4096)
            .map(|i| {
                let c = i as f32 / 4095.0;
                let v = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
                (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            })
            .collect()
    })
}

// ---- BT.2390 highlight roll-off (EETF) ----
// HDR is mapped into SDR by treating "SDR white" as the target peak: values well
// below white pass through unchanged, and everything between the knee and the
// brightest content is squeezed along a Hermite spline in PQ space, the
// perceptual scale HDR itself is encoded in. Applied to max(R,G,B) as one ratio
// for all three channels, so hues stay put.

const PQ_M1: f32 = 0.159_301_76;
const PQ_M2: f32 = 78.843_75;
const PQ_C1: f32 = 0.835_937_5;
const PQ_C2: f32 = 18.851_563;
const PQ_C3: f32 = 18.6875;

/// Nits → PQ signal (0..1, 10,000 nits = 1).
fn pq(nits: f32) -> f32 {
    let y = (nits / 10_000.0).max(0.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y)).powf(PQ_M2)
}

/// PQ signal → nits.
fn pq_inv(e: f32) -> f32 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
    ((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1) * 10_000.0
}

/// ITU-R BT.2390 EETF: maps `nits` from content peaking at `src_peak` onto a
/// display peaking at `dst_peak` (both in nits).
pub fn bt2390(nits: f32, src_peak: f32, dst_peak: f32) -> f32 {
    if src_peak <= dst_peak {
        return nits.min(dst_peak);
    }
    let lw = pq(src_peak);
    let e1 = pq(nits.min(src_peak)) / lw;
    let max_lum = pq(dst_peak) / lw;
    let ks = (1.5 * max_lum - 0.5).max(0.0);
    let e2 = if e1 < ks {
        e1
    } else {
        let t = (e1 - ks) / (1.0 - ks);
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * ks + (t3 - 2.0 * t2 + t) * (1.0 - ks) + (-2.0 * t3 + 3.0 * t2) * max_lum
    };
    pq_inv(e2 * lw)
}

/// Where the BT.2390 curve starts bending, in nits (below it nothing changes).
fn bt2390_knee(src_peak: f32, dst_peak: f32) -> f32 {
    let lw = pq(src_peak);
    pq_inv((1.5 * pq(dst_peak) / lw - 0.5).max(0.0) * lw)
}

/// The EETF for one screenshot, tabulated: `ratio(m)` scales a pixel whose
/// brightest channel is `m` (1.0 = SDR white) so it ends up ≤ 1.0.
struct Eetf {
    /// brightest value present, relative to SDR white
    peak: f32,
    /// output / input for m = peak * i / (N - 1)
    table: Vec<f32>,
}

impl Eetf {
    const N: usize = 4096;
    fn new(peak: f32, sdr_nits: f32) -> Eetf {
        let peak = peak.max(1.0);
        let table = (0..Self::N)
            .map(|i| {
                let m = peak * i as f32 / (Self::N - 1) as f32;
                if m <= 0.0 {
                    return 1.0;
                }
                bt2390(m * sdr_nits, peak * sdr_nits, sdr_nits) / sdr_nits / m
            })
            .collect();
        Eetf { peak, table }
    }
    #[inline]
    fn ratio(&self, m: f32) -> f32 {
        if self.peak <= 1.0 {
            return 1.0; // nothing brighter than SDR white on screen
        }
        let m = m.min(self.peak); // the panel can't show more than its peak either
        let f = m / self.peak * (Self::N - 1) as f32;
        let i = (f as usize).min(Self::N - 2);
        let t = f - i as f32;
        self.table[i] * (1.0 - t) + self.table[i + 1] * t
    }
}

/// Re-captures every HDR monitor through Desktop Duplication (FP16) and writes
/// tone-mapped sRGB pixels into `dst` (the virtual-screen image GDI already filled).
/// SDR monitors are left untouched, so this costs nothing when HDR is off.
pub fn patch_hdr(vx: i32, vy: i32, vw: i32, vh: i32, dst: &mut [u32]) {
    patch_hdr_report(vx, vy, vw, vh, dst);
}

/// Same as `patch_hdr`, plus one line per HDR monitor saying what happened.
pub fn patch_hdr_report(vx: i32, vy: i32, vw: i32, vh: i32, dst: &mut [u32]) -> Vec<String> {
    let outs = outputs();
    let mut log = Vec::new();
    for o in outs.iter().filter(|o| o.hdr) {
        let at = format!("Monitor at {},{}", o.rect.left, o.rect.top);
        if o.rotated {
            log.push(format!("{at}: skipped (rotated monitors aren't corrected yet)"));
            continue;
        }
        let r = unsafe { grab_hdr(o, vx, vy, vw, vh, dst) };
        log.push(match r {
            Ok(msg) => format!("{at}: {msg}"),
            Err(e) => format!("{at}: HDR capture failed ({}), the plain Windows capture was kept", e.message()),
        });
    }
    log
}

unsafe fn grab_hdr(o: &Out, vx: i32, vy: i32, vw: i32, vh: i32, dst: &mut [u32]) -> windows::core::Result<String> {
    let (dev, ctx) = device(&o.adapter)?;
    let dup = duplicate(&o.output, &dev)?;
    let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut res: Option<IDXGIResource> = None;
    let mut ok = false;
    for _ in 0..5 {
        if dup.AcquireNextFrame(250, &mut info, &mut res).is_ok() {
            ok = true;
            break;
        }
    }
    if !ok {
        return Err(windows::core::Error::from_hresult(DXGI_ERROR_WAIT_TIMEOUT));
    }
    let tex: ID3D11Texture2D = res.unwrap().cast()?;
    let mut d = D3D11_TEXTURE2D_DESC::default();
    tex.GetDesc(&mut d);
    let sd = D3D11_TEXTURE2D_DESC {
        Width: d.Width,
        Height: d.Height,
        MipLevels: 1,
        ArraySize: 1,
        Format: d.Format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut st = None;
    dev.CreateTexture2D(&sd, None, Some(&mut st))?;
    let st = st.unwrap();
    ctx.CopyResource(&st, &tex);
    drop(tex);
    let _ = dup.ReleaseFrame();
    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
    ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
    let (w, h) = (d.Width as usize, d.Height as usize);
    let pitch = m.RowPitch as usize;
    let base = m.pData as *const u8;
    let ox = o.rect.left - vx;
    let oy = o.rect.top - vy;
    let mut msg = format!("got an 8-bit image ({:?}), the plain Windows capture was kept", d.Format);
    if d.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
        let hl = half_lut();
        let sl = srgb_lut();
        let scale = 80.0 / o.sdr_nits.max(1.0);
        // brightest value actually on screen (sampled), capped at what the panel can show
        let mut lmax = 0f32;
        for y in (0..h).step_by(3) {
            let row = std::slice::from_raw_parts(base.add(y * pitch) as *const u16, w * 4);
            for x in (0..w).step_by(3) {
                let p = &row[x * 4..x * 4 + 3];
                let mm = hl[p[0] as usize].max(hl[p[1] as usize]).max(hl[p[2] as usize]) * scale;
                lmax = lmax.max(mm);
            }
        }
        let lw = lmax.min(o.max_nits / o.sdr_nits.max(1.0)).max(1.0);
        let eetf = Eetf::new(lw, o.sdr_nits.max(1.0));
        let eetf = &eetf;
        msg = if lw > 1.0 {
            format!(
                "corrected from HDR ({w}x{h}): SDR white {:.0} nits; HDR highlights up to {:.0} nits fitted in with the BT.2390 curve (unchanged below {:.0} nits)",
                o.sdr_nits,
                lw * o.sdr_nits,
                bt2390_knee(lw * o.sdr_nits, o.sdr_nits)
            )
        } else {
            format!("corrected from HDR ({w}x{h}): SDR white {:.0} nits mapped to full white; nothing brighter was on screen, so colors are unchanged", o.sdr_nits)
        };
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
        let rows_per = h.div_ceil(threads);
        let dst_ptr = dst.as_mut_ptr() as usize;
        let base_u = base as usize;
        std::thread::scope(|sc| {
            for t in 0..threads {
                sc.spawn(move || {
                    let dst = dst_ptr as *mut u32;
                    for y in t * rows_per..((t + 1) * rows_per).min(h) {
                        let gy = y as i32 + oy;
                        if gy < 0 || gy >= vh {
                            continue;
                        }
                        let row = std::slice::from_raw_parts((base_u as *const u8).add(y * pitch) as *const u16, w * 4);
                        for x in 0..w {
                            let gx = x as i32 + ox;
                            if gx < 0 || gx >= vw {
                                continue;
                            }
                            let p = &row[x * 4..x * 4 + 3];
                            let mut r = (hl[p[0] as usize] * scale).max(0.0);
                            let mut g = (hl[p[1] as usize] * scale).max(0.0);
                            let mut b = (hl[p[2] as usize] * scale).max(0.0);
                            let k = eetf.ratio(r.max(g).max(b));
                            r *= k;
                            g *= k;
                            b *= k;
                            let q = |c: f32| sl[(c.min(1.0) * 4095.0) as usize] as u32;
                            *dst.add(gy as usize * vw as usize + gx as usize) = 0xFF00_0000 | (q(r) << 16) | (q(g) << 8) | q(b);
                        }
                    }
                });
            }
        });
    }
    // an 8-bit duplication of an HDR desktop is clipped (too bright), so the GDI
    // capture already in `dst` is the better picture: leave it
    ctx.Unmap(&st, 0);
    Ok(msg)
}

// ---------------------------------------------------------------- video engine

const SHADER: &str = r#"
Texture2D<float4> src : register(t0);
cbuffer C : register(b0) { float2 off; float hdr; float scale; float lw; float2 k; float pad; };
float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}
float3 enc(float3 c) {
    c = saturate(c);
    float3 lo = c * 12.92;
    float3 hi = 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    return lerp(hi, lo, step(c, 0.0031308));
}
float pq(float nits) {
    float y = pow(max(nits / 10000.0, 0), 0.1593017578125);
    return pow((0.8359375 + 18.8515625 * y) / (1 + 18.6875 * y), 78.84375);
}
float pq_inv(float e) {
    float p = pow(saturate(e), 1.0 / 78.84375);
    return pow(max(p - 0.8359375, 0) / (18.8515625 - 18.6875 * p), 1.0 / 0.1593017578125) * 10000.0;
}
float4 ps(float4 p : SV_Position) : SV_Target {
    float4 c;
    if (k.x > 1.001 || k.y > 1.001) {
        // downscale: box-average an n x n grid over the source footprint of this pixel
        int nx = clamp((int)ceil(k.x), 1, 4);
        int ny = clamp((int)ceil(k.y), 1, 4);
        float2 o = off + (p.xy - 0.5) * k;
        c = 0;
        for (int j = 0; j < ny; j++)
            for (int i = 0; i < nx; i++)
                c += src.Load(int3(int2(o + (float2(i, j) + 0.5) * k / float2(nx, ny)), 0));
        c /= nx * ny;
    } else {
        c = src.Load(int3(int2(p.xy + off), 0));
    }
    if (hdr > 0.5) {
        // BT.2390 EETF on max(R,G,B): SDR white is the target peak, `lw` (relative
        // to SDR white) the source peak; same curve as the screenshot path
        float3 l = max(c.rgb * scale, 0);
        float m = min(max(max(l.r, l.g), l.b), lw);
        if (m > 0 && lw > 1.0) {
            float sdr = 80.0 / scale;
            float pw = pq(lw * sdr);
            float e1 = pq(m * sdr) / pw;
            float ml = pq(sdr) / pw;
            float ks = max(1.5 * ml - 0.5, 0);
            float e2 = e1;
            if (e1 >= ks) {
                float t = (e1 - ks) / (1 - ks);
                float t2 = t * t, t3 = t2 * t;
                e2 = (2 * t3 - 3 * t2 + 1) * ks + (t3 - 2 * t2 + t) * (1 - ks) + (-2 * t3 + 3 * t2) * ml;
            }
            float outm = pq_inv(e2 * pw) / sdr;
            l *= outm / max(max(max(l.r, l.g), l.b), 1e-6);
        }
        return float4(enc(l), 1);
    }
    return float4(c.rgb, 1);
}
// RGB -> NV12 (BT.709, limited range) so the encoder gets its native format straight from the GPU
float4 psY(float4 p : SV_Position) : SV_Target {
    float3 c = src.Load(int3(int2(p.xy), 0)).rgb;
    return float4(dot(c, float3(0.2126, 0.7152, 0.0722)) * (219.0 / 255.0) + 16.0 / 255.0, 0, 0, 1);
}
float4 psUV(float4 p : SV_Position) : SV_Target {
    int2 q = int2(p.xy) * 2;
    float3 c = (src.Load(int3(q, 0)).rgb + src.Load(int3(q + int2(1, 0), 0)).rgb + src.Load(int3(q + int2(0, 1), 0)).rgb + src.Load(int3(q + int2(1, 1), 0)).rgb) * 0.25;
    float y = dot(c, float3(0.2126, 0.7152, 0.0722));
    return float4((c.b - y) / 1.8556 * (224.0 / 255.0) + 128.0 / 255.0, (c.r - y) / 1.5748 * (224.0 / 255.0) + 128.0 / 255.0, 0, 1);
}
"#;

pub const WM_MEDIA: u32 = 0x8000 + 7;

pub enum MediaEvent {
    Saved { path: PathBuf, secs: f64, clip: bool },
    Error(String),
}

static NOTIFY: AtomicUsize = AtomicUsize::new(0);
pub fn set_notify(hwnd: usize) {
    NOTIFY.store(hwnd, SeqCst);
}
pub fn report(e: MediaEvent) {
    notify(e)
}
fn notify(e: MediaEvent) {
    let p = Box::into_raw(Box::new(e)) as isize;
    unsafe {
        let _ = PostMessageW(Some(HWND(NOTIFY.load(SeqCst) as *mut _)), WM_MEDIA, WPARAM(0), LPARAM(p));
    }
}

fn mf_ready() -> bool {
    static M: OnceLock<bool> = OnceLock::new();
    *M.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL).is_ok() })
}

pub use crate::video::bitrate;

struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

struct Gpu {
    dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    ps_y: ID3D11PixelShader,
    ps_uv: ID3D11PixelShader,
    cb: ID3D11Buffer,
    mgr: IMFDXGIDeviceManager,
}

unsafe fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<Vec<u8>> {
    let mut blob = None;
    D3DCompile(SHADER.as_ptr() as *const _, SHADER.len(), PCSTR::null(), None, None, entry, target, 0, 0, &mut blob, None)?;
    let b = blob.unwrap();
    Ok(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()).to_vec())
}

impl Gpu {
    unsafe fn new(adapter: &IDXGIAdapter1) -> windows::core::Result<Gpu> {
        let (dev, ctx) = device(adapter)?;
        if let Ok(mt) = dev.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
        let vsb = compile(s!("vs"), s!("vs_4_0"))?;
        let psb = compile(s!("ps"), s!("ps_4_0"))?;
        let pyb = compile(s!("psY"), s!("ps_4_0"))?;
        let puvb = compile(s!("psUV"), s!("ps_4_0"))?;
        let (mut vs, mut ps, mut cb, mut ps_y, mut ps_uv) = (None, None, None, None, None);
        dev.CreateVertexShader(&vsb, None, Some(&mut vs))?;
        dev.CreatePixelShader(&psb, None, Some(&mut ps))?;
        dev.CreatePixelShader(&pyb, None, Some(&mut ps_y))?;
        dev.CreatePixelShader(&puvb, None, Some(&mut ps_uv))?;
        let bd = D3D11_BUFFER_DESC { ByteWidth: 32, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
        dev.CreateBuffer(&bd, None, Some(&mut cb))?;
        let mut token = 0u32;
        let mut mgr = None;
        MFCreateDXGIDeviceManager(&mut token, &mut mgr)?;
        let mgr = mgr.unwrap();
        mgr.ResetDevice(&dev, token)?;
        Ok(Gpu { dev, ctx, vs: vs.unwrap(), ps: ps.unwrap(), ps_y: ps_y.unwrap(), ps_uv: ps_uv.unwrap(), cb: cb.unwrap(), mgr })
    }
}

unsafe fn make_writer(path: &Path, w: u32, h: u32, fps: u32, mgr: &IMFDXGIDeviceManager, nv12: bool) -> windows::core::Result<IMFSinkWriter> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 4)?;
    let attrs = attrs.unwrap();
    attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, mgr)?;
    attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
    attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
    let wr = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs)?;
    let size = ((w as u64) << 32) | h as u64;
    let rate = ((fps as u64) << 32) | 1;
    let out = MFCreateMediaType()?;
    out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
    out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate(w, h, fps))?;
    out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    out.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
    out.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
    out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
    let idx = wr.AddStream(&out)?;
    let inp = MFCreateMediaType()?;
    inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    inp.SetGUID(&MF_MT_SUBTYPE, if nv12 { &MFVideoFormat_NV12 } else { &MFVideoFormat_RGB32 })?;
    inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    inp.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
    inp.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
    inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
    wr.SetInputMediaType(idx, &inp, None)?;
    wr.BeginWriting()?;
    Ok(wr)
}

unsafe fn write_frame(wr: &IMFSinkWriter, tex: &ID3D11Texture2D, t: i64, dur: i64) -> windows::core::Result<()> {
    let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, tex, 0, false)?;
    if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
        if let Ok(l) = b2.GetContiguousLength() {
            buf.SetCurrentLength(l)?;
        }
    }
    let s = MFCreateSample()?;
    s.AddBuffer(&buf)?;
    s.SetSampleTime(t)?;
    s.SetSampleDuration(dur)?;
    wr.WriteSample(0, &s)
}

static FINALIZING: AtomicUsize = AtomicUsize::new(0);

fn finalize_async(wr: IMFSinkWriter) -> JoinHandle<bool> {
    let b = SendBox(wr);
    FINALIZING.fetch_add(1, SeqCst);
    std::thread::spawn(move || {
        let b = b;
        let ok = unsafe { b.0.Finalize().is_ok() };
        FINALIZING.fetch_sub(1, SeqCst);
        ok
    })
}

/// On quit: let engines stop and in-flight MP4 files finish writing (max ~6 s).
pub fn shutdown() {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(6) {
        let engines_busy = ENGINES.lock().map(|g| g.iter().any(|e| !e.join.is_finished())).unwrap_or(false);
        if !engines_busy && FINALIZING.load(SeqCst) == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("Aperture").join("replay"));
}

/// Replay segment length; clips are cut at segment/keyframe boundaries.
fn seg_secs() -> u64 {
    4
}

struct Seg {
    path: PathBuf,
    start: u64,
    frames: u64,
    fin: Option<JoinHandle<bool>>,
}

struct Replay {
    secs: u32,
    dir: PathBuf,
    seq: u64,
    cur: Option<(IMFSinkWriter, Seg)>,
    next: Option<Receiver<SendBox<windows::core::Result<IMFSinkWriter>>>>,
    done: VecDeque<Seg>,
    busy: Arc<AtomicUsize>,
    nv12: bool,
}

enum Kind {
    Record { id: u64, path: PathBuf, wr: IMFSinkWriter, frames: u64 },
    Replay(Replay),
}

struct Consumer {
    kind: Kind,
    off: (f32, f32),
    origin: (i32, i32), // screen coords of top-left
    /// source pixels per output pixel (1.0 = native resolution)
    k: (f32, f32),
    w: u32,
    h: u32,
    /// encoder input textures: (texture, BGRA or luma RTV, chroma RTV when NV12)
    pool: Vec<(ID3D11Texture2D, ID3D11RenderTargetView, Option<ID3D11RenderTargetView>)>,
    /// NV12 mode: frames are composed here first (cursor drawn via GDI), then converted
    stage: Option<(ID3D11Texture2D, ID3D11RenderTargetView, ID3D11ShaderResourceView)>,
    pi: usize,
    /// tick of the last encoded frame (unchanged screens are skipped: variable frame rate)
    last: Option<u64>,
    start: u64,
}

enum Cmd {
    Record { id: u64, rect: RECT, path: PathBuf, res: u32, reply: Sender<Result<(), String>> },
    Stop { id: u64 },
    Replay { secs: u32, res: u32, dir: PathBuf, reply: Sender<Result<(), String>> },
    StopReplay,
    SaveClip { path: PathBuf },
}

struct EngineHandle {
    tx: Sender<Cmd>,
    rect: RECT,
    fps: u32,
    join: JoinHandle<()>,
}

static ENGINES: Mutex<Vec<EngineHandle>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

fn monitor_for(rect: &RECT) -> Option<RECT> {
    let (cx, cy) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
    outputs()
        .into_iter()
        .find(|o| cx >= o.rect.left && cx < o.rect.right && cy >= o.rect.top && cy < o.rect.bottom)
        .map(|o| o.rect)
}

fn send(mon: RECT, fps: u32, mut cmd: Cmd) {
    let mut g = ENGINES.lock().unwrap();
    g.retain(|e| !e.join.is_finished());
    // an engine for that monitor with a different fps must finish first
    if let Some(i) = g.iter().position(|e| e.rect == mon && e.fps != fps) {
        let old = g.remove(i);
        drop(g);
        let t = Instant::now();
        while !old.join.is_finished() && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        g = ENGINES.lock().unwrap();
    }
    if let Some(e) = g.iter().find(|e| e.rect == mon) {
        match e.tx.send(cmd) {
            Ok(()) => return,
            Err(std::sync::mpsc::SendError(c)) => cmd = c,
        }
    }
    g.retain(|e| e.rect != mon);
    let (tx, rx) = channel();
    let _ = tx.send(cmd);
    let join = std::thread::spawn(move || engine(mon, fps, rx));
    g.push(EngineHandle { tx, rect: mon, fps, join });
}

fn send_existing(pred: impl Fn(&EngineHandle) -> bool, cmd: impl Fn() -> Cmd) {
    let g = ENGINES.lock().unwrap();
    for e in g.iter().filter(|e| pred(e)) {
        let _ = e.tx.send(cmd());
    }
}

pub fn primary_rect() -> RECT {
    outputs().into_iter().find(|o| o.rect.left == 0 && o.rect.top == 0).map(|o| o.rect).unwrap_or_default()
}

// ---- public API (called from the UI thread) ----

/// Starts recording `rect` (screen coords, clamped to one monitor). Returns a recording id.
/// `res` caps the video height in pixels (0 = native); it never upscales.
pub fn start_record(rect: RECT, path: PathBuf, fps: u32, res: u32) -> Result<u64, String> {
    let mon = monitor_for(&rect).ok_or("No monitor found for that area")?;
    let id = NEXT_ID.fetch_add(1, SeqCst) as u64;
    let (rtx, rrx) = channel();
    send(mon, fps, Cmd::Record { id, rect, path, res, reply: rtx });
    rrx.recv_timeout(Duration::from_secs(10)).map_err(|_| "The video encoder didn't start".to_string())??;
    Ok(id)
}

pub fn stop_record(id: u64) {
    send_existing(|_| true, || Cmd::Stop { id });
}

pub fn start_replay(secs: u32, fps: u32, res: u32, dir: PathBuf) -> Result<(), String> {
    let mon = primary_rect();
    let (rtx, rrx) = channel();
    send(mon, fps, Cmd::Replay { secs, res, dir, reply: rtx });
    rrx.recv_timeout(Duration::from_secs(10)).map_err(|_| "The video encoder didn't start".to_string())?
}

pub fn stop_replay() {
    send_existing(|_| true, || Cmd::StopReplay);
}

pub fn save_clip(path: PathBuf) {
    send_existing(|_| true, || Cmd::SaveClip { path: path.clone() });
}

// ---- engine thread ----

pub use crate::video::scaled_size;

unsafe fn make_consumer(g: &Gpu, kind_fn: impl FnOnce(u32, u32, bool) -> Result<Kind, String>, out_rect: &RECT, rect: RECT, res: u32, tick: u64) -> Result<Consumer, String> {
    let ow = out_rect.right - out_rect.left;
    let oh = out_rect.bottom - out_rect.top;
    let mut l = (rect.left - out_rect.left).clamp(0, ow);
    let mut t = (rect.top - out_rect.top).clamp(0, oh);
    let mut r = (rect.right - out_rect.left).clamp(0, ow);
    let mut b = (rect.bottom - out_rect.top).clamp(0, oh);
    // encoders want even sizes and a sane minimum
    for (a, z, max) in [(&mut l, &mut r, ow), (&mut t, &mut b, oh)] {
        if *z - *a < 64 {
            *z = (*a + 64).min(max);
            *a = (*z - 64).max(0);
        }
        if (*z - *a) % 2 == 1 {
            if *z < max { *z += 1 } else { *a -= 1 }
        }
    }
    let (sw, sh) = ((r - l) as u32, (b - t) as u32);
    let (w, h) = scaled_size(sw, sh, res);
    let k = (sw as f32 / w as f32, sh as f32 / h as f32);
    let (pool, stage) = match nv12_targets(g, w, h) {
        Some(x) => x,
        None => (bgra_targets(g, w, h)?, None),
    };
    let kind = kind_fn(w, h, stage.is_some())?;
    Ok(Consumer { kind, off: (l as f32, t as f32), origin: (out_rect.left + l, out_rect.top + t), k, w, h, pool, stage, pi: 0, start: tick, last: None })
}

type Pool = Vec<(ID3D11Texture2D, ID3D11RenderTargetView, Option<ID3D11RenderTargetView>)>;

unsafe fn nv12_targets(g: &Gpu, w: u32, h: u32) -> Option<(Pool, Option<(ID3D11Texture2D, ID3D11RenderTargetView, ID3D11ShaderResourceView)>)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let view = |f: DXGI_FORMAT| D3D11_RENDER_TARGET_VIEW_DESC {
        Format: f,
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } },
    };
    let mut pool = Vec::new();
    for _ in 0..4 {
        let mut tex = None;
        g.dev.CreateTexture2D(&desc, None, Some(&mut tex)).ok()?;
        let tex = tex?;
        let (mut y, mut uv) = (None, None);
        g.dev.CreateRenderTargetView(&tex, Some(&view(DXGI_FORMAT_R8_UNORM)), Some(&mut y)).ok()?;
        g.dev.CreateRenderTargetView(&tex, Some(&view(DXGI_FORMAT_R8G8_UNORM)), Some(&mut uv)).ok()?;
        pool.push((tex, y?, uv));
    }
    let sd = D3D11_TEXTURE2D_DESC {
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        MiscFlags: D3D11_RESOURCE_MISC_GDI_COMPATIBLE.0 as u32,
        ..desc
    };
    let mut st = None;
    g.dev.CreateTexture2D(&sd, None, Some(&mut st)).ok()?;
    let st = st?;
    let (mut rtv, mut srv) = (None, None);
    g.dev.CreateRenderTargetView(&st, None, Some(&mut rtv)).ok()?;
    g.dev.CreateShaderResourceView(&st, None, Some(&mut srv)).ok()?;
    Some((pool, Some((st, rtv?, srv?))))
}

unsafe fn bgra_targets(g: &Gpu, w: u32, h: u32) -> Result<Pool, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_GDI_COMPATIBLE.0 as u32,
    };
    let mut pool = Vec::new();
    for _ in 0..6 {
        let mut tex = None;
        g.dev.CreateTexture2D(&desc, None, Some(&mut tex)).map_err(|e| e.message())?;
        let tex = tex.unwrap();
        let mut rtv = None;
        g.dev.CreateRenderTargetView(&tex, None, Some(&mut rtv)).map_err(|e| e.message())?;
        pool.push((tex, rtv.unwrap(), None));
    }
    Ok(pool)
}

struct CursorCache {
    h: isize,
    hot: (i32, i32),
    size: (i32, i32),
}

unsafe fn render(g: &Gpu, srv: &ID3D11ShaderResourceView, hdr: (bool, f32, f32), c: &mut Consumer, cur: &mut CursorCache) -> ID3D11Texture2D {
    let (tex, out_rtv, uv_rtv) = c.pool[c.pi].clone();
    c.pi = (c.pi + 1) % c.pool.len();
    // in NV12 mode compose into the BGRA stage texture first
    let (canvas, rtv) = match &c.stage {
        Some((st, srtv, _)) => (st.clone(), srtv.clone()),
        None => (tex.clone(), out_rtv.clone()),
    };
    let cb: [f32; 8] = [c.off.0, c.off.1, if hdr.0 { 1.0 } else { 0.0 }, hdr.1, hdr.2, c.k.0, c.k.1, 0.0];
    g.ctx.UpdateSubresource(&g.cb, 0, None, cb.as_ptr() as *const _, 0, 0);
    g.ctx.OMSetRenderTargets(Some(&[Some(rtv)]), None);
    g.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: c.w as f32, Height: c.h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
    g.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    g.ctx.IASetInputLayout(None);
    g.ctx.VSSetShader(&g.vs, None);
    g.ctx.PSSetShader(&g.ps, None);
    g.ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
    g.ctx.PSSetConstantBuffers(0, Some(&[Some(g.cb.clone())]));
    g.ctx.Draw(3, 0);
    g.ctx.OMSetRenderTargets(None, None);
    g.ctx.PSSetShaderResources(0, Some(&[None]));
    // mouse pointer (Desktop Duplication frames don't contain it)
    let mut ci = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
    if GetCursorInfo(&mut ci).is_ok() && ci.flags.0 & CURSOR_SHOWING.0 != 0 && !ci.hCursor.is_invalid() {
        if cur.h != ci.hCursor.0 as isize {
            let mut ii = ICONINFO::default();
            if GetIconInfo(HICON(ci.hCursor.0), &mut ii).is_ok() {
                cur.hot = (ii.xHotspot as i32, ii.yHotspot as i32);
                cur.size = (GetSystemMetrics(SM_CXCURSOR), GetSystemMetrics(SM_CYCURSOR));
                let mut bm = BITMAP::default();
                let (hb, tall) = if !ii.hbmColor.is_invalid() { (ii.hbmColor, false) } else { (ii.hbmMask, true) };
                if !hb.is_invalid() && GetObjectW(hb.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut _)) != 0 {
                    // a monochrome cursor stacks its AND and XOR masks in one bitmap
                    cur.size = (bm.bmWidth, if tall { bm.bmHeight / 2 } else { bm.bmHeight });
                }
                if !ii.hbmMask.is_invalid() {
                    let _ = DeleteObject(ii.hbmMask.into());
                }
                if !ii.hbmColor.is_invalid() {
                    let _ = DeleteObject(ii.hbmColor.into());
                }
            }
            cur.h = ci.hCursor.0 as isize;
        }
        // positions and size follow the output scale so the pointer keeps its on-screen proportions
        let x = ((ci.ptScreenPos.x - cur.hot.0 - c.origin.0) as f32 / c.k.0).round() as i32;
        let y = ((ci.ptScreenPos.y - cur.hot.1 - c.origin.1) as f32 / c.k.1).round() as i32;
        let (cw, chh) = (((cur.size.0 as f32 / c.k.0).round() as i32).max(1), ((cur.size.1 as f32 / c.k.1).round() as i32).max(1));
        if x > -64 && y > -64 && x < c.w as i32 && y < c.h as i32 {
            if let Ok(s1) = canvas.cast::<IDXGISurface1>() {
                if let Ok(hdc) = s1.GetDC(false) {
                    let scaled = c.k.0 > 1.001 || c.k.1 > 1.001;
                    let _ = DrawIconEx(hdc, x, y, HICON(ci.hCursor.0), if scaled { cw } else { 0 }, if scaled { chh } else { 0 }, 0, None, DI_NORMAL);
                    let _ = s1.ReleaseDC(None);
                }
            }
        }
    }
    if let (Some((_, _, ssrv)), Some(uv)) = (&c.stage, uv_rtv) {
        g.ctx.PSSetShaderResources(0, Some(&[Some(ssrv.clone())]));
        g.ctx.OMSetRenderTargets(Some(&[Some(out_rtv)]), None);
        g.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: c.w as f32, Height: c.h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
        g.ctx.PSSetShader(&g.ps_y, None);
        g.ctx.Draw(3, 0);
        g.ctx.OMSetRenderTargets(Some(&[Some(uv)]), None);
        g.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: (c.w / 2) as f32, Height: (c.h / 2) as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
        g.ctx.PSSetShader(&g.ps_uv, None);
        g.ctx.Draw(3, 0);
        g.ctx.OMSetRenderTargets(None, None);
        g.ctx.PSSetShaderResources(0, Some(&[None]));
    }
    tex
}

fn seg_path(dir: &Path, seq: u64) -> PathBuf {
    dir.join(format!("seg_{seq:08}.mp4"))
}

unsafe fn prepare_next(rp: &mut Replay, g: &Gpu, w: u32, h: u32, fps: u32) {
    if rp.next.is_some() {
        return;
    }
    let nv12 = rp.nv12;
    let path = seg_path(&rp.dir, rp.seq + 1);
    let mgr = SendBox(g.mgr.clone());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mgr = mgr;
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let r = unsafe { make_writer(&path, w, h, fps, &mgr.0, nv12) };
        let _ = tx.send(SendBox(r));
    });
    rp.next = Some(rx);
}

/// Swap to the next segment writer. `force` waits for it if it isn't ready yet.
unsafe fn rotate(rp: &mut Replay, g: &Gpu, w: u32, h: u32, fps: u32, next_tick: u64, force: bool) {
    if rp.next.is_none() {
        prepare_next(rp, g, w, h, fps);
    }
    let ready = match &rp.next {
        Some(rx) => {
            if force {
                rx.recv_timeout(Duration::from_secs(5)).ok()
            } else {
                rx.try_recv().ok()
            }
        }
        None => None,
    };
    let Some(SendBox(res)) = ready else { return };
    rp.next = None;
    let Ok(wr) = res else { return };
    rp.seq += 1;
    let seg = Seg { path: seg_path(&rp.dir, rp.seq), start: next_tick, frames: 0, fin: None };
    if let Some((old, mut s)) = rp.cur.replace((wr, seg)) {
        s.fin = Some(finalize_async(old));
        rp.done.push_back(s);
    }
    // drop segments that are older than the buffer needs
    let keep_ticks = (rp.secs as u64 + seg_secs()) * fps as u64;
    while let Some(f) = rp.done.front() {
        if next_tick.saturating_sub(f.start + f.frames) > keep_ticks && rp.busy.load(SeqCst) == 0 {
            let mut f = rp.done.pop_front().unwrap();
            if let Some(j) = f.fin.take() {
                let _ = j.join();
            }
            let _ = std::fs::remove_file(&f.path);
        } else {
            break;
        }
    }
}

fn engine(mon: RECT, fps: u32, rx: Receiver<Cmd>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let fail = |cmd: Cmd, msg: String| match cmd {
            Cmd::Record { reply, .. } | Cmd::Replay { reply, .. } => {
                let _ = reply.send(Err(msg));
            }
            _ => {}
        };
        let Some(out) = outputs().into_iter().find(|o| o.rect == mon) else {
            while let Ok(c) = rx.try_recv() {
                fail(c, "Monitor not found".into());
            }
            return;
        };
        if !mf_ready() {
            while let Ok(c) = rx.try_recv() {
                fail(c, "Media Foundation isn't available (Windows N edition?)".into());
            }
            return;
        }
        let g = match Gpu::new(&out.adapter) {
            Ok(g) => g,
            Err(e) => {
                while let Ok(c) = rx.try_recv() {
                    fail(c, format!("GPU setup failed: {}", e.message()));
                }
                return;
            }
        };
        let mut dup = match duplicate(&out.output, &g.dev) {
            Ok(d) => Some(d),
            Err(e) => {
                while let Ok(c) = rx.try_recv() {
                    fail(c, format!("Screen capture unavailable: {}", e.message()));
                }
                return;
            }
        };
        let hdr = (out.hdr, 80.0 / out.sdr_nits.max(1.0), out.max_nits / out.sdr_nits.max(1.0));
        let period = 10_000_000i64 / fps as i64;
        let t0 = Instant::now();
        let mut tick: u64 = 0;
        let mut consumers: Vec<Consumer> = Vec::new();
        let mut desk: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, DXGI_FORMAT)> = None;
        let mut cur = CursorCache { h: 0, hot: (0, 0), size: (32, 32) };
        let mut changed: u64 = 0;
        let mut idle_since: Option<Instant> = None;
        // Display changes (HDR on/off, resolution, monitors) make DXGI's view stale;
        // a stale duplication can go silent, which used to freeze replay clips on an
        // old picture. Watch for both.
        let mut factory = CreateDXGIFactory1::<IDXGIFactory1>().ok();
        let mut last_check = Instant::now();
        let mut last_frame = Instant::now();
        let mut out = out;
        let mut hdr = hdr;
        loop {
            // ---- commands
            while let Ok(c) = rx.try_recv() {
                match c {
                    Cmd::Record { id, rect, path, res, reply } => {
                        let mgr = g.mgr.clone();
                        let p2 = path.clone();
                        let r = make_consumer(
                            &g,
                            |w, h, nv12| make_writer(&p2, w, h, fps, &mgr, nv12).map(|wr| Kind::Record { id, path: p2.clone(), wr, frames: 0 }).map_err(|e| e.message()),
                            &out.rect,
                            rect,
                            res,
                            tick,
                        );
                        match r {
                            Ok(c) => {
                                consumers.push(c);
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                    Cmd::Stop { id } => {
                        if let Some(i) = consumers.iter().position(|c| matches!(&c.kind, Kind::Record { id: x, .. } if *x == id)) {
                            let c = consumers.remove(i);
                            let secs = tick.saturating_sub(c.start) as f64 / fps as f64;
                            if let Kind::Record { path, wr, .. } = c.kind {
                                let j = finalize_async(wr);
                                std::thread::spawn(move || {
                                    if j.join().unwrap_or(false) {
                                        notify(MediaEvent::Saved { path, secs, clip: false });
                                    } else {
                                        notify(MediaEvent::Error("The recording couldn't be finished".into()));
                                    }
                                });
                            }
                        }
                    }
                    Cmd::Replay { secs, res, dir, reply } => {
                        if let Some(c) = consumers.iter_mut().find(|c| matches!(c.kind, Kind::Replay(_))) {
                            if let Kind::Replay(rp) = &mut c.kind {
                                rp.secs = secs;
                            }
                            let _ = reply.send(Ok(()));
                            continue;
                        }
                        let _ = std::fs::remove_dir_all(&dir);
                        let _ = std::fs::create_dir_all(&dir);
                        let mgr = g.mgr.clone();
                        let d2 = dir.clone();
                        let r = make_consumer(
                            &g,
                            |w, h, nv12| {
                                let p = seg_path(&d2, 0);
                                make_writer(&p, w, h, fps, &mgr, nv12)
                                    .map(|wr| {
                                        Kind::Replay(Replay {
                                            secs,
                                            dir: d2.clone(),
                                            seq: 0,
                                            cur: Some((wr, Seg { path: p, start: tick, frames: 0, fin: None })),
                                            next: None,
                                            done: VecDeque::new(),
                                            busy: Arc::new(AtomicUsize::new(0)),
                                            nv12,
                                        })
                                    })
                                    .map_err(|e| e.message())
                            },
                            &out.rect,
                            out.rect,
                            res,
                            tick,
                        );
                        match r {
                            Ok(c) => {
                                consumers.push(c);
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                    Cmd::StopReplay => {
                        if let Some(i) = consumers.iter().position(|c| matches!(c.kind, Kind::Replay(_))) {
                            if let Kind::Replay(mut rp) = consumers.remove(i).kind {
                                rp.next = None;
                                let mut joins = Vec::new();
                                if let Some((wr, _)) = rp.cur.take() {
                                    joins.push(finalize_async(wr));
                                }
                                for s in rp.done.iter_mut() {
                                    if let Some(j) = s.fin.take() {
                                        joins.push(j);
                                    }
                                }
                                let dir = rp.dir.clone();
                                let busy = rp.busy.clone();
                                std::thread::spawn(move || {
                                    for j in joins {
                                        let _ = j.join();
                                    }
                                    let t = Instant::now();
                                    while busy.load(SeqCst) > 0 && t.elapsed() < Duration::from_secs(120) {
                                        std::thread::sleep(Duration::from_millis(100));
                                    }
                                    let _ = std::fs::remove_dir_all(dir);
                                });
                            }
                        }
                    }
                    Cmd::SaveClip { path } => {
                        let Some(c) = consumers.iter_mut().find(|c| matches!(c.kind, Kind::Replay(_))) else { continue };
                        let (w, h) = (c.w, c.h);
                        let Kind::Replay(rp) = &mut c.kind else { continue };
                        rotate(rp, &g, w, h, fps, tick, true);
                        let end = tick;
                        let cutoff = end.saturating_sub(rp.secs as u64 * fps as u64);
                        let mut segs = Vec::new();
                        for s in rp.done.iter_mut() {
                            if s.start + s.frames > cutoff && s.frames > 0 {
                                segs.push((s.path.clone(), s.start, s.fin.take()));
                            }
                        }
                        let busy = rp.busy.clone();
                        busy.fetch_add(1, SeqCst);
                        std::thread::spawn(move || {
                            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                            let mut list = Vec::new();
                            for (p, st, j) in segs {
                                if let Some(j) = j {
                                    let _ = j.join();
                                }
                                list.push((p, st));
                            }
                            let r = merge(&list, cutoff, fps, &path);
                            busy.fetch_sub(1, SeqCst);
                            match r {
                                Ok(secs) => notify(MediaEvent::Saved { path, secs, clip: true }),
                                Err(e) => notify(MediaEvent::Error(format!("Couldn't save the clip: {}", e.message()))),
                            }
                        });
                    }
                }
            }
            if consumers.is_empty() {
                // linger briefly so back-to-back commands reuse the engine
                let since = *idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_millis(400) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            idle_since = None;

            // ---- keep the capture healthy
            if last_check.elapsed() > Duration::from_millis(500) {
                last_check = Instant::now();
                if factory.as_ref().is_some_and(|f| !f.IsCurrent().as_bool()) {
                    // the display setup changed: find this monitor again and re-read its HDR values
                    factory = CreateDXGIFactory1::<IDXGIFactory1>().ok();
                    let (cx, cy) = ((out.rect.left + out.rect.right) / 2, (out.rect.top + out.rect.bottom) / 2);
                    let fresh = outputs().into_iter().find(|o| o.rect == out.rect).or_else(|| {
                        outputs().into_iter().find(|o| cx >= o.rect.left && cx < o.rect.right && cy >= o.rect.top && cy < o.rect.bottom)
                    });
                    if let Some(o) = fresh {
                        hdr = (o.hdr, 80.0 / o.sdr_nits.max(1.0), o.max_nits / o.sdr_nits.max(1.0));
                        out = o;
                    }
                    dup = None;
                }
                if dup.is_some() && last_frame.elapsed() > Duration::from_secs(2) {
                    // no new picture for a while: reconnect. On a still screen this is
                    // harmless (a new duplication hands over the current image at once);
                    // on a duplication that silently died it brings frames back.
                    dup = duplicate(&out.output, &g.dev).ok();
                    last_frame = Instant::now();
                }
            }

            // ---- grab desktop updates until the next frame is due
            let due = t0 + Duration::from_nanos(tick * 1_000_000_000 / fps as u64);
            let wait = due.saturating_duration_since(Instant::now()).as_millis() as u32;
            let Some(d) = &dup else {
                dup = duplicate(&out.output, &g.dev).ok();
                if dup.is_none() {
                    std::thread::sleep(Duration::from_millis(100));
                    // still failing: the output object itself may be stale
                    if let Some(o) = outputs().into_iter().find(|o| o.rect == out.rect) {
                        hdr = (o.hdr, 80.0 / o.sdr_nits.max(1.0), o.max_nits / o.sdr_nits.max(1.0));
                        out = o;
                    }
                }
                last_frame = Instant::now();
                continue;
            };
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match d.AcquireNextFrame(wait, &mut info, &mut res) {
                Ok(()) => {
                    if let Some(Ok(t)) = res.map(|r| r.cast::<ID3D11Texture2D>()) {
                        let mut td = D3D11_TEXTURE2D_DESC::default();
                        t.GetDesc(&mut td);
                        let need_new = match &desk {
                            Some((dt, _, f)) => {
                                let mut dd = D3D11_TEXTURE2D_DESC::default();
                                dt.GetDesc(&mut dd);
                                *f != td.Format || dd.Width != td.Width || dd.Height != td.Height
                            }
                            None => true,
                        };
                        if need_new {
                            let nd = D3D11_TEXTURE2D_DESC {
                                Width: td.Width,
                                Height: td.Height,
                                MipLevels: 1,
                                ArraySize: 1,
                                Format: td.Format,
                                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                                Usage: D3D11_USAGE_DEFAULT,
                                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                                CPUAccessFlags: 0,
                                MiscFlags: 0,
                            };
                            let mut nt = None;
                            if g.dev.CreateTexture2D(&nd, None, Some(&mut nt)).is_ok() {
                                let nt = nt.unwrap();
                                let mut srv = None;
                                if g.dev.CreateShaderResourceView(&nt, None, Some(&mut srv)).is_ok() {
                                    desk = Some((nt, srv.unwrap(), td.Format));
                                }
                            }
                        }
                        if info.LastPresentTime != 0 || need_new {
                            if let Some((dt, _, _)) = &desk {
                                g.ctx.CopyResource(dt, &t);
                            }
                        }
                    }
                    if info.LastPresentTime != 0 || info.LastMouseUpdateTime != 0 {
                        changed = tick;
                    }
                    if info.LastPresentTime != 0 {
                        last_frame = Instant::now();
                    }
                    let _ = d.ReleaseFrame();
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
                Err(_) => {
                    // access lost (UAC, lock screen, mode change) or anything else: reconnect
                    dup = None;
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            }
            if Instant::now() < due {
                continue;
            }
            let Some((_, srv, fmt)) = &desk else { continue };
            let is_hdr = *fmt == DXGI_FORMAT_R16G16B16A16_FLOAT;
            for c in consumers.iter_mut() {
                // encode only when something changed (plus a heartbeat twice a second)
                let due_write = match c.last {
                    None => true,
                    Some(l) => changed >= l + 1 || tick >= l + (fps as u64 / 2).max(1),
                };
                let tex = if due_write { Some(render(&g, srv, (is_hdr, hdr.1, hdr.2), c, &mut cur)) } else { None };
                let t = (tick - c.start) as i64 * period;
                let (w, h) = (c.w, c.h);
                match &mut c.kind {
                    Kind::Record { wr, frames, .. } => {
                        if let Some(tex) = &tex {
                            if write_frame(wr, tex, t, period).is_ok() {
                                *frames += 1;
                                c.last = Some(tick);
                            }
                        }
                    }
                    Kind::Replay(rp) => {
                        if let Some((wr, seg)) = &mut rp.cur {
                            if let Some(tex) = &tex {
                                let lt = (tick - seg.start) as i64 * period;
                                if write_frame(wr, tex, lt, period).is_ok() {
                                    c.last = Some(tick);
                                }
                            }
                            seg.frames = tick - seg.start + 1;
                            let f = seg.frames;
                            let per_seg = seg_secs() * fps as u64;
                            if f >= per_seg / 2 {
                                prepare_next(rp, &g, w, h, fps);
                            }
                            if f >= per_seg {
                                rotate(rp, &g, w, h, fps, tick + 1, false);
                            }
                        }
                    }
                }
            }
            tick += 1;
            // fell behind (e.g. a stall)? skip ahead instead of bursting frames
            let now_tick = (t0.elapsed().as_nanos() * fps as u128 / 1_000_000_000) as u64;
            if now_tick > tick + 2 {
                tick = now_tick;
            }
        }
        for c in consumers {
            if let Kind::Record { wr, .. } = c.kind {
                let _ = wr.Finalize();
            }
        }
        // a command that raced with shutdown: tell the caller instead of hanging
        while let Ok(c) = rx.try_recv() {
            fail(c, "Capture engine was restarting, please try again".into());
        }
    }
}

/// Joins replay segments into one MP4 without re-encoding, starting at the last
/// keyframe before `cutoff_tick`.
unsafe fn merge(segs: &[(PathBuf, u64)], cutoff_tick: u64, fps: u32, out: &Path) -> windows::core::Result<f64> {
    let period = 10_000_000i64 / fps as i64;
    let cutoff = cutoff_tick as i64 * period;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut wr: Option<IMFSinkWriter> = None;
    let mut base: Option<i64> = None;
    let mut pending: Vec<(IMFSample, i64)> = Vec::new();
    let mut last = 0i64;
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    for (path, start) in segs {
        let Ok(reader) = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None) else { continue };
        if wr.is_none() {
            let ty = reader.GetNativeMediaType(stream, 0)?;
            let w = MFCreateSinkWriterFromURL(&HSTRING::from(out.as_os_str()), None, None)?;
            w.AddStream(&ty)?;
            w.SetInputMediaType(0, &ty, None)?;
            w.BeginWriting()?;
            wr = Some(w);
        }
        let w = wr.as_ref().unwrap();
        let off = *start as i64 * period;
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            if reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)).is_err() {
                break;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(s) = sample else { continue };
            let gt = off + s.GetSampleTime().unwrap_or(0);
            let key = s.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            if base.is_none() {
                if gt < cutoff {
                    if key {
                        pending.clear();
                    }
                    if key || !pending.is_empty() {
                        pending.push((s, gt));
                    }
                    continue;
                }
                if pending.is_empty() && !key {
                    continue;
                }
                let b = pending.first().map(|p| p.1).unwrap_or(gt);
                base = Some(b);
                for (ps, pt) in pending.drain(..) {
                    ps.SetSampleTime(pt - b)?;
                    w.WriteSample(0, &ps)?;
                }
            }
            let b = base.unwrap();
            s.SetSampleTime(gt - b)?;
            w.WriteSample(0, &s)?;
            last = gt - b;
        }
    }
    let Some(w) = wr else { return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL)) };
    if base.is_none() {
        let b = pending.first().map(|p| p.1).unwrap_or(0);
        for (ps, pt) in pending.drain(..) {
            ps.SetSampleTime(pt - b)?;
            w.WriteSample(0, &ps)?;
            last = pt - b;
        }
    }
    w.Finalize()?;
    Ok((last + period) as f64 / 1e7)
}

/// Dev helper (`--probe in.mp4 out.png`): decodes the middle frame of a video and
/// writes it as PNG plus `out.png.txt` with "frames WxH".
pub fn probe(input: &str, out: &str) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if !mf_ready() {
            return;
        }
        let mut attrs = None;
        let _ = MFCreateAttributes(&mut attrs, 1);
        let attrs = attrs.unwrap();
        let _ = attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);
        let Ok(r) = MFCreateSourceReaderFromURL(&HSTRING::from(input), &attrs) else { return };
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let t = MFCreateMediaType().unwrap();
        let _ = t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
        let _ = t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32);
        if r.SetCurrentMediaType(stream, None, &t).is_err() {
            return;
        }
        let cur = r.GetCurrentMediaType(stream).unwrap();
        let size = cur.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
        let (w, h) = ((size >> 32) as i32, (size & 0xFFFF_FFFF) as i32);
        let mut frames: Vec<IMFSample> = Vec::new();
        loop {
            let mut flags = 0u32;
            let mut s = None;
            if r.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut s)).is_err() || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            if let Some(s) = s {
                frames.push(s);
                if frames.len() > 400 {
                    frames.remove(0);
                }
            }
        }
        let n = frames.len();
        let _ = std::fs::write(format!("{out}.txt"), format!("{n} {w}x{h}"));
        let Some(s) = frames.get(n / 2) else { return };
        let Ok(buf) = s.ConvertToContiguousBuffer() else { return };
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        if buf.Lock(&mut p, None, Some(&mut len)).is_err() {
            return;
        }
        let (aw, ah) = ((w + 15) & !15, (h + 15) & !15);
        let pitch = if len as i32 == aw * ah * 4 { aw * 4 } else { (len as i32 / h.max(1)).max(w * 4) } as isize;
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h as isize {
            let row = p.offset(y * pitch) as *const u32;
            px.extend(std::slice::from_raw_parts(row, w as usize).iter().map(|v| v | 0xFF00_0000));
        }
        let _ = std::fs::write(format!("{out}.txt"), format!("{n} {w}x{h} pitch={pitch} len={len}"));
        let _ = buf.Unlock();
        let img = crate::util::Img { w, h, px, alpha: false };
        let _ = std::fs::write(out, crate::gfx::encode_png(&img));
    }
}

#[allow(dead_code)]
fn _guid_used(_: GUID) {}
