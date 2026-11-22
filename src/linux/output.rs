//! Hand a finished capture to the clipboard, the save folder and the history,
//! then tell the user with a desktop notification.
use crate::config::Config;
use crate::img::Img;
use crate::{gfx, store};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq)]
pub enum Out {
    Default,
    Copy,
    Save,
}

/// "2026-11-21 at 14.05.09", like the Windows build.
pub fn timestamp() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:04}-{:02}-{:02} at {:02}.{:02}.{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

fn unique_file(folder: &str) -> String {
    let base = std::path::Path::new(folder).join(format!("Aperture {}", timestamp())).to_string_lossy().into_owned();
    let mut p = format!("{base}.png");
    let mut n = 2;
    while std::path::Path::new(&p).exists() {
        p = format!("{base} ({n}).png");
        n += 1;
    }
    p
}

pub fn has_cmd(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(cmd).is_file()))
}

pub fn open_path(p: &str) {
    let _ = Command::new("xdg-open").arg(p).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

// X11 (and XWayland) clipboard owner: must stay alive to keep serving the image.
static CLIP: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

/// Puts a PNG (and the raw image for X11 apps) on the clipboard.
pub fn copy_image(img: &Img, png: &[u8], wayland: bool) -> bool {
    // wl-copy serves the native Wayland clipboard from its own background process
    if wayland && has_cmd("wl-copy") {
        if let Ok(mut child) = Command::new("wl-copy").args(["--type", "image/png"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            let ok = child.stdin.take().is_some_and(|mut s| s.write_all(png).is_ok());
            let _ = child.wait();
            if ok {
                return true;
            }
        }
    }
    let mut g = CLIP.lock().unwrap();
    if g.is_none() {
        *g = arboard::Clipboard::new().ok();
    }
    let Some(cb) = g.as_mut() else { return false };
    let mut rgba = Vec::with_capacity(img.px.len() * 4);
    for &p in &img.px {
        rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, if img.alpha { (p >> 24) as u8 } else { 255 }]);
    }
    cb.set_image(arboard::ImageData { width: img.w as usize, height: img.h as usize, bytes: rgba.into() }).is_ok()
}

/// Puts a file on the clipboard (so a video can be pasted into a chat or file manager).
pub fn copy_file(path: &std::path::Path, wayland: bool) -> bool {
    let uri = format!("file://{}", path.to_string_lossy().replace('%', "%25").replace(' ', "%20"));
    if wayland && has_cmd("wl-copy") {
        if let Ok(mut child) = Command::new("wl-copy").args(["--type", "text/uri-list"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            let ok = child.stdin.take().is_some_and(|mut s| s.write_all(format!("{uri}\r\n").as_bytes()).is_ok());
            let _ = child.wait();
            if ok {
                return true;
            }
        }
    }
    let mut g = CLIP.lock().unwrap();
    if g.is_none() {
        *g = arboard::Clipboard::new().ok();
    }
    g.as_mut().is_some_and(|cb| cb.set().file_list(&[path]).is_ok())
}

/// Desktop notification; clicking "Open" opens `file`.
pub fn notify(title: &str, body: &str, icon: &str, file: Option<String>) {
    if !has_cmd("notify-send") {
        return;
    }
    let (title, body, icon) = (title.to_string(), body.to_string(), icon.to_string());
    std::thread::spawn(move || {
        let base = |c: &mut Command| {
            c.args(["-a", "Aperture", "-i", &icon, "-t", "4000", &title, &body]).stdin(Stdio::null()).stderr(Stdio::null());
        };
        if let Some(f) = &file {
            // -A waits for the click (libnotify 0.7.9+); older versions fail and we fall back
            let mut c = Command::new("notify-send");
            c.args(["-A", "open=Open"]);
            base(&mut c);
            if let Ok(out) = c.stdout(Stdio::piped()).output() {
                if out.status.success() {
                    if String::from_utf8_lossy(&out.stdout).trim() == "open" {
                        open_path(f);
                    }
                    return;
                }
            }
        }
        let mut c = Command::new("notify-send");
        base(&mut c);
        let _ = c.stdout(Stdio::null()).status();
    });
}

fn notify_icon_path() -> String {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("aperture-last.png").to_string_lossy().into_owned()
}

/// Copies / saves `img` per the settings (or the button pressed) and records it in history.
/// `done` runs on the worker thread once history has been updated.
pub fn deliver(img: Img, how: Out, cfg: &Config, wayland: bool, done: impl FnOnce() + Send + 'static) {
    let copy = cfg.copy || how == Out::Copy;
    let save = cfg.save || how == Out::Save;
    let folder = cfg.folder.clone();
    std::thread::spawn(move || {
        let png = gfx::encode_png(&img);
        let copied = copy && copy_image(&img, &png, wayland);
        let file = if save { Some(unique_file(&folder)) } else { None };
        if let Some(f) = &file {
            let _ = std::fs::create_dir_all(&folder);
            let _ = std::fs::write(f, &png);
        }
        let th = gfx::encode_png(&img.fit(320, 240));
        store::add(&png, &th);
        let icon = notify_icon_path();
        let _ = std::fs::write(&icon, &th);
        let title = match (copied, save) {
            (true, true) => "Copied & saved",
            (true, false) => "Copied to clipboard",
            (false, true) => "Saved",
            _ if copy => "Couldn't copy",
            _ => "Captured",
        };
        let sub = format!("{} × {}  ·  {}", img.w, img.h, if copied { "Paste with Ctrl+V" } else if save { "In your screenshots folder" } else { "Kept in history" });
        notify(title, &sub, &icon, file);
        done();
    });
}
