//! Fullscreen capture overlay (one window per monitor): region / freeform /
//! full-screen selection, resize handles, and the annotation toolbar.
//! Same behavior and look as the Windows overlay, drawn into a pixel buffer.
use super::output::Out;
use super::Surface;
use crate::gfx::*;
use crate::icons::{self, Icon};
use crate::img::Img;
use std::rc::Rc;
use std::time::Instant;
use tiny_skia as sk;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window};

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
#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct R {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
}
impl R {
    fn w(&self) -> i32 {
        self.r - self.l
    }
    fn h(&self) -> i32 {
        self.b - self.t
    }
    fn has(&self, x: i32, y: i32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    fn inter(&self, o: &R) -> R {
        R { l: self.l.max(o.l), t: self.t.max(o.t), r: self.r.min(o.r), b: self.b.min(o.b) }
    }
}
impl R {
    fn tup(&self) -> (i32, i32, i32, i32) {
        (self.l, self.t, self.r, self.b)
    }
    fn union(&self, o: &R) -> R {
        let (l, t, r, b) = crate::img::union(self.tup(), o.tup());
        R { l, t, r, b }
    }
}
fn norm(a: (i32, i32), b: (i32, i32)) -> R {
    R { l: a.0.min(b.0), t: a.1.min(b.1), r: a.0.max(b.0) + 1, b: a.1.max(b.1) + 1 }
}

#[derive(Clone)]
enum Ann {
    Line { a: (f32, f32), b: (f32, f32), c: u32, w: f32, arrow: bool },
    Stroke { pts: Vec<(f32, f32)>, c: u32, w: f32 },
    /// area and strength (1.0 = normal)
    Blur(R, f32),
    Censor(R),
    Text { x: f32, y: f32, s: String, c: u32, size: f32 },
}
#[derive(PartialEq, Clone, Copy)]
enum Drag {
    None,
    Select,
    Ann,
    /// edge mask: 1 left, 2 top, 4 right, 8 bottom
    Resize(u8),
}
struct TextEdit {
    x: f32,
    y: f32,
    s: String,
}

/// What the user decided.
pub enum Done {
    Cancel,
    Deliver(Img, Out),
    /// record this area: (x, y, w, h) in the monitor's own pixels; `whole` = the full monitor
    Record { area: (i32, i32, i32, i32), whole: bool },
}

const PALETTE: [u32; 6] = [0xFFFF3B30, 0xFFFFCC00, 0xFF34C759, 0xFF0A84FF, 0xFFFFFFFF, 0xFF111111];

pub struct Overlay {
    pub win: Rc<Window>,
    surf: Surface,
    /// this monitor's part of the desktop image, at its native resolution
    src: Img,
    shot: Vec<u32>,
    dim: Vec<u32>,
    work: Vec<u32>,
    back: Vec<u32>,
    /// annotations (lines, pencil, text), premultiplied, drawn over the screen pixels
    layer: Vec<u32>,
    /// part of `layer` that holds something
    layer_box: R,
    /// part of `work` that differs from `shot`
    dirty: R,
    w: i32,
    h: i32,
    s: f32,
    mode: Mode,
    pub editing: bool,
    drag: Drag,
    start: (i32, i32),
    cur: (i32, i32),
    inside: bool,
    /// line / arrow / pencil thickness (before UI scaling) and blur strength; the scroll wheel changes them
    width: f32,
    blur_k: f32,
    wheel: f32,
    /// short-lived label by the cursor ("Thickness 6") and when it goes away
    pub hint: Option<(String, Instant)>,
    sel: R,
    lasso: Vec<(i32, i32)>,
    tool: Tool,
    color: usize,
    anns: Vec<Ann>,
    pending: Option<Ann>,
    text: Option<TextEdit>,
    hover: Option<Btn>,
    btns: Vec<(Btn, R)>,
    bar: R,
    mods: ModifiersState,
    last_click: Option<(Instant, i32, i32)>,
    cursor: CursorIcon,
    pub record: bool,
    /// this monitor in desktop pixels (x, y, w, h)
    pub mon: (i32, i32, i32, i32),
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
        Btn::Tool(Tool::Text) => "Text   T   ·   Shift+Enter for a new line",
        Btn::Tool(Tool::Blur) => "Blur   B   ·   scroll for strength",
        Btn::Tool(Tool::Censor) => "Black bar   X",
        Btn::Tool(Tool::Arrow) => "Arrow   A   ·   scroll for thickness",
        Btn::Tool(Tool::Line) => "Line   L   ·   scroll for thickness",
        Btn::Tool(Tool::Pen) => "Pencil   P   ·   scroll for thickness",
        Btn::Color => "Color   C",
        Btn::Undo => "Undo   Ctrl+Z",
        Btn::Save => "Save to folder   Ctrl+S",
        Btn::Copy => "Copy   Enter / Ctrl+C",
        Btn::Close => "Close   Esc",
        Btn::Record => "Record video   V",
        _ => "",
    }
}

/// Nearest-neighbour resize (only used when the compositor gives the window a
/// size different from the monitor's pixels, e.g. fractional scaling quirks).
fn resample(img: &Img, w: i32, h: i32) -> Vec<u32> {
    if img.w == w && img.h == h {
        return img.px.clone();
    }
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let sy = ((y as i64 * img.h as i64) / h.max(1) as i64) as usize;
        for x in 0..w {
            let sx = ((x as i64 * img.w as i64) / w.max(1) as i64) as usize;
            out.push(img.px[sy * img.w as usize + sx]);
        }
    }
    out
}

impl Overlay {
    pub fn new(win: Rc<Window>, surf: Surface, src: Img, mode: Mode, record: bool, mon: (i32, i32, i32, i32)) -> Overlay {
        let mode = if record && mode == Mode::Free { Mode::Region } else { mode };
        let size = win.inner_size();
        let mut o = Overlay {
            s: win.scale_factor() as f32,
            win,
            surf,
            src,
            shot: Vec::new(),
            dim: Vec::new(),
            work: Vec::new(),
            back: Vec::new(),
            layer: Vec::new(),
            layer_box: R::default(),
            dirty: R::default(),
            w: 0,
            h: 0,
            mode,
            editing: false,
            drag: Drag::None,
            start: (0, 0),
            cur: (-1000, -1000),
            inside: false,
            width: 4.0,
            blur_k: 1.0,
            wheel: 0.0,
            hint: None,
            sel: R::default(),
            lasso: Vec::new(),
            tool: Tool::None,
            color: 0,
            anns: Vec::new(),
            pending: None,
            text: None,
            hover: None,
            btns: Vec::new(),
            bar: R::default(),
            mods: ModifiersState::empty(),
            last_click: None,
            cursor: CursorIcon::Crosshair,
            record,
            mon,
        };
        o.win.set_cursor(CursorIcon::Crosshair);
        o.resize(size.width as i32, size.height as i32);
        o
    }

    fn resize(&mut self, w: i32, h: i32) {
        let (w, h) = (w.max(1), h.max(1));
        if w == self.w && h == self.h {
            return;
        }
        let (Some(nw), Some(nh)) = (std::num::NonZeroU32::new(w as u32), std::num::NonZeroU32::new(h as u32)) else { return };
        let _ = self.surf.resize(nw, nh);
        self.w = w;
        self.h = h;
        self.shot = resample(&self.src, w, h);
        self.dim = self.shot.iter().map(|s| ((s >> 1) & 0x007F_7F7F) | 0xFF00_0000).collect();
        self.work = self.shot.clone();
        self.layer = vec![0; (w * h) as usize];
        self.layer_box = R::default();
        self.dirty = R::default();
        self.anns.clear();
        self.editing = false;
        self.drag = Drag::None;
        if self.mode == Mode::Full {
            self.select_monitor();
        }
        self.layout();
    }

    /// Size of this monitor's screenshot (its own pixels).
    pub fn src_size(&self) -> (i32, i32) {
        (self.src.w, self.src.h)
    }

    fn full(&self) -> R {
        R { l: 0, t: 0, r: self.w, b: self.h }
    }

    fn select_monitor(&mut self) {
        self.sel = self.full();
        self.lasso.clear();
        self.mode = Mode::Full;
        self.enter_edit();
    }

    fn enter_edit(&mut self) {
        self.sel = self.sel.inter(&self.full());
        self.editing = true;
        self.render_work();
        self.layout();
    }

    pub fn back_to_select(&mut self) {
        self.editing = false;
        self.anns.clear();
        self.pending = None;
        self.text = None;
        self.lasso.clear();
        self.tool = Tool::None;
        if self.mode == Mode::Full {
            self.mode = Mode::Region;
        }
        self.layout();
        self.win.request_redraw();
    }

    /// Where edits may land: the selection box, or (freeform) anywhere on screen,
    /// since freeform edits may reach past the drawn shape.
    fn edit_bounds(&self) -> R {
        if self.mode == Mode::Free { self.full() } else { self.sel }
    }

    fn blur_rects(&self) -> Vec<R> {
        let b = self.edit_bounds();
        self.anns
            .iter()
            .chain(self.pending.iter())
            .filter_map(|a| match a {
                Ann::Blur(rc, _) | Ann::Censor(rc) => Some(rc.inter(&b)),
                _ => None,
            })
            .filter(|r| r.w() > 0 && r.h() > 0)
            .collect()
    }

    fn render_work(&mut self) {
        let bound = self.edit_bounds();
        let full = self.full();
        let stride = self.w as usize;
        // undo last time's blur/black bars, then copy the area this render needs
        let mut need = self.sel;
        for rc in self.blur_rects() {
            need = need.union(&rc);
        }
        let copy = self.dirty.union(&need).inter(&full);
        for y in copy.t..copy.b {
            let o = y as usize * stride;
            self.work[o + copy.l as usize..o + copy.r as usize].copy_from_slice(&self.shot[o + copy.l as usize..o + copy.r as usize]);
        }
        self.dirty = need;
        let lb = self.layer_box.inter(&full);
        for y in lb.t..lb.b {
            let o = y as usize * stride;
            self.layer[o + lb.l as usize..o + lb.r as usize].fill(0);
        }
        let mut used = R::default();
        let s = self.s;
        let anns: Vec<Ann> = self.anns.iter().chain(self.pending.iter()).cloned().collect();
        let pad = |w: f32| (w * 3.2 + 10.0).ceil() as i32;
        for a in &anns {
            match a {
                Ann::Blur(rc, _) | Ann::Censor(rc) => {
                    let rr = rc.inter(&bound);
                    if rr.w() <= 0 || rr.h() <= 0 {
                        continue;
                    }
                    if let Ann::Blur(_, k) = a {
                        let rad = (((rr.w().min(rr.h()) / 5).clamp(4, 14) as f32 * s.max(1.0) * k) as usize).clamp(1, 120);
                        crate::img::box_blur(&mut self.work, stride, rr.l, rr.t, rr.w(), rr.h(), rad);
                    } else {
                        for y in rr.t..rr.b {
                            let o = y as usize * stride;
                            self.work[o + rr.l as usize..o + rr.r as usize].fill(0xFF00_0000);
                        }
                    }
                }
                _ => {
                    let g = Gfx::over_slice(&mut self.layer, self.w, self.h);
                    g.clip_rect(bound.l as f32, bound.t as f32, bound.w() as f32, bound.h() as f32);
                    let bx = match a {
                        Ann::Line { a, b, c, w, arrow } => {
                            if *arrow {
                                g.arrow(a.0 + 1.0, a.1 + 1.5, b.0 + 1.0, b.1 + 1.5, 0x50000000, *w);
                                g.arrow(a.0, a.1, b.0, b.1, *c, *w);
                            } else {
                                g.line(a.0 + 1.0, a.1 + 1.5, b.0 + 1.0, b.1 + 1.5, 0x50000000, *w);
                                g.line(a.0, a.1, b.0, b.1, *c, *w);
                            }
                            let p = pad(*w);
                            R { l: a.0.min(b.0) as i32 - p, t: a.1.min(b.1) as i32 - p, r: a.0.max(b.0) as i32 + p, b: a.1.max(b.1) as i32 + p }
                        }
                        Ann::Stroke { pts, c, w } => {
                            let mut p = pts.clone();
                            if p.len() == 1 {
                                p.push((p[0].0 + 0.1, p[0].1 + 0.1));
                            }
                            let sh: Vec<(f32, f32)> = p.iter().map(|&(x, y)| (x + 1.0, y + 1.5)).collect();
                            g.polyline(&sh, 0x50000000, *w);
                            g.polyline(&p, *c, *w);
                            let q = pad(*w);
                            let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                            for &(x, y) in &p {
                                (l, t, r, b) = (l.min(x), t.min(y), r.max(x), b.max(y));
                            }
                            R { l: l as i32 - q, t: t as i32 - q, r: r as i32 + q, b: b as i32 + q }
                        }
                        Ann::Text { x, y, s, c, size } => {
                            draw_text_ann(&g, *x, *y, s, *c, *size);
                            let (tw, th) = g.measure_at(s, *size, SEMI);
                            let q = (*size / 6.0) as i32 + 4;
                            R { l: *x as i32 - q, t: *y as i32 - q, r: (*x + tw) as i32 + q * 2, b: (*y + th) as i32 + q * 2 }
                        }
                        _ => continue,
                    };
                    used = used.union(&bx.inter(&bound));
                }
            }
        }
        self.layer_box = used.inter(&full);
    }

    fn layout(&mut self) {
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
            let cx = (self.sel.l + self.sel.r) / 2;
            let margin = (10.0 * s) as i32;
            x = (cx - tw / 2).clamp(margin, (self.w - tw - margin).max(margin));
            let below = self.sel.b + margin;
            let above = self.sel.t - bh - margin;
            y = if below + bh <= self.h - margin {
                below
            } else if above >= margin {
                above
            } else {
                self.sel.b - bh - margin
            };
        } else {
            x = self.w / 2 - tw / 2;
            y = (20.0 * s) as i32;
        }
        self.bar = R { l: x, t: y, r: x + tw, b: y + bh };
        self.btns.clear();
        let mut cx = x + pad;
        for b in items {
            let wdt = if b == Btn::Sep { sep } else { bs };
            self.btns.push((b, R { l: cx, t: y + pad, r: cx + wdt, b: y + pad + bs }));
            cx += wdt + gap;
        }
    }

    fn hit(&self, x: i32, y: i32) -> Option<Btn> {
        self.btns.iter().find(|(b, r)| *b != Btn::Sep && r.has(x, y)).map(|(b, _)| *b)
    }

    fn in_bar(&self, x: i32, y: i32) -> bool {
        self.bar.has(x, y)
    }

    /// Which selection edges a press at (x, y) would grab (corners grab two).
    fn handle_at(&self, x: i32, y: i32) -> Option<u8> {
        if !self.editing || self.mode == Mode::Free || self.text.is_some() || self.in_bar(x, y) {
            return None;
        }
        let r = self.sel;
        let c = (11.0 * self.s).round() as i32;
        let e = (6.0 * self.s).round() as i32;
        let near = |a: i32, b: i32, d: i32| (a - b).abs() <= d;
        for (cx, cy, m) in [(r.l, r.t, 1 | 2), (r.r, r.t, 4 | 2), (r.r, r.b, 4 | 8), (r.l, r.b, 1 | 8)] {
            if near(x, cx, c) && near(y, cy, c) {
                return Some(m);
            }
        }
        let in_y = y > r.t && y < r.b;
        let in_x = x > r.l && x < r.r;
        if in_y && near(x, r.l, e) {
            Some(1)
        } else if in_y && near(x, r.r, e) {
            Some(4)
        } else if in_x && near(y, r.t, e) {
            Some(2)
        } else if in_x && near(y, r.b, e) {
            Some(8)
        } else {
            None
        }
    }

    fn lasso_bounds(&self) -> R {
        let (mut a, mut b) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
        for p in &self.lasso {
            a = (a.0.min(p.0), a.1.min(p.1));
            b = (b.0.max(p.0), b.1.max(p.1));
        }
        if self.lasso.is_empty() { norm(self.cur, self.cur) } else { norm(a, b) }
    }

    fn lasso_mask(&self) -> Option<sk::Mask> {
        if self.lasso.len() < 3 {
            return None;
        }
        let mut pb = sk::PathBuilder::new();
        for (i, &(x, y)) in self.lasso.iter().enumerate() {
            let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        pb.close();
        let path = pb.finish()?;
        let mut m = sk::Mask::new(self.w as u32, self.h as u32)?;
        m.fill_path(&path, sk::FillRule::Winding, true, sk::Transform::identity());
        Some(m)
    }

    pub fn paint(&mut self) {
        let (w, h, s) = (self.w, self.h, self.s);
        if w <= 0 || self.shot.is_empty() {
            return;
        }
        let n = (w * h) as usize;
        let mut back = std::mem::take(&mut self.back);
        back.resize(n, 0);
        let buf: &mut [u32] = &mut back;
        buf.copy_from_slice(&self.dim);
        let selecting = self.drag == Drag::Select;
        let free = self.mode == Mode::Free;
        // bright (selected) area
        let src = if self.editing { &self.work } else { &self.shot };
        if self.editing || selecting {
            if free {
                if let Some(m) = self.lasso_mask() {
                    let r = self.lasso_bounds().inter(&self.full());
                    let md = m.data();
                    for y in r.t..r.b {
                        for x in r.l..r.r {
                            let i = (y * w + x) as usize;
                            let a = md[i] as u32;
                            if a == 255 {
                                buf[i] = src[i];
                            } else if a > 0 {
                                let (p, q) = (src[i], buf[i]);
                                let mix = |sh: u32| ((((p >> sh) & 255) * a + ((q >> sh) & 255) * (255 - a)) / 255) << sh;
                                buf[i] = 0xFF00_0000 | mix(16) | mix(8) | mix(0);
                            }
                        }
                    }
                }
            } else {
                let r = if self.editing { self.sel } else { norm(self.start, self.cur) }.inter(&self.full());
                for y in r.t..r.b {
                    let o = (y * w) as usize;
                    buf[o + r.l as usize..o + r.r as usize].copy_from_slice(&src[o + r.l as usize..o + r.r as usize]);
                }
            }
        }
        if self.editing {
            if free {
                // blur / black bars count as part of the shape, even past its edge
                for r in self.blur_rects() {
                    for y in r.t..r.b {
                        let o = (y * w) as usize;
                        buf[o + r.l as usize..o + r.r as usize].copy_from_slice(&self.work[o + r.l as usize..o + r.r as usize]);
                    }
                }
            }
            let lb = self.layer_box;
            crate::img::blend_over(buf, &self.layer, w as usize, lb.l, lb.t, lb.r, lb.b);
        }
        {
            let g = Gfx::over_slice(buf, w, h);
            // selection outline
            if (self.editing || selecting) && free && self.lasso.len() >= 2 {
                let pts: Vec<(f32, f32)> = self.lasso.iter().map(|p| (p.0 as f32 + 0.5, p.1 as f32 + 0.5)).collect();
                let path = if self.editing { Path::poly(&pts) } else { Path::polyline(&pts) };
                g.stroke(&path, 0x70000000, 3.2 * s);
                g.stroke(&path, 0xFFFFFFFF, 1.5 * s);
            } else if self.editing || (selecting && !free) {
                let r = if self.editing { self.sel } else { norm(self.start, self.cur) };
                let (x, y, rw_, rh_) = (r.l as f32, r.t as f32, r.w() as f32, r.h() as f32);
                g.stroke(&Path::rect(x, y, rw_, rh_), 0x60000000, 3.0 * s);
                g.stroke(&Path::rect(x, y, rw_, rh_), if self.record { 0xFFFF3B30 } else { 0xE6FFFFFF }, 1.2 * s);
                let l = (16.0 * s).min(rw_ / 2.0).min(rh_ / 2.0);
                let t = 3.0 * s;
                for (cx, cy, dx, dy) in [(x, y, 1.0, 1.0), (x + rw_, y, -1.0, 1.0), (x + rw_, y + rh_, -1.0, -1.0), (x, y + rh_, 1.0, -1.0)] {
                    g.polyline(&[(cx + dx * l, cy), (cx, cy), (cx, cy + dy * l)], 0xFFFFFFFF, t);
                }
                if self.editing {
                    let gl = 8.0 * s;
                    if rw_ > gl * 6.0 {
                        let mx = x + rw_ / 2.0;
                        g.line(mx - gl, y, mx + gl, y, 0xFFFFFFFF, t);
                        g.line(mx - gl, y + rh_, mx + gl, y + rh_, 0xFFFFFFFF, t);
                    }
                    if rh_ > gl * 6.0 {
                        let my = y + rh_ / 2.0;
                        g.line(x, my - gl, x, my + gl, 0xFFFFFFFF, t);
                        g.line(x + rw_, my - gl, x + rw_, my + gl, 0xFFFFFFFF, t);
                    }
                }
            }
            // size badge
            if self.editing || selecting {
                let r = if self.editing {
                    self.sel
                } else if free {
                    self.lasso_bounds()
                } else {
                    norm(self.start, self.cur)
                };
                let label = format!("{} × {}", r.w(), r.h());
                let (tw, _) = g.measure_at(&label, 12.0 * s, SEMI);
                let (bw, bh) = (tw + 16.0 * s, 22.0 * s);
                let mut by = r.t as f32 - bh - 6.0 * s;
                if by < 2.0 {
                    by = r.t as f32 + 6.0 * s;
                }
                let bx = r.l as f32;
                g.fill(&Path::rrect(bx, by, bw, bh, bh / 2.0), 0xD0141518);
                g.text(&label, bx, by, bw, bh, 12.0 * s, 0xFFFFFFFF, SEMI | CENTER);
            }
            // crosshair while choosing
            if !self.editing && !selecting && self.inside && !self.in_bar(self.cur.0, self.cur.1) {
                let (cx, cy) = (self.cur.0 as f32 + 0.5, self.cur.1 as f32 + 0.5);
                g.line(0.0, cy, w as f32, cy, 0x50FFFFFF, 1.0);
                g.line(cx, 0.0, cx, h as f32, 0x50FFFFFF, 1.0);
            }
            // in-progress text + caret
            if let Some(t) = &self.text {
                let size = 26.0 * s;
                let c = PALETTE[self.color];
                draw_text_ann(&g, t.x, t.y, &t.s, c, size);
                // caret at the end of the last line (Shift+Enter starts a new one)
                let lines = t.s.split('\n').count() as f32;
                let last = t.s.rsplit('\n').next().unwrap_or("");
                let lh = g.measure_at("Ag", size, SEMI).1;
                let tw = if last.is_empty() { 0.0 } else { g.measure_at(last, size, SEMI).0 };
                let ty = t.y + (lines - 1.0) * lh;
                g.line(t.x + tw + 2.0, ty + 2.0, t.x + tw + 2.0, ty + lh, c, 2.0 * s);
            }
            // what the scroll wheel just changed
            if let Some((label, _)) = &self.hint {
                let (tw, _) = g.measure_at(label, 12.0 * s, SEMI);
                let (bw, bh) = (tw + 18.0 * s, 24.0 * s);
                let bx = (self.cur.0 as f32 + 18.0 * s).min(w as f32 - bw - 4.0);
                let by = (self.cur.1 as f32 + 18.0 * s).min(h as f32 - bh - 4.0);
                g.fill(&Path::rrect(bx, by, bw, bh, bh / 2.0), 0xE6141518);
                g.text(label, bx, by, bw, bh, 12.0 * s, 0xFFFFFFFF, SEMI | CENTER);
            }
            self.draw_bar(&g);
            // only recording needs explaining; hidden while a tooltip would sit on top of it
            if !self.editing && !selecting && self.record && self.hover.is_none() {
                let hint = "Drag to choose what to record   ·   Click to record full screen   ·   Esc to cancel";
                let (tw, _) = g.measure_at(hint, 12.0 * s, 0);
                let (bw, bh) = (tw + 28.0 * s, 26.0 * s);
                let bx = (self.bar.l + self.bar.r) as f32 / 2.0 - bw / 2.0;
                let by = self.bar.b as f32 + 10.0 * s;
                g.fill(&Path::rrect(bx, by, bw, bh, bh / 2.0), 0xC0141518);
                g.text(hint, bx, by, bw, bh, 12.0 * s, 0xFFEDEFF2, CENTER);
            }
        }
        if let Ok(mut out) = self.surf.buffer_mut() {
            if out.len() == n {
                out.copy_from_slice(&back);
                self.win.pre_present_notify();
                let _ = out.present();
            }
        }
        self.back = back;
    }

    fn draw_bar(&self, g: &Gfx) {
        let s = self.s;
        let b = self.bar;
        let (x, y, w, h) = (b.l as f32, b.t as f32, b.w() as f32, b.h() as f32);
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
            let (bx, by, bw, bh) = (r.l as f32, r.t as f32, r.w() as f32, r.h() as f32);
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
            let hov = self.hover == Some(*btn) && !disabled;
            let rr = Path::rrect(bx, by, bw, bh, 11.0 * s);
            if active {
                g.fill_v(&rr, by, by + bh, pl.bar_on0, pl.bar_on1);
                g.stroke(&rr, 0x30000000, 1.0 * s);
                g.stroke(&Path::rrect(bx + 1.0 * s, by + 1.5 * s, bw - 2.0 * s, bh - 2.0 * s, 10.0 * s), 0x18000000, 1.5 * s);
            } else if hov {
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
        if let Some(hb) = self.hover {
            if let Some((_, r)) = self.btns.iter().find(|(b, _)| *b == hb) {
                let t = tip(hb);
                let (tw, _) = g.measure_at(t, 12.0 * s, SEMI);
                let (bw, bh) = (tw + 18.0 * s, 24.0 * s);
                let bx = (r.l + r.r) as f32 / 2.0 - bw / 2.0;
                let above = self.bar.t as f32 - bh - 8.0 * s;
                let by = if above > 0.0 { above } else { self.bar.b as f32 + 8.0 * s };
                g.fill(&Path::rrect(bx, by, bw, bh, bh / 2.0), 0xE6141518);
                g.text(t, bx, by, bw, bh, 12.0 * s, 0xFFFFFFFF, SEMI | CENTER);
            }
        }
    }

    fn commit_text(&mut self) {
        if let Some(t) = self.text.take() {
            if !t.s.trim().is_empty() {
                self.anns.push(Ann::Text { x: t.x, y: t.y, s: t.s, c: PALETTE[self.color], size: 26.0 * self.s });
                self.render_work();
            }
        }
    }

    fn crop(&self) -> Option<Img> {
        let stride = self.w as usize;
        let mask = if self.mode == Mode::Free { self.lasso_mask() } else { None };
        let Some(mask) = mask else {
            let r = self.sel;
            if r.w() <= 0 || r.h() <= 0 {
                return None;
            }
            return Some(crate::img::compose(&self.work, &self.layer, stride, (r.l, r.t, r.w(), r.h()), |_, _| 255));
        };
        // freeform: the shape plus whatever was drawn past its edge
        let blurs = self.blur_rects();
        let mut r = self.sel.union(&self.layer_box);
        for b in &blurs {
            r = r.union(b);
        }
        let r = r.inter(&self.full());
        if r.w() <= 0 || r.h() <= 0 {
            return None;
        }
        let md = mask.data();
        let cover = |x: i32, y: i32| {
            if blurs.iter().any(|b| b.has(x, y)) {
                return 255;
            }
            md[y as usize * stride + x as usize] as u32
        };
        Some(crate::img::compose(&self.work, &self.layer, stride, (r.l, r.t, r.w(), r.h()), cover))
    }

    fn finish(&mut self, how: Out) -> Option<Done> {
        self.commit_text();
        Some(match self.crop() {
            Some(img) => Done::Deliver(img, how),
            None => Done::Cancel,
        })
    }

    fn press(&mut self, b: Btn) -> Option<Done> {
        match b {
            Btn::Mode(Mode::Full) => self.select_monitor(),
            Btn::Mode(m) => {
                if self.record && m == Mode::Free {
                    return None;
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
            Btn::Color => self.color = (self.color + 1) % PALETTE.len(),
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
            Btn::Close => return Some(Done::Cancel),
            Btn::Sep => {}
        }
        self.layout();
        self.win.request_redraw();
        None
    }

    fn clamp_pt(&self, x: i32, y: i32) -> (f32, f32) {
        let r = self.edit_bounds();
        (x.clamp(r.l, r.r - 1) as f32 + 0.5, y.clamp(r.t, r.b - 1) as f32 + 0.5)
    }

    fn on_down(&mut self, x: i32, y: i32) -> Option<Done> {
        if let Some(b) = self.hit(x, y) {
            return self.press(b);
        }
        if self.in_bar(x, y) {
            return None;
        }
        if let Some(m) = self.handle_at(x, y) {
            self.commit_text();
            self.cur = (x, y);
            self.drag = Drag::Resize(m);
            return None;
        }
        self.commit_text();
        self.cur = (x, y);
        if self.editing {
            let inside = self.sel.has(x, y);
            match self.tool {
                Tool::None => {
                    if !inside {
                        self.back_to_select();
                    } else {
                        return None;
                    }
                }
                Tool::Text => {
                    if inside || self.mode == Mode::Free {
                        let size = 26.0 * self.s;
                        self.text = Some(TextEdit { x: x as f32, y: y as f32 - size * 0.62, s: String::new() });
                    }
                    self.win.request_redraw();
                    return None;
                }
                t => {
                    let p = self.clamp_pt(x, y);
                    let c = PALETTE[self.color];
                    let w = self.width * self.s;
                    self.pending = Some(match t {
                        Tool::Arrow => Ann::Line { a: p, b: p, c, w, arrow: true },
                        Tool::Line => Ann::Line { a: p, b: p, c, w, arrow: false },
                        Tool::Pen => Ann::Stroke { pts: vec![p], c, w },
                        Tool::Blur => Ann::Blur(norm((x, y), (x, y)), self.blur_k),
                        _ => Ann::Censor(norm((x, y), (x, y))),
                    });
                    self.start = (x, y);
                    self.drag = Drag::Ann;
                    return None;
                }
            }
        }
        self.drag = Drag::Select;
        self.start = (x, y);
        self.lasso.clear();
        if self.mode == Mode::Free {
            self.lasso.push((x, y));
        }
        if self.mode == Mode::Full {
            self.mode = Mode::Region;
        }
        self.win.request_redraw();
        None
    }

    fn update_cursor(&mut self) {
        let (x, y) = self.cur;
        let c = if self.drag == Drag::None && self.hit(x, y).is_some() {
            CursorIcon::Pointer
        } else if let Some(m) = if self.drag == Drag::None { self.handle_at(x, y) } else { None } {
            match m {
                3 | 12 => CursorIcon::NwseResize,
                6 | 9 => CursorIcon::NeswResize,
                1 | 4 => CursorIcon::EwResize,
                _ => CursorIcon::NsResize,
            }
        } else if self.in_bar(x, y) {
            CursorIcon::Default
        } else if self.editing && self.tool == Tool::Text && self.sel.has(x, y) {
            CursorIcon::Text
        } else if self.editing && self.tool == Tool::None && self.sel.has(x, y) {
            CursorIcon::Default
        } else {
            CursorIcon::Crosshair
        };
        if c != self.cursor && self.drag == Drag::None {
            self.cursor = c;
            self.win.set_cursor(c);
        }
    }

    fn on_move(&mut self, x: i32, y: i32) {
        let x = x.clamp(0, self.w - 1);
        let y = y.clamp(0, self.h - 1);
        self.cur = (x, y);
        let hv = if self.drag == Drag::None { self.hit(x, y) } else { None };
        let hover_changed = hv != self.hover;
        self.hover = hv;
        self.update_cursor();
        match self.drag {
            Drag::Select => {
                if self.mode == Mode::Free {
                    if let Some(l) = self.lasso.last() {
                        if (l.0 - x).abs() + (l.1 - y).abs() >= 2 {
                            self.lasso.push((x, y));
                        }
                    }
                }
                self.win.request_redraw();
            }
            Drag::Resize(m) => {
                let min = (8.0 * self.s).round() as i32;
                let mut r = self.sel;
                if m & 1 != 0 {
                    r.l = x.min(r.r - min).max(0);
                }
                if m & 4 != 0 {
                    r.r = (x + 1).max(r.l + min).min(self.w);
                }
                if m & 2 != 0 {
                    r.t = y.min(r.b - min).max(0);
                }
                if m & 8 != 0 {
                    r.b = (y + 1).max(r.t + min).min(self.h);
                }
                if r != self.sel {
                    self.sel = r;
                    if self.mode == Mode::Full {
                        self.mode = Mode::Region;
                    }
                    self.render_work();
                    self.layout();
                    self.win.request_redraw();
                }
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
                    Some(Ann::Blur(r, _)) | Some(Ann::Censor(r)) => *r = norm(start, (x, y)),
                    _ => {}
                }
                self.render_work();
                self.win.request_redraw();
            }
            Drag::None => {
                if !self.editing || hover_changed {
                    self.win.request_redraw();
                }
            }
        }
    }

    fn on_up(&mut self, x: i32, y: i32) {
        let x = x.clamp(0, self.w - 1);
        let y = y.clamp(0, self.h - 1);
        match self.drag {
            Drag::Select => {
                self.drag = Drag::None;
                if self.mode == Mode::Free {
                    let r = self.lasso_bounds();
                    if self.lasso.len() < 3 || r.w() < 5 || r.h() < 5 {
                        self.lasso.clear();
                    } else {
                        self.sel = r;
                        self.enter_edit();
                    }
                } else {
                    let r = norm(self.start, (x, y));
                    if r.w() < 4 && r.h() < 4 {
                        self.select_monitor();
                    } else {
                        self.sel = r;
                        self.enter_edit();
                    }
                }
                self.layout();
            }
            Drag::Resize(_) => {
                self.drag = Drag::None;
                self.layout();
            }
            Drag::Ann => {
                self.drag = Drag::None;
                if let Some(p) = self.pending.take() {
                    let keep = match &p {
                        Ann::Line { a, b, .. } => (a.0 - b.0).abs() + (a.1 - b.1).abs() > 3.0,
                        Ann::Blur(r, _) | Ann::Censor(r) => r.w() > 2 && r.h() > 2,
                        _ => true,
                    };
                    if keep {
                        self.anns.push(p);
                    }
                }
                self.render_work();
            }
            Drag::None => return,
        }
        self.update_cursor();
        self.win.request_redraw();
    }

    fn on_key(&mut self, key: &Key, text: Option<&str>) -> Option<Done> {
        let ctrl = self.mods.control_key();
        let ch = match key {
            Key::Character(c) => c.to_lowercase().chars().next(),
            _ => None,
        };
        if self.text.is_some() && ctrl {
            // Ctrl+C / Ctrl+S still finish the capture while typing (the text is kept)
            match ch {
                Some('c') => return self.finish(Out::Copy),
                Some('s') => return self.finish(Out::Save),
                _ => return None,
            }
        }
        if let Some(t) = &mut self.text {
            match key {
                Key::Named(NamedKey::Backspace) => {
                    t.s.pop();
                }
                Key::Named(NamedKey::Enter) if self.mods.shift_key() => t.s.push('\n'), // new line
                Key::Named(NamedKey::Enter) => self.commit_text(),
                Key::Named(NamedKey::Escape) => self.text = None,
                _ => {
                    if let Some(s) = text.filter(|_| !ctrl) {
                        t.s.extend(s.chars().filter(|c| !c.is_control()));
                    }
                }
            }
            self.win.request_redraw();
            return None;
        }
        match (key, ch) {
            (Key::Named(NamedKey::Escape), _) => {
                if self.drag != Drag::None {
                    self.drag = Drag::None;
                    self.pending = None;
                    self.render_work();
                    self.win.request_redraw();
                    None
                } else {
                    Some(Done::Cancel)
                }
            }
            (Key::Named(NamedKey::Enter), _) if self.editing => self.finish(Out::Default),
            (_, Some('c')) if ctrl && self.editing => self.finish(Out::Copy),
            (_, Some('s')) if ctrl && self.editing => self.finish(Out::Save),
            (_, Some('z')) if ctrl && self.editing => self.press(Btn::Undo),
            _ if ctrl => None,
            (_, Some(c)) if self.editing => match c {
                't' => self.press(Btn::Tool(Tool::Text)),
                'b' => self.press(Btn::Tool(Tool::Blur)),
                'x' => self.press(Btn::Tool(Tool::Censor)),
                'a' => self.press(Btn::Tool(Tool::Arrow)),
                'l' => self.press(Btn::Tool(Tool::Line)),
                'p' => self.press(Btn::Tool(Tool::Pen)),
                'c' => self.press(Btn::Color),
                _ => None,
            },
            (_, Some(c)) => match c {
                'r' => self.press(Btn::Mode(Mode::Region)),
                'f' => self.press(Btn::Mode(Mode::Free)),
                'w' => self.press(Btn::Mode(Mode::Full)),
                'v' => self.press(Btn::Record),
                _ => None,
            },
            _ => None,
        }
    }

    /// Scroll wheel: thickness for line / arrow / pencil, strength for blur.
    fn on_wheel(&mut self, notches: f32) {
        if !self.editing {
            return;
        }
        self.wheel += notches;
        let steps = self.wheel.trunc() as i32;
        if steps == 0 {
            return;
        }
        self.wheel -= steps as f32;
        match self.tool {
            Tool::Arrow | Tool::Line | Tool::Pen => {
                self.width = (self.width + steps as f32).clamp(1.0, 30.0);
                let w = self.width * self.s;
                if let Some(Ann::Line { w: pw, .. } | Ann::Stroke { w: pw, .. }) = &mut self.pending {
                    *pw = w;
                }
                self.hint = Some((format!("Thickness {}", self.width as i32), Instant::now() + std::time::Duration::from_millis(1100)));
            }
            Tool::Blur => {
                self.blur_k = (self.blur_k * 1.25f32.powi(steps)).clamp(0.25, 6.0);
                if let Some(Ann::Blur(_, k)) = &mut self.pending {
                    *k = self.blur_k;
                }
                self.hint = Some((format!("Blur {}%", (self.blur_k * 100.0).round() as i32), Instant::now() + std::time::Duration::from_millis(1100)));
            }
            _ => return,
        }
        self.render_work();
        self.win.request_redraw();
    }

    /// Hides the scroll label once its time is up; returns when to check again.
    pub fn tick(&mut self) -> Option<Instant> {
        let until = self.hint.as_ref()?.1;
        if Instant::now() >= until {
            self.hint = None;
            self.win.request_redraw();
            return None;
        }
        Some(until)
    }

    /// Feeds a window event; returns what to do once the user has decided.
    pub fn event(&mut self, ev: &WindowEvent) -> Option<Done> {
        let done = self.handle(ev);
        if done.is_none() && self.record && self.editing {
            // in record mode, choosing the area is the whole job
            let (sx, sy) = (self.src.w as f32 / self.w as f32, self.src.h as f32 / self.h as f32);
            let r = self.sel;
            let area = ((r.l as f32 * sx) as i32, (r.t as f32 * sy) as i32, (r.w() as f32 * sx) as i32, (r.h() as f32 * sy) as i32);
            return Some(Done::Record { area, whole: self.mode == Mode::Full });
        }
        done
    }

    fn handle(&mut self, ev: &WindowEvent) -> Option<Done> {
        match ev {
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::Resized(sz) => {
                self.resize(sz.width as i32, sz.height as i32);
                self.win.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.s = *scale_factor as f32;
                self.layout();
                self.win.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::CursorEntered { .. } => self.inside = true,
            WindowEvent::CursorLeft { .. } => {
                self.inside = false;
                self.hover = None;
                self.win.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => *y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                };
                self.on_wheel(notches);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.inside = true;
                self.on_move(position.x as i32, position.y as i32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (x, y) = self.cur;
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        let now = Instant::now();
                        let dbl = self.last_click.is_some_and(|(t, px, py)| now.duration_since(t).as_millis() < 450 && (px - x).abs() < 5 && (py - y).abs() < 5);
                        self.last_click = if dbl { None } else { Some((now, x, y)) };
                        if dbl && self.editing && self.tool == Tool::None && self.sel.has(x, y) && self.handle_at(x, y).is_none() && self.hit(x, y).is_none() {
                            return self.finish(Out::Default);
                        }
                        return self.on_down(x, y);
                    }
                    (MouseButton::Left, ElementState::Released) => self.on_up(x, y),
                    (MouseButton::Right, ElementState::Pressed) => {
                        if self.text.is_some() {
                            self.text = None;
                            self.win.request_redraw();
                        } else if self.editing {
                            self.back_to_select();
                        } else {
                            return Some(Done::Cancel);
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                return self.on_key(&event.logical_key, event.text.as_deref());
            }
            WindowEvent::CloseRequested => return Some(Done::Cancel),
            _ => {}
        }
        None
    }
}
