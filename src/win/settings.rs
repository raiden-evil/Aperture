//! Settings window (opened from the tray icon): capture tiles, output options,
//! and tabs for clipboard history, shortcuts, recording and theme.
use crate::app::{self, app};
use crate::config;
use crate::gfx::*;
use crate::hook;
use crate::icons::{self, Icon};
use crate::overlay::Dib;
use crate::theme::{self, Pal, pal};
use crate::util::*;
use crate::{clip, store, toast};
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::GdiPlus::GpBitmap;
use windows_sys::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::SHCreateItemFromParsingName;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Close,
    Cap(usize),
    Copy,
    Save,
    Folder,
    OpenFolder,
    Override,
    Autostart,
    Thumb(usize),
    Clear,
    Uninstall,
    Tab(u8),
    Bind(usize, usize),
    ResetKeys,
    Replay,
    ClipMinus,
    ClipPlus,
    ClipEdit,
    FpsMinus,
    FpsPlus,
    FpsEdit,
    Res(u32),
    VideoFolder,
    OpenVideoFolder,
    Theme(u8),
}

/// A number being typed into one of the recording steppers.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Field {
    Clip,
    Fps,
}
struct Edit {
    field: Field,
    text: String,
    /// true until the first key: typing replaces the prefilled value
    fresh: bool,
}

struct Thumb {
    path: PathBuf,
    img: Img,
    bmp: *mut GpBitmap,
}
impl Drop for Thumb {
    fn drop(&mut self) {
        unsafe { dispose(self.bmp) };
    }
}

pub struct Settings {
    hwnd: HWND,
    s: f32,
    hover: Option<Hit>,
    items: Vec<(Hit, [f32; 4])>,
    thumbs: Vec<Thumb>,
    back: Option<Dib>,
    tracking: bool,
    autostart: bool,
    tab: u8,
    recording: Option<(usize, usize)>,
    edit: Option<Edit>,
}

static SET: G<Settings> = G::new();
const CLASS: &str = "ApertureSettings";
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_NCMOUSELEAVE: u32 = 0x02A2;
const W: f32 = 760.0;
const H: f32 = 540.0;

// ---------- shared widgets (theme aware) ----------
pub fn pill(g: &Gfx, x: f32, y: f32, w: f32, h: f32, label: &str, primary: bool, hover: bool) {
    let p = pal();
    let path = Path::rrect(x, y, w, h, h / 2.0);
    g.fill(&Path::rrect(x, y + 1.2, w, h, h / 2.0), if p.dark { 0x40000000 } else { 0x16000000 });
    let tc = if primary {
        let (a, b) = if hover { (0xFF62A8FF, 0xFF2068E6) } else { (0xFF4D96FF, 0xFF1A59CC) };
        g.fill_v(&path, y, y + h, a, b);
        g.stroke(&path, 0x70002070, 1.0);
        g.stroke(&Path::rrect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, h / 2.0 - 1.0), 0x55FFFFFF, 1.0);
        0xFFFFFFFF
    } else {
        g.fill_v(&path, y, y + h, p.btn0, if hover { p.btn_hover1 } else { p.btn1 });
        g.stroke(&path, p.btn_line, 1.0);
        p.btn_text
    };
    g.text(label, x, y, w, h, 12.5, tc, SEMI | CENTER);
}

pub fn toggle(g: &Gfx, x: f32, y: f32, on: bool) {
    let p = pal();
    let (w, h) = (44.0, 24.0);
    let tr = Path::rrect(x, y, w, h, h / 2.0);
    if on {
        g.fill_v(&tr, y, y + h, 0xFF27A145, 0xFF4CD964);
    } else {
        g.fill_v(&tr, y, y + h, p.off0, p.off1);
    }
    g.stroke(&Path::rrect(x + 1.0, y + 1.6, w - 2.0, h - 2.2, h / 2.0 - 1.0), 0x22000000, 1.6);
    g.stroke(&tr, 0x38000000, 1.0);
    let d = h - 4.0;
    let kx = if on { x + w - d - 2.0 } else { x + 2.0 };
    g.fill(&Path::ellipse(kx, y + 3.2, d, d), 0x40000000);
    let knob = Path::ellipse(kx, y + 2.0, d, d);
    g.fill_v(&knob, y + 2.0, y + 2.0 + d, 0xFFFFFFFF, p.knob1);
    g.stroke(&knob, 0x33000000, 0.8);
}

fn card(g: &Gfx, x: f32, y: f32, w: f32, h: f32) {
    let p = pal();
    g.fill(&Path::rrect(x, y + 1.5, w, h, 14.0), p.card_shadow);
    let c = Path::rrect(x, y, w, h, 14.0);
    g.fill(&c, p.card);
    g.stroke(&c, p.card_line, 1.0);
}

fn section(g: &Gfx, x: f32, y: f32, t: &str) {
    g.text(t, x + 2.0, y, 300.0, 16.0, 10.5, pal().text3, SEMI);
}

/// Skeuomorphic keyboard key. state: 0 = set, 1 = empty, 2 = recording.
fn keycap(g: &Gfx, x: f32, y: f32, w: f32, h: f32, label: &str, state: u8, hover: bool) {
    let p = pal();
    match state {
        2 => {
            let r = Path::rrect(x, y, w, h, 7.0);
            g.fill_v(&r, y, y + h, if p.dark { 0xFF1D3357 } else { 0xFFEAF2FF }, if p.dark { 0xFF16294A } else { 0xFFD3E4FF });
            g.stroke(&r, 0xFF2D7FF9, 1.6);
            g.text("Press keys…", x, y, w, h, 10.5, p.link, SEMI | CENTER);
        }
        1 => {
            let r = Path::rrect(x, y, w, h, 7.0);
            g.fill(&r, if hover { p.key_empty_hover } else { p.key_empty });
            g.stroke(&r, p.card_line, 1.0);
            g.text("+ Add", x, y, w, h, 11.0, if hover { p.text2 } else { p.text3 }, CENTER);
        }
        _ => {
            g.fill(&Path::rrect(x, y + 1.0, w, h, 7.0), p.key_side);
            let top = Path::rrect(x, y, w, h - 2.5, 7.0);
            g.fill_v(&top, y, y + h - 2.5, p.key0, if hover { p.key_hover1 } else { p.key1 });
            g.stroke(&Path::rrect(x, y, w, h + 1.0, 7.0), p.key_line, 0.8);
            g.text(label, x + 3.0, y, w - 6.0, h - 2.5, 11.0, p.key_text, SEMI | CENTER);
        }
    }
}

fn segmented(g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, x: f32, y: f32, w: f32, h: f32, labels: &[(&str, Hit)], sel: usize, hv: Option<Hit>) {
    let p = pal();
    let path = Path::rrect(x, y, w, h, h / 2.0);
    g.fill_v(&path, y, y + h, p.seg0, p.seg1);
    g.stroke(&Path::rrect(x + 0.5, y + 1.0, w - 1.0, h - 1.0, h / 2.0 - 0.5), 0x16000000, 1.0);
    let sw = w / labels.len() as f32;
    for (i, (label, hit)) in labels.iter().enumerate() {
        let sx = x + i as f32 * sw;
        if i == sel {
            g.fill(&Path::rrect(sx + 2.0, y + 3.2, sw - 4.0, h - 4.0, (h - 4.0) / 2.0), 0x26000000);
            let k = Path::rrect(sx + 2.0, y + 2.0, sw - 4.0, h - 4.0, (h - 4.0) / 2.0);
            g.fill_v(&k, y + 2.0, y + h - 2.0, p.seg_on0, p.seg_on1);
            g.text(label, sx, y, sw, h, 11.5, p.text, SEMI | CENTER);
        } else {
            g.text(label, sx, y, sw, h, 11.5, if hv == Some(*hit) { p.text } else { p.text2 }, SEMI | CENTER);
            items.push((*hit, [sx, y, sw, h]));
        }
    }
}

/// `− value +` control. Clicking the value turns it into a text field (`edit`).
#[allow(clippy::too_many_arguments)]
fn stepper(g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, right: f32, y: f32, value: &str, edit: Option<&Edit>, hits: (Hit, Hit, Hit), hv: Option<Hit>) {
    let p = pal();
    let (minus_h, val_h, plus_h) = hits;
    let plus = [right - 28.0, y, 28.0, 28.0];
    let val = [plus[0] - 82.0, y, 78.0, 28.0];
    let minus = [val[0] - 32.0, y, 28.0, 28.0];
    pill(g, minus[0], minus[1], minus[2], minus[3], "−", false, hv == Some(minus_h));
    pill(g, plus[0], plus[1], plus[2], plus[3], "+", false, hv == Some(plus_h));
    let field = Path::rrect(val[0], val[1], val[2], val[3], 7.0);
    if let Some(e) = edit {
        g.fill_v(&field, val[1], val[1] + val[3], if p.dark { 0xFF1D3357 } else { 0xFFF4F8FF }, if p.dark { 0xFF16294A } else { 0xFFE6EFFF });
        g.stroke(&field, 0xFF2D7FF9, 1.6);
        let (tw, _) = if e.text.is_empty() { (0.0, 0.0) } else { g.measure_at(&e.text, 12.5, SEMI) };
        let cx = val[0] + val[2] / 2.0;
        if e.fresh && !e.text.is_empty() {
            g.fill(&Path::rrect(cx - tw / 2.0 - 2.0, val[1] + 6.0, tw + 4.0, val[3] - 12.0, 3.0), 0x552D7FF9);
        }
        g.text(&e.text, val[0], val[1], val[2], val[3], 12.5, p.text, SEMI | CENTER);
        let caret = cx + tw / 2.0 + 1.5;
        g.line(caret, val[1] + 7.0, caret, val[1] + val[3] - 7.0, p.link, 1.4);
    } else {
        let on = hv == Some(val_h);
        if on {
            g.fill(&field, p.hover);
            g.stroke(&field, p.btn_line, 1.0);
        }
        g.text(value, val[0], val[1], val[2], val[3], 12.5, p.text, SEMI | CENTER);
    }
    items.push((minus_h, minus));
    items.push((val_h, val));
    items.push((plus_h, plus));
}

/// Miniature of the settings window in a given palette (theme picker).
fn theme_preview(g: &Gfx, p: &Pal, x: f32, y: f32, w: f32, h: f32) {
    let win = Path::rrect(x, y, w, h, 9.0);
    g.fill(&win, p.bg);
    g.clip(&win);
    g.fill_v(&Path::rect(x, y, w, 20.0), y, y + 20.0, p.head0, p.head1);
    icons::camera(g, x + 6.0, y + 4.0, 12.0);
    g.fill(&Path::rrect(x + 22.0, y + 8.0, 30.0, 4.0, 2.0), p.title);
    let c = Path::rrect(x + 6.0, y + 26.0, w - 12.0, h - 32.0, 6.0);
    g.fill(&c, p.card);
    g.stroke(&c, p.card_line, 0.8);
    for (i, on) in [true, false].iter().enumerate() {
        let ry = y + 34.0 + i as f32 * 18.0;
        g.fill(&Path::rrect(x + 12.0, ry + 2.0, w * 0.38, 4.0, 2.0), p.text2);
        let t = Path::rrect(x + w - 32.0, ry, 18.0, 9.0, 4.5);
        if *on {
            g.fill(&t, 0xFF34C759);
        } else {
            g.fill(&t, p.off0);
        }
        g.fill(&Path::circle(if *on { x + w - 18.5 } else { x + w - 27.5 }, ry + 4.5, 3.5), 0xFFFFFFFF);
    }
    g.unclip();
    g.stroke(&win, if p.dark { 0x40FFFFFF } else { 0x30000000 }, 1.0);
}

// ---------- window ----------
pub unsafe fn show() {
    if SET.try_get().is_none() {
        let cls = wz(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: app().hinst,
            hIcon: app().icon,
            hIconSm: app().icon_sm,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: cls.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        SET.set(Settings {
            hwnd: null_mut(),
            s: 1.0,
            hover: None,
            items: Vec::new(),
            thumbs: Vec::new(),
            back: None,
            tracking: false,
            autostart: false,
            tab: 0,
            recording: None,
            edit: None,
        });
    }
    let st = SET.get();
    if !st.hwnd.is_null() {
        ShowWindow(st.hwnd, SW_RESTORE);
        force_foreground(st.hwnd);
        return;
    }
    let c = cursor();
    let (_, work, s) = monitor_at(c.x, c.y);
    let (w, h) = ((W * s) as i32, (H * s) as i32);
    let x = work.left + ((work.right - work.left) - w) / 2;
    let y = (work.top + ((work.bottom - work.top) - h) / 2).max(work.top);
    st.s = s;
    st.autostart = config::autostart();
    st.hover = None;
    st.edit = None;
    let hwnd = CreateWindowExW(
        WS_EX_APPWINDOW,
        wz(CLASS).as_ptr(),
        wz("Aperture").as_ptr(),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        x,
        y,
        w,
        h,
        null_mut(),
        null_mut(),
        app().hinst,
        null(),
    );
    st.hwnd = hwnd;
    round_corners(hwnd);
    theme::apply_window(hwnd);
    SetWindowPos(hwnd, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
    st.load_thumbs();
    ShowWindow(hwnd, SW_SHOW);
    force_foreground(hwnd);
}

pub unsafe fn refresh() {
    if let Some(st) = SET.try_get() {
        if !st.hwnd.is_null() {
            st.load_thumbs();
            InvalidateRect(st.hwnd, null(), 0);
        }
    }
}

/// The keyboard hook captured a combination for the shortcut being edited.
pub unsafe fn on_recorded(packed: usize) {
    let Some(st) = SET.try_get() else { return };
    hook::RECORD.store(false, std::sync::atomic::Ordering::SeqCst);
    let Some((a, s)) = st.recording.take() else { return };
    let b = config::Bind::new(((packed >> 16) & 0xFF) as u8, (packed & 0xFFFF) as u16);
    let cfg = &mut app().cfg;
    let mut changed = false;
    if b.mods == 0 && b.vk == 0x1B {
        // Esc: cancel
    } else if b.mods == 0 && (b.vk == 0x08 || b.vk == 0x2E) {
        cfg.binds[a][s] = config::Bind::NONE;
        changed = true;
    } else if !b.valid() {
        toast::show(None, "Add Ctrl, Alt or Win", &format!("{} on its own would get in the way of typing", b.name()), None);
    } else {
        for slot in cfg.binds.iter_mut().flatten() {
            if *slot == b {
                *slot = config::Bind::NONE; // a shortcut can only do one thing
            }
        }
        cfg.binds[a][s] = b;
        changed = true;
    }
    if changed {
        for row in cfg.binds.iter_mut() {
            if !row[0].is_set() && row[1].is_set() {
                row.swap(0, 1);
            }
        }
        cfg.save();
        config::apply_binds(cfg);
        app::refresh_tray();
    }
    if !st.hwnd.is_null() {
        InvalidateRect(st.hwnd, null(), 0);
    }
}

pub unsafe fn close() {
    if let Some(st) = SET.try_get() {
        if !st.hwnd.is_null() {
            DestroyWindow(st.hwnd);
        }
    }
}

fn key_name(i: usize) -> String {
    app().cfg.binds[i].iter().find(|b| b.is_set()).map(|b| b.name()).unwrap_or_else(|| "No shortcut".into())
}

impl Settings {
    unsafe fn load_thumbs(&mut self) {
        self.thumbs.clear();
        for p in store::list() {
            let tp = store::thumb_of(&p);
            let img = match decode_file(&tp.to_string_lossy()) {
                Some(i) => i,
                None => match decode_file(&p.to_string_lossy()) {
                    Some(i) => i.fit(320, 240),
                    None => continue,
                },
            };
            let mut t = Thumb { path: p, img, bmp: null_mut() };
            t.bmp = bitmap_over(t.img.w, t.img.h, t.img.px.as_mut_ptr(), PF_ARGB);
            self.thumbs.push(t);
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.items.iter().rev().find(|(_, r)| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]).map(|(h, _)| *h)
    }

    unsafe fn stop_recording(&mut self) {
        if self.recording.take().is_some() {
            hook::RECORD.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    unsafe fn paint(&mut self, hdc: HDC) {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut rc);
        let (cw, ch) = (rc.right.max(1), rc.bottom.max(1));
        if self.back.as_ref().is_none_or(|b| b.w != cw || b.h != ch) {
            self.back = Dib::new(cw, ch);
        }
        let Some(back) = &self.back else { return };
        let dc = back.dc;
        let mut items: Vec<(Hit, [f32; 4])> = Vec::new();
        let hv = self.hover;
        let cfg = &app().cfg;
        let p = pal();
        {
            let g = Gfx::from_hdc(dc);
            g.scale(self.s);
            g.rect(0.0, 0.0, W + 2.0, H + 2.0, p.bg);
            g.fill_v(&Path::rect(0.0, 0.0, W + 2.0, 76.0), 0.0, 76.0, p.head0, p.head1);

            // header
            icons::camera(&g, 22.0, 14.0, 46.0);
            g.text("Aperture", 78.0, 16.0, 300.0, 28.0, 22.0, p.title, SEMI);
            g.text("Screenshots at the speed of light", 79.0, 44.0, 300.0, 18.0, 12.5, p.text2, 0);
            let cr = [W - 50.0, 22.0, 28.0, 28.0];
            if hv == Some(Hit::Close) {
                icons::draw(&g, Icon::Close, cr[0], cr[1], cr[2]);
            } else {
                let c = Path::circle(cr[0] + 14.0, cr[1] + 14.0, 11.0);
                g.fill_v(&c, cr[1] + 3.0, cr[1] + 25.0, p.close0, p.close1);
                g.stroke(&c, p.card_line, 1.0);
                g.line(cr[0] + 10.0, cr[1] + 10.0, cr[0] + 18.0, cr[1] + 18.0, p.close_x, 1.6);
                g.line(cr[0] + 18.0, cr[1] + 10.0, cr[0] + 10.0, cr[1] + 18.0, p.close_x, 1.6);
            }
            items.push((Hit::Close, cr));

            // ---- left column ----
            let lx = 22.0;
            let lw = 360.0;
            section(&g, lx, 86.0, "CAPTURE");
            let tw = (lw - 30.0) / 4.0;
            let tiles = [(Icon::Region, "Region", 0), (Icon::Free, "Freeform", 1), (Icon::Full, "Full screen", 2), (Icon::Record, "Record", 4)];
            for (i, (ic, name, bind)) in tiles.iter().enumerate() {
                let x = lx + i as f32 * (tw + 10.0);
                let on = hv == Some(Hit::Cap(i));
                let y = if on { 103.0 } else { 104.0 };
                let th = 86.0;
                g.fill(&Path::rrect(x, y + 2.0, tw, th, 14.0), if on { 0x1A1F6FEB } else { p.card_shadow });
                let path = Path::rrect(x, y, tw, th, 14.0);
                g.fill_v(&path, y, y + th, p.tile0, if on { p.tile_hover1 } else { p.tile1 });
                g.stroke(&path, if on { 0x902D7FF9 } else { p.tile_line }, 1.0);
                g.stroke(&Path::rrect(x + 1.0, y + 1.0, tw - 2.0, th - 2.0, 13.0), p.tile_hi, 1.0);
                icons::draw(&g, *ic, x + (tw - 32.0) / 2.0, y + 11.0, 32.0);
                g.text(name, x, y + 48.0, tw, 18.0, 12.5, p.text, SEMI | CENTER);
                g.text(&key_name(*bind), x + 2.0, y + 66.0, tw - 4.0, 14.0, 10.0, p.text3, CENTER);
                items.push((Hit::Cap(i), [x, 104.0, tw, th]));
            }

            section(&g, lx, 206.0, "OUTPUT");
            let cy = 224.0;
            card(&g, lx, cy, lw, 138.0);
            let rows = [(Hit::Copy, "Copy to clipboard", cfg.copy), (Hit::Save, "Save to folder", cfg.save)];
            for (i, (hit, label, on)) in rows.iter().enumerate() {
                let y = cy + i as f32 * 46.0;
                if hv == Some(*hit) {
                    g.fill(&Path::rrect4(lx + 1.0, y + 1.0, lw - 2.0, 45.0, if i == 0 { 13.0 } else { 0.0 }, if i == 0 { 13.0 } else { 0.0 }, 0.0, 0.0), p.hover);
                }
                g.text(label, lx + 16.0, y, 250.0, 46.0, 13.5, p.text, 0);
                toggle(&g, lx + lw - 60.0, y + 11.0, *on);
                g.line(lx + 16.0, y + 46.0, lx + lw - 16.0, y + 46.0, p.divider, 1.0);
                items.push((*hit, [lx, y, lw, 46.0]));
            }
            let fy = cy + 92.0;
            icons::draw(&g, Icon::Folder, lx + 14.0, fy + 12.0, 22.0);
            g.text(&cfg.folder, lx + 44.0, fy, lw - 44.0 - 138.0, 46.0, 12.0, p.text2, PATH_TRIM);
            let b1 = [lx + lw - 136.0, fy + 9.0, 66.0, 28.0];
            let b2 = [lx + lw - 64.0, fy + 9.0, 50.0, 28.0];
            pill(&g, b1[0], b1[1], b1[2], b1[3], "Change", false, hv == Some(Hit::Folder));
            pill(&g, b2[0], b2[1], b2[2], b2[3], "Open", false, hv == Some(Hit::OpenFolder));
            items.push((Hit::Folder, b1));
            items.push((Hit::OpenFolder, b2));

            section(&g, lx, 378.0, "SHORTCUTS");
            let sy = 396.0;
            card(&g, lx, sy, lw, 104.0);
            let rows2 = [
                (Hit::Override, "Global shortcuts", "Take over Windows' snipping keys · edit in Shortcuts", cfg.override_keys),
                (Hit::Autostart, "Start with Windows", "Waits quietly in the system tray", self.autostart),
            ];
            for (i, (hit, label, sub, on)) in rows2.iter().enumerate() {
                let y = sy + i as f32 * 52.0;
                g.text(label, lx + 16.0, y + 9.0, 280.0, 18.0, 13.5, p.text, 0);
                g.text(sub, lx + 16.0, y + 28.0, 280.0, 16.0, 11.0, p.text3, 0);
                toggle(&g, lx + lw - 60.0, y + 14.0, *on);
                if i == 0 {
                    g.line(lx + 16.0, y + 52.0, lx + lw - 16.0, y + 52.0, p.divider, 1.0);
                }
                items.push((*hit, [lx, y, lw, 52.0]));
            }

            // ---- right column: tabs ----
            let rx = 402.0;
            let rwid = W - rx - 22.0;
            let tabs = [("History", Hit::Tab(0)), ("Shortcuts", Hit::Tab(1)), ("Recording", Hit::Tab(2)), ("Theme", Hit::Tab(3))];
            segmented(&g, &mut items, rx, 80.0, rwid, 26.0, &tabs, self.tab as usize, hv);
            let py = 114.0;
            let ph = 386.0;
            card(&g, rx, py, rwid, ph);
            match self.tab {
                1 => self.paint_keys(&g, &mut items, rx, py, rwid, ph),
                2 => self.paint_recording(&g, &mut items, rx, py, rwid),
                3 => self.paint_theme(&g, &mut items, rx, py, rwid),
                _ => self.paint_history(&g, &mut items, rx, py, rwid, ph),
            }

            // footer
            let un = [lx, H - 30.0, 130.0, 20.0];
            g.text("Uninstall Aperture", un[0], un[1], un[2], un[3], 11.5, if hv == Some(Hit::Uninstall) { 0xFFE0453A } else { p.text3 }, 0);
            items.push((Hit::Uninstall, un));
        }
        self.items = items;
        BitBlt(hdc, 0, 0, cw, ch, dc, 0, 0, SRCCOPY);
    }

    fn paint_history(&self, g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, rx: f32, py: f32, rwid: f32, ph: f32) {
        let p = pal();
        let hv = self.hover;
        let pad = 12.0;
        let gap = 10.0;
        let cwid = (rwid - pad * 2.0 - gap) / 2.0;
        let chgt = (ph - pad * 2.0 - gap * 4.0) / 5.0;
        for i in 0..store::MAX {
            let (col, row) = (i % 2, i / 2);
            let x = rx + pad + col as f32 * (cwid + gap);
            let y = py + pad + row as f32 * (chgt + gap);
            let cell = Path::rrect(x, y, cwid, chgt, 10.0);
            if let Some(t) = self.thumbs.get(i) {
                let on = hv == Some(Hit::Thumb(i));
                g.fill(&cell, p.cell);
                g.clip(&cell);
                g.image_cover(t.bmp, t.img.w as f32, t.img.h as f32, x, y, cwid, chgt);
                if on {
                    g.rect(x, y, cwid, chgt, 0x401F6FEB);
                }
                g.unclip();
                g.stroke(&cell, if on { 0xFF2D7FF9 } else { p.card_line }, if on { 2.0 } else { 1.0 });
                if on {
                    pill(g, x + cwid / 2.0 - 30.0, y + chgt / 2.0 - 12.0, 60.0, 24.0, "Copy", true, false);
                }
                items.push((Hit::Thumb(i), [x, y, cwid, chgt]));
            } else {
                g.fill(&cell, p.cell);
                g.stroke(&cell, p.divider, 1.0);
            }
        }
        if self.thumbs.is_empty() {
            g.fill(&Path::rrect(rx + 40.0, py + ph / 2.0 - 34.0, rwid - 80.0, 68.0, 14.0), (p.card & 0x00FF_FFFF) | 0xF000_0000);
            g.text("No captures yet", rx, py + ph / 2.0 - 26.0, rwid, 22.0, 14.0, p.text, SEMI | CENTER);
            g.text("Your last 10 screenshots will appear here", rx, py + ph / 2.0 - 2.0, rwid, 18.0, 11.5, p.text3, CENTER);
        } else {
            let clr = [rx + rwid - 60.0, H - 30.0, 60.0, 20.0];
            g.text("Clear all", clr[0], clr[1], clr[2], clr[3], 11.5, if hv == Some(Hit::Clear) { p.link_hover } else { p.link }, SEMI | RIGHT);
            items.push((Hit::Clear, clr));
        }
        g.text("Click a capture to copy it again  ·  right-click to open", rx, H - 30.0, rwid - 70.0, 20.0, 11.0, p.text3, 0);
    }

    fn paint_keys(&self, g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, rx: f32, py: f32, rwid: f32, ph: f32) {
        let cfg = &app().cfg;
        let p = pal();
        let hv = self.hover;
        let ics = [Icon::Region, Icon::Free, Icon::Full, Icon::Copy, Icon::Record, Icon::Undo];
        let clip_sub = format!("Last {}", config::fmt_secs(cfg.replay_secs));
        let subs = ["Drag a box", "Draw any shape", "One monitor", "All screens, instantly", "Start / stop", clip_sub.as_str()];
        let (kw, kh) = (80.0, 28.0);
        let name_w = rwid - 46.0 - 14.0 - kw * 2.0 - 6.0 - 8.0;
        for a in 0..6 {
            let y = py + 6.0 + a as f32 * 46.0;
            icons::draw(g, ics[a], rx + 14.0, y + 12.0, 22.0);
            g.text(config::ACTION_NAMES[a], rx + 46.0, y + 6.0, name_w, 18.0, 12.5, p.text, SEMI);
            g.text(subs[a], rx + 46.0, y + 24.0, name_w, 15.0, 10.5, p.text3, 0);
            for s in 0..2 {
                let x = rx + rwid - 14.0 - (2 - s) as f32 * kw - (1 - s) as f32 * 6.0;
                let b = cfg.binds[a][s];
                let state = if self.recording == Some((a, s)) {
                    2
                } else if b.is_set() {
                    0
                } else {
                    1
                };
                keycap(g, x, y + 9.0, kw, kh, &b.name(), state, hv == Some(Hit::Bind(a, s)));
                items.push((Hit::Bind(a, s), [x, y + 9.0, kw, kh]));
            }
            if a < 5 {
                g.line(rx + 14.0, y + 46.0, rx + rwid - 14.0, y + 46.0, p.divider, 1.0);
            }
        }
        let ty = py + 6.0 + 6.0 * 46.0 + 6.0;
        if cfg.override_keys {
            g.text(
                "Click a key, then press the shortcut you want. Backspace removes it, Esc cancels. Letters need Ctrl, Alt or Win; PrtSc and F-keys work alone.",
                rx + 16.0,
                ty,
                rwid - 32.0,
                48.0,
                10.5,
                p.text3,
                WRAP,
            );
        } else {
            g.text("Global shortcuts are turned off. Switch them on under Shortcuts on the left.", rx + 16.0, ty, rwid - 32.0, 48.0, 11.0, p.warn, WRAP | SEMI);
        }
        let rr = [rx + rwid / 2.0 - 70.0, py + ph - 40.0, 140.0, 28.0];
        pill(g, rr[0], rr[1], rr[2], rr[3], "Reset to defaults", false, hv == Some(Hit::ResetKeys));
        items.push((Hit::ResetKeys, rr));
    }

    fn paint_recording(&self, g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, rx: f32, py: f32, rwid: f32) {
        let cfg = &app().cfg;
        let p = pal();
        let hv = self.hover;
        let right = rx + rwid - 14.0;
        // replay toggle
        let y = py + 8.0;
        if hv == Some(Hit::Replay) {
            g.fill(&Path::rrect4(rx + 1.0, py + 1.0, rwid - 2.0, 62.0, 13.0, 13.0, 0.0, 0.0), p.hover);
        }
        icons::draw(g, Icon::Record, rx + 14.0, y + 14.0, 24.0);
        g.text("Replay clips", rx + 48.0, y + 8.0, 220.0, 18.0, 13.5, p.text, SEMI);
        g.text(&format!("Keeps the last {} ready to save", config::fmt_secs(cfg.replay_secs)), rx + 48.0, y + 28.0, 220.0, 16.0, 11.0, p.text3, 0);
        toggle(g, right - 44.0, y + 16.0, cfg.replay);
        items.push((Hit::Replay, [rx, py, rwid, 64.0]));
        g.line(rx + 14.0, py + 64.0, right, py + 64.0, p.divider, 1.0);

        // clip length stepper
        let edit = self.edit.as_ref();
        let y = py + 64.0;
        g.text("Clip length", rx + 16.0, y, 160.0, 48.0, 13.0, p.text, 0);
        let clip_edit = edit.filter(|e| e.field == Field::Clip);
        stepper(g, items, right, y + 10.0, &config::fmt_secs(cfg.replay_secs), clip_edit, (Hit::ClipMinus, Hit::ClipEdit, Hit::ClipPlus), hv);
        if clip_edit.is_some() {
            g.text("e.g. 90, 2m, 1:30", rx + 16.0, y + 30.0, 120.0, 14.0, 10.0, p.text3, 0);
        }
        g.line(rx + 14.0, y + 48.0, right, y + 48.0, p.divider, 1.0);

        // frame rate stepper
        let y = py + 112.0;
        g.text("Frame rate", rx + 16.0, y, 160.0, 48.0, 13.0, p.text, 0);
        let fps_edit = edit.filter(|e| e.field == Field::Fps);
        stepper(g, items, right, y + 10.0, &format!("{} fps", cfg.fps), fps_edit, (Hit::FpsMinus, Hit::FpsEdit, Hit::FpsPlus), hv);
        if fps_edit.is_some() {
            g.text(&format!("{} to {}", config::FPS_MIN, config::FPS_MAX), rx + 16.0, y + 30.0, 120.0, 14.0, 10.0, p.text3, 0);
        }
        g.line(rx + 14.0, y + 48.0, right, y + 48.0, p.divider, 1.0);

        // resolution
        let y = py + 160.0;
        g.text("Resolution", rx + 16.0, y, 100.0, 48.0, 13.0, p.text, 0);
        let labels: Vec<(String, Hit)> = config::RES_STEPS.iter().map(|r| (if *r == 0 { "Native".to_string() } else { format!("{r}p") }, Hit::Res(*r))).collect();
        let refs: Vec<(&str, Hit)> = labels.iter().map(|(l, h)| (l.as_str(), *h)).collect();
        let sel = config::RES_STEPS.iter().position(|r| *r == cfg.res).unwrap_or(usize::MAX);
        let sw = 226.0;
        segmented(g, items, right - sw, y + 11.0, sw, 26.0, &refs, sel, hv);
        g.line(rx + 14.0, y + 48.0, right, y + 48.0, p.divider, 1.0);

        // video folder
        let y = py + 208.0;
        g.text("Save videos to", rx + 16.0, y + 8.0, 180.0, 18.0, 13.0, p.text, 0);
        g.text(&cfg.video_folder, rx + 16.0, y + 30.0, rwid - 32.0 - 130.0, 18.0, 11.5, p.text2, PATH_TRIM);
        let b1 = [right - 122.0, y + 14.0, 66.0, 28.0];
        let b2 = [right - 50.0, y + 14.0, 50.0, 28.0];
        pill(g, b1[0], b1[1], b1[2], b1[3], "Change", false, hv == Some(Hit::VideoFolder));
        pill(g, b2[0], b2[1], b2[2], b2[3], "Open", false, hv == Some(Hit::OpenVideoFolder));
        items.push((Hit::VideoFolder, b1));
        items.push((Hit::OpenVideoFolder, b2));
        g.line(rx + 14.0, y + 60.0, right, y + 60.0, p.divider, 1.0);

        // info
        let y = py + 280.0;
        let rec = cfg.binds[4].iter().find(|b| b.is_set()).map(|b| b.name()).unwrap_or_else(|| "tray menu".into());
        let clip = cfg.binds[5].iter().find(|b| b.is_set()).map(|b| b.name()).unwrap_or_else(|| "tray menu".into());
        g.text(&format!("Record: {rec}   ·   Save clip: {clip}"), rx + 16.0, y, rwid - 32.0, 18.0, 11.5, p.text, SEMI);
        g.text(
            "Uses your graphics card's video encoder. HDR screens are converted so videos look like what you see. While Replay clips is off, nothing runs in the background.",
            rx + 16.0,
            y + 22.0,
            rwid - 32.0,
            50.0,
            10.5,
            p.text3,
            WRAP,
        );
        if cfg.replay {
            let r = crate::media::primary_rect();
            let (w, h) = crate::media::scaled_size((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32, cfg.res);
            let mb = crate::media::bitrate(w, h, cfg.fps) as u64 * (cfg.replay_secs as u64 + 8) / 8 / 1_000_000;
            g.text(&format!("Buffer: about {mb} MB of temporary disk space, no extra memory."), rx + 16.0, y + 74.0, rwid - 32.0, 18.0, 10.5, p.text2, 0);
        }
    }

    fn paint_theme(&self, g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, rx: f32, py: f32, rwid: f32) {
        let p = pal();
        let hv = self.hover;
        let gap = 10.0;
        let cw = (rwid - 28.0 - gap * 2.0) / 3.0;
        for t in 0..3u8 {
            let x = rx + 14.0 + t as f32 * (cw + gap);
            let y = py + 14.0;
            let sel = theme::current() == t;
            let on = hv == Some(Hit::Theme(t));
            let frame = Path::rrect(x, y, cw, 148.0, 12.0);
            g.fill(&frame, if on && !sel { p.hover } else { 0 });
            theme_preview(g, theme::get(t), x + 8.0, y + 8.0, cw - 16.0, 96.0);
            g.text(theme::NAMES[t as usize], x, y + 110.0, cw, 18.0, 12.5, p.text, SEMI | CENTER);
            let sub = ["Default", "Easy on the eyes", "Pure black · OLED"][t as usize];
            g.text(sub, x, y + 128.0, cw, 14.0, 10.0, p.text3, CENTER);
            if sel {
                g.stroke(&frame, 0xFF2D7FF9, 2.0);
                let (bx, by) = (x + cw - 12.0, y + 12.0);
                let badge = Path::circle(bx, by, 9.0);
                g.fill_v(&badge, by - 9.0, by + 9.0, 0xFF5AA2FF, 0xFF1A59CC);
                g.polyline(&[(bx - 4.0, by), (bx - 1.0, by + 3.0), (bx + 4.0, by - 3.0)], 0xFFFFFFFF, 1.8);
            } else {
                g.stroke(&frame, if on { p.btn_line } else { p.divider }, 1.0);
            }
            items.push((Hit::Theme(t), [x, y, cw, 148.0]));
        }
        g.text(
            "Applies to this window, the capture toolbar and notifications. Charcoal uses true black, so OLED pixels switch off.",
            rx + 16.0,
            py + 178.0,
            rwid - 32.0,
            40.0,
            11.0,
            p.text3,
            WRAP,
        );
    }

    /// Apply (or, if it doesn't parse, drop) the number being typed.
    unsafe fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let a = app();
        match e.field {
            Field::Clip => {
                if let Some(v) = config::parse_secs(&e.text) {
                    set_clip(v.clamp(config::CLIP_MIN, config::CLIP_MAX));
                }
            }
            Field::Fps => {
                let t = e.text.trim().to_ascii_lowercase();
                if let Ok(v) = t.trim_end_matches("fps").trim().parse::<f32>() {
                    set_fps((v.round() as u32).clamp(config::FPS_MIN, config::FPS_MAX));
                }
            }
        }
        a.cfg.save();
        InvalidateRect(self.hwnd, null(), 0);
    }

    unsafe fn on_char(&mut self, ch: u32) {
        let Some(e) = &mut self.edit else { return };
        match ch {
            8 => {
                if e.fresh {
                    e.text.clear();
                } else {
                    e.text.pop();
                }
                e.fresh = false;
            }
            13 => return self.commit_edit(),
            27 => self.edit = None,
            c => {
                let Some(c) = char::from_u32(c) else { return };
                let ok = match e.field {
                    Field::Clip => c.is_ascii_digit() || matches!(c, ':' | '.' | ' ' | 'm' | 'M' | 's' | 'S' | 'i' | 'n' | 'e' | 'c'),
                    Field::Fps => c.is_ascii_digit() || c == '.',
                };
                if !ok {
                    return;
                }
                if e.fresh {
                    e.text.clear();
                    e.fresh = false;
                }
                if e.text.chars().count() < 8 {
                    e.text.push(c);
                }
            }
        }
        InvalidateRect(self.hwnd, null(), 0);
    }

    unsafe fn click(&mut self, hit: Hit) {
        let a = app();
        let was = self.recording;
        self.stop_recording();
        let editing = self.edit.as_ref().map(|e| e.field);
        let own = matches!((editing, hit), (Some(Field::Clip), Hit::ClipEdit) | (Some(Field::Fps), Hit::FpsEdit));
        if !own {
            self.commit_edit();
        }
        match hit {
            Hit::Tab(t) => self.tab = t,
            Hit::Bind(i, s) => {
                if was != Some((i, s)) {
                    self.recording = Some((i, s));
                    hook::RECORD.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            Hit::ResetKeys => {
                a.cfg.binds = config::DEFAULT_BINDS;
                config::apply_binds(&a.cfg);
                app::refresh_tray();
            }
            Hit::Close => {
                DestroyWindow(self.hwnd);
                return;
            }
            Hit::Cap(i) => {
                DestroyWindow(self.hwnd);
                app::delayed_action([1, 2, 3, 5][i]);
                return;
            }
            Hit::Copy => a.cfg.copy = !a.cfg.copy,
            Hit::Save => a.cfg.save = !a.cfg.save,
            Hit::Folder => {
                if let Some(f) = pick_folder(self.hwnd, &a.cfg.folder, "Choose where Aperture saves screenshots") {
                    a.cfg.folder = f;
                    a.cfg.save = true;
                }
            }
            Hit::OpenFolder => {
                let _ = std::fs::create_dir_all(&a.cfg.folder);
                open_path(&a.cfg.folder);
            }
            Hit::VideoFolder => {
                if let Some(f) = pick_folder(self.hwnd, &a.cfg.video_folder, "Choose where Aperture saves videos") {
                    a.cfg.video_folder = f;
                }
            }
            Hit::OpenVideoFolder => {
                let _ = std::fs::create_dir_all(&a.cfg.video_folder);
                open_path(&a.cfg.video_folder);
            }
            Hit::Override => {
                a.cfg.override_keys = !a.cfg.override_keys;
                config::apply_binds(&a.cfg);
            }
            Hit::Autostart => {
                self.autostart = !self.autostart;
                config::set_autostart(self.autostart);
            }
            Hit::Replay => {
                a.cfg.replay = !a.cfg.replay;
                a.cfg.save();
                app::replay_apply();
                app::refresh_tray();
            }
            Hit::ClipMinus | Hit::ClipPlus => set_clip(config::step(&config::CLIP_STEPS, a.cfg.replay_secs, hit == Hit::ClipPlus)),
            Hit::FpsMinus | Hit::FpsPlus => set_fps(config::step(&config::FPS_STEPS, a.cfg.fps, hit == Hit::FpsPlus)),
            Hit::ClipEdit | Hit::FpsEdit => {
                if !own {
                    let (field, text) = if hit == Hit::ClipEdit { (Field::Clip, a.cfg.replay_secs.to_string()) } else { (Field::Fps, a.cfg.fps.to_string()) };
                    self.edit = Some(Edit { field, text, fresh: true });
                }
            }
            Hit::Res(r) => {
                if a.cfg.res != r {
                    a.cfg.res = r;
                    if a.cfg.replay {
                        crate::media::stop_replay();
                        app::replay_apply();
                    }
                }
            }
            Hit::Theme(t) => {
                a.cfg.theme = t;
                theme::set(t);
                theme::apply_window(self.hwnd);
            }
            Hit::Thumb(i) => {
                if let Some(t) = self.thumbs.get(i) {
                    let p = t.path.to_string_lossy().into_owned();
                    if let Some(img) = decode_file(&p) {
                        let bytes = std::fs::read(&t.path).ok();
                        clip::set_image(a.main, &img, bytes.as_deref());
                        toast::show(Some(&img), "Copied from history", &format!("{} × {}  ·  Paste with Ctrl+V", img.w, img.h), Some(p));
                    }
                }
            }
            Hit::Clear => {
                self.thumbs.clear();
                store::clear();
            }
            Hit::Uninstall => {
                let _ = std::process::Command::new(exe_path()).arg("--uninstall").spawn();
            }
        }
        a.cfg.save();
        InvalidateRect(self.hwnd, null(), 0);
    }
}

unsafe fn set_clip(v: u32) {
    let a = app();
    if a.cfg.replay_secs != v {
        a.cfg.replay_secs = v;
        if a.cfg.replay {
            app::replay_apply();
        }
    }
}

unsafe fn set_fps(f: u32) {
    let a = app();
    if a.cfg.fps != f {
        a.cfg.fps = f;
        if a.cfg.replay {
            crate::media::stop_replay();
            app::replay_apply();
        }
    }
}

type FnU32 = unsafe extern "system" fn(*mut c_void, u32) -> i32;

unsafe fn pick_folder(owner: HWND, start: &str, title: &str) -> Option<String> {
    let clsid = GUID::from_u128(0xDC1C5A9C_E88A_4DDE_A5A1_60F82A20AEF7);
    let iid = GUID::from_u128(0xD57C7288_D4AD_4768_BE02_9D969532D960);
    let iid_item = GUID::from_u128(0x43826D1E_E718_42EE_BC55_A1E261C37BFE);
    let mut dlg: *mut c_void = null_mut();
    if CoCreateInstance(&clsid, null_mut(), CLSCTX_INPROC_SERVER, &iid, &mut dlg) < 0 || dlg.is_null() {
        return None;
    }
    let vt = *(dlg as *const *const usize);
    let show: unsafe extern "system" fn(*mut c_void, HWND) -> i32 = std::mem::transmute(*vt.add(3));
    let set_options: FnU32 = std::mem::transmute(*vt.add(9));
    let get_options: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 = std::mem::transmute(*vt.add(10));
    let set_folder: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 = std::mem::transmute(*vt.add(12));
    let set_title: unsafe extern "system" fn(*mut c_void, *const u16) -> i32 = std::mem::transmute(*vt.add(17));
    let get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 = std::mem::transmute(*vt.add(20));
    let mut o = 0u32;
    get_options(dlg, &mut o);
    set_options(dlg, o | 0x20 | 0x40); // FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM
    set_title(dlg, wz(title).as_ptr());
    let _ = std::fs::create_dir_all(start);
    let mut si: *mut c_void = null_mut();
    if SHCreateItemFromParsingName(wz(start).as_ptr(), null_mut(), &iid_item, &mut si) >= 0 && !si.is_null() {
        set_folder(dlg, si);
        com_release(si);
    }
    let mut out = None;
    if show(dlg, owner) >= 0 {
        let mut item: *mut c_void = null_mut();
        if get_result(dlg, &mut item) >= 0 && !item.is_null() {
            let ivt = *(item as *const *const usize);
            let get_name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> i32 = std::mem::transmute(*ivt.add(5));
            let mut p: *mut u16 = null_mut();
            if get_name(item, 0x8005_8000, &mut p) >= 0 && !p.is_null() {
                out = Some(from_wide(p));
                CoTaskMemFree(p as *const c_void);
            }
            com_release(item);
        }
    }
    com_release(dlg);
    out
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_NCCALCSIZE if wp == 1 => return 0,
        WM_NCACTIVATE => return DefWindowProcW(h, m, wp, -1),
        _ => {}
    }
    let Some(st) = SET.try_get() else { return DefWindowProcW(h, m, wp, lp) };
    if st.hwnd != h {
        return DefWindowProcW(h, m, wp, lp);
    }
    let s = st.s;
    match m {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(h, &mut ps);
            st.paint(hdc);
            EndPaint(h, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_NCHITTEST => {
            let (sx, sy) = lparam_xy(lp);
            let mut pt = POINT { x: sx, y: sy };
            ScreenToClient(h, &mut pt);
            let (x, y) = (pt.x as f32 / s, pt.y as f32 / s);
            if y < 76.0 && st.hit(x, y).is_none() { HTCAPTION as isize } else { HTCLIENT as isize }
        }
        WM_MOUSEMOVE => {
            if !st.tracking {
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                TrackMouseEvent(&mut t);
                st.tracking = true;
            }
            let (x, y) = lparam_xy(lp);
            let hv = st.hit(x as f32 / s, y as f32 / s);
            if hv != st.hover {
                st.hover = hv;
                InvalidateRect(h, null(), 0);
            }
            0
        }
        // Leaving a button into the draggable header (non-client) area or out of the
        // window must clear the hover state too.
        WM_MOUSELEAVE | WM_NCMOUSEMOVE | WM_NCMOUSELEAVE => {
            st.tracking = false;
            if st.hover.is_some() {
                st.hover = None;
                InvalidateRect(h, null(), 0);
            }
            if m == WM_NCMOUSEMOVE { DefWindowProcW(h, m, wp, lp) } else { 0 }
        }
        WM_SETCURSOR => {
            if (lp & 0xFFFF) as u32 == HTCLIENT {
                SetCursor(LoadCursorW(null_mut(), if st.hover.is_some() { IDC_HAND } else { IDC_ARROW }));
                return 1;
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            if let Some(hit) = st.hit(x as f32 / s, y as f32 / s) {
                st.click(hit);
            } else {
                st.commit_edit();
            }
            0
        }
        WM_CHAR => {
            st.on_char(wp as u32);
            0
        }
        WM_RBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            if let Some(Hit::Thumb(i)) = st.hit(x as f32 / s, y as f32 / s) {
                if let Some(t) = st.thumbs.get(i) {
                    open_path(&t.path.to_string_lossy());
                }
            }
            0
        }
        WM_KEYDOWN => {
            if st.edit.is_some() {
                return 0; // Enter / Esc / text arrive as WM_CHAR
            }
            if wp == 0x1B {
                DestroyWindow(h);
            }
            0
        }
        WM_DPICHANGED => {
            st.s = ((wp >> 16) & 0xFFFF) as f32 / 96.0;
            let r = &*(lp as *const RECT);
            SetWindowPos(h, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            InvalidateRect(h, null(), 0);
            0
        }
        WM_ACTIVATE => {
            if wp & 0xFFFF == 0 && st.recording.is_some() {
                st.stop_recording();
                InvalidateRect(h, null(), 0);
            }
            if wp & 0xFFFF == 0 {
                st.commit_edit();
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_DESTROY => {
            st.edit = None;
            st.stop_recording();
            st.hwnd = null_mut();
            st.thumbs.clear();
            st.back = None;
            st.items.clear();
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}
