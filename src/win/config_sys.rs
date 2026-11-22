//! Windows side of the settings: folders, autostart (registry Run key) and
//! handing shortcuts to the keyboard hook.
use crate::config::Config;
use crate::util::*;
use windows_sys::Win32::UI::Shell::{FOLDERID_Pictures, FOLDERID_RoamingAppData, FOLDERID_Videos};

/// What the Windows key is called in shortcut names.
pub const SUPER_NAME: &str = "Win";

pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

pub fn config_dir() -> String {
    format!("{}\\Aperture", known_folder(&FOLDERID_RoamingAppData))
}
pub fn data_dir() -> String {
    config_dir()
}
pub fn pictures_dir() -> String {
    known_folder(&FOLDERID_Pictures)
}
pub fn videos_dir() -> String {
    known_folder(&FOLDERID_Videos)
}

/// Punctuation keys: ask the active keyboard layout.
pub fn key_label(vk: u16) -> Option<String> {
    let c = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::MapVirtualKeyW(vk as u32, 2) } & 0xFFFF;
    match char::from_u32(c) {
        Some(ch) if c != 0 => Some(ch.to_uppercase().to_string()),
        _ => None,
    }
}

/// Push the shortcuts to the keyboard hook and keep Windows' own PrtSc handler out of the way.
pub fn apply_binds(c: &Config) {
    crate::hook::set_binds(&c.binds);
    crate::hook::OVERRIDE.store(c.override_keys, std::sync::atomic::Ordering::SeqCst);
    // only the installed copy touches the Windows setting (portable/dev runs leave it alone)
    if crate::install::running_from_install_dir() {
        set_windows_prtsc(!windows_prtsc_wanted_off(c));
    }
}

pub fn windows_prtsc_wanted_off(c: &Config) -> bool {
    c.override_keys && c.binds.iter().flatten().any(|b| b.vk == 0x2C && b.mods == 0)
}

pub fn autostart() -> bool {
    reg_get_str(RUN_KEY, "Aperture").is_some()
}

pub fn set_autostart(on: bool) {
    if on {
        reg_set_str(RUN_KEY, "Aperture", &format!("\"{}\" --tray", exe_path()));
    } else {
        reg_del_value(RUN_KEY, "Aperture");
    }
}

/// Windows 11 "Print Screen opens Snipping Tool" switch.
pub fn set_windows_prtsc(windows_owns_it: bool) {
    reg_set_dword("Control Panel\\Keyboard", "PrintScreenKeyForSnippingEnabled", windows_owns_it as u32);
}
