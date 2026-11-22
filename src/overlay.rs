//! Fullscreen capture overlay: region / freeform (lasso) / full-screen selection
//! plus the annotation toolbar (text, blur, black bar, arrow, line).
use crate::app::{self, Out};
use crate::gfx::*;
use crate::icons::{self, Icon};
use crate::util::*;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::GdiPlus::GdipFlush;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub struct Dib {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    pub bits: *mut u32,
    pub w: i32,
    pub h: i32,
}
impl Dib {
    pub unsafe fn new(w: i32, h: i32) -> Option<Dib> {
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let sdc = GetDC(null_mut());
        let mut bits = null_mut();
        let bmp = CreateDIBSection(sdc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let dc = CreateCompatibleDC(sdc);
        ReleaseDC(null_mut(), sdc);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old = SelectObject(dc, bmp);
        Some(Dib { dc, bmp, old, bits: bits as *mut u32, w, h })
    }
    #[allow(clippy::mut_from_ref)]
    pub fn px(&self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) }
    }
}
impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Region,
    Free,
    Full,
}
#[derive(Clone, Copy, PartialEq, Debug)]
enum Tool {
    None,
    Text,
    Blur,
    Censor,
    Arrow,
    Line,
    Pen,
}
#[derive(Clone, Copy, PartialEq, Debug)]
enum Btn {
    Mode(Mode),
    Tool(Tool),
    Color,
    Undo,
    Save,
    Copy,
    Close,
    Record,
    Sep,
}
#[derive(Clone)]
enum Ann {
    Line { a: (f32, f32), b: (f32, f32), c: u32, w: f32, arrow: bool },
    Stroke { pts: Vec<(f32, f32)>, c: u32, w: f32 },
    Blur(RECT),
    Censor(RECT),
    Text { x: f32, y: f32, s: String, c: u32, size: f32 },
}
#[derive(PartialEq)]
enum Drag {
    None,
    Select,
    Ann,
}
struct TextEdit {
    x: f32,
    y: f32,
    s: String,
}

const PALETTE: [u32; 6] = [0xFFFF3B30, 0xFFFFCC00, 0xFF34C759, 0xFF0A84FF, 0xFFFFFFFF, 0xFF111111];

pub struct Overlay {
    pub hwnd: HWND,
    shot: Option<Dib>,
    dim: Option<Dib>,
    work: Option<Dib>,
    back: Option<Dib>,
    pub visible: bool,
    vx: i32,
    vy: i32,
    w: i32,
    h: i32,
    mode: Mode,
    editing: bool,
    drag: Drag,
    start: (i32, i32),
    cur: (i32, i32),
    sel: RECT,
    lasso: Vec<POINT>,
    tool: Tool,
    color: usize,
    anns: Vec<Ann>,
    pending: Option<Ann>,
    text: Option<TextEdit>,
    hover: Option<Btn>,
    btns: Vec<(Btn, RECT)>,
    bar: RECT,
    mon: RECT,
    s: f32,
    record: bool,
}

pub static OV: G<Overlay> = G::new();
const CLASS: &str = "ApertureOverlay";

pub unsafe fn init(hinst: HINSTANCE) {
    let cls = wz(CLASS);
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        lpszClassName: cls.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassExW(&wc);
    let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, cls.as_ptr(), wz("Aperture Capture").as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), hinst, null());
    let off: u32 = 1;
    DwmSetWindowAttribute(hwnd, 3 /* DWMWA_TRANSITIONS_FORCEDISABLED */, &off as *const u32 as *const _, 4);
    OV.set(Overlay {
        hwnd,
        shot: None,
        dim: None,
        work: None,
        back: None,
        visible: false,
        vx: 0,
        vy: 0,
        w: 0,
        h: 0,
        mode: Mode::Region,
        editing: false,
        drag: Drag::None,
        start: (0, 0),
        cur: (0, 0),
        sel: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        lasso: Vec::new(),
        tool: Tool::None,
        color: 0,
        anns: Vec::new(),
        pending: None,
        text: None,
        hover: None,
        btns: Vec::new(),
        bar: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        mon: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        s: 1.0,
        record: false,
    });
}

pub unsafe fn open(mode: Mode, record: bool) {
    OV.get().open(mode, record)
}

fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

unsafe fn grab(d: &Dib, vx: i32, vy: i32) {
    let sdc = GetDC(null_mut());
    BitBlt(d.dc, 0, 0, d.w, d.h, sdc, vx, vy, SRCCOPY | CAPTUREBLT);
    ReleaseDC(null_mut(), sdc);
    GdiFlush();
}

/// Instant capture of every monitor, no UI.
pub unsafe fn capture_virtual() -> Option<Img> {
    let (vx, vy, w, h) = virtual_screen();
    let d = Dib::new(w, h)?;
    grab(&d, vx, vy);
    crate::media::patch_hdr(vx, vy, w, h, d.px());
    let px: Vec<u32> = d.px().iter().map(|p| p | 0xFF00_0000).collect();
    Some(Img { w, h, px, alpha: false })
}

fn norm(a: (i32, i32), b: (i32, i32)) -> RECT {
    RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0) + 1, bottom: a.1.max(b.1) + 1 }
}
fn inter(a: &RECT, b: &RECT) -> RECT {
    RECT { left: a.left.max(b.left), top: a.top.max(b.top), right: a.right.min(b.right), bottom: a.bottom.min(b.bottom) }
}
fn rw(r: &RECT) -> i32 {
    r.right - r.left
}
fn rh(r: &RECT) -> i32 {
    r.bottom - r.top
}

fn box_blur(px: &mut [u32], stride: usize, r: &RECT, rad: usize) {
    let (x0, y0) = (r.left as usize, r.top as usize);
    let (w, h) = (rw(r).max(0) as usize, rh(r).max(0) as usize);
    if w < 2 || h < 2 {
        return;
    }
    let n = w.max(h);
    let mut src = vec![0u32; n];
    let mut dst = vec![0u32; n];
    let blur = |src: &[u32], dst: &mut [u32]| {
        let len = src.len();
        let r = rad.min(len - 1) as isize;
        let div = (2 * r + 1) as u32;
        let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
        let at = |i: isize| src[i.clamp(0, len as isize - 1) as usize];
        for i in -r..=r {
            let p = at(i);
            sr += (p >> 16) & 255;
            sg += (p >> 8) & 255;
            sb += p & 255;
        }
        for i in 0..len as isize {
            dst[i as usize] = 0xFF00_0000 | ((sr / div) << 16) | ((sg / div) << 8) | (sb / div);
            let (a, d) = (at(i + r + 1), at(i - r));
            sr = sr + ((a >> 16) & 255) - ((d >> 16) & 255);
            sg = sg + ((a >> 8) & 255) - ((d >> 8) & 255);
            sb = sb + (a & 255) - (d & 255);
        }
    };
    for _ in 0..3 {
        for y in 0..h {
            let o = (y0 + y) * stride + x0;
            src[..w].copy_from_slice(&px[o..o + w]);
            blur(&src[..w], &mut dst[..w]);
            px[o..o + w].copy_from_slice(&dst[..w]);
        }
        for x in 0..w {
            for y in 0..h {
                src[y] = px[(y0 + y) * stride + x0 + x];
            }
            blur(&src[..h], &mut dst[..h]);
            for y in 0..h {
                px[(y0 + y) * stride + x0 + x] = dst[y];
            }
        }
    }
}

fn draw_text_ann(g: &Gfx, x: f32, y: f32, s: &str, c: u32, size: f32) {
    let lum = ((c >> 16) & 255) * 3 + ((c >> 8) & 255) * 6 + (c & 255);
    let shadow = if lum > 1100 { 0xA0000000 } else { 0xB0FFFFFF };
    let o = (size / 18.0).max(1.0);
    g.text_at(s, x + o, y + o, size, shadow, SEMI);
    g.text_at(s, x, y, size, c, SEMI);
}

fn tip(b: Btn) -> &'static str {
    match b {
        Btn::Mode(Mode::Region) => "Region   R",
        Btn::Mode(Mode::Free) => "Freeform   F",
        Btn::Mode(Mode::Full) => "Full screen   W",
        Btn::Tool(Tool::Text) => "Text   T",
        Btn::Tool(Tool::Blur) => "Blur   B",
        Btn::Tool(Tool::Censor) => "Black bar   X",
        Btn::Tool(Tool::Arrow) => "Arrow   A",
        Btn::Tool(Tool::Line) => "Line   L",
        Btn::Tool(Tool::Pen) => "Pencil   P",
        Btn::Color => "Color   C",
        Btn::Undo => "Undo   Ctrl+Z",
        Btn::Save => "Save to folder   Ctrl+S",
        Btn::Copy => "Copy   Enter",
        Btn::Close => "Close   Esc",
        Btn::Record => "Record video   V",
        _ => "",
    }
}

impl Overlay {
    unsafe fn open(&mut self, mode: Mode, record: bool) {
        if self.visible {
            return;
        }
        let (vx, vy, w, h) = virtual_screen();
        let Some(shot) = Dib::new(w, h) else { return };
        grab(&shot, vx, vy);
        crate::media::patch_hdr(vx, vy, w, h, shot.px());
        self.record = record;
        let mode = if record && mode == Mode::Free { Mode::Region } else { mode };
        let (Some(dim), Some(work), Some(back)) = (Dib::new(w, h), Dib::new(w, h), Dib::new(w, h)) else { return };
        for (d, s) in dim.px().iter_mut().zip(shot.px().iter()) {
            *d = ((s >> 1) & 0x007F_7F7F) | 0xFF00_0000;
        }
        (self.vx, self.vy, self.w, self.h) = (vx, vy, w, h);
        self.shot = Some(shot);
        self.dim = Some(dim);
        self.work = Some(work);
        self.back = Some(back);
        self.mode = mode;
        self.editing = false;
        self.drag = Drag::None;
        self.lasso.clear();
        self.anns.clear();
        self.pending = None;
        self.text = None;
        self.tool = Tool::None;
        self.hover = None;
        let c = cursor();
        self.cur = (c.x - vx, c.y - vy);
        self.set_monitor(c.x, c.y);
        if mode == Mode::Full {
            self.select_monitor();
        }
        self.layout();
        self.visible = true;
        self.paint_to_back();
        SetWindowPos(self.hwnd, HWND_TOPMOST, vx, vy, w, h, SWP_SHOWWINDOW);
        let hdc = GetDC(self.hwnd);
        self.blit(hdc);
        ReleaseDC(self.hwnd, hdc);
        force_foreground(self.hwnd);
    }

    unsafe fn close(&mut self) {
        ShowWindow(self.hwnd, SW_HIDE);
        ReleaseCapture();
        self.visible = false;
        self.shot = None;
        self.dim = None;
        self.work = None;
        self.back = None;
        self.lasso = Vec::new();
        self.anns.clear();
    }

    unsafe fn set_monitor(&mut self, sx: i32, sy: i32) {
        let (m, _, s) = monitor_at(sx, sy);
        self.mon = RECT { left: m.left - self.vx, top: m.top - self.vy, right: m.right - self.vx, bottom: m.bottom - self.vy };
        self.s = s;
    }

    unsafe fn select_monitor(&mut self) {
        self.sel = self.mon;
        self.lasso.clear();
        self.mode = Mode::Full;
        self.enter_edit();
    }

    unsafe fn enter_edit(&mut self) {
        let full = RECT { left: 0, top: 0, right: self.w, bottom: self.h };
        self.sel = inter(&self.sel, &full);
        if self.record {
            let r = self.sel;
            let rect = RECT { left: r.left + self.vx, top: r.top + self.vy, right: r.right + self.vx, bottom: r.bottom + self.vy };
            let whole = self.mode == Mode::Full;
            self.close();
            crate::rec::start(rect, whole);
            return;
        }
        self.editing = true;
        self.render_work();
        self.layout();
    }

    fn back_to_select(&mut self) {
        self.editing = false;
        self.anns.clear();
        self.pending = None;
        self.text = None;
        self.lasso.clear();
        if self.mode == Mode::Full {
            self.mode = Mode::Region;
        }
        unsafe { self.layout() };
    }

    unsafe fn render_work(&mut self) {
        let (Some(shot), Some(work)) = (&self.shot, &self.work) else { return };
        GdiFlush();
        let r = self.sel;
        let stride = self.w as usize;
        let (sp, wp) = (shot.px(), work.px());
        for y in r.top..r.bottom {
            let o = y as usize * stride;
            wp[o + r.left as usize..o + r.right as usize].copy_from_slice(&sp[o + r.left as usize..o + r.right as usize]);
        }
        let mut g: Option<Gfx> = None;
        for a in self.anns.iter().chain(self.pending.iter()) {
            match a {
                Ann::Blur(rc) | Ann::Censor(rc) => {
                    if let Some(gg) = g.take() {
                        GdipFlush(gg.0, 1);
                    }
                    GdiFlush();
                    let rr = inter(rc, &r);
                    if rw(&rr) <= 0 || rh(&rr) <= 0 {
                        continue;
                    }
                    if let Ann::Blur(_) = a {
                        let rad = ((rw(&rr).min(rh(&rr)) / 5).clamp(4, 14) as f32 * self.s.max(1.0)) as usize;
                        box_blur(wp, stride, &rr, rad);
                    } else {
                        for y in rr.top..rr.bottom {
                            let o = y as usize * stride;
                            wp[o + rr.left as usize..o + rr.right as usize].fill(0xFF00_0000);
                        }
                    }
                }
                Ann::Line { a, b, c, w, arrow } => {
                    let gg = g.get_or_insert_with(|| {
                        let gg = Gfx::from_hdc(work.dc);
                        gg.clip_rect(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32);
                        gg
                    });
                    if *arrow {
                        gg.arrow(a.0 + 1.0, a.1 + 1.5, b.0 + 1.0, b.1 + 1.5, 0x50000000, *w);
                        gg.arrow(a.0, a.1, b.0, b.1, *c, *w);
                    } else {
                        gg.line(a.0 + 1.0, a.1 + 1.5, b.0 + 1.0, b.1 + 1.5, 0x50000000, *w);
                        gg.line(a.0, a.1, b.0, b.1, *c, *w);
                    }
                }
                Ann::Stroke { pts, c, w } => {
                    let gg = g.get_or_insert_with(|| {
                        let gg = Gfx::from_hdc(work.dc);
                        gg.clip_rect(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32);
                        gg
                    });
                    let mut p = pts.clone();
                    if p.len() == 1 {
                        p.push((p[0].0 + 0.1, p[0].1 + 0.1));
                    }
                    let sh: Vec<(f32, f32)> = p.iter().map(|&(x, y)| (x + 1.0, y + 1.5)).collect();
                    gg.polyline(&sh, 0x50000000, *w);
                    gg.polyline(&p, *c, *w);
                }
                Ann::Text { x, y, s, c, size } => {
                    let gg = g.get_or_insert_with(|| {
                        let gg = Gfx::from_hdc(work.dc);
                        gg.clip_rect(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32);
                        gg
                    });
                    draw_text_ann(gg, *x, *y, s, *c, *size);
                }
            }
        }
        drop(g);
        GdiFlush();
    }

    unsafe fn layout(&mut self) {
        let s = self.s;
        let items: Vec<Btn> = if self.editing {
            vec![
                Btn::Tool(Tool::Text),
                Btn::Tool(Tool::Blur),
                Btn::Tool(Tool::Censor),
                Btn::Tool(Tool::Arrow),
                Btn::Tool(Tool::Line),
                Btn::Tool(Tool::Pen),
                Btn::Sep,
                Btn::Color,
                Btn::Undo,
                Btn::Sep,
                Btn::Save,
                Btn::Copy,
                Btn::Close,
            ]
        } else {
            vec![Btn::Mode(Mode::Region), Btn::Mode(Mode::Free), Btn::Mode(Mode::Full), Btn::Sep, Btn::Record, Btn::Sep, Btn::Close]
        };
        let bs = (38.0 * s).round() as i32;
        let gap = (3.0 * s).round() as i32;
        let pad = (7.0 * s).round() as i32;
        let sep = (11.0 * s).round() as i32;
        let tw: i32 = pad * 2 + items.iter().map(|b| if *b == Btn::Sep { sep } else { bs }).sum::<i32>() + gap * (items.len() as i32 - 1);
        let bh = bs + pad * 2;
        let (x, y);
        if self.editing {
            let cx = (self.sel.left + self.sel.right) / 2;
            let cy = (self.sel.top + self.sel.bottom) / 2;
            let (m, _, ms) = monitor_at(cx + self.vx, cy + self.vy);
            let m = RECT { left: m.left - self.vx, top: m.top - self.vy, right: m.right - self.vx, bottom: m.bottom - self.vy };
            let _ = ms;
            let margin = (10.0 * s) as i32;
            x = (cx - tw / 2).clamp(m.left + margin, (m.right - tw - margin).max(m.left + margin));
            let below = self.sel.bottom + margin;
            let above = self.sel.top - bh - margin;
            y = if below + bh <= m.bottom - margin {
                below
            } else if above >= m.top + margin {
                above
            } else {
                self.sel.bottom - bh - margin
            };
        } else {
            x = (self.mon.left + self.mon.right) / 2 - tw / 2;
            y = self.mon.top + (20.0 * s) as i32;
        }
        self.bar = RECT { left: x, top: y, right: x + tw, bottom: y + bh };
        self.btns.clear();
        let mut cx = x + pad;
        for b in items {
            let wdt = if b == Btn::Sep { sep } else { bs };
            self.btns.push((b, RECT { left: cx, top: y + pad, right: cx + wdt, bottom: y + pad + bs }));
            cx += wdt + gap;
        }
    }

    fn hit(&self, x: i32, y: i32) -> Option<Btn> {
        self.btns.iter().find(|(b, r)| *b != Btn::Sep && rect_has(r, x, y)).map(|(b, _)| *b)
    }

    fn in_bar(&self, x: i32, y: i32) -> bool {
        rect_has(&self.bar, x, y)
    }

    unsafe fn bright_rgn(&self) -> Option<HRGN> {
        let lasso_rgn = || {
            if self.lasso.len() >= 3 {
                Some(CreatePolygonRgn(self.lasso.as_ptr(), self.lasso.len() as i32, WINDING))
            } else {
                None
            }
        };
        if self.editing {
            if self.mode == Mode::Free {
                return lasso_rgn();
            }
            let r = self.sel;
            return Some(CreateRectRgn(r.left, r.top, r.right, r.bottom));
        }
        if self.drag == Drag::Select {
            if self.mode == Mode::Free {
                return lasso_rgn();
            }
            let r = norm(self.start, self.cur);
            return Some(CreateRectRgn(r.left, r.top, r.right, r.bottom));
        }
        None
    }

    unsafe fn paint_to_back(&mut self) {
        let (Some(back), Some(dim), Some(shot), Some(work)) = (&self.back, &self.dim, &self.shot, &self.work) else { return };
        let (w, h, s) = (self.w, self.h, self.s);
        BitBlt(back.dc, 0, 0, w, h, dim.dc, 0, 0, SRCCOPY);
        if let Some(rgn) = self.bright_rgn() {
            ExtSelectClipRgn(back.dc, rgn, RGN_COPY);
            BitBlt(back.dc, 0, 0, w, h, if self.editing { work.dc } else { shot.dc }, 0, 0, SRCCOPY);
            SelectClipRgn(back.dc, null_mut());
            DeleteObject(rgn);
        }
        let g = Gfx::from_hdc(back.dc);

        // selection outline
        let selecting = self.drag == Drag::Select;
        let free = self.mode == Mode::Free;
        if (self.editing || selecting) && free && self.lasso.len() >= 2 {
            let pts: Vec<(f32, f32)> = self.lasso.iter().map(|p| (p.x as f32 + 0.5, p.y as f32 + 0.5)).collect();
            let path = if self.editing { Path::poly(&pts) } else { Path::polyline(&pts) };
            g.stroke(&path, 0x70000000, 3.2 * s);
            g.stroke(&path, 0xFFFFFFFF, 1.5 * s);
        } else if self.editing || (selecting && !free) {
            let r = if self.editing { self.sel } else { norm(self.start, self.cur) };
            let (x, y, rw_, rh_) = (r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32);
            g.stroke(&Path::rect(x, y, rw_, rh_), 0x60000000, 3.0 * s);
            g.stroke(&Path::rect(x, y, rw_, rh_), if self.record { 0xFFFF3B30 } else { 0xE6FFFFFF }, 1.2 * s);
            // viewfinder corner accents
            let l = (16.0 * s).min(rw_ / 2.0).min(rh_ / 2.0);
            let t = 3.0 * s;
            for (cx, cy, dx, dy) in [(x, y, 1.0, 1.0), (x + rw_, y, -1.0, 1.0), (x + rw_, y + rh_, -1.0, -1.0), (x, y + rh_, 1.0, -1.0)] {
                g.polyline(&[(cx + dx * l, cy), (cx, cy), (cx, cy + dy * l)], 0xFFFFFFFF, t);
            }
        }

        // size badge
        if self.editing || selecting {
            let r = if self.editing {
                self.sel
            } else if free {
                let (mut a, mut b) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
                for p in &self.lasso {
                    a = (a.0.min(p.x), a.1.min(p.y));
                    b = (b.0.max(p.x), b.1.max(p.y));
                }
                if self.lasso.is_empty() { norm(self.cur, self.cur) } else { norm(a, b) }
            } else {
                norm(self.start, self.cur)
            };
            let label = format!("{} × {}", rw(&r), rh(&r));
            let (tw, _) = g.measure_at(&label, 12.0 * s, SEMI);
            let (bw, bh) = (tw + 16.0 * s, 22.0 * s);
            let mut by = r.top as f32 - bh - 6.0 * s;
            if by < self.mon.top as f32 + 2.0 {
                by = r.top as f32 + 6.0 * s;
            }
            let bx = r.left as f32;
            g.fill(&Path::rrect(bx, by, bw, bh, bh / 2.0), 0xD0141518);
            g.text(&label, bx, by, bw, bh, 12.0 * s, 0xFFFFFFFF, SEMI | CENTER);
        }

        // crosshair while choosing
        if !self.editing && !selecting && !self.in_bar(self.cur.0, self.cur.1) {
            let (cx, cy) = (self.cur.0 as f32 + 0.5, self.cur.1 as f32 + 0.5);
            g.line(self.mon.left as f32, cy, self.mon.right as f32, cy, 0x50FFFFFF, 1.0);
            g.line(cx, self.mon.top as f32, cx, self.mon.bottom as f32, 0x50FFFFFF, 1.0);
        }

        // in-progress text + caret
        if let Some(t) = &self.text {
            let size = 26.0 * s;
            let c = PALETTE[self.color];
            draw_text_ann(&g, t.x, t.y, &t.s, c, size);
            let (tw, th) = if t.s.is_empty() { (0.0, size * 1.2) } else { g.measure_at(&t.s, size, SEMI) };
            g.line(t.x + tw + 2.0, t.y + 2.0, t.x + tw + 2.0, t.y + th, c, 2.0 * s);
        }

        // toolbar / mode bar
        self.draw_bar(&g);
        if !self.editing && !selecting {
            let hint = match (self.record, self.mode) {
                (true, _) => "Drag to choose what to record   ·   Click to record full screen   ·   Esc to cancel",
                (_, Mode::Free) => "Draw around anything   ·   Esc to cancel",
                _ => "Drag to capture   ·   Click for full screen   ·   Esc to cancel",
            };
            let (tw, _) = g.measure_at(hint, 12.0 * s, 0);
            let (bw, bh) = (tw + 28.0 * s, 26.0 * s);
            let bx = (self.bar.left + self.bar.right) as f32 / 2.0 - bw / 2.0;
            let by = self.bar.bottom as f32 + 10.0 * s;
            g.fill(&Path::rrect(bx, by, bw, bh, bh / 2.0), 0xC0141518);
            g.text(hint, bx, by, bw, bh, 12.0 * s, 0xFFEDEFF2, CENTER);
        }
    }

    unsafe fn draw_bar(&self, g: &Gfx) {
        let s = self.s;
        let b = self.bar;
        let (x, y, w, h) = (b.left as f32, b.top as f32, rw(&b) as f32, rh(&b) as f32);
        for i in 1..=5 {
            let e = i as f32 * 1.6 * s;
            g.fill(&Path::rrect(x - e, y - e + 3.0 * s, w + e * 2.0, h + e * 2.0, h / 2.0 + e), 0x0C000000);
        }
        let pl = crate::theme::pal();
        let pill = Path::rrect(x, y, w, h, h / 2.0);
        g.fill_v(&pill, y, y + h, pl.bar0, pl.bar1);
        g.stroke(&pill, pl.bar_line, 1.0 * s);
        g.stroke(&Path::rrect(x + 1.2 * s, y + 1.2 * s, w - 2.4 * s, h - 2.4 * s, h / 2.0 - 1.2 * s), pl.bar_hi, 1.0 * s);

        for (btn, r) in &self.btns {
            let (bx, by, bw, bh) = (r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32);
            if *btn == Btn::Sep {
                let mx = bx + bw / 2.0;
                g.line(mx, by + 7.0 * s, mx, by + bh - 7.0 * s, pl.sep_a, 1.0 * s);
                g.line(mx + 1.0 * s, by + 7.0 * s, mx + 1.0 * s, by + bh - 7.0 * s, pl.sep_b, 1.0 * s);
                continue;
            }
            let active = matches!(btn, Btn::Tool(t) if *t == self.tool && self.tool != Tool::None)
                || matches!(btn, Btn::Mode(m) if *m == self.mode && !self.editing)
                || (*btn == Btn::Record && self.record);
            let disabled = self.record && *btn == Btn::Mode(Mode::Free);
            let hov = self.hover == Some(*btn);
            let rr = Path::rrect(bx, by, bw, bh, 11.0 * s);
            if active {
                g.fill_v(&rr, by, by + bh, pl.bar_on0, pl.bar_on1);
                g.stroke(&rr, 0x30000000, 1.0 * s);
                g.stroke(&Path::rrect(bx + 1.0 * s, by + 1.5 * s, bw - 2.0 * s, bh - 2.0 * s, 10.0 * s), 0x18000000, 1.5 * s);
            } else if hov && !disabled {
                g.fill_v(&rr, by, by + bh, pl.bar_hov0, pl.bar_hov1);
                g.stroke(&rr, 0x22000000, 1.0 * s);
            }
            let icon = match btn {
                Btn::Mode(Mode::Region) => Icon::Region,
                Btn::Mode(Mode::Free) => Icon::Free,
                Btn::Mode(Mode::Full) => Icon::Full,
                Btn::Tool(Tool::Text) => Icon::Text,
                Btn::Tool(Tool::Blur) => Icon::Blur,
                Btn::Tool(Tool::Censor) => Icon::Censor,
                Btn::Tool(Tool::Arrow) => Icon::Arrow,
                Btn::Tool(Tool::Line) | Btn::Tool(Tool::None) => Icon::Line,
                Btn::Tool(Tool::Pen) => Icon::Pen,
                Btn::Color => Icon::Color(PALETTE[self.color]),
                Btn::Undo => Icon::Undo,
                Btn::Save => Icon::Save,
                Btn::Copy => Icon::Copy,
                Btn::Record => Icon::Record,
                Btn::Close | Btn::Sep => Icon::Close,
            };
            let is = 24.0 * s;
            let off = if active { 0.8 * s } else { 0.0 };
            icons::draw(g, icon, bx + (bw - is) / 2.0, by + (bh - is) / 2.0 + off, is);
            if disabled {
                g.fill(&rr, (pl.bar1 & 0x00FF_FFFF) | 0xB000_0000);
            }
        }

        // tooltip
        if let Some(hb) = self.hover {
            if let Some((_, r)) = self.btns.iter().find(|(b, _)| *b == hb) {
                let t = tip(hb);
                let (tw, _) = g.measure_at(t, 12.0 * s, SEMI);
                let (bw, bh) = (tw + 18.0 * s, 24.0 * s);
                let bx = (r.left + r.right) as f32 / 2.0 - bw / 2.0;
                let above = self.bar.top as f32 - bh - 8.0 * s;
                let by = if above > self.mon.top as f32 { above } else { self.bar.bottom as f32 + 8.0 * s };
                g.fill(&Path::rrect(bx, by, bw, bh, bh / 2.0), 0xE6141518);
                g.text(t, bx, by, bw, bh, 12.0 * s, 0xFFFFFFFF, SEMI | CENTER);
            }
        }
    }

    unsafe fn blit(&self, hdc: HDC) {
        if let Some(b) = &self.back {
            BitBlt(hdc, 0, 0, self.w, self.h, b.dc, 0, 0, SRCCOPY);
        }
    }

    unsafe fn redraw(&mut self) {
        self.paint_to_back();
        let hdc = GetDC(self.hwnd);
        self.blit(hdc);
        ReleaseDC(self.hwnd, hdc);
    }

    unsafe fn commit_text(&mut self) {
        if let Some(t) = self.text.take() {
            if !t.s.trim().is_empty() {
                self.anns.push(Ann::Text { x: t.x, y: t.y, s: t.s, c: PALETTE[self.color], size: 26.0 * self.s });
                self.render_work();
            }
        }
    }

    unsafe fn crop(&self) -> Option<Img> {
        let work = self.work.as_ref()?;
        GdiFlush();
        let r = self.sel;
        let (w, h) = (rw(&r), rh(&r));
        if w <= 0 || h <= 0 {
            return None;
        }
        let wp = work.px();
        let stride = self.w as usize;
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in r.top..r.bottom {
            let o = y as usize * stride + r.left as usize;
            px.extend(wp[o..o + w as usize].iter().map(|p| p | 0xFF00_0000));
        }
        let mut alpha = false;
        if self.mode == Mode::Free && self.lasso.len() >= 3 {
            let mut mask = vec![0u32; (w * h) as usize];
            let bmp = bitmap_over(w, h, mask.as_mut_ptr(), PF_PARGB);
            {
                let g = Gfx::from_image(bmp);
                let pts: Vec<(f32, f32)> = self.lasso.iter().map(|p| ((p.x - r.left) as f32 + 0.5, (p.y - r.top) as f32 + 0.5)).collect();
                g.fill_poly_winding(&pts, 0xFFFFFFFF);
            }
            dispose(bmp);
            for (p, m) in px.iter_mut().zip(mask.iter()) {
                *p = (*p & 0x00FF_FFFF) | (m & 0xFF00_0000);
            }
            alpha = true;
        }
        Some(Img { w, h, px, alpha })
    }

    unsafe fn finish(&mut self, how: Out) {
        self.commit_text();
        let img = self.crop();
        self.close();
        if let Some(img) = img {
            app::deliver(img, how);
        }
    }

    unsafe fn cancel(&mut self) {
        self.close();
    }

    unsafe fn press(&mut self, b: Btn) {
        match b {
            Btn::Mode(Mode::Full) => {
                let (x, y) = ((self.bar.left + self.bar.right) / 2, self.bar.top);
                self.set_monitor(x + self.vx, y + self.vy);
                self.select_monitor();
            }
            Btn::Mode(m) => {
                if self.record && m == Mode::Free {
                    return;
                }
                self.mode = m;
                self.lasso.clear();
            }
            Btn::Record => {
                self.record = !self.record;
                if self.record && self.mode == Mode::Free {
                    self.mode = Mode::Region;
                    self.lasso.clear();
                }
            }
            Btn::Tool(t) => {
                self.commit_text();
                self.tool = if self.tool == t { Tool::None } else { t };
            }
            Btn::Color => {
                self.color = (self.color + 1) % PALETTE.len();
            }
            Btn::Undo => {
                if self.text.is_some() {
                    self.text = None;
                } else {
                    self.anns.pop();
                    self.render_work();
                }
            }
            Btn::Save => return self.finish(Out::Save),
            Btn::Copy => return self.finish(Out::Copy),
            Btn::Close => return self.cancel(),
            Btn::Sep => {}
        }
        self.layout();
        self.redraw();
    }

    fn clamp_pt(&self, x: i32, y: i32) -> (f32, f32) {
        let r = self.sel;
        (x.clamp(r.left, r.right - 1) as f32 + 0.5, y.clamp(r.top, r.bottom - 1) as f32 + 0.5)
    }

    unsafe fn on_down(&mut self, x: i32, y: i32) {
        if let Some(b) = self.hit(x, y) {
            return self.press(b);
        }
        if self.in_bar(x, y) {
            return;
        }
        self.commit_text();
        self.cur = (x, y);
        if self.editing {
            let inside = rect_has(&self.sel, x, y);
            match self.tool {
                Tool::None => {
                    if !inside {
                        self.back_to_select();
                    } else {
                        return;
                    }
                }
                Tool::Text => {
                    if inside {
                        let size = 26.0 * self.s;
                        self.text = Some(TextEdit { x: x as f32, y: y as f32 - size * 0.62, s: String::new() });
                    }
                    self.redraw();
                    return;
                }
                t => {
                    let p = self.clamp_pt(x, y);
                    let c = PALETTE[self.color];
                    let w = 4.0 * self.s;
                    self.pending = Some(match t {
                        Tool::Arrow => Ann::Line { a: p, b: p, c, w, arrow: true },
                        Tool::Line => Ann::Line { a: p, b: p, c, w, arrow: false },
                        Tool::Pen => Ann::Stroke { pts: vec![p], c, w },
                        Tool::Blur => Ann::Blur(norm((x, y), (x, y))),
                        _ => Ann::Censor(norm((x, y), (x, y))),
                    });
                    self.start = (x, y);
                    self.drag = Drag::Ann;
                    SetCapture(self.hwnd);
                    return;
                }
            }
        }
        // start a new selection
        self.drag = Drag::Select;
        self.start = (x, y);
        self.lasso.clear();
        if self.mode == Mode::Free {
            self.lasso.push(POINT { x, y });
        }
        if self.mode == Mode::Full {
            self.mode = Mode::Region;
        }
        SetCapture(self.hwnd);
        self.redraw();
    }

    unsafe fn on_move(&mut self, x: i32, y: i32) {
        let x = x.clamp(0, self.w - 1);
        let y = y.clamp(0, self.h - 1);
        self.cur = (x, y);
        let hv = if self.drag == Drag::None { self.hit(x, y) } else { None };
        let hover_changed = hv != self.hover;
        self.hover = hv;
        match self.drag {
            Drag::Select => {
                if self.mode == Mode::Free {
                    if let Some(l) = self.lasso.last() {
                        if (l.x - x).abs() + (l.y - y).abs() >= 2 {
                            self.lasso.push(POINT { x, y });
                        }
                    }
                }
                self.redraw();
            }
            Drag::Ann => {
                let p = self.clamp_pt(x, y);
                let start = self.start;
                match &mut self.pending {
                    Some(Ann::Line { b, .. }) => *b = p,
                    Some(Ann::Stroke { pts, .. }) => {
                        if let Some(l) = pts.last() {
                            if (l.0 - p.0).abs() + (l.1 - p.1).abs() >= 1.5 {
                                pts.push(p);
                            }
                        }
                    }
                    Some(Ann::Blur(r)) | Some(Ann::Censor(r)) => *r = norm(start, (x, y)),
                    _ => {}
                }
                self.render_work();
                self.redraw();
            }
            Drag::None => {
                if !self.editing {
                    let c = cursor();
                    let (m, _, _) = monitor_at(c.x, c.y);
                    if m.left - self.vx != self.mon.left || m.top - self.vy != self.mon.top {
                        self.set_monitor(c.x, c.y);
                        self.layout();
                    }
                    self.redraw();
                } else if hover_changed {
                    self.redraw();
                }
            }
        }
    }

    unsafe fn on_up(&mut self, x: i32, y: i32) {
        let x = x.clamp(0, self.w - 1);
        let y = y.clamp(0, self.h - 1);
        match self.drag {
            Drag::Select => {
                self.drag = Drag::None;
                ReleaseCapture();
                if self.mode == Mode::Free {
                    let (mut a, mut b) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
                    for p in &self.lasso {
                        a = (a.0.min(p.x), a.1.min(p.y));
                        b = (b.0.max(p.x), b.1.max(p.y));
                    }
                    if self.lasso.len() < 3 || b.0 - a.0 < 4 || b.1 - a.1 < 4 {
                        self.lasso.clear();
                    } else {
                        self.sel = norm(a, b);
                        self.enter_edit();
                    }
                } else {
                    let r = norm(self.start, (x, y));
                    if rw(&r) < 4 && rh(&r) < 4 {
                        self.set_monitor(x + self.vx, y + self.vy);
                        self.select_monitor();
                    } else {
                        self.sel = r;
                        self.enter_edit();
                    }
                }
                self.layout();
                self.redraw();
            }
            Drag::Ann => {
                self.drag = Drag::None;
                ReleaseCapture();
                if let Some(p) = self.pending.take() {
                    let keep = match &p {
                        Ann::Line { a, b, .. } => (a.0 - b.0).abs() + (a.1 - b.1).abs() > 3.0,
                        Ann::Blur(r) | Ann::Censor(r) => rw(r) > 2 && rh(r) > 2,
                        _ => true,
                    };
                    if keep {
                        self.anns.push(p);
                    }
                }
                self.render_work();
                self.redraw();
            }
            Drag::None => {}
        }
    }

    unsafe fn on_key(&mut self, vk: u32) {
        let ctrl = GetKeyState(0x11) < 0;
        if self.text.is_some() {
            return; // handled in WM_CHAR
        }
        match vk {
            0x1B => {
                if self.drag != Drag::None {
                    self.drag = Drag::None;
                    self.pending = None;
                    ReleaseCapture();
                    self.render_work();
                    self.redraw();
                } else {
                    self.cancel();
                }
            }
            0x0D if self.editing => self.finish(Out::Default),
            0x43 if ctrl && self.editing => self.finish(Out::Copy),
            0x53 if ctrl && self.editing => self.finish(Out::Save),
            0x5A if ctrl && self.editing => self.press(Btn::Undo),
            _ if ctrl => {}
            0x54 if self.editing => self.press(Btn::Tool(Tool::Text)),
            0x42 if self.editing => self.press(Btn::Tool(Tool::Blur)),
            0x58 if self.editing => self.press(Btn::Tool(Tool::Censor)),
            0x41 if self.editing => self.press(Btn::Tool(Tool::Arrow)),
            0x4C if self.editing => self.press(Btn::Tool(Tool::Line)),
            0x50 if self.editing => self.press(Btn::Tool(Tool::Pen)),
            0x43 if self.editing => self.press(Btn::Color),
            0x52 if !self.editing => self.press(Btn::Mode(Mode::Region)),
            0x46 if !self.editing => self.press(Btn::Mode(Mode::Free)),
            0x57 if !self.editing => self.press(Btn::Mode(Mode::Full)),
            0x56 if !self.editing => self.press(Btn::Record),
            _ => {}
        }
    }

    unsafe fn on_char(&mut self, ch: u32) {
        let Some(t) = &mut self.text else { return };
        match ch {
            8 => {
                t.s.pop();
            }
            13 => {
                self.commit_text();
            }
            27 => {
                self.text = None;
            }
            c if c >= 32 => {
                if let Some(c) = char::from_u32(c) {
                    t.s.push(c);
                }
            }
            _ => {}
        }
        self.redraw();
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(ov) = OV.try_get() else { return DefWindowProcW(h, m, wp, lp) };
    if !ov.visible {
        return DefWindowProcW(h, m, wp, lp);
    }
    let (x, y) = lparam_xy(lp);
    match m {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(h, &mut ps);
            ov.blit(hdc);
            EndPaint(h, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETCURSOR => {
            let c = cursor();
            let (cx, cy) = (c.x - ov.vx, c.y - ov.vy);
            let id = if ov.drag == Drag::None && ov.hit(cx, cy).is_some() {
                IDC_HAND
            } else if ov.in_bar(cx, cy) {
                IDC_ARROW
            } else if ov.editing && ov.tool == Tool::Text && rect_has(&ov.sel, cx, cy) {
                IDC_IBEAM
            } else if ov.editing && ov.tool == Tool::None && rect_has(&ov.sel, cx, cy) {
                IDC_ARROW
            } else {
                IDC_CROSS
            };
            SetCursor(LoadCursorW(null_mut(), id));
            1
        }
        WM_MOUSEMOVE => {
            ov.on_move(x, y);
            0
        }
        WM_LBUTTONDOWN => {
            ov.on_down(x, y);
            0
        }
        WM_LBUTTONUP => {
            ov.on_up(x, y);
            0
        }
        WM_LBUTTONDBLCLK => {
            if ov.editing && ov.tool == Tool::None && rect_has(&ov.sel, x, y) {
                ov.finish(Out::Default);
            } else {
                ov.on_down(x, y);
            }
            0
        }
        WM_RBUTTONDOWN => {
            if ov.text.is_some() {
                ov.text = None;
                ov.redraw();
            } else if ov.editing {
                ov.back_to_select();
                ov.redraw();
            } else {
                ov.cancel();
            }
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            ov.on_key(wp as u32);
            0
        }
        WM_CHAR => {
            ov.on_char(wp as u32);
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}
