//! tiny-skia drawing with the same API as the Windows GDI+ wrapper, so shared
//! code (icons) and ported UI code draw identically on both platforms.
//!
//! Pixels are the same u32 0xAARRGGBB values Windows uses (premultiplied while
//! drawing). In memory that is B,G,R,A, while tiny-skia thinks in R,G,B,A, so
//! colors are handed to tiny-skia with red and blue swapped. The result is a
//! buffer softbuffer can show directly.
use crate::img::Img;
use ab_glyph::{Font, FontVec, OutlineCurve, PxScale, ScaleFont};
use std::cell::RefCell;
use std::sync::OnceLock;
use tiny_skia as sk;

// text flags (same values as on Windows)
pub const BOLD: u32 = 1;
pub const CENTER: u32 = 2;
pub const RIGHT: u32 = 4;
pub const WRAP: u32 = 8;
pub const PATH_TRIM: u32 = 16;
pub const TOP: u32 = 32;
pub const SEMI: u32 = 64;
pub const SERIF: u32 = 128;

fn col(c: u32) -> sk::Color {
    sk::Color::from_rgba8((c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8, (c >> 24) as u8)
}

// ---------------------------------------------------------------- paths

pub struct Path(pub Option<sk::Path>);

/// Appends an elliptical arc (GDI+ AddPathArc semantics: bounding box, degrees,
/// clockwise on screen) as cubic segments.
fn arc_into(pb: &mut sk::PathBuilder, x: f32, y: f32, w: f32, h: f32, start: f32, sweep: f32, connect: bool) {
    let (rx, ry) = (w / 2.0, h / 2.0);
    let (cx, cy) = (x + rx, y + ry);
    let n = (sweep.abs() / 90.0).ceil().max(1.0) as usize;
    let step = sweep.to_radians() / n as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut a = start.to_radians();
    let pt = |a: f32| (cx + rx * a.cos(), cy + ry * a.sin());
    let (sx, sy) = pt(a);
    if connect && pb.len() > 0 {
        pb.line_to(sx, sy);
    } else {
        pb.move_to(sx, sy);
    }
    for _ in 0..n {
        let b = a + step;
        let (x0, y0) = pt(a);
        let (x3, y3) = pt(b);
        let (x1, y1) = (x0 - k * rx * a.sin(), y0 + k * ry * a.cos());
        let (x2, y2) = (x3 + k * rx * b.sin(), y3 - k * ry * b.cos());
        pb.cubic_to(x1, y1, x2, y2, x3, y3);
        a = b;
    }
}

impl Path {
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Path {
        Path(sk::Rect::from_xywh(x, y, w.max(0.01), h.max(0.01)).map(sk::PathBuilder::from_rect))
    }
    pub fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
        Path::rrect4(x, y, w, h, r, r, r, r)
    }
    pub fn rrect4(x: f32, y: f32, w: f32, h: f32, tl: f32, tr: f32, br: f32, bl: f32) -> Path {
        let m = w.min(h) / 2.0;
        let c = |r: f32| r.min(m).max(0.01);
        let (tl, tr, br, bl) = (c(tl), c(tr), c(br), c(bl));
        let mut pb = sk::PathBuilder::new();
        arc_into(&mut pb, x, y, tl * 2.0, tl * 2.0, 180.0, 90.0, false);
        arc_into(&mut pb, x + w - tr * 2.0, y, tr * 2.0, tr * 2.0, 270.0, 90.0, true);
        arc_into(&mut pb, x + w - br * 2.0, y + h - br * 2.0, br * 2.0, br * 2.0, 0.0, 90.0, true);
        arc_into(&mut pb, x, y + h - bl * 2.0, bl * 2.0, bl * 2.0, 90.0, 90.0, true);
        pb.close();
        Path(pb.finish())
    }
    pub fn ellipse(x: f32, y: f32, w: f32, h: f32) -> Path {
        Path(sk::Rect::from_xywh(x, y, w.max(0.01), h.max(0.01)).and_then(sk::PathBuilder::from_oval))
    }
    pub fn circle(cx: f32, cy: f32, r: f32) -> Path {
        Path::ellipse(cx - r, cy - r, r * 2.0, r * 2.0)
    }
    pub fn poly(pts: &[(f32, f32)]) -> Path {
        let mut pb = sk::PathBuilder::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        pb.close();
        Path(pb.finish())
    }
    pub fn polyline(pts: &[(f32, f32)]) -> Path {
        let mut pb = sk::PathBuilder::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        Path(pb.finish())
    }
    pub fn arc(x: f32, y: f32, w: f32, h: f32, start: f32, sweep: f32) -> Path {
        let mut pb = sk::PathBuilder::new();
        arc_into(&mut pb, x, y, w, h, start, sweep, false);
        Path(pb.finish())
    }
}

// ---------------------------------------------------------------- fonts

struct Fonts {
    regular: Option<FontVec>,
    semi: Option<FontVec>,
    bold: Option<FontVec>,
    serif: Option<FontVec>,
}

fn fc_match(pattern: &str) -> Option<String> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    (!s.is_empty()).then_some(s)
}

fn load_font(pattern: &str, fallbacks: &[&str]) -> Option<FontVec> {
    let mut paths: Vec<String> = fc_match(pattern).into_iter().collect();
    paths.extend(fallbacks.iter().map(|s| s.to_string()));
    for p in paths {
        if let Ok(data) = std::fs::read(&p) {
            // .ttc collections: take the first face
            if let Ok(f) = FontVec::try_from_vec_and_index(data, 0) {
                return Some(f);
            }
        }
    }
    None
}

fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(|| {
        let reg = ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"];
        let bold = ["/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf"];
        Fonts {
            regular: load_font("sans-serif", &reg),
            semi: load_font("sans-serif:weight=demibold", &bold).or_else(|| load_font("sans-serif:bold", &bold)),
            bold: load_font("sans-serif:bold", &bold),
            serif: load_font("serif", &["/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf"]),
        }
    })
}

fn font_for(fl: u32) -> Option<&'static FontVec> {
    let f = fonts();
    let pick = if fl & SERIF != 0 {
        f.serif.as_ref()
    } else if fl & BOLD != 0 {
        f.bold.as_ref()
    } else if fl & SEMI != 0 {
        f.semi.as_ref()
    } else {
        f.regular.as_ref()
    };
    pick.or(f.regular.as_ref())
}

/// `size` is the em size in pixels, like GDI+ UnitWorld.
fn scale_for(font: &FontVec, size: f32) -> PxScale {
    let upem = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(size * font.height_unscaled() / upem)
}

fn advance(font: &FontVec, size: f32, s: &str) -> f32 {
    let sf = font.as_scaled(scale_for(font, size));
    let mut w = 0.0;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

fn line_height(font: &FontVec, size: f32) -> (f32, f32) {
    let sf = font.as_scaled(scale_for(font, size));
    (sf.ascent(), sf.ascent() - sf.descent() + sf.line_gap())
}

/// Glyph outlines of `s` with its baseline starting at (x, y), as one path.
fn text_path(font: &FontVec, size: f32, s: &str, x: f32, y: f32) -> Option<sk::Path> {
    let scale = scale_for(font, size);
    let sf = font.as_scaled(scale);
    let k = size / font.units_per_em().unwrap_or(1000.0);
    let mut pb = sk::PathBuilder::new();
    let mut pen = x;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            pen += sf.kern(p, id);
        }
        if let Some(o) = font.outline(id) {
            let m = |p: ab_glyph::Point| (pen + p.x * k, y - p.y * k);
            let mut last: Option<(f32, f32)> = None;
            for c in &o.curves {
                let (start, end) = match c {
                    OutlineCurve::Line(a, b) => (*a, *b),
                    OutlineCurve::Quad(a, _, b) => (*a, *b),
                    OutlineCurve::Cubic(a, _, _, b) => (*a, *b),
                };
                let s0 = m(start);
                if last.is_none_or(|l| (l.0 - s0.0).abs() > 0.01 || (l.1 - s0.1).abs() > 0.01) {
                    if last.is_some() {
                        pb.close();
                    }
                    pb.move_to(s0.0, s0.1);
                }
                match c {
                    OutlineCurve::Line(_, b) => {
                        let b = m(*b);
                        pb.line_to(b.0, b.1)
                    }
                    OutlineCurve::Quad(_, c1, b) => {
                        let (c1, b) = (m(*c1), m(*b));
                        pb.quad_to(c1.0, c1.1, b.0, b.1)
                    }
                    OutlineCurve::Cubic(_, c1, c2, b) => {
                        let (c1, c2, b) = (m(*c1), m(*c2), m(*b));
                        pb.cubic_to(c1.0, c1.1, c2.0, c2.1, b.0, b.1)
                    }
                }
                last = Some(m(end));
            }
            if last.is_some() {
                pb.close();
            }
        }
        pen += sf.h_advance(id);
        prev = Some(id);
    }
    pb.finish()
}

/// Shortens `s` with an ellipsis (at the end, or in the middle for paths) to fit `w`.
fn ellipsize(font: &FontVec, size: f32, s: &str, w: f32, middle: bool) -> String {
    if advance(font, size, s) <= w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let t: String = if middle {
            let head = n / 3;
            let tail = n - head;
            chars[..head].iter().chain(['…'].iter()).chain(chars[chars.len() - tail..].iter()).collect()
        } else {
            chars[..n].iter().chain(['…'].iter()).collect()
        };
        if advance(font, size, &t) <= w {
            return t;
        }
    }
    "…".into()
}

fn wrap_lines(font: &FontVec, size: f32, s: &str, w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in s.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if !cur.is_empty() && advance(font, size, &cand) > w {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_string();
            } else {
                cur = cand;
            }
        }
        lines.push(cur);
    }
    lines
}

// ---------------------------------------------------------------- canvas

struct State {
    ts: sk::Transform,
    clip: Option<sk::Mask>,
    stack: Vec<(sk::Transform, Option<sk::Mask>)>,
}

/// A drawing context over a borrowed pixel buffer (`w`×`h` u32s).
pub struct Gfx {
    px: *mut u8,
    w: u32,
    h: u32,
    st: RefCell<State>,
}

#[allow(dead_code)] // keeps the full API of the Windows wrapper
impl Gfx {
    /// # Safety
    /// `px` must point to `w * h` u32s that stay valid and unaliased while the Gfx lives.
    pub unsafe fn over(px: *mut u32, w: i32, h: i32) -> Gfx {
        Gfx { px: px as *mut u8, w: w.max(1) as u32, h: h.max(1) as u32, st: RefCell::new(State { ts: sk::Transform::identity(), clip: None, stack: Vec::new() }) }
    }
    pub fn over_slice(px: &mut [u32], w: i32, h: i32) -> Gfx {
        assert!(px.len() >= (w * h) as usize);
        unsafe { Gfx::over(px.as_mut_ptr(), w, h) }
    }

    fn with<R>(&self, f: impl FnOnce(&mut sk::PixmapMut, sk::Transform, Option<&sk::Mask>) -> R) -> Option<R> {
        let bytes = unsafe { std::slice::from_raw_parts_mut(self.px, (self.w * self.h * 4) as usize) };
        let mut pm = sk::PixmapMut::from_bytes(bytes, self.w, self.h)?;
        let st = self.st.borrow();
        Some(f(&mut pm, st.ts, st.clip.as_ref()))
    }

    fn solid(c: u32) -> sk::Paint<'static> {
        let mut p = sk::Paint::default();
        p.set_color(col(c));
        p.anti_alias = true;
        p
    }

    fn fill_paint(&self, p: &Path, paint: &sk::Paint, rule: sk::FillRule) {
        let Some(path) = &p.0 else { return };
        self.with(|pm, ts, clip| pm.fill_path(path, paint, rule, ts, clip));
    }

    pub fn save(&self) -> u32 {
        let mut st = self.st.borrow_mut();
        let snap = (st.ts, st.clip.clone());
        st.stack.push(snap);
        st.stack.len() as u32 - 1
    }
    pub fn restore(&self, s: u32) {
        let mut st = self.st.borrow_mut();
        if (s as usize) < st.stack.len() {
            let (ts, clip) = st.stack[s as usize].clone();
            st.stack.truncate(s as usize);
            st.ts = ts;
            st.clip = clip;
        }
    }
    pub fn scale(&self, s: f32) {
        let mut st = self.st.borrow_mut();
        st.ts = st.ts.pre_scale(s, s);
    }
    pub fn translate(&self, x: f32, y: f32) {
        let mut st = self.st.borrow_mut();
        st.ts = st.ts.pre_translate(x, y);
    }
    pub fn clear(&self, c: u32) {
        let color = col(c).premultiply().to_color_u8();
        let v = u32::from_ne_bytes([color.red(), color.green(), color.blue(), color.alpha()]);
        let px = unsafe { std::slice::from_raw_parts_mut(self.px as *mut u32, (self.w * self.h) as usize) };
        px.fill(v);
    }
    pub fn fill(&self, p: &Path, c: u32) {
        self.fill_paint(p, &Self::solid(c), sk::FillRule::Winding);
    }
    pub fn rect(&self, x: f32, y: f32, w: f32, h: f32, c: u32) {
        self.fill(&Path::rect(x, y, w, h), c);
    }
    pub fn fill_v(&self, p: &Path, y0: f32, y1: f32, c0: u32, c1: u32) {
        self.fill_lin(p, (0.0, y0), (0.0, y1), c0, c1)
    }
    pub fn fill_lin(&self, p: &Path, a: (f32, f32), b: (f32, f32), c0: u32, c1: u32) {
        let stops = vec![sk::GradientStop::new(0.0, col(c0)), sk::GradientStop::new(1.0, col(c1))];
        let Some(shader) = sk::LinearGradient::new(sk::Point::from_xy(a.0, a.1 - 0.01), sk::Point::from_xy(b.0, b.1 + 0.01), stops, sk::SpreadMode::Pad, sk::Transform::identity()) else {
            return self.fill(p, c0);
        };
        let paint = sk::Paint { shader, anti_alias: true, ..Default::default() };
        self.fill_paint(p, &paint, sk::FillRule::Winding);
    }
    /// Center color at (cx, cy) fading to `surround` at the shape's edge.
    pub fn fill_radial(&self, p: &Path, cx: f32, cy: f32, center: u32, surround: u32) {
        let Some(path) = &p.0 else { return };
        let b = path.bounds();
        let rx = (cx - b.left()).max(b.right() - cx).max(0.5);
        let ry = (cy - b.top()).max(b.bottom() - cy).max(0.5);
        let stops = vec![sk::GradientStop::new(0.0, col(center)), sk::GradientStop::new(1.0, col(surround))];
        // circular gradient of radius rx, squashed vertically to an ellipse around the center
        let ts = sk::Transform::from_translate(cx, cy).pre_scale(1.0, ry / rx).pre_translate(-cx, -cy);
        let Some(shader) = sk::RadialGradient::new(sk::Point::from_xy(cx, cy), sk::Point::from_xy(cx, cy), rx, stops, sk::SpreadMode::Pad, ts) else {
            return self.fill(p, center);
        };
        let paint = sk::Paint { shader, anti_alias: true, ..Default::default() };
        self.fill_paint(p, &paint, sk::FillRule::Winding);
    }
    pub fn stroke(&self, p: &Path, c: u32, w: f32) {
        let Some(path) = &p.0 else { return };
        let stroke = sk::Stroke { width: w, line_cap: sk::LineCap::Round, line_join: sk::LineJoin::Round, ..Default::default() };
        let paint = Self::solid(c);
        self.with(|pm, ts, clip| pm.stroke_path(path, &paint, &stroke, ts, clip));
    }
    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, c: u32, w: f32) {
        self.stroke(&Path::polyline(&[(x1, y1), (x2, y2)]), c, w);
    }
    pub fn polyline(&self, pts: &[(f32, f32)], c: u32, w: f32) {
        if pts.len() >= 2 {
            self.stroke(&Path::polyline(pts), c, w)
        }
    }
    pub fn fill_poly_winding(&self, pts: &[(f32, f32)], c: u32) {
        self.fill_paint(&Path::poly(pts), &Self::solid(c), sk::FillRule::Winding);
    }
    /// Line with a filled, slightly rounded arrow head at (x2,y2).
    pub fn arrow(&self, x1: f32, y1: f32, x2: f32, y2: f32, c: u32, w: f32) {
        let (dx, dy) = (x2 - x1, y2 - y1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0 {
            return;
        }
        let (ux, uy) = (dx / len, dy / len);
        let head = (w * 3.2 + 6.0).min(len * 0.55);
        let hw = head * 0.62;
        let (bx, by) = (x2 - ux * head, y2 - uy * head);
        self.line(x1, y1, bx + ux * 1.5, by + uy * 1.5, c, w);
        let tri = Path::poly(&[(x2, y2), (bx - uy * hw, by + ux * hw), (bx + uy * hw, by - ux * hw)]);
        self.fill(&tri, c);
        self.stroke(&tri, c, w * 0.45);
    }

    fn draw_line_at(&self, font: &FontVec, s: &str, x: f32, baseline: f32, size: f32, c: u32) {
        if let Some(path) = text_path(font, size, s, x, baseline) {
            let paint = Self::solid(c);
            self.with(|pm, ts, clip| pm.fill_path(&path, &paint, sk::FillRule::Winding, ts, clip));
        }
    }

    /// Text laid out in a box (alignment, wrapping and ellipsis per `fl`).
    pub fn text(&self, s: &str, x: f32, y: f32, w: f32, h: f32, size: f32, c: u32, fl: u32) {
        let Some(font) = font_for(fl) else { return };
        let lines = if fl & WRAP != 0 { wrap_lines(font, size, s, w) } else { vec![ellipsize(font, size, s, w, fl & PATH_TRIM != 0)] };
        let (ascent, lh) = line_height(font, size);
        let total = lh * lines.len() as f32;
        let mut ty = if fl & (TOP | WRAP) != 0 { y } else { y + (h - total) / 2.0 };
        for line in &lines {
            if fl & WRAP != 0 && ty + lh > y + h + 0.5 {
                break;
            }
            let lw = advance(font, size, line);
            let lx = if fl & CENTER != 0 {
                x + (w - lw) / 2.0
            } else if fl & RIGHT != 0 {
                x + w - lw
            } else {
                x
            };
            self.draw_line_at(font, line, lx, ty + ascent, size, c);
            ty += lh;
        }
    }
    /// Unbounded text whose top-left is (x,y).
    pub fn text_at(&self, s: &str, x: f32, y: f32, size: f32, c: u32, fl: u32) {
        let Some(font) = font_for(fl) else { return };
        let (ascent, _) = line_height(font, size);
        self.draw_line_at(font, s, x, y + ascent, size, c);
    }
    pub fn measure_at(&self, s: &str, size: f32, fl: u32) -> (f32, f32) {
        let Some(font) = font_for(fl) else { return (s.chars().count() as f32 * size * 0.55, size * 1.2) };
        let (_, lh) = line_height(font, size);
        (advance(font, size, s), lh.max(size * 1.2))
    }

    /// Draws `img` covering the destination rect (center-cropped).
    pub fn image_cover(&self, img: &Pixels, x: f32, y: f32, w: f32, h: f32) {
        let (iw, ih) = (img.0.width() as f32, img.0.height() as f32);
        let sc = (w / iw).max(h / ih);
        let (ox, oy) = (x + (w - iw * sc) / 2.0, y + (h - ih * sc) / 2.0);
        let pattern = sk::Pattern::new(img.0.as_ref(), sk::SpreadMode::Pad, sk::FilterQuality::Bicubic, 1.0, sk::Transform::from_row(sc, 0.0, 0.0, sc, ox, oy));
        let paint = sk::Paint { shader: pattern, anti_alias: true, ..Default::default() };
        self.fill_paint(&Path::rect(x, y, w, h), &paint, sk::FillRule::Winding);
    }
    pub fn clip(&self, p: &Path) {
        let Some(path) = &p.0 else { return };
        let mut st = self.st.borrow_mut();
        let mut m = sk::Mask::new(self.w, self.h).unwrap();
        m.fill_path(path, sk::FillRule::Winding, true, st.ts);
        st.clip = Some(m);
    }
    pub fn clip_rect(&self, x: f32, y: f32, w: f32, h: f32) {
        self.clip(&Path::rect(x, y, w, h))
    }
    pub fn unclip(&self) {
        self.st.borrow_mut().clip = None;
    }
}

/// An image ready to be drawn (premultiplied, channel order as above).
pub struct Pixels(sk::Pixmap);

impl Pixels {
    pub fn from_img(img: &Img) -> Option<Pixels> {
        let mut pm = sk::Pixmap::new(img.w.max(1) as u32, img.h.max(1) as u32)?;
        for (d, &p) in pm.pixels_mut().iter_mut().zip(img.px.iter()) {
            let a = p >> 24;
            let pre = |v: u32| ((v * a + 127) / 255) as u8;
            *d = sk::PremultipliedColorU8::from_rgba(pre(p & 255), pre((p >> 8) & 255), pre((p >> 16) & 255), a as u8).unwrap_or(sk::PremultipliedColorU8::TRANSPARENT);
        }
        Some(Pixels(pm))
    }
}

/// Draws into a new transparent `w`×`h` image (straight alpha).
pub fn paint(w: i32, h: i32, f: impl FnOnce(&Gfx)) -> Img {
    let mut px = vec![0u32; (w * h).max(0) as usize];
    {
        let g = Gfx::over_slice(&mut px, w, h);
        f(&g);
    }
    for p in px.iter_mut() {
        let a = *p >> 24;
        if a != 0 && a != 255 {
            let un = |v: u32| ((v * 255 + a / 2) / a).min(255);
            *p = (a << 24) | (un((*p >> 16) & 255) << 16) | (un((*p >> 8) & 255) << 8) | un(*p & 255);
        }
    }
    Img { w, h, px, alpha: true }
}

// ---------------------------------------------------------------- PNG

pub fn encode_png(img: &Img) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.w as u32, img.h as u32);
        enc.set_color(if img.alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let Ok(mut wr) = enc.write_header() else { return out };
        let mut data = Vec::with_capacity(img.px.len() * 4);
        for &p in &img.px {
            data.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            if img.alpha {
                data.push((p >> 24) as u8);
            }
        }
        let _ = wr.write_image_data(&data);
    }
    out
}

pub fn decode_png(bytes: &[u8]) -> Option<Img> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut rd = dec.read_info().ok()?;
    let mut buf = vec![0u8; rd.output_buffer_size()];
    let info = rd.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as i32, info.height as i32);
    let ch = info.color_type.samples();
    let mut px = Vec::with_capacity((w * h) as usize);
    for c in buf[..info.buffer_size()].chunks_exact(ch) {
        let (r, g, b, a) = match ch {
            1 => (c[0], c[0], c[0], 255),
            2 => (c[0], c[0], c[0], c[1]),
            3 => (c[0], c[1], c[2], 255),
            _ => (c[0], c[1], c[2], c[3]),
        };
        px.push(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
    }
    let alpha = px.iter().any(|p| p >> 24 != 255);
    Some(Img { w, h, px, alpha })
}

pub fn decode_file(path: &str) -> Option<Img> {
    decode_png(&std::fs::read(path).ok()?)
}
