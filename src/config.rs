use crate::util::*;
use windows_sys::Win32::UI::Shell::{FOLDERID_Pictures, FOLDERID_RoamingAppData, FOLDERID_Videos};

pub struct Config {
    pub copy: bool,
    pub save: bool,
    pub folder: String,
    pub override_keys: bool,
    /// [action][slot]; actions: 0 region, 1 freeform, 2 full screen, 3 instant capture, 4 record, 5 save clip
    pub binds: [[Bind; 2]; 6],
    pub theme: u8,
    pub replay: bool,
    pub replay_secs: u32,
    pub fps: u32,
    pub video_folder: String,
}

pub const CLIP_STEPS: [u32; 12] = [15, 30, 45, 60, 90, 120, 180, 300, 600, 900, 1200, 1800];

pub fn fmt_secs(s: u32) -> String {
    if s < 60 {
        format!("{s} sec")
    } else if s % 60 == 0 {
        format!("{} min", s / 60)
    } else {
        format!("{}:{:02} min", s / 60, s % 60)
    }
}

pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

pub const M_WIN: u8 = 1;
pub const M_CTRL: u8 = 2;
pub const M_SHIFT: u8 = 4;
pub const M_ALT: u8 = 8;

pub const ACTION_NAMES: [&str; 6] = ["Region", "Freeform", "Full screen", "Instant capture", "Record video", "Save replay clip"];
const ACTION_KEYS: [&str; 6] = ["keys_region", "keys_free", "keys_full", "keys_instant", "keys_record", "keys_clip"];

/// A global shortcut: modifier mask + virtual key (vk 0 = unset).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Bind {
    pub mods: u8,
    pub vk: u16,
}

impl Bind {
    pub const NONE: Bind = Bind { mods: 0, vk: 0 };
    pub const fn new(mods: u8, vk: u16) -> Bind {
        Bind { mods, vk }
    }
    pub fn is_set(&self) -> bool {
        self.vk != 0
    }
    /// Plain keys are only allowed for keys nobody types with (PrtSc, F-keys, Pause, ScrLk);
    /// everything else needs Ctrl, Alt or Win so it can't hijack normal typing.
    pub fn valid(&self) -> bool {
        let special = matches!(self.vk, 0x2C | 0x13 | 0x91 | 0x70..=0x87);
        self.vk != 0 && (special || self.mods & (M_WIN | M_CTRL | M_ALT) != 0)
    }
    pub fn name(&self) -> String {
        if !self.is_set() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        for (m, n) in [(M_WIN, "Win"), (M_CTRL, "Ctrl"), (M_ALT, "Alt"), (M_SHIFT, "Shift")] {
            if self.mods & m != 0 {
                parts.push(n.into());
            }
        }
        parts.push(key_name(self.vk));
        parts.join("+")
    }
}

pub fn key_name(vk: u16) -> String {
    let s = match vk {
        0x30..=0x39 | 0x41..=0x5A => return (vk as u8 as char).to_string(),
        0x70..=0x87 => return format!("F{}", vk - 0x6F),
        0x60..=0x69 => return format!("Num{}", vk - 0x60),
        0x2C => "PrtSc",
        0x13 => "Pause",
        0x91 => "ScrLk",
        0x2D => "Ins",
        0x2E => "Del",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PgUp",
        0x22 => "PgDn",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0x20 => "Space",
        0x09 => "Tab",
        0x0D => "Enter",
        0x1B => "Esc",
        0x08 => "Backspace",
        0x6A => "Num*",
        0x6B => "Num+",
        0x6D => "Num-",
        0x6E => "Num.",
        0x6F => "Num/",
        _ => {
            // punctuation: ask the active keyboard layout
            let c = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::MapVirtualKeyW(vk as u32, 2) } & 0xFFFF;
            return match char::from_u32(c) {
                Some(ch) if c != 0 => ch.to_uppercase().to_string(),
                _ => format!("Key {vk}"),
            };
        }
    };
    s.into()
}

pub const DEFAULT_BINDS: [[Bind; 2]; 6] = [
    [Bind::new(M_WIN | M_SHIFT, 0x53), Bind::new(0, 0x2C)], // Win+Shift+S, PrtSc
    [Bind::new(M_SHIFT, 0x2C), Bind::NONE],                 // Shift+PrtSc
    [Bind::NONE, Bind::NONE],                               // (unset)
    [Bind::new(M_CTRL, 0x2C), Bind::new(M_WIN, 0x2C)],      // Ctrl+PrtSc, Win+PrtSc
    [Bind::new(M_WIN | M_ALT, 0x52), Bind::NONE],          // Win+Alt+R (like Game Bar)
    [Bind::new(M_WIN | M_ALT, 0x47), Bind::NONE],          // Win+Alt+G (like Game Bar)
];

fn parse_binds(v: &str) -> [Bind; 2] {
    let mut out = [Bind::NONE; 2];
    for (i, part) in v.split(',').take(2).enumerate() {
        if let Some((m, k)) = part.trim().split_once(':') {
            let b = Bind::new(m.parse().unwrap_or(0), k.parse().unwrap_or(0));
            if b.valid() {
                out[i] = b;
            }
        }
    }
    out
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

impl Config {
    pub fn dir() -> String {
        format!("{}\\Aperture", known_folder(&FOLDERID_RoamingAppData))
    }
    pub fn default_video_folder() -> String {
        format!("{}\\Aperture", known_folder(&FOLDERID_Videos))
    }
    pub fn replay_dir() -> std::path::PathBuf {
        std::env::temp_dir().join("Aperture").join("replay")
    }
    pub fn default_folder() -> String {
        format!("{}\\Aperture", known_folder(&FOLDERID_Pictures))
    }
    pub fn load() -> Config {
        let mut c = Config { copy: true, save: true, folder: Self::default_folder(), override_keys: true,
            binds: DEFAULT_BINDS,
            theme: 0,
            replay: false,
            replay_secs: 60,
            fps: 30,
            video_folder: Self::default_video_folder(),
        };
        if let Ok(s) = std::fs::read_to_string(format!("{}\\settings.ini", Self::dir())) {
            for line in s.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    let v = v.trim();
                    match k.trim() {
                        "copy" => c.copy = v == "1",
                        "save" => c.save = v == "1",
                        "override" => c.override_keys = v == "1",
                        "folder" if !v.is_empty() => c.folder = v.to_string(),
                        "video_folder" if !v.is_empty() => c.video_folder = v.to_string(),
                        "theme" => c.theme = v.parse::<u8>().unwrap_or(0).min(2),
                        "replay" => c.replay = v == "1",
                        "replay_secs" => c.replay_secs = v.parse::<u32>().unwrap_or(60).clamp(15, 1800),
                        "fps" => c.fps = if v == "60" { 60 } else { 30 },
                        k => {
                            if let Some(i) = ACTION_KEYS.iter().position(|a| *a == k) {
                                c.binds[i] = parse_binds(v);
                            }
                        }
                    }
                }
            }
        }
        c
    }
    pub fn save(&self) {
        let d = Self::dir();
        let _ = std::fs::create_dir_all(&d);
        let b = |v: bool| if v { "1" } else { "0" };
        let mut s = format!(
            "copy={}\nsave={}\noverride={}\nfolder={}\nvideo_folder={}\ntheme={}\nreplay={}\nreplay_secs={}\nfps={}\n",
            b(self.copy),
            b(self.save),
            b(self.override_keys),
            self.folder,
            self.video_folder,
            self.theme,
            b(self.replay),
            self.replay_secs,
            self.fps
        );
        for (i, k) in ACTION_KEYS.iter().enumerate() {
            let [a, c] = self.binds[i];
            s += &format!("{k}={}:{},{}:{}\n", a.mods, a.vk, c.mods, c.vk);
        }
        let _ = std::fs::write(format!("{}\\settings.ini", d), s);
    }
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
