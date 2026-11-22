//! Procedural skeuomorphic icons (glossy gradients, bevels, highlights).
use crate::gfx::*;
use crate::util::Img;
use std::f32::consts::PI;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Icon {
    Region,
    Free,
    Full,
    Text,
    Blur,
    Censor,
    Arrow,
    Line,
    Pen,
    Color(u32),
    Undo,
    Save,
    Copy,
    Close,
    Record,
    Gear,
    Folder,
}

fn lighten(c: u32, t: f32) -> u32 {
    let f = |v: u32| ((v as f32) + (255.0 - v as f32) * t) as u32;
    0xFF00_0000 | (f((c >> 16) & 255) << 16) | (f((c >> 8) & 255) << 8) | f(c & 255)
}
fn darken(c: u32, t: f32) -> u32 {
    let f = |v: u32| ((v as f32) * (1.0 - t)) as u32;
    0xFF00_0000 | (f((c >> 16) & 255) << 16) | (f((c >> 8) & 255) << 8) | f(c & 255)
}

fn shifted(pts: &[(f32, f32)], dx: f32, dy: f32) -> Vec<(f32, f32)> {
    pts.iter().map(|&(x, y)| (x + dx, y + dy)).collect()
}

/// Glossy tube stroke: drop shadow, body, specular highlight.
fn tube(g: &Gfx, pts: &[(f32, f32)], c: u32, w: f32) {
    g.polyline(&shifted(pts, 0.35, 0.9), 0x55000000, w + 0.3);
    g.polyline(pts, c, w);
    g.polyline(&shifted(pts, -0.3, -0.35), 0xA0FFFFFF & 0xFFFFFFFF, w * 0.28);
}

pub fn draw(g: &Gfx, icon: Icon, x: f32, y: f32, s: f32) {
    let st = g.save();
    g.translate(x, y);
    g.scale(s / 24.0);
    match icon {
        Icon::Region => {
            g.fill(&Path::rrect(4.5, 4.5, 15.0, 15.0, 1.5), 0x2A2D7FF9);
            let cs: [[(f32, f32); 3]; 4] = [
                [(3.5, 9.0), (3.5, 3.5), (9.0, 3.5)],
                [(15.0, 3.5), (20.5, 3.5), (20.5, 9.0)],
                [(20.5, 15.0), (20.5, 20.5), (15.0, 20.5)],
                [(9.0, 20.5), (3.5, 20.5), (3.5, 15.0)],
            ];
            for c in &cs {
                tube(g, c, 0xFF1F6FEB, 2.7);
            }
        }
        Icon::Free => {
            let rope = 0xFFB97A3B;
            let e = Path::ellipse(3.5, 2.8, 17.0, 11.5);
            let es = Path::ellipse(3.85, 3.7, 17.0, 11.5);
            g.stroke(&es, 0x50000000, 2.7);
            g.stroke(&e, rope, 2.4);
            g.stroke(&Path::arc(3.2, 2.45, 17.0, 11.5, 190.0, 120.0), 0xB0F3D3A1, 0.8);
            let tail = [(8.0, 13.0), (6.6, 16.5), (8.8, 20.8)];
            tube(g, &tail, rope, 2.3);
            g.fill_radial(&Path::circle(8.0, 13.0, 1.9), 7.5, 12.4, 0xFFE0A866, 0xFF7A4A1E);
        }
        Icon::Full => {
            let bezel = Path::rrect(2.0, 3.0, 20.0, 14.0, 2.2);
            g.fill_v(&bezel, 3.0, 17.0, 0xFF5E646E, 0xFF1A1D22);
            g.stroke(&bezel, 0x80000000, 0.6);
            let scr = Path::rrect(3.6, 4.6, 16.8, 10.8, 0.8);
            g.fill_lin(&scr, (3.6, 4.6), (20.4, 15.4), 0xFF86CBFF, 0xFF1A50C2);
            g.fill(&Path::poly(&[(3.8, 4.8), (20.2, 4.8), (20.2, 7.2), (3.8, 12.2)]), 0x3CFFFFFF);
            g.fill_v(&Path::rect(10.4, 17.0, 3.2, 2.2), 17.0, 19.2, 0xFFB9BEC6, 0xFF7D838C);
            let base = Path::rrect(6.5, 19.0, 11.0, 2.4, 1.2);
            g.fill_v(&base, 19.0, 21.4, 0xFFEFF1F4, 0xFF959BA4);
            g.stroke(&base, 0x60000000, 0.5);
        }
        Icon::Text => {
            let tile = Path::rrect(2.5, 2.5, 19.0, 19.0, 4.2);
            g.fill_v(&tile, 2.5, 21.5, 0xFFFFFDF4, 0xFFE6DCBE);
            g.stroke(&tile, 0xFFB2A47A, 0.9);
            g.stroke(&Path::rrect(3.4, 3.4, 17.2, 17.2, 3.4), 0x90FFFFFF, 0.7);
            g.text("T", 2.5, 3.3, 19.0, 19.0, 17.0, 0xB0FFFFFF, BOLD | SERIF | CENTER);
            g.text("T", 2.5, 2.5, 19.0, 19.0, 17.0, 0xFF3B3125, BOLD | SERIF | CENTER);
        }
        Icon::Blur => {
            let c = Path::circle(12.0, 12.0, 9.6);
            g.fill_radial(&c, 9.0, 8.5, 0xFFF4F9FF, 0xFF4A82D6);
            g.stroke(&c, 0x70103060, 0.8);
            g.fill_radial(&Path::circle(9.0, 10.0, 5.0), 9.0, 10.0, 0xC0FFFFFF, 0x00FFFFFF);
            g.fill_radial(&Path::circle(15.5, 14.5, 4.2), 15.5, 14.5, 0x90FFFFFF, 0x00FFFFFF);
            g.fill_radial(&Path::circle(13.5, 7.0, 2.6), 13.5, 7.0, 0xA0FFFFFF, 0x00FFFFFF);
            g.fill(&Path::ellipse(6.0, 3.6, 12.0, 6.0), 0x50FFFFFF);
        }
        Icon::Censor => {
            let paper = Path::rrect(2.0, 3.5, 20.0, 17.0, 2.5);
            g.fill_v(&paper, 3.5, 20.5, 0xFFFFFFFF, 0xFFE4E7EB);
            g.stroke(&paper, 0xFFA9B0BA, 0.8);
            g.fill(&Path::rrect(5.0, 6.8, 14.0, 1.6, 0.8), 0xFFBAC0C9);
            g.fill(&Path::rrect(5.0, 16.4, 9.0, 1.6, 0.8), 0xFFBAC0C9);
            let bar = Path::rrect(4.0, 9.8, 16.0, 5.0, 1.3);
            g.fill(&Path::rrect(4.2, 10.4, 16.0, 5.0, 1.3), 0x40000000);
            g.fill_v(&bar, 9.8, 14.8, 0xFF4A4C50, 0xFF050506);
            g.line(5.2, 10.6, 18.8, 10.6, 0x55FFFFFF, 0.6);
        }
        Icon::Arrow => {
            g.arrow(4.9, 20.4, 19.9, 5.4, 0x50000000, 3.3);
            g.arrow(4.5, 19.5, 19.5, 4.5, 0xFFE5392A, 3.0);
            g.line(5.0, 18.2, 15.0, 8.2, 0x70FFFFFF, 0.7);
        }
        Icon::Line => {
            // glossy straight line with round end caps
            g.line(5.4, 19.4, 19.4, 5.4, 0x50000000, 3.2);
            g.line(5.0, 19.0, 19.0, 5.0, 0xFF1F6FEB, 2.6);
            g.line(5.6, 17.6, 17.6, 5.6, 0x90FFFFFF, 0.7);
            for (cx, cy) in [(5.0f32, 19.0f32), (19.0, 5.0)] {
                let k = Path::circle(cx, cy, 2.9);
                g.fill(&Path::circle(cx + 0.4, cy + 0.8, 2.9), 0x40000000);
                g.fill_v(&k, cy - 2.9, cy + 2.9, 0xFFFFFFFF, 0xFFC5CBD3);
                g.stroke(&k, 0x70000000, 0.6);
            }
        }
        Icon::Pen => {
            // glossy faceted pencil
            let (tx, ty) = (4.0f32, 20.0f32);
            let d = (0.7071f32, -0.7071f32);
            let n = (0.7071f32, 0.7071f32);
            let hw = 2.7;
            let p = |t: f32, k: f32| (tx + d.0 * t + n.0 * k, ty + d.1 * t + n.1 * k);
            let quad = |t0: f32, t1: f32, k0: f32, k1: f32| Path::poly(&[p(t0, k0), p(t1, k0), p(t1, k1), p(t0, k1)]);
            g.fill(&Path::poly(&[p(0.6, 0.9), p(20.5, 0.9 + 0.9), p(20.5, hw + 0.9), p(5.0, hw + 0.9)]), 0x30000000);
            g.fill(&Path::poly(&[p(0.0, 0.0), p(2.4, -0.95), p(2.4, 0.95)]), 0xFF2A2A2A);
            g.fill(&Path::poly(&[p(2.4, -0.95), p(5.2, -hw), p(5.2, hw), p(2.4, 0.95)]), 0xFFEBC594);
            g.fill(&quad(5.2, 15.2, -hw, -0.9), 0xFFFFDA5C);
            g.fill(&quad(5.2, 15.2, -0.9, 0.9), 0xFFF6B900);
            g.fill(&quad(5.2, 15.2, 0.9, hw), 0xFFC98C00);
            g.fill_lin(&quad(15.2, 17.4, -hw, hw), p(16.0, -hw), p(16.0, hw), 0xFFF5F5F5, 0xFF7F858E);
            g.fill_lin(&quad(17.4, 20.2, -hw, hw), p(18.0, -hw), p(18.0, hw), 0xFFFFA3B1, 0xFFC4566A);
        }
        Icon::Color(c) => {
            let ring = Path::circle(12.0, 12.0, 9.8);
            g.fill_v(&ring, 2.2, 21.8, 0xFFFFFFFF, 0xFFCDD2D9);
            g.stroke(&ring, 0x50000000, 0.7);
            let inner = Path::circle(12.0, 12.0, 7.2);
            g.fill_radial(&inner, 10.0, 9.0, lighten(c, 0.45), darken(c, 0.15));
            g.stroke(&inner, 0x40000000, 0.6);
            g.fill(&Path::ellipse(8.0, 5.8, 8.0, 4.2), 0x60FFFFFF);
        }
        Icon::Undo => {
            let col = 0xFF4B5563;
            g.stroke(&Path::arc(5.3, 6.9, 14.0, 13.0, 180.0, 225.0), 0x45000000, 2.9);
            g.stroke(&Path::arc(5.0, 6.0, 14.0, 13.0, 180.0, 225.0), col, 2.6);
            let tri = Path::poly(&[(1.6, 11.0), (8.4, 11.0), (5.0, 16.3)]);
            g.fill(&tri, col);
            g.stroke(&tri, col, 0.8);
            g.stroke(&Path::arc(4.7, 5.7, 14.0, 13.0, 200.0, 120.0), 0x70FFFFFF, 0.7);
        }
        Icon::Save => {
            let body = Path::rrect(3.0, 3.0, 18.0, 18.0, 2.2);
            g.fill_v(&body, 3.0, 21.0, 0xFF5E9DFF, 0xFF1C4DB5);
            g.stroke(&body, 0x80001040, 0.7);
            let sh = Path::rrect(7.0, 3.0, 10.0, 6.6, 0.8);
            g.fill_v(&sh, 3.0, 9.6, 0xFFF6F7F9, 0xFFA9AFB7);
            g.fill(&Path::rect(13.0, 4.0, 2.2, 4.4), 0xFF454B55);
            let lab = Path::rrect(5.5, 12.0, 13.0, 8.0, 1.0);
            g.fill_v(&lab, 12.0, 20.0, 0xFFFFFFFF, 0xFFE0E4EA);
            g.fill(&Path::rect(7.5, 14.4, 9.0, 0.9), 0xFFC3C8CF);
            g.fill(&Path::rect(7.5, 16.9, 9.0, 0.9), 0xFFC3C8CF);
            g.line(4.2, 4.0, 6.0, 4.0, 0x80FFFFFF, 0.6);
        }
        Icon::Copy => {
            let board = Path::rrect(4.0, 3.5, 16.0, 18.5, 2.5);
            g.fill_v(&board, 3.5, 22.0, 0xFFD7A066, 0xFF8C5424);
            g.stroke(&board, 0x90000000, 0.6);
            let paper = Path::rrect(6.0, 6.5, 12.0, 13.5, 1.0);
            g.fill_v(&paper, 6.5, 20.0, 0xFFFFFFFF, 0xFFE8EAEE);
            for (yy, ww) in [(10.0, 8.0), (12.8, 8.0), (15.6, 5.0)] {
                g.fill(&Path::rect(8.0, yy, ww, 1.0), 0xFFBFC5CC);
            }
            let clip = Path::rrect(8.0, 1.8, 8.0, 4.4, 1.6);
            g.fill_v(&clip, 1.8, 6.2, 0xFFF7F8F9, 0xFF959BA4);
            g.stroke(&clip, 0x90000000, 0.6);
        }
        Icon::Record => {
            let ring = Path::circle(12.0, 12.0, 10.0);
            g.fill(&Path::circle(12.3, 12.8, 10.0), 0x40000000);
            g.fill_v(&ring, 2.0, 22.0, 0xFFF6F7F9, 0xFF9198A2);
            g.stroke(&ring, 0x80000000, 0.7);
            g.stroke(&Path::circle(12.0, 12.0, 8.6), 0x40000000, 0.8);
            let dot = Path::circle(12.0, 12.0, 6.6);
            g.fill_radial(&dot, 10.4, 9.8, 0xFFFF8273, 0xFFC0170B);
            g.stroke(&dot, 0x80400000, 0.6);
            g.fill(&Path::ellipse(8.4, 6.6, 7.2, 3.8), 0x78FFFFFF);
        }
        Icon::Close => {
            let c = Path::circle(12.0, 12.0, 9.6);
            g.fill_v(&c, 2.4, 21.6, 0xFFFF7D6E, 0xFFCC2417);
            g.stroke(&c, 0x70500000, 0.7);
            g.fill(&Path::ellipse(5.5, 3.6, 13.0, 7.5), 0x48FFFFFF);
            g.line(8.6, 8.6, 15.4, 15.4, 0xFFFFFFFF, 2.2);
            g.line(15.4, 8.6, 8.6, 15.4, 0xFFFFFFFF, 2.2);
        }
        Icon::Gear => {
            let mut pts = Vec::new();
            for i in 0..8 {
                let a0 = i as f32 * 2.0 * PI / 8.0;
                for (da, r) in [(-0.33f32, 7.8f32), (-0.17, 10.6), (0.17, 10.6), (0.33, 7.8)] {
                    pts.push((12.0 + r * (a0 + da).cos(), 12.0 + r * (a0 + da).sin()));
                }
            }
            let gear = Path::poly(&pts);
            g.fill_lin(&gear, (4.0, 3.0), (20.0, 21.0), 0xFFF4F5F7, 0xFF7E858F);
            g.stroke(&gear, 0x90000000, 0.6);
            let hub = Path::circle(12.0, 12.0, 3.6);
            g.fill_v(&hub, 8.4, 15.6, 0xFF3A3F47, 0xFF757C87);
            g.stroke(&hub, 0x60000000, 0.6);
        }
        Icon::Folder => {
            g.fill_v(&Path::rrect(2.0, 4.0, 8.5, 4.5, 1.5), 4.0, 8.5, 0xFFFFC94F, 0xFFE9A42A);
            g.fill_v(&Path::rrect(2.0, 5.5, 20.0, 14.5, 2.0), 5.5, 20.0, 0xFFF2B53C, 0xFFD38A12);
            let front = Path::rrect(2.0, 8.5, 20.0, 11.5, 2.0);
            g.fill_v(&front, 8.5, 20.0, 0xFFFFDE85, 0xFFF0AE2E);
            g.stroke(&front, 0x60603C00, 0.6);
            g.line(3.5, 9.5, 20.5, 9.5, 0x90FFFFFF, 0.6);
        }
    }
    g.restore(st);
}

/// The Aperture app icon: a skeuomorphic camera, drawn on a 256 grid.
pub fn camera(g: &Gfx, x: f32, y: f32, s: f32) {
    let st = g.save();
    g.translate(x, y);
    g.scale(s / 256.0);
    // soft drop shadow
    g.fill(&Path::rrect(14.0, 72.0, 228.0, 162.0, 40.0), 0x2A000000);
    g.fill(&Path::rrect(18.0, 70.0, 220.0, 160.0, 38.0), 0x22000000);
    // viewfinder hump
    let hump = Path::rrect(84.0, 36.0, 88.0, 48.0, 14.0);
    g.fill_v(&hump, 36.0, 84.0, 0xFFF7F8FA, 0xFFA3A9B2);
    g.stroke(&hump, 0x80000000, 3.0);
    // shutter button
    let sb = Path::rrect(178.0, 44.0, 48.0, 26.0, 9.0);
    g.fill_v(&sb, 44.0, 70.0, 0xFFFF7E6B, 0xFFB92416);
    g.stroke(&sb, 0x90400000, 2.5);
    g.fill(&Path::rrect(186.0, 48.0, 32.0, 7.0, 3.5), 0x60FFFFFF);
    // body (silver top plate)
    let body = Path::rrect(12.0, 62.0, 232.0, 162.0, 38.0);
    g.fill_v(&body, 62.0, 224.0, 0xFFF8F9FB, 0xFFB0B6BE);
    // leather lower half
    let leather = Path::rrect4(12.0, 108.0, 232.0, 116.0, 0.0, 0.0, 38.0, 38.0);
    g.fill_v(&leather, 108.0, 224.0, 0xFF42464F, 0xFF131519);
    for i in 0..9 {
        let yy = 120.0 + i as f32 * 11.0;
        g.line(26.0, yy, 230.0, yy, 0x0CFFFFFF, 1.5);
    }
    g.line(14.0, 109.0, 242.0, 109.0, 0x60FFFFFF, 2.0);
    g.stroke(&body, 0xA0000000, 3.0);
    g.fill(&Path::rrect(34.0, 68.0, 188.0, 9.0, 4.5), 0x80FFFFFF);
    // flash window
    let fl = Path::rrect(28.0, 76.0, 46.0, 24.0, 6.0);
    g.fill_v(&fl, 76.0, 100.0, 0xFFFFFFFF, 0xFFC9D3DF);
    g.stroke(&fl, 0x70000000, 2.0);
    g.line(34.0, 82.0, 52.0, 82.0, 0xC0FFFFFF, 2.0);
    // lens
    let (cx, cy) = (128.0f32, 150.0f32);
    let outer = Path::circle(cx, cy, 80.0);
    g.fill(&Path::circle(cx + 2.0, cy + 5.0, 80.0), 0x50000000);
    g.fill_v(&outer, cy - 80.0, cy + 80.0, 0xFFF6F7F9, 0xFF767C86);
    g.stroke(&outer, 0xB0000000, 3.0);
    g.stroke(&Path::circle(cx, cy, 74.0), 0x50FFFFFF, 2.0);
    let ring = Path::circle(cx, cy, 68.0);
    g.fill_v(&ring, cy - 68.0, cy + 68.0, 0xFF2E333A, 0xFF08090B);
    for i in 0..48 {
        let a = i as f32 * PI / 24.0;
        g.line(cx + 62.0 * a.cos(), cy + 62.0 * a.sin(), cx + 67.0 * a.cos(), cy + 67.0 * a.sin(), 0x22FFFFFF, 1.5);
    }
    let glass = Path::circle(cx, cy, 54.0);
    g.fill_radial(&glass, cx - 14.0, cy - 16.0, 0xFF5B9CF5, 0xFF050F26);
    // aperture blades
    let mut hex = Vec::new();
    for i in 0..6 {
        let a = i as f32 * PI / 3.0 + 0.3;
        hex.push((cx + 28.0 * a.cos(), cy + 28.0 * a.sin()));
    }
    g.fill(&Path::poly(&hex), 0xC0020714);
    for i in 0..6 {
        let (x0, y0) = hex[i];
        let a = i as f32 * PI / 3.0 + 0.3 + PI / 2.0;
        g.line(x0, y0, x0 + 26.0 * a.cos(), y0 + 26.0 * a.sin(), 0x704A7FD8, 1.6);
    }
    g.fill(&Path::circle(cx, cy, 14.0), 0xFF000000);
    g.stroke(&glass, 0x90000000, 2.0);
    // highlights
    g.fill_radial(&Path::ellipse(cx - 44.0, cy - 44.0, 44.0, 28.0), cx - 22.0, cy - 30.0, 0xE0FFFFFF, 0x00FFFFFF);
    g.fill(&Path::circle(cx + 22.0, cy + 20.0, 6.0), 0x70FFFFFF);
    g.fill(&Path::circle(cx - 18.0, cy - 22.0, 4.0), 0xF0FFFFFF);
    g.restore(st);
}

pub fn render_camera(size: i32) -> Img {
    let mut img = Img { w: size, h: size, px: vec![0u32; (size * size) as usize], alpha: true };
    unsafe {
        let b = bitmap_over(size, size, img.px.as_mut_ptr(), PF_ARGB);
        {
            let g = Gfx::from_image(b);
            camera(&g, 0.0, 0.0, size as f32);
        }
        dispose(b);
    }
    img
}

/// Dev helper: `Aperture.exe --write-icon assets/aperture.ico`
pub fn write_ico(path: &str) {
    let sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256];
    let pngs: Vec<Vec<u8>> = sizes.iter().map(|&s| encode_png(&render_camera(s))).collect();
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((sizes.len() as u16).to_le_bytes());
    let mut off = 6 + 16 * sizes.len();
    for (i, &s) in sizes.iter().enumerate() {
        let b = if s >= 256 { 0u8 } else { s as u8 };
        out.extend([b, b, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((pngs[i].len() as u32).to_le_bytes());
        out.extend((off as u32).to_le_bytes());
        off += pngs[i].len();
    }
    for p in &pngs {
        out.extend(p);
    }
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, out);
}
