//! Thin GDI+ wrapper: anti-aliased paths, gradients, text, PNG encode/decode.
use crate::util::{wz, Img};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::HGLOBAL;
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::Graphics::GdiPlus::*;
use windows_sys::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows_sys::Win32::UI::WindowsAndMessaging::HICON;

pub const PF_RGB: i32 = 0x0002_2009;
pub const PF_ARGB: i32 = 0x0026_200A;
pub const PF_PARGB: i32 = 0x000E_200B;

// text flags
pub const BOLD: u32 = 1;
pub const CENTER: u32 = 2;
pub const RIGHT: u32 = 4;
pub const WRAP: u32 = 8;
pub const PATH_TRIM: u32 = 16;
pub const TOP: u32 = 32;
pub const SEMI: u32 = 64;
pub const SERIF: u32 = 128;

pub fn startup() {
    unsafe {
        let mut tok = 0usize;
        let inp = GdiplusStartupInput { GdiplusVersion: 1, DebugEventCallback: 0, SuppressBackgroundThread: 0, SuppressExternalCodecs: 0 };
        GdiplusStartup(&mut tok, &inp, null_mut());
    }
}

pub struct Path(pub *mut GpPath);
impl Drop for Path {
    fn drop(&mut self) {
        unsafe { GdipDeletePath(self.0) };
    }
}
impl Path {
    pub fn new() -> Path {
        let mut p = null_mut();
        unsafe { GdipCreatePath(FillModeAlternate, &mut p) };
        Path(p)
    }
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Path {
        let p = Path::new();
        unsafe { GdipAddPathRectangle(p.0, x, y, w, h) };
        p
    }
    pub fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
        Path::rrect4(x, y, w, h, r, r, r, r)
    }
    pub fn rrect4(x: f32, y: f32, w: f32, h: f32, tl: f32, tr: f32, br: f32, bl: f32) -> Path {
        let p = Path::new();
        let m = w.min(h) / 2.0;
        let c = |r: f32| r.min(m).max(0.01);
        let (tl, tr, br, bl) = (c(tl), c(tr), c(br), c(bl));
        unsafe {
            GdipAddPathArc(p.0, x, y, tl * 2.0, tl * 2.0, 180.0, 90.0);
            GdipAddPathArc(p.0, x + w - tr * 2.0, y, tr * 2.0, tr * 2.0, 270.0, 90.0);
            GdipAddPathArc(p.0, x + w - br * 2.0, y + h - br * 2.0, br * 2.0, br * 2.0, 0.0, 90.0);
            GdipAddPathArc(p.0, x, y + h - bl * 2.0, bl * 2.0, bl * 2.0, 90.0, 90.0);
            GdipClosePathFigure(p.0);
        }
        p
    }
    pub fn ellipse(x: f32, y: f32, w: f32, h: f32) -> Path {
        let p = Path::new();
        unsafe { GdipAddPathEllipse(p.0, x, y, w, h) };
        p
    }
    pub fn circle(cx: f32, cy: f32, r: f32) -> Path {
        Path::ellipse(cx - r, cy - r, r * 2.0, r * 2.0)
    }
    pub fn poly(pts: &[(f32, f32)]) -> Path {
        let p = Path::new();
        let v: Vec<PointF> = pts.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
        unsafe { GdipAddPathPolygon(p.0, v.as_ptr(), v.len() as i32) };
        p
    }
    pub fn polyline(pts: &[(f32, f32)]) -> Path {
        let p = Path::new();
        let v: Vec<PointF> = pts.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
        unsafe { GdipAddPathLine2(p.0, v.as_ptr(), v.len() as i32) };
        p
    }
    pub fn arc(x: f32, y: f32, w: f32, h: f32, start: f32, sweep: f32) -> Path {
        let p = Path::new();
        unsafe { GdipAddPathArc(p.0, x, y, w, h, start, sweep) };
        p
    }
}

unsafe fn pen(c: u32, w: f32) -> *mut GpPen {
    let mut p = null_mut();
    GdipCreatePen1(c, w, UnitWorld, &mut p);
    GdipSetPenStartCap(p, LineCapRound);
    GdipSetPenEndCap(p, LineCapRound);
    GdipSetPenLineJoin(p, LineJoinRound);
    p
}

unsafe fn font(fl: u32, size: f32) -> (*mut GpFontFamily, *mut GpFont) {
    let name = if fl & SERIF != 0 { "Georgia" } else if fl & SEMI != 0 { "Segoe UI Semibold" } else { "Segoe UI" };
    let mut fam = null_mut();
    if GdipCreateFontFamilyFromName(wz(name).as_ptr(), null_mut(), &mut fam) != 0 {
        GdipGetGenericFontFamilySansSerif(&mut fam);
    }
    let mut f = null_mut();
    GdipCreateFont(fam, size, if fl & BOLD != 0 { FontStyleBold } else { FontStyleRegular }, UnitWorld, &mut f);
    (fam, f)
}

unsafe fn free_font(ff: (*mut GpFontFamily, *mut GpFont)) {
    GdipDeleteFont(ff.1);
    GdipDeleteFontFamily(ff.0);
}

unsafe fn typographic() -> *mut GpStringFormat {
    let mut g = null_mut();
    GdipStringFormatGetGenericTypographic(&mut g);
    let mut f = null_mut();
    GdipCloneStringFormat(g, &mut f);
    GdipSetStringFormatFlags(f, 0x0800 | 0x1000 | 0x4000); // trailing spaces, no wrap, no clip
    f
}

pub struct Gfx(pub *mut GpGraphics);
impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe { GdipDeleteGraphics(self.0) };
    }
}

impl Gfx {
    pub unsafe fn from_hdc(hdc: HDC) -> Gfx {
        let mut g = null_mut();
        GdipCreateFromHDC(hdc, &mut g);
        Gfx(g).init()
    }
    pub unsafe fn from_image(img: *mut GpBitmap) -> Gfx {
        let mut g = null_mut();
        GdipGetImageGraphicsContext(img as *mut GpImage, &mut g);
        Gfx(g).init()
    }
    unsafe fn init(self) -> Gfx {
        GdipSetSmoothingMode(self.0, SmoothingModeAntiAlias);
        GdipSetPixelOffsetMode(self.0, PixelOffsetModeHalf);
        GdipSetTextRenderingHint(self.0, TextRenderingHintAntiAliasGridFit);
        GdipSetInterpolationMode(self.0, InterpolationModeHighQualityBicubic);
        GdipSetCompositingQuality(self.0, CompositingQualityHighQuality);
        self
    }
    pub fn save(&self) -> u32 {
        let mut s = 0;
        unsafe { GdipSaveGraphics(self.0, &mut s) };
        s
    }
    pub fn restore(&self, s: u32) {
        unsafe { GdipRestoreGraphics(self.0, s) };
    }
    pub fn scale(&self, s: f32) {
        unsafe { GdipScaleWorldTransform(self.0, s, s, MatrixOrderPrepend) };
    }
    pub fn translate(&self, x: f32, y: f32) {
        unsafe { GdipTranslateWorldTransform(self.0, x, y, MatrixOrderPrepend) };
    }
    pub fn clear(&self, c: u32) {
        unsafe { GdipGraphicsClear(self.0, c) };
    }
    pub fn fill(&self, p: &Path, c: u32) {
        unsafe {
            let mut b = null_mut();
            GdipCreateSolidFill(c, &mut b);
            GdipFillPath(self.0, b as *mut GpBrush, p.0);
            GdipDeleteBrush(b as *mut GpBrush);
        }
    }
    pub fn rect(&self, x: f32, y: f32, w: f32, h: f32, c: u32) {
        unsafe {
            let mut b = null_mut();
            GdipCreateSolidFill(c, &mut b);
            GdipFillRectangle(self.0, b as *mut GpBrush, x, y, w, h);
            GdipDeleteBrush(b as *mut GpBrush);
        }
    }
    pub fn fill_v(&self, p: &Path, y0: f32, y1: f32, c0: u32, c1: u32) {
        self.fill_lin(p, (0.0, y0), (0.0, y1), c0, c1)
    }
    pub fn fill_lin(&self, p: &Path, a: (f32, f32), b: (f32, f32), c0: u32, c1: u32) {
        unsafe {
            let pa = PointF { X: a.0, Y: a.1 - 0.01 };
            let pb = PointF { X: b.0, Y: b.1 + 0.01 };
            let mut br = null_mut();
            GdipCreateLineBrush(&pa, &pb, c0, c1, WrapModeTileFlipXY, &mut br);
            GdipFillPath(self.0, br as *mut GpBrush, p.0);
            GdipDeleteBrush(br as *mut GpBrush);
        }
    }
    pub fn fill_radial(&self, p: &Path, cx: f32, cy: f32, center: u32, surround: u32) {
        unsafe {
            let mut br = null_mut();
            GdipCreatePathGradientFromPath(p.0, &mut br);
            GdipSetPathGradientCenterColor(br, center);
            let mut n = 1;
            GdipSetPathGradientSurroundColorsWithCount(br, &surround, &mut n);
            GdipSetPathGradientCenterPoint(br, &PointF { X: cx, Y: cy });
            GdipFillPath(self.0, br as *mut GpBrush, p.0);
            GdipDeleteBrush(br as *mut GpBrush);
        }
    }
    pub fn stroke(&self, p: &Path, c: u32, w: f32) {
        unsafe {
            let pn = pen(c, w);
            GdipDrawPath(self.0, pn, p.0);
            GdipDeletePen(pn);
        }
    }
    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, c: u32, w: f32) {
        unsafe {
            let pn = pen(c, w);
            GdipDrawLine(self.0, pn, x1, y1, x2, y2);
            GdipDeletePen(pn);
        }
    }
    pub fn polyline(&self, pts: &[(f32, f32)], c: u32, w: f32) {
        if pts.len() >= 2 {
            self.stroke(&Path::polyline(pts), c, w)
        }
    }
    pub fn fill_poly_winding(&self, pts: &[(f32, f32)], c: u32) {
        unsafe {
            let v: Vec<PointF> = pts.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
            let mut b = null_mut();
            GdipCreateSolidFill(c, &mut b);
            GdipFillPolygon(self.0, b as *mut GpBrush, v.as_ptr(), v.len() as i32, FillModeWinding);
            GdipDeleteBrush(b as *mut GpBrush);
        }
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
    pub fn text(&self, s: &str, x: f32, y: f32, w: f32, h: f32, size: f32, c: u32, fl: u32) {
        unsafe {
            let ff = font(fl, size);
            let mut fmt = null_mut();
            GdipCreateStringFormat(if fl & WRAP != 0 { 0 } else { 0x1000 }, 0, &mut fmt);
            GdipSetStringFormatAlign(fmt, if fl & CENTER != 0 { 1 } else if fl & RIGHT != 0 { 2 } else { 0 });
            GdipSetStringFormatLineAlign(fmt, if fl & (TOP | WRAP) != 0 { 0 } else { 1 });
            GdipSetStringFormatTrimming(fmt, if fl & PATH_TRIM != 0 { 5 } else { 3 });
            let ws: Vec<u16> = s.encode_utf16().collect();
            let rc = RectF { X: x, Y: y, Width: w, Height: h };
            let mut b = null_mut();
            GdipCreateSolidFill(c, &mut b);
            GdipDrawString(self.0, ws.as_ptr(), ws.len() as i32, ff.1, &rc, fmt, b as *mut GpBrush);
            GdipDeleteBrush(b as *mut GpBrush);
            GdipDeleteStringFormat(fmt);
            free_font(ff);
        }
    }
    /// Unbounded typographic text whose top-left is (x,y).
    pub fn text_at(&self, s: &str, x: f32, y: f32, size: f32, c: u32, fl: u32) {
        unsafe {
            let ff = font(fl, size);
            let fmt = typographic();
            let ws: Vec<u16> = s.encode_utf16().collect();
            let rc = RectF { X: x, Y: y, Width: 0.0, Height: 0.0 };
            let mut b = null_mut();
            GdipCreateSolidFill(c, &mut b);
            GdipDrawString(self.0, ws.as_ptr(), ws.len() as i32, ff.1, &rc, fmt, b as *mut GpBrush);
            GdipDeleteBrush(b as *mut GpBrush);
            GdipDeleteStringFormat(fmt);
            free_font(ff);
        }
    }
    pub fn measure_at(&self, s: &str, size: f32, fl: u32) -> (f32, f32) {
        unsafe {
            let ff = font(fl, size);
            let fmt = typographic();
            let ws: Vec<u16> = s.encode_utf16().collect();
            let rc = RectF { X: 0.0, Y: 0.0, Width: 0.0, Height: 0.0 };
            let mut out = RectF { X: 0.0, Y: 0.0, Width: 0.0, Height: 0.0 };
            GdipMeasureString(self.0, ws.as_ptr(), ws.len() as i32, ff.1, &rc, fmt, &mut out, null_mut(), null_mut());
            GdipDeleteStringFormat(fmt);
            free_font(ff);
            (out.Width, out.Height.max(size * 1.2))
        }
    }
    pub fn image(&self, img: *mut GpBitmap, x: f32, y: f32, w: f32, h: f32) {
        unsafe { GdipDrawImageRect(self.0, img as *mut GpImage, x, y, w, h) };
    }
    pub fn image_src(&self, img: *mut GpBitmap, d: (f32, f32, f32, f32), s: (f32, f32, f32, f32)) {
        unsafe {
            GdipDrawImageRectRect(self.0, img as *mut GpImage, d.0, d.1, d.2, d.3, s.0, s.1, s.2, s.3, UnitPixel, null(), 0, null_mut());
        }
    }
    /// Draws `img` covering the destination rect (center-cropped).
    pub fn image_cover(&self, img: *mut GpBitmap, iw: f32, ih: f32, x: f32, y: f32, w: f32, h: f32) {
        let sc = (w / iw).max(h / ih);
        let (sw, sh) = (w / sc, h / sc);
        self.image_src(img, (x, y, w, h), ((iw - sw) / 2.0, (ih - sh) / 2.0, sw, sh));
    }
    pub fn clip(&self, p: &Path) {
        unsafe { GdipSetClipPath(self.0, p.0, CombineModeReplace) };
    }
    pub fn clip_rect(&self, x: f32, y: f32, w: f32, h: f32) {
        unsafe { GdipSetClipRect(self.0, x, y, w, h, CombineModeReplace) };
    }
    pub fn unclip(&self) {
        unsafe { GdipResetClip(self.0) };
    }
}

/// Wraps existing memory as a GDI+ bitmap (memory must outlive the bitmap).
pub unsafe fn bitmap_over(w: i32, h: i32, px: *mut u32, fmt: i32) -> *mut GpBitmap {
    let mut b = null_mut();
    GdipCreateBitmapFromScan0(w, h, w * 4, fmt, px as *const u8, &mut b);
    b
}

pub unsafe fn dispose(b: *mut GpBitmap) {
    if !b.is_null() {
        GdipDisposeImage(b as *mut GpImage);
    }
}

pub unsafe fn com_release(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    let vt = *(p as *const *const usize);
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vt.add(2));
    f(p);
}

pub fn encode_png(img: &Img) -> Vec<u8> {
    unsafe {
        let bmp = bitmap_over(img.w, img.h, img.px.as_ptr() as *mut u32, if img.alpha { PF_ARGB } else { PF_RGB });
        let mut stream: *mut c_void = null_mut();
        CreateStreamOnHGlobal(null_mut(), 1, &mut stream);
        let clsid = GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);
        GdipSaveImageToStream(bmp as *mut GpImage, stream, &clsid, null());
        dispose(bmp);
        let vt = *(stream as *const *const usize);
        let seek: unsafe extern "system" fn(*mut c_void, i64, u32, *mut u64) -> i32 = std::mem::transmute(*vt.add(5));
        let mut size = 0u64;
        seek(stream, 0, 2, &mut size);
        let mut hg: HGLOBAL = null_mut();
        GetHGlobalFromStream(stream, &mut hg);
        let p = GlobalLock(hg) as *const u8;
        let out = std::slice::from_raw_parts(p, size as usize).to_vec();
        GlobalUnlock(hg);
        com_release(stream);
        out
    }
}

pub fn decode_file(path: &str) -> Option<Img> {
    unsafe {
        let mut bmp = null_mut();
        if GdipCreateBitmapFromFile(wz(path).as_ptr(), &mut bmp) != 0 || bmp.is_null() {
            return None;
        }
        let (mut w, mut h) = (0u32, 0u32);
        GdipGetImageWidth(bmp as *mut GpImage, &mut w);
        GdipGetImageHeight(bmp as *mut GpImage, &mut h);
        let rect = Rect { X: 0, Y: 0, Width: w as i32, Height: h as i32 };
        let mut bd: BitmapData = std::mem::zeroed();
        if GdipBitmapLockBits(bmp, &rect, ImageLockModeRead as u32, PF_ARGB, &mut bd) != 0 {
            dispose(bmp);
            return None;
        }
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h as isize {
            let row = (bd.Scan0 as *const u8).offset(y * bd.Stride as isize) as *const u32;
            px.extend_from_slice(std::slice::from_raw_parts(row, w as usize));
        }
        GdipBitmapUnlockBits(bmp, &mut bd);
        dispose(bmp);
        let alpha = px.iter().any(|p| p >> 24 != 255);
        Some(Img { w: w as i32, h: h as i32, px, alpha })
    }
}

pub unsafe fn hicon_from(img: &mut Img) -> HICON {
    let b = bitmap_over(img.w, img.h, img.px.as_mut_ptr(), PF_ARGB);
    let mut hi: HICON = null_mut();
    GdipCreateHICONFromBitmap(b, &mut hi);
    dispose(b);
    hi
}
