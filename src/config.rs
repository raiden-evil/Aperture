//! Settings shared by every platform: the values, their defaults and the
//! settings.ini format. Paths, autostart and global shortcuts come from the
//! platform backend (`config_sys`).
pub use crate::config_sys::*;

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
    /// maximum video height in pixels (0 = native, never upscaled)
    pub res: u32,
    pub video_folder: String,
}

pub const CLIP_STEPS: [u32; 12] = [15, 30, 45, 60, 90, 120, 180, 300, 600, 900, 1200, 1800];
pub const CLIP_MIN: u32 = 5;
pub const CLIP_MAX: u32 = 3600;
pub const FPS_STEPS: [u32; 10] = [15, 24, 30, 48, 60, 90, 120, 144, 165, 240];
pub const FPS_MIN: u32 = 1;
pub const FPS_MAX: u32 = 240;
/// Resolution presets (max output height; 0 = native).
pub const RES_STEPS: [u32; 5] = [0, 1440, 1080, 720, 480];

/// Parses a clip length: "90", "90s", "2m", "1:30", "1m30s", "1.5m".
pub fn parse_secs(t: &str) -> Option<u32> {
    let t = t.trim().to_ascii_lowercase().replace(' ', "");
    if t.is_empty() {
        return None;
    }
    if let Some((m, s)) = t.split_once(':') {
        let m: u32 = m.parse().ok()?;
        let s: u32 = s.parse().ok()?;
        return Some(m * 60 + s);
    }
    let t = t.trim_end_matches("ec").trim_end_matches("in");
    if let Some((m, rest)) = t.split_once('m') {
        let m: f32 = m.parse().ok()?;
        let s: f32 = if rest.is_empty() { 0.0 } else { rest.trim_end_matches('s').parse().ok()? };
        return Some((m * 60.0 + s).round() as u32);
    }
    let v: f32 = t.trim_end_matches('s').parse().ok()?;
    Some(v.round() as u32)
}

/// Step to the next/previous preset from an arbitrary current value.
pub fn step(steps: &[u32], cur: u32, up: bool) -> u32 {
    if up {
        steps.iter().copied().find(|s| *s > cur).unwrap_or(*steps.last().unwrap())
    } else {
        steps.iter().rev().copied().find(|s| *s < cur).unwrap_or(steps[0])
    }
}

pub fn fmt_secs(s: u32) -> String {
    if s < 60 {
        format!("{s} sec")
    } else if s % 60 == 0 {
        format!("{} min", s / 60)
    } else {
        format!("{}:{:02} min", s / 60, s % 60)
    }
}

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
        for (m, n) in [(M_WIN, SUPER_NAME), (M_CTRL, "Ctrl"), (M_ALT, "Alt"), (M_SHIFT, "Shift")] {
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
        _ => return key_label(vk).unwrap_or_else(|| format!("Key {vk}")),
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

impl Config {
    /// Where settings.ini lives.
    pub fn dir() -> String {
        config_dir()
    }
    /// Where the capture history lives.
    pub fn data_dir() -> String {
        data_dir()
    }
    pub fn default_video_folder() -> String {
        join(&videos_dir(), "Aperture")
    }
    pub fn replay_dir() -> std::path::PathBuf {
        std::env::temp_dir().join("Aperture").join("replay")
    }
    pub fn default_folder() -> String {
        join(&pictures_dir(), "Aperture")
    }
    pub fn load() -> Config {
        let mut c = Config { copy: true, save: true, folder: Self::default_folder(), override_keys: true,
            binds: DEFAULT_BINDS,
            theme: 0,
            replay: false,
            replay_secs: 60,
            fps: 30,
            res: 0,
            video_folder: Self::default_video_folder(),
        };
        if let Ok(s) = std::fs::read_to_string(std::path::Path::new(&Self::dir()).join("settings.ini")) {
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
                        "replay_secs" => c.replay_secs = v.parse::<u32>().unwrap_or(60).clamp(CLIP_MIN, CLIP_MAX),
                        "fps" => c.fps = v.parse::<u32>().unwrap_or(30).clamp(FPS_MIN, FPS_MAX),
                        "res" => c.res = v.parse::<u32>().unwrap_or(0).min(4320),
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
            "copy={}\nsave={}\noverride={}\nfolder={}\nvideo_folder={}\ntheme={}\nreplay={}\nreplay_secs={}\nfps={}\nres={}\n",
            b(self.copy),
            b(self.save),
            b(self.override_keys),
            self.folder,
            self.video_folder,
            self.theme,
            b(self.replay),
            self.replay_secs,
            self.fps,
            self.res
        );
        for (i, k) in ACTION_KEYS.iter().enumerate() {
            let [a, c] = self.binds[i];
            s += &format!("{k}={}:{},{}:{}\n", a.mods, a.vk, c.mods, c.vk);
        }
        let _ = std::fs::write(std::path::Path::new(&d).join("settings.ini"), s);
    }
}

fn join(dir: &str, name: &str) -> String {
    std::path::Path::new(dir).join(name).to_string_lossy().into_owned()
}
