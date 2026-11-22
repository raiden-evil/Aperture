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
