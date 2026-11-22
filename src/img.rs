//! Plain pixel images and the pixel maths both platforms share.

/// 32-bit image, one u32 per pixel as 0xAARRGGBB (straight alpha).
pub struct Img {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
    pub alpha: bool,
}

impl Img {
    /// Fast area-sampled downscale by factor `sc` (<= 1).
    pub fn scaled(&self, sc: f32) -> Img {
        let sc = sc.min(1.0);
        let tw = ((self.w as f32 * sc).round() as i32).max(1);
        let th = ((self.h as f32 * sc).round() as i32).max(1);
        let mut px = Vec::with_capacity((tw * th) as usize);
        let (w, h) = (self.w as usize, self.h as usize);
        for ty in 0..th as usize {
            let y0 = ty * h / th as usize;
            let y1 = ((ty + 1) * h / th as usize).max(y0 + 1).min(h);
            let sy = ((y1 - y0) / 4).max(1);
            for tx in 0..tw as usize {
                let x0 = tx * w / tw as usize;
                let x1 = ((tx + 1) * w / tw as usize).max(x0 + 1).min(w);
                let sx = ((x1 - x0) / 4).max(1);
                let (mut a, mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                let mut y = y0;
                while y < y1 {
                    let mut x = x0;
                    while x < x1 {
                        let p = self.px[y * w + x];
                        a += p >> 24;
                        r += (p >> 16) & 255;
                        g += (p >> 8) & 255;
                        b += p & 255;
                        n += 1;
                        x += sx;
                    }
                    y += sy;
                }
                px.push(((a / n) << 24) | ((r / n) << 16) | ((g / n) << 8) | (b / n));
            }
        }
        Img { w: tw, h: th, px, alpha: self.alpha }
    }
    pub fn fit(&self, maxw: i32, maxh: i32) -> Img {
        self.scaled((maxw as f32 / self.w as f32).min(maxh as f32 / self.h as f32))
    }
}

/// Three-pass box blur (≈ gaussian) of the `w`×`h` block at (x0, y0) in an image
/// `stride` pixels wide. Output pixels are opaque.
pub fn box_blur(px: &mut [u32], stride: usize, x0: i32, y0: i32, w: i32, h: i32, rad: usize) {
    let (x0, y0) = (x0.max(0) as usize, y0.max(0) as usize);
    let (w, h) = (w.max(0) as usize, h.max(0) as usize);
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

/// Draws a premultiplied 0xAARRGGBB `layer` over opaque `dst` inside the box
/// [x0, x1) × [y0, y1). Both buffers are `stride` pixels wide.
pub fn blend_over(dst: &mut [u32], layer: &[u32], stride: usize, x0: i32, y0: i32, x1: i32, y1: i32) {
    for y in y0.max(0)..y1 {
        let o = y as usize * stride;
        for x in x0.max(0)..x1 {
            let i = o + x as usize;
            let (Some(&s), Some(d)) = (layer.get(i), dst.get_mut(i)) else { continue };
            let a = s >> 24;
            if a == 0 {
                continue;
            }
            if a == 255 {
                *d = s;
                continue;
            }
            let k = 255 - a;
            let ch = |sh: u32| (((*d >> sh) & 255) * k / 255 + ((s >> sh) & 255)).min(255) << sh;
            *d = 0xFF00_0000 | ch(16) | ch(8) | ch(0);
        }
    }
}

/// The finished capture of box (x, y, w, h): screen pixels `work` shaped by
/// `cover(x, y)` (0..=255, the selection), with the annotation `layer`
/// (premultiplied) on top. Annotations count as part of the shape, so they may
/// reach past the selection. Output is straight alpha.
pub fn compose(work: &[u32], layer: &[u32], stride: usize, (bx, by, w, h): (i32, i32, i32, i32), cover: impl Fn(i32, i32) -> u32) -> Img {
    let mut px = Vec::with_capacity((w * h).max(0) as usize);
    let mut alpha = false;
    for y in by..by + h {
        for x in bx..bx + w {
            let i = y as usize * stride + x as usize;
            let (wc, l) = (work[i], layer[i]);
            let m = cover(x, y).min(255);
            let la = l >> 24;
            // layer over (work with coverage m), premultiplied
            let a = la + m * (255 - la) / 255;
            if a == 0 {
                px.push(0);
                alpha = true;
                continue;
            }
            if a < 255 {
                alpha = true;
            }
            let ch = |sh: u32| {
                let pre = ((l >> sh) & 255) + ((wc >> sh) & 255) * m / 255 * (255 - la) / 255;
                ((pre * 255 + a / 2) / a).min(255) << sh
            };
            px.push((a << 24) | ch(16) | ch(8) | ch(0));
        }
    }
    Img { w, h, px, alpha }
}

/// Union of two (left, top, right, bottom) boxes; empty boxes are ignored.
pub fn union(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let empty = |r: (i32, i32, i32, i32)| r.2 <= r.0 || r.3 <= r.1;
    match (empty(a), empty(b)) {
        (true, _) => b,
        (_, true) => a,
        _ => (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)),
    }
}
