//! Grabbing the whole desktop as one image.
//!  * X11: read the root window directly (instant, no prompt).
//!  * Wayland: ask the desktop portal (org.freedesktop.portal.Screenshot). The
//!    compositor may ask the user for permission the first time.
use crate::img::Img;

/// The desktop image. `x`, `y` are the desktop coordinates of its top-left pixel.
pub struct Shot {
    pub img: Img,
    pub x: i32,
    pub y: i32,
}

pub fn grab(wayland: bool) -> Result<Shot, String> {
    if wayland { grab_portal() } else { grab_x11() }
}

fn grab_x11() -> Result<Shot, String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("Can't reach the X server: {e}"))?;
    let root = &conn.setup().roots[screen];
    let (w, h) = (root.width_in_pixels, root.height_in_pixels);
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, root.root, 0, 0, w, h, !0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("Couldn't read the screen: {e}"))?;
    let bpp = conn.setup().pixmap_formats.iter().find(|f| f.depth == reply.depth).map(|f| f.bits_per_pixel).unwrap_or(32);
    if bpp != 32 {
        return Err(format!("Unsupported screen format ({} bits per pixel)", bpp));
    }
    let lsb = conn.setup().image_byte_order == x11rb::protocol::xproto::ImageOrder::LSB_FIRST;
    let px: Vec<u32> = reply
        .data
        .chunks_exact(4)
        .map(|c| {
            let v = if lsb { u32::from_le_bytes([c[0], c[1], c[2], c[3]]) } else { u32::from_be_bytes([c[0], c[1], c[2], c[3]]) };
            v | 0xFF00_0000
        })
        .collect();
    if px.len() < w as usize * h as usize {
        return Err("The screen image came back incomplete".into());
    }
    Ok(Shot { img: Img { w: w as i32, h: h as i32, px, alpha: false }, x: 0, y: 0 })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn grab_portal() -> Result<Shot, String> {
    use ashpd::desktop::screenshot::Screenshot;
    let uri = pollster::block_on(async {
        let req = Screenshot::request().interactive(false).modal(false).send().await?;
        let shot = req.response()?;
        Ok::<String, ashpd::Error>(shot.uri().as_str().to_string())
    })
    .map_err(|e| match e {
        ashpd::Error::Response(_) => "Screenshot permission was denied".to_string(),
        e => format!("The screenshot portal isn't available: {e}"),
    })?;
    let path = percent_decode(uri.strip_prefix("file://").ok_or("The portal returned an unexpected location")?);
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read the screenshot: {e}"))?;
    // the portal saved a copy for us (often in ~/Pictures); it was only a hand-off
    let _ = std::fs::remove_file(&path);
    let mut img = crate::gfx::decode_png(&bytes).ok_or("Couldn't decode the screenshot")?;
    for p in img.px.iter_mut() {
        *p |= 0xFF00_0000;
    }
    img.alpha = false;
    Ok(Shot { img, x: 0, y: 0 })
}
