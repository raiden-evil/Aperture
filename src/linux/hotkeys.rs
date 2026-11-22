//! Global shortcuts.
//!  * X11: XGrabKey on the root window, re-grabbed whenever the settings change.
//!  * Wayland: the GlobalShortcuts portal (KDE Plasma, GNOME 48+, Hyprland…). The
//!    desktop owns the final key choice; ours is only the suggested trigger.
//!    Without the portal, users bind `aperture --region` etc. in their desktop's
//!    keyboard settings (the command-line forwarding in `ipc` handles those).
use super::{post, UserEvent};
use crate::config::{Bind, M_ALT, M_CTRL, M_SHIFT, M_WIN};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::Mutex;

/// region, freeform, full screen, instant, record, save clip
pub const ACTIONS: usize = 6;

static BINDS: Mutex<[[Bind; 2]; ACTIONS]> = Mutex::new([[Bind::NONE; 2]; ACTIONS]);
static ENABLED: AtomicBool = AtomicBool::new(true);
static VERSION: AtomicU64 = AtomicU64::new(1);
static STARTED: AtomicBool = AtomicBool::new(false);
static STATUS: Mutex<String> = Mutex::new(String::new());

pub fn set(binds: &[[Bind; 2]], enabled: bool) {
    let mut b = BINDS.lock().unwrap();
    for (dst, src) in b.iter_mut().zip(binds.iter()) {
        *dst = *src;
    }
    ENABLED.store(enabled, SeqCst);
    VERSION.fetch_add(1, SeqCst);
}

/// One line for the settings window describing how shortcuts work here.
pub fn status() -> String {
    STATUS.lock().unwrap().clone()
}
fn set_status(s: impl Into<String>) {
    *STATUS.lock().unwrap() = s.into();
}

pub fn start(wayland: bool) {
    if STARTED.swap(true, SeqCst) {
        return;
    }
    if wayland {
        set_status("Connecting to your desktop's shortcut service…");
        std::thread::spawn(portal_thread);
    } else {
        set_status("Shortcuts work everywhere while Aperture runs.");
        std::thread::spawn(|| {
            if let Err(e) = x11_thread() {
                set_status(format!("Global shortcuts unavailable: {e}"));
            }
        });
    }
}

// ---------------------------------------------------------------- key names

/// Windows virtual-key code → X keysym.
fn keysym(vk: u16) -> Option<u32> {
    Some(match vk {
        0x41..=0x5A => vk as u32 + 0x20, // a..z
        0x30..=0x39 => vk as u32,
        0x70..=0x87 => 0xFFBE + (vk as u32 - 0x70), // F1..F24
        0x60..=0x69 => 0xFFB0 + (vk as u32 - 0x60), // KP_0..KP_9
        0x2C => 0xFF61,                             // Print
        0x13 => 0xFF13,                             // Pause
        0x91 => 0xFF14,                             // Scroll_Lock
        0x2D => 0xFF63,
        0x2E => 0xFFFF,
        0x24 => 0xFF50,
        0x23 => 0xFF57,
        0x21 => 0xFF55,
        0x22 => 0xFF56,
        0x25 => 0xFF51,
        0x26 => 0xFF52,
        0x27 => 0xFF53,
        0x28 => 0xFF54,
        0x20 => 0x20,
        0x09 => 0xFF09,
        0x0D => 0xFF0D,
        0x08 => 0xFF08,
        0xBA => b';' as u32,
        0xBB => b'=' as u32,
        0xBC => b',' as u32,
        0xBD => b'-' as u32,
        0xBE => b'.' as u32,
        0xBF => b'/' as u32,
        0xC0 => b'`' as u32,
        0xDB => b'[' as u32,
        0xDC => b'\\' as u32,
        0xDD => b']' as u32,
        0xDE => b'\'' as u32,
        _ => return None,
    })
}

/// XKB keysym name, as the shortcuts portal expects in a trigger ("LOGO+SHIFT+s").
fn keysym_name(vk: u16) -> Option<String> {
    Some(match vk {
        0x41..=0x5A => ((vk as u8 + 0x20) as char).to_string(),
        0x30..=0x39 => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x60..=0x69 => format!("KP_{}", vk - 0x60),
        0x2C => "Print".into(),
        0x13 => "Pause".into(),
        0x91 => "Scroll_Lock".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x24 => "Home".into(),
        0x23 => "End".into(),
        0x21 => "Prior".into(),
        0x22 => "Next".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x20 => "space".into(),
        0x09 => "Tab".into(),
        0x0D => "Return".into(),
        0xBA => "semicolon".into(),
        0xBB => "equal".into(),
        0xBC => "comma".into(),
        0xBD => "minus".into(),
        0xBE => "period".into(),
        0xBF => "slash".into(),
        0xC0 => "grave".into(),
        0xDB => "bracketleft".into(),
        0xDC => "backslash".into(),
        0xDD => "bracketright".into(),
        0xDE => "apostrophe".into(),
        _ => return None,
    })
}

fn trigger(b: &Bind) -> Option<String> {
    let mut parts = Vec::new();
    for (m, n) in [(M_CTRL, "CTRL"), (M_ALT, "ALT"), (M_SHIFT, "SHIFT"), (M_WIN, "LOGO")] {
        if b.mods & m != 0 {
            parts.push(n.to_string());
        }
    }
    parts.push(keysym_name(b.vk)?);
    Some(parts.join("+"))
}

// ---------------------------------------------------------------- X11

fn x_mods(m: u8) -> u16 {
    let mut x = 0;
    if m & M_SHIFT != 0 {
        x |= 1;
    }
    if m & M_CTRL != 0 {
        x |= 4;
    }
    if m & M_ALT != 0 {
        x |= 8; // Mod1
    }
    if m & M_WIN != 0 {
        x |= 64; // Mod4
    }
    x
}

fn x11_thread() -> Result<(), String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};
    use x11rb::protocol::Event;
    let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = conn.setup().roots[screen].root;
    let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let map = conn.get_keyboard_mapping(min, max - min + 1).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
    let per = map.keysyms_per_keycode.max(1) as usize;
    let keycode_of = |sym: u32| -> Option<u8> {
        map.keysyms.chunks(per).position(|c| c.contains(&sym)).map(|i| min + i as u8)
    };
    // Caps Lock and Num Lock must not stop a shortcut from firing
    const LOCKS: [u16; 4] = [0, 2, 16, 18];
    let mut seen = 0u64;
    let mut grabbed: Vec<(u8, u16, usize)> = Vec::new();
    loop {
        let v = VERSION.load(SeqCst);
        if v != seen {
            seen = v;
            for (kc, m, _) in grabbed.drain(..) {
                for l in LOCKS {
                    let _ = conn.ungrab_key(kc, root, ModMask::from(m | l));
                }
            }
            if ENABLED.load(SeqCst) {
                let binds = *BINDS.lock().unwrap();
                let mut failed = 0;
                for (a, row) in binds.iter().enumerate() {
                    for b in row.iter().filter(|b| b.is_set()) {
                        let Some(kc) = keysym(b.vk).and_then(keycode_of) else { continue };
                        let m = x_mods(b.mods);
                        for l in LOCKS {
                            let ok = conn
                                .grab_key(false, root, ModMask::from(m | l), kc, GrabMode::ASYNC, GrabMode::ASYNC)
                                .ok()
                                .and_then(|c| c.check().ok())
                                .is_some();
                            if !ok && l == 0 {
                                failed += 1; // another program owns this combination
                            }
                        }
                        grabbed.push((kc, m, a));
                    }
                }
                set_status(if failed > 0 {
                    format!("{failed} shortcut(s) are taken by another app or your desktop. Pick different keys.")
                } else {
                    "Shortcuts work everywhere while Aperture runs.".into()
                });
            } else {
                set_status("Global shortcuts are off.");
            }
            let _ = conn.flush();
        }
        while let Some(ev) = conn.poll_for_event().map_err(|e| e.to_string())? {
            if let Event::KeyPress(k) = ev {
                let m = u16::from(k.state) & (1 | 4 | 8 | 64);
                if let Some(&(_, _, a)) = grabbed.iter().find(|(kc, gm, _)| *kc == k.detail && *gm == m) {
                    post(UserEvent::Action(a + 1, None));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

// ---------------------------------------------------------------- Wayland portal

const IDS: [&str; ACTIONS] = ["region", "freeform", "full", "instant", "record", "clip"];

fn portal_thread() {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;
    let res = pollster::block_on(async {
        let gs = GlobalShortcuts::new().await?;
        let session = gs.create_session(Default::default()).await?;
        let binds = *BINDS.lock().unwrap();
        let names = crate::config::ACTION_NAMES;
        let triggers: Vec<Option<String>> = binds.iter().map(|row| row.iter().find(|b| b.is_set()).and_then(trigger)).collect();
        let list: Vec<NewShortcut> =
            IDS.iter().enumerate().map(|(i, id)| NewShortcut::new(*id, format!("Aperture: {}", names[i])).preferred_trigger(triggers[i].as_deref())).collect();
        let bound = gs.bind_shortcuts(&session, &list, None, Default::default()).await?.response()?;
        let keys: Vec<String> = bound
            .shortcuts()
            .iter()
            .filter(|s| !s.trigger_description().is_empty())
            .map(|s| format!("{}: {}", s.description().trim_start_matches("Aperture: "), s.trigger_description()))
            .collect();
        set_status(if keys.is_empty() {
            "Shortcuts are registered with your desktop. Assign keys in its keyboard settings.".to_string()
        } else {
            format!("Set by your desktop: {}", keys.join("  ·  "))
        });
        let mut stream = gs.receive_activated().await?;
        while let Some(a) = stream.next().await {
            if !ENABLED.load(SeqCst) {
                continue;
            }
            if let Some(i) = IDS.iter().position(|id| *id == a.shortcut_id()) {
                let token = a.options().get("activation_token").and_then(|v| String::try_from(v.try_clone().ok()?).ok());
                post(UserEvent::Action(i + 1, token));
            }
        }
        Ok::<(), ashpd::Error>(())
    });
    if res.is_err() {
        set_status("No shortcut service here: add desktop shortcuts that run aperture --region, --record, --clip…");
    }
}
