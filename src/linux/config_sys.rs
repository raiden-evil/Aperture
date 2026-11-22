//! Linux side of the settings: XDG folders, autostart via ~/.config/autostart,
//! and handing shortcuts to the hotkey backend.
use crate::config::Config;
use std::path::PathBuf;

/// What the Super (Windows/Command) key is called in shortcut names.
pub const SUPER_NAME: &str = "Super";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

fn s(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

pub fn config_dir() -> String {
    s(xdg("XDG_CONFIG_HOME", ".config").join("aperture"))
}
pub fn data_dir() -> String {
    s(xdg("XDG_DATA_HOME", ".local/share").join("aperture"))
}

/// A folder from ~/.config/user-dirs.dirs (XDG_PICTURES_DIR="$HOME/Pictures").
fn user_dir(key: &str, fallback: &str) -> String {
    let file = xdg("XDG_CONFIG_HOME", ".config").join("user-dirs.dirs");
    if let Ok(text) = std::fs::read_to_string(file) {
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
                let v = v.trim().trim_matches('"').replace("$HOME", &home().to_string_lossy());
                if !v.is_empty() {
                    return v;
                }
            }
        }
    }
    s(home().join(fallback))
}

pub fn pictures_dir() -> String {
    user_dir("XDG_PICTURES_DIR", "Pictures")
}
pub fn videos_dir() -> String {
    user_dir("XDG_VIDEOS_DIR", "Videos")
}

/// Labels for the punctuation keys (Windows OEM virtual-key codes, which binds use everywhere).
pub fn key_label(vk: u16) -> Option<String> {
    let c = match vk {
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        0xC0 => "`",
        0xDB => "[",
        0xDC => "\\",
        0xDD => "]",
        0xDE => "'",
        _ => return None,
    };
    Some(c.into())
}

pub fn apply_binds(c: &Config) {
    super::hotkeys::set(&c.binds, c.override_keys);
}

fn autostart_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("autostart").join("aperture.desktop")
}

pub fn autostart() -> bool {
    autostart_file().exists()
}

pub fn set_autostart(on: bool) {
    let f = autostart_file();
    if on {
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "aperture".into());
        let body = format!(
            "[Desktop Entry]\nType=Application\nName=Aperture\nComment=Screenshot tool\nExec=\"{exe}\" --tray\nIcon=aperture\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(f, body);
    } else {
        let _ = std::fs::remove_file(f);
    }
}
