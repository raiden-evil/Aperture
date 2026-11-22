//! Clipboard history: the last 10 captures, kept as PNGs (+ small thumbnails).
use crate::config::Config;
use std::path::PathBuf;

pub const MAX: usize = 10;

pub fn dir() -> PathBuf {
    PathBuf::from(Config::dir()).join("history")
}

pub fn thumb_of(p: &std::path::Path) -> PathBuf {
    dir().join("thumbs").join(p.file_name().unwrap_or_default())
}

fn all() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "png"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.reverse();
    v
}

/// Newest first, at most MAX.
pub fn list() -> Vec<PathBuf> {
    let mut v = all();
    v.truncate(MAX);
    v
}

pub fn add(png: &[u8], thumb: &[u8]) {
    let d = dir();
    let _ = std::fs::create_dir_all(d.join("thumbs"));
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let p = d.join(format!("{ms:014}.png"));
    let _ = std::fs::write(&p, png);
    let _ = std::fs::write(thumb_of(&p), thumb);
    for old in all().into_iter().skip(MAX) {
        let _ = std::fs::remove_file(thumb_of(&old));
        let _ = std::fs::remove_file(old);
    }
}

pub fn clear() {
    let _ = std::fs::remove_dir_all(dir());
}
