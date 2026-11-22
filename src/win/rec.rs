//! Screen-recording session UI: a floating timer/stop pill and a thin red frame
//! around the recorded area. Both are excluded from capture, so they never show
//! up in the video.
use crate::app::app;
use crate::gfx::*;
use crate::media;
use crate::overlay::Dib;
use crate::toast;
use crate::util::*;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::time::Instant;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct Active {
    id: u64,
    start: Instant,
    rect: RECT,
    full: bool,
}

struct Rec {
    ctrl: HWND,
    borders: [HWND; 4],
    s: f32,
    hover_stop: bool,
    blink: bool,
    active: Option<Active>,
    pending: Option<(RECT, bool)>,
}

static R: G<Rec> = G::new();
const W: f32 = 176.0;
const H: f32 = 44.0;
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;

pub unsafe fn init(hinst: HINSTANCE) {
    let cls = wz("ApertureRecCtrl");
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(ctrl_proc),
        hInstance: hinst,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: cls.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassExW(&wc);
    let bcls = wz("ApertureRecBorder");
    let wc2 = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(DefWindowProcW),
        hInstance: hinst,
        hbrBackground: CreateSolidBrush(0x00303BFF),
        lpszClassName: bcls.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassExW(&wc2);
    let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let ctrl = CreateWindowExW(ex, cls.as_ptr(), wz("Aperture Recording").as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), hinst, null());
    SetWindowDisplayAffinity(ctrl, WDA_EXCLUDEFROMCAPTURE);
    let mut borders = [null_mut(); 4];
    for b in borders.iter_mut() {
        *b = CreateWindowExW(ex | WS_EX_TRANSPARENT, bcls.as_ptr(), null(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), hinst, null());
        SetLayeredWindowAttributes(*b, 0, 210, LWA_ALPHA);
        SetWindowDisplayAffinity(*b, WDA_EXCLUDEFROMCAPTURE);
    }
    R.set(Rec { ctrl, borders, s: 1.0, hover_stop: false, blink: true, active: None, pending: None });
}

pub fn active() -> bool {
    R.try_get().is_some_and(|r| r.active.is_some() || r.pending.is_some())
}

/// Queue a recording of `rect` (screen coords). It starts a moment later so the
/// selection overlay is fully gone from the screen first.
pub unsafe fn start(rect: RECT, full: bool) {
    let Some(r) = R.try_get() else { return };
    if r.active.is_some() {
        return;
    }
    r.pending = Some((rect, full));
    SetTimer(r.ctrl, 2, 120, None);
}

unsafe fn begin(rect: RECT, full: bool) {
    let r = R.get();
    let cfg = &app().cfg;
    let _ = std::fs::create_dir_all(&cfg.video_folder);
    let path = PathBuf::from(format!("{}\\Aperture Recording {}.mp4", cfg.video_folder.trim_end_matches('\\'), timestamp()));
    let wr = windows::Win32::Foundation::RECT { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
    match media::start_record(wr, path, cfg.fps, cfg.res) {
        Ok(id) => {
            r.active = Some(Active { id, start: Instant::now(), rect, full });
            show_ui(r);
        }
        Err(e) => toast::show(None, "Couldn't start recording", &e, None),
    }
}

pub unsafe fn stop() {
    let Some(r) = R.try_get() else { return };
    r.pending = None;
    if let Some(a) = r.active.take() {
        media::stop_record(a.id);
        toast::show_icon(crate::icons::Icon::Record, "Finishing recording…", "One moment", None);
    }
    KillTimer(r.ctrl, 1);
    ShowWindow(r.ctrl, SW_HIDE);
    for b in r.borders {
        ShowWindow(b, SW_HIDE);
    }
}

unsafe fn show_ui(r: &mut Rec) {
    let Some(a) = &r.active else { return };
    let rect = a.rect;
    let (_, work, s) = monitor_at((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
    r.s = s;
    if !a.full {
        let t = (2.0 * s).round().max(2.0) as i32;
        let (l, tp, rr, b) = (rect.left - t, rect.top - t, rect.right + t, rect.bottom + t);
        let pos = [(l, tp, rr - l, t), (l, rect.bottom, rr - l, t), (l, rect.top, t, rect.bottom - rect.top), (rect.right, rect.top, t, rect.bottom - rect.top)];
        let _ = b;
        for (h, (x, y, w, hh)) in r.borders.iter().zip(pos) {
            SetWindowPos(*h, HWND_TOPMOST, x, y, w, hh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }
    let (w, h) = ((W * s) as i32, (H * s) as i32);
    let gap = (10.0 * s) as i32;
    let x = ((rect.left + rect.right) / 2 - w / 2).clamp(work.left + gap, work.right - w - gap);
    let mut y = rect.bottom + gap;
    if y + h > work.bottom - gap {
        y = rect.top - h - gap;
        if y < work.top + gap {
            y = rect.bottom - h - gap * 2;
        }
    }
    SetWindowPos(r.ctrl, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
    redraw(r);
    ShowWindow(r.ctrl, SW_SHOWNOACTIVATE);
    SetTimer(r.ctrl, 1, 500, None);
}

unsafe fn redraw(r: &mut Rec) {
    let s = r.s;
    let (w, h) = ((W * s) as i32, (H * s) as i32);
    let Some(dib) = Dib::new(w, h) else { return };
    let secs = r.active.as_ref().map(|a| a.start.elapsed().as_secs()).unwrap_or(0);
    let bmp = bitmap_over(w, h, dib.bits, PF_PARGB);
    {
        let g = Gfx::from_image(bmp);
        g.clear(0);
        g.scale(s);
        let pill = Path::rrect(1.0, 1.0, W - 2.0, H - 2.0, (H - 2.0) / 2.0);
        g.fill_v(&pill, 1.0, H - 1.0, 0xF2303338, 0xF2141518);
        g.stroke(&pill, 0x60000000, 1.0);
        g.stroke(&Path::rrect(2.0, 2.0, W - 4.0, H - 4.0, (H - 4.0) / 2.0), 0x22FFFFFF, 1.0);
        // pulsing record light
        let (cx, cy) = (22.0, H / 2.0);
        if r.blink {
            g.fill_radial(&Path::circle(cx, cy, 10.0), cx, cy, 0x70FF3B30, 0x00FF3B30);
        }
        let dot = Path::circle(cx, cy, 6.0);
        g.fill_v(&dot, cy - 6.0, cy + 6.0, if r.blink { 0xFFFF6B5E } else { 0xFFB8342A }, if r.blink { 0xFFD3261A } else { 0xFF7C1C14 });
        g.fill(&Path::ellipse(cx - 3.5, cy - 5.0, 7.0, 4.0), 0x60FFFFFF);
        let t = format!("{}:{:02}", secs / 60, secs % 60);
        g.text(&t, 38.0, 0.0, 70.0, H, 15.0, 0xFFFFFFFF, SEMI);
        g.text("REC", 84.0, 0.0, 40.0, H, 10.5, 0xFFFF8A80, SEMI);
        // stop button
        let (bx, by, bs) = (W - 40.0, (H - 30.0) / 2.0, 30.0);
        let b = Path::circle(bx + bs / 2.0, by + bs / 2.0, bs / 2.0);
        g.fill_v(&b, by, by + bs, if r.hover_stop { 0xFF5B6069 } else { 0xFF464A52 }, 0xFF26282D);
        g.stroke(&b, 0x80000000, 1.0);
        g.fill(&Path::rrect(bx + 10.0, by + 10.0, 10.0, 10.0, 2.0), 0xFFFFFFFF);
    }
    dispose(bmp);
    let mut rc: RECT = std::mem::zeroed();
    GetWindowRect(r.ctrl, &mut rc);
    let pos = POINT { x: rc.left, y: rc.top };
    let size = SIZE { cx: w, cy: h };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 1 };
    let sdc = GetDC(null_mut());
    UpdateLayeredWindow(r.ctrl, sdc, &pos, &size, dib.dc, &src, 0, &blend, ULW_ALPHA);
    ReleaseDC(null_mut(), sdc);
}

fn on_stop(r: &Rec, x: i32) -> bool {
    x as f32 / r.s > W - 44.0
}

unsafe extern "system" fn ctrl_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(r) = R.try_get() else { return DefWindowProcW(h, m, wp, lp) };
    match m {
        WM_TIMER => {
            if wp == 2 {
                KillTimer(h, 2);
                if let Some((rect, full)) = r.pending.take() {
                    begin(rect, full);
                }
            } else {
                r.blink = !r.blink;
                redraw(r);
            }
            0
        }
        WM_NCHITTEST => {
            let (sx, sy) = lparam_xy(lp);
            let mut pt = POINT { x: sx, y: sy };
            ScreenToClient(h, &mut pt);
            if on_stop(r, pt.x) { HTCLIENT as isize } else { HTCAPTION as isize }
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_MOUSEMOVE => {
            if !r.hover_stop {
                r.hover_stop = true;
                redraw(r);
            }
            0
        }
        WM_NCMOUSEMOVE => {
            if r.hover_stop {
                r.hover_stop = false;
                redraw(r);
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(null_mut(), if r.hover_stop { IDC_HAND } else { IDC_SIZEALL }));
            1
        }
        WM_LBUTTONUP => {
            let (x, _) = lparam_xy(lp);
            if on_stop(r, x) {
                stop();
            }
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}
