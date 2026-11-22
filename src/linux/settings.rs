//! Settings window: capture tiles, output options, and tabs for capture
//! history, shortcuts and theme. Same look as the Windows window.
use super::output::open_path;
use super::Surface;
use crate::config::{self, Bind, Config, M_ALT, M_CTRL, M_SHIFT, M_WIN};
use crate::gfx::*;
use crate::icons::{self, Icon};
use crate::theme::{self, pal, Pal};
use crate::store;
use std::path::PathBuf;
use std::rc::Rc;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorIcon, Window};

pub const W: f32 = 760.0;
pub const H: f32 = 540.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Close,
    Cap(usize),
    Copy,
    Save,
    Folder,
    OpenFolder,
    Override,
    Autostart,
    Thumb(usize),
    Clear,
    Tab(u8),
    Bind(usize, usize),
    ResetKeys,
    Theme(u8),
    Replay,
    ClipMinus,
    ClipPlus,
    ClipEdit,
    FpsMinus,
    FpsPlus,
    FpsEdit,
    Res(u32),
    VideoFolder,
    OpenVideoFolder,
    WarnPlain,
    ConfirmYes,
    ConfirmNo,
    ConfirmNever,
}

/// A number being typed into one of the recording steppers.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Field {
    Clip,
    Fps,
}
struct Edit {
    field: Field,
    text: String,
    /// true until the first key: typing replaces the prefilled value
    fresh: bool,
}

/// Things only the app can do.
pub enum Cmd {
    Close,
    Capture(usize),
    /// pick the screenshot folder (false) or the video folder (true)
    PickFolder(bool),
    CopyFile(PathBuf),
    /// replay on/off or its fps / resolution / length changed
    ReplayChanged,
    BindsChanged,
}

struct Thumb {
    path: PathBuf,
    px: Pixels,
}

pub struct Settings {
    pub win: Rc<Window>,
    surf: Surface,
    s: f32,
    hover: Option<Hit>,
    items: Vec<(Hit, [f32; 4])>,
    thumbs: Vec<Thumb>,
    back: Vec<u32>,
    autostart: bool,
    tab: u8,
    recording: Option<(usize, usize)>,
    mods: ModifiersState,
    cur: (f32, f32),
    pub tray: bool,
    edit: Option<Edit>,
    /// a single-key shortcut waiting for "Use anyway": (action, slot, shortcut, don't ask again)
    confirm: Option<(usize, usize, Bind, bool)>,
}

// ---------- shared widgets (theme aware) ----------
pub fn pill(g: &Gfx, x: f32, y: f32, w: f32, h: f32, label: &str, primary: bool, hover: bool) {
    let p = pal();
    let path = Path::rrect(x, y, w, h, h / 2.0);
    g.fill(&Path::rrect(x, y + 1.2, w, h, h / 2.0), if p.dark { 0x40000000 } else { 0x16000000 });
    let tc = if primary {
        let (a, b) = if hover { (0xFF62A8FF, 0xFF2068E6) } else { (0xFF4D96FF, 0xFF1A59CC) };
        g.fill_v(&path, y, y + h, a, b);
        g.stroke(&path, 0x70002070, 1.0);
        g.stroke(&Path::rrect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, h / 2.0 - 1.0), 0x55FFFFFF, 1.0);
        0xFFFFFFFF
    } else {
        g.fill_v(&path, y, y + h, p.btn0, if hover { p.btn_hover1 } else { p.btn1 });
        g.stroke(&path, p.btn_line, 1.0);
        p.btn_text
    };
    g.text(label, x, y, w, h, 12.5, tc, SEMI | CENTER);
}

pub fn toggle(g: &Gfx, x: f32, y: f32, on: bool) {
    let p = pal();
    let (w, h) = (44.0, 24.0);
    let tr = Path::rrect(x, y, w, h, h / 2.0);
    if on {
        g.fill_v(&tr, y, y + h, 0xFF27A145, 0xFF4CD964);
    } else {
        g.fill_v(&tr, y, y + h, p.off0, p.off1);
    }
    g.stroke(&Path::rrect(x + 1.0, y + 1.6, w - 2.0, h - 2.2, h / 2.0 - 1.0), 0x22000000, 1.6);
    g.stroke(&tr, 0x38000000, 1.0);
    let d = h - 4.0;
    let kx = if on { x + w - d - 2.0 } else { x + 2.0 };
    g.fill(&Path::ellipse(kx, y + 3.2, d, d), 0x40000000);
    let knob = Path::ellipse(kx, y + 2.0, d, d);
    g.fill_v(&knob, y + 2.0, y + 2.0 + d, 0xFFFFFFFF, p.knob1);
    g.stroke(&knob, 0x33000000, 0.8);
}

fn card(g: &Gfx, x: f32, y: f32, w: f32, h: f32) {
    let p = pal();
    g.fill(&Path::rrect(x, y + 1.5, w, h, 14.0), p.card_shadow);
    let c = Path::rrect(x, y, w, h, 14.0);
    g.fill(&c, p.card);
    g.stroke(&c, p.card_line, 1.0);
}

fn section(g: &Gfx, x: f32, y: f32, t: &str) {
    g.text(t, x + 2.0, y, 300.0, 16.0, 10.5, pal().text3, SEMI);
}

/// Skeuomorphic keyboard key. state: 0 = set, 1 = empty, 2 = recording.
fn keycap(g: &Gfx, x: f32, y: f32, w: f32, h: f32, label: &str, state: u8, hover: bool) {
    let p = pal();
    match state {
        2 => {
            let r = Path::rrect(x, y, w, h, 7.0);
            g.fill_v(&r, y, y + h, if p.dark { 0xFF1D3357 } else { 0xFFEAF2FF }, if p.dark { 0xFF16294A } else { 0xFFD3E4FF });
            g.stroke(&r, 0xFF2D7FF9, 1.6);
            g.text("Press keys…", x, y, w, h, 10.5, p.link, SEMI | CENTER);
        }
        1 => {
            let r = Path::rrect(x, y, w, h, 7.0);
            g.fill(&r, if hover { p.key_empty_hover } else { p.key_empty });
            g.stroke(&r, p.card_line, 1.0);
            g.text("+ Add", x, y, w, h, 11.0, if hover { p.text2 } else { p.text3 }, CENTER);
        }
        _ => {
            g.fill(&Path::rrect(x, y + 1.0, w, h, 7.0), p.key_side);
            let top = Path::rrect(x, y, w, h - 2.5, 7.0);
            g.fill_v(&top, y, y + h - 2.5, p.key0, if hover { p.key_hover1 } else { p.key1 });
            g.stroke(&Path::rrect(x, y, w, h + 1.0, 7.0), p.key_line, 0.8);
            g.text(label, x + 3.0, y, w - 6.0, h - 2.5, 11.0, p.key_text, SEMI | CENTER);
        }
    }
}

fn segmented(g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, x: f32, y: f32, w: f32, h: f32, labels: &[(&str, Hit)], sel: usize, hv: Option<Hit>) {
    let p = pal();
    let path = Path::rrect(x, y, w, h, h / 2.0);
    g.fill_v(&path, y, y + h, p.seg0, p.seg1);
    g.stroke(&Path::rrect(x + 0.5, y + 1.0, w - 1.0, h - 1.0, h / 2.0 - 0.5), 0x16000000, 1.0);
    let sw = w / labels.len() as f32;
    for (i, (label, hit)) in labels.iter().enumerate() {
        let sx = x + i as f32 * sw;
        if i == sel {
            g.fill(&Path::rrect(sx + 2.0, y + 3.2, sw - 4.0, h - 4.0, (h - 4.0) / 2.0), 0x26000000);
            let k = Path::rrect(sx + 2.0, y + 2.0, sw - 4.0, h - 4.0, (h - 4.0) / 2.0);
            g.fill_v(&k, y + 2.0, y + h - 2.0, p.seg_on0, p.seg_on1);
            g.text(label, sx, y, sw, h, 11.5, p.text, SEMI | CENTER);
        } else {
            g.text(label, sx, y, sw, h, 11.5, if hv == Some(*hit) { p.text } else { p.text2 }, SEMI | CENTER);
            items.push((*hit, [sx, y, sw, h]));
        }
    }
}

/// Miniature of the settings window in a given palette (theme picker).
fn theme_preview(g: &Gfx, p: &Pal, x: f32, y: f32, w: f32, h: f32) {
    let win = Path::rrect(x, y, w, h, 9.0);
    g.fill(&win, p.bg);
    g.clip(&win);
    g.fill_v(&Path::rect(x, y, w, 20.0), y, y + 20.0, p.head0, p.head1);
    icons::camera(g, x + 6.0, y + 4.0, 12.0);
    g.fill(&Path::rrect(x + 22.0, y + 8.0, 30.0, 4.0, 2.0), p.title);
    let c = Path::rrect(x + 6.0, y + 26.0, w - 12.0, h - 32.0, 6.0);
    g.fill(&c, p.card);
    g.stroke(&c, p.card_line, 0.8);
    for (i, on) in [true, false].iter().enumerate() {
        let ry = y + 34.0 + i as f32 * 18.0;
        g.fill(&Path::rrect(x + 12.0, ry + 2.0, w * 0.38, 4.0, 2.0), p.text2);
        let t = Path::rrect(x + w - 32.0, ry, 18.0, 9.0, 4.5);
        g.fill(&t, if *on { 0xFF34C759 } else { p.off0 });
        g.fill(&Path::circle(if *on { x + w - 18.5 } else { x + w - 27.5 }, ry + 4.5, 3.5), 0xFFFFFFFF);
    }
    g.unclip();
    g.stroke(&win, if p.dark { 0x40FFFFFF } else { 0x30000000 }, 1.0);
}

/// `− value +` control. Clicking the value turns it into a text field (`edit`).
fn stepper(g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, right: f32, y: f32, value: &str, edit: Option<&Edit>, hits: (Hit, Hit, Hit), hv: Option<Hit>) {
    let p = pal();
    let (minus_h, val_h, plus_h) = hits;
    let plus = [right - 28.0, y, 28.0, 28.0];
    let val = [plus[0] - 82.0, y, 78.0, 28.0];
    let minus = [val[0] - 32.0, y, 28.0, 28.0];
    pill(g, minus[0], minus[1], minus[2], minus[3], "−", false, hv == Some(minus_h));
    pill(g, plus[0], plus[1], plus[2], plus[3], "+", false, hv == Some(plus_h));
    let field = Path::rrect(val[0], val[1], val[2], val[3], 7.0);
    if let Some(e) = edit {
        g.fill_v(&field, val[1], val[1] + val[3], if p.dark { 0xFF1D3357 } else { 0xFFF4F8FF }, if p.dark { 0xFF16294A } else { 0xFFE6EFFF });
        g.stroke(&field, 0xFF2D7FF9, 1.6);
        let (tw, _) = if e.text.is_empty() { (0.0, 0.0) } else { g.measure_at(&e.text, 12.5, SEMI) };
        let cx = val[0] + val[2] / 2.0;
        if e.fresh && !e.text.is_empty() {
            g.fill(&Path::rrect(cx - tw / 2.0 - 2.0, val[1] + 6.0, tw + 4.0, val[3] - 12.0, 3.0), 0x552D7FF9);
        }
        g.text(&e.text, val[0], val[1], val[2], val[3], 12.5, p.text, SEMI | CENTER);
        let caret = cx + tw / 2.0 + 1.5;
        g.line(caret, val[1] + 7.0, caret, val[1] + val[3] - 7.0, p.link, 1.4);
    } else {
        if hv == Some(val_h) {
            g.fill(&field, p.hover);
            g.stroke(&field, p.btn_line, 1.0);
        }
        g.text(value, val[0], val[1], val[2], val[3], 12.5, p.text, SEMI | CENTER);
    }
    items.push((minus_h, minus));
    items.push((val_h, val));
    items.push((plus_h, plus));
}

/// Little square checkbox.
fn checkbox(g: &Gfx, x: f32, y: f32, on: bool, hover: bool) {
    let p = pal();
    let r = Path::rrect(x, y, 18.0, 18.0, 5.0);
    if on {
        g.fill_v(&r, y, y + 18.0, 0xFF4D96FF, 0xFF1A59CC);
        g.polyline(&[(x + 4.5, y + 9.5), (x + 7.8, y + 12.8), (x + 13.5, y + 5.8)], 0xFFFFFFFF, 2.0);
    } else {
        g.fill_v(&r, y, y + 18.0, p.btn0, if hover { p.btn_hover1 } else { p.btn1 });
        g.stroke(&r, p.btn_line, 1.0);
    }
}

/// Stores a shortcut, taking it away from any other action that had it.
fn set_bind(cfg: &mut Config, a: usize, s: usize, b: Bind) {
    for slot in cfg.binds.iter_mut().flatten() {
        if *slot == b {
            *slot = Bind::NONE; // a shortcut can only do one thing
        }
    }
    cfg.binds[a][s] = b;
    for row in cfg.binds.iter_mut() {
        if !row[0].is_set() && row[1].is_set() {
            row.swap(0, 1);
        }
    }
    cfg.save();
}

fn key_name(cfg: &Config, i: usize) -> String {
    cfg.binds[i].iter().find(|b| b.is_set()).map(|b| b.name()).unwrap_or_else(|| "No shortcut".into())
}

/// Physical key → Windows virtual-key code (the portable code binds are stored in).
fn vk_of(k: KeyCode) -> Option<u16> {
    use KeyCode::*;
    let letters = [KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ];
    if let Some(i) = letters.iter().position(|x| *x == k) {
        return Some(0x41 + i as u16);
    }
    let digits = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    if let Some(i) = digits.iter().position(|x| *x == k) {
        return Some(0x30 + i as u16);
    }
    let fkeys = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24];
    if let Some(i) = fkeys.iter().position(|x| *x == k) {
        return Some(0x70 + i as u16);
    }
    let pad = [Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9];
    if let Some(i) = pad.iter().position(|x| *x == k) {
        return Some(0x60 + i as u16);
    }
    Some(match k {
        PrintScreen => 0x2C,
        Pause => 0x13,
        ScrollLock => 0x91,
        Insert => 0x2D,
        Delete => 0x2E,
        Home => 0x24,
        End => 0x23,
        PageUp => 0x21,
        PageDown => 0x22,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        Space => 0x20,
        Tab => 0x09,
        Enter => 0x0D,
        Escape => 0x1B,
        Backspace => 0x08,
        Semicolon => 0xBA,
        Equal => 0xBB,
        Comma => 0xBC,
        Minus => 0xBD,
        Period => 0xBE,
        Slash => 0xBF,
        Backquote => 0xC0,
        BracketLeft => 0xDB,
        Backslash => 0xDC,
        BracketRight => 0xDD,
        Quote => 0xDE,
        _ => return None,
    })
}

impl Settings {
    pub fn new(win: Rc<Window>, surf: Surface, tray: bool) -> Settings {
        let mut st = Settings {
            s: win.scale_factor() as f32,
            win,
            surf,
            hover: None,
            items: Vec::new(),
            thumbs: Vec::new(),
            back: Vec::new(),
            autostart: config::autostart(),
            tab: 0,
            recording: None,
            mods: ModifiersState::empty(),
            cur: (0.0, 0.0),
            tray,
            edit: None,
            confirm: None,
        };
        st.refresh();
        st
    }

    /// Reload the history thumbnails.
    pub fn refresh(&mut self) {
        self.thumbs.clear();
        for p in store::list() {
            let tp = store::thumb_of(&p);
            let img = match decode_file(&tp.to_string_lossy()) {
                Some(i) => i,
                None => match decode_file(&p.to_string_lossy()) {
                    Some(i) => i.fit(320, 240),
                    None => continue,
                },
            };
            if let Some(px) = Pixels::from_img(&img) {
                self.thumbs.push(Thumb { path: p, px });
            }
        }
        self.win.request_redraw();
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.items.iter().rev().find(|(_, r)| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]).map(|(h, _)| *h)
    }

    fn paint(&mut self, cfg: &Config) {
        let size = self.win.inner_size();
        let (cw, ch) = (size.width.max(1) as i32, size.height.max(1) as i32);
        let (Some(nw), Some(nh)) = (std::num::NonZeroU32::new(cw as u32), std::num::NonZeroU32::new(ch as u32)) else { return };
        let _ = self.surf.resize(nw, nh);
        let mut back = std::mem::take(&mut self.back);
        back.resize((cw * ch) as usize, 0);
        let mut items: Vec<(Hit, [f32; 4])> = Vec::new();
        let hv = self.hover;
        let p = pal();
        {
            let g = Gfx::over_slice(&mut back, cw, ch);
            g.scale(self.s);
            g.rect(0.0, 0.0, W + 2.0, H + 2.0, p.bg);
            g.fill_v(&Path::rect(0.0, 0.0, W + 2.0, 76.0), 0.0, 76.0, p.head0, p.head1);
            g.stroke(&Path::rect(0.5, 0.5, W - 1.0, H - 1.0), p.card_line, 1.0);

            // header
            icons::camera(&g, 22.0, 14.0, 46.0);
            g.text("Aperture", 78.0, 16.0, 300.0, 28.0, 22.0, p.title, SEMI);
            g.text("Screenshots at the speed of light", 79.0, 44.0, 300.0, 18.0, 12.5, p.text2, 0);
            let cr = [W - 50.0, 22.0, 28.0, 28.0];
            if hv == Some(Hit::Close) {
                icons::draw(&g, Icon::Close, cr[0], cr[1], cr[2]);
            } else {
                let c = Path::circle(cr[0] + 14.0, cr[1] + 14.0, 11.0);
                g.fill_v(&c, cr[1] + 3.0, cr[1] + 25.0, p.close0, p.close1);
                g.stroke(&c, p.card_line, 1.0);
                g.line(cr[0] + 10.0, cr[1] + 10.0, cr[0] + 18.0, cr[1] + 18.0, p.close_x, 1.6);
                g.line(cr[0] + 18.0, cr[1] + 10.0, cr[0] + 10.0, cr[1] + 18.0, p.close_x, 1.6);
            }
            items.push((Hit::Close, cr));

            // ---- left column ----
            let lx = 22.0;
            let lw = 360.0;
            section(&g, lx, 86.0, "CAPTURE");
            let tw = (lw - 30.0) / 4.0;
            let tiles = [(Icon::Region, "Region", 0), (Icon::Free, "Freeform", 1), (Icon::Full, "Full screen", 2), (Icon::Record, "Record", 4)];
            for (i, (ic, name, bind)) in tiles.iter().enumerate() {
                let x = lx + i as f32 * (tw + 10.0);
                let on = hv == Some(Hit::Cap(i));
                let y = if on { 103.0 } else { 104.0 };
                let th = 86.0;
                g.fill(&Path::rrect(x, y + 2.0, tw, th, 14.0), if on { 0x1A1F6FEB } else { p.card_shadow });
                let path = Path::rrect(x, y, tw, th, 14.0);
                g.fill_v(&path, y, y + th, p.tile0, if on { p.tile_hover1 } else { p.tile1 });
                g.stroke(&path, if on { 0x902D7FF9 } else { p.tile_line }, 1.0);
                g.stroke(&Path::rrect(x + 1.0, y + 1.0, tw - 2.0, th - 2.0, 13.0), p.tile_hi, 1.0);
                icons::draw(&g, *ic, x + (tw - 32.0) / 2.0, y + 11.0, 32.0);
                g.text(name, x, y + 48.0, tw, 18.0, 12.5, p.text, SEMI | CENTER);
                g.text(&key_name(cfg, *bind), x + 2.0, y + 66.0, tw - 4.0, 14.0, 10.0, p.text3, CENTER);
                items.push((Hit::Cap(i), [x, 104.0, tw, th]));
            }

            section(&g, lx, 206.0, "OUTPUT");
            let cy = 224.0;
            card(&g, lx, cy, lw, 138.0);
            let rows = [(Hit::Copy, "Copy to clipboard", cfg.copy), (Hit::Save, "Save to folder", cfg.save)];
            for (i, (hit, label, on)) in rows.iter().enumerate() {
                let y = cy + i as f32 * 46.0;
                if hv == Some(*hit) {
                    g.fill(&Path::rrect4(lx + 1.0, y + 1.0, lw - 2.0, 45.0, if i == 0 { 13.0 } else { 0.0 }, if i == 0 { 13.0 } else { 0.0 }, 0.0, 0.0), p.hover);
                }
                g.text(label, lx + 16.0, y, 250.0, 46.0, 13.5, p.text, 0);
                toggle(&g, lx + lw - 60.0, y + 11.0, *on);
                g.line(lx + 16.0, y + 46.0, lx + lw - 16.0, y + 46.0, p.divider, 1.0);
                items.push((*hit, [lx, y, lw, 46.0]));
            }
            let fy = cy + 92.0;
            icons::draw(&g, Icon::Folder, lx + 14.0, fy + 12.0, 22.0);
            g.text(&cfg.folder, lx + 44.0, fy, lw - 44.0 - 138.0, 46.0, 12.0, p.text2, PATH_TRIM);
            let b1 = [lx + lw - 136.0, fy + 9.0, 66.0, 28.0];
            let b2 = [lx + lw - 64.0, fy + 9.0, 50.0, 28.0];
            pill(&g, b1[0], b1[1], b1[2], b1[3], "Change", false, hv == Some(Hit::Folder));
            pill(&g, b2[0], b2[1], b2[2], b2[3], "Open", false, hv == Some(Hit::OpenFolder));
            items.push((Hit::Folder, b1));
            items.push((Hit::OpenFolder, b2));

            section(&g, lx, 378.0, "SHORTCUTS");
            let sy = 396.0;
            card(&g, lx, sy, lw, 104.0);
            let bg_sub = if self.tray { "Waits quietly in the tray" } else { "Runs quietly in the background" };
            let rows2 = [(Hit::Override, "Global shortcuts", "Capture from anywhere · edit in Shortcuts", cfg.override_keys), (Hit::Autostart, "Start at login", bg_sub, self.autostart)];
            for (i, (hit, label, sub, on)) in rows2.iter().enumerate() {
                let y = sy + i as f32 * 52.0;
                g.text(label, lx + 16.0, y + 9.0, 280.0, 18.0, 13.5, p.text, 0);
                g.text(sub, lx + 16.0, y + 28.0, 280.0, 16.0, 11.0, p.text3, 0);
                toggle(&g, lx + lw - 60.0, y + 14.0, *on);
                if i == 0 {
                    g.line(lx + 16.0, y + 52.0, lx + lw - 16.0, y + 52.0, p.divider, 1.0);
                }
                items.push((*hit, [lx, y, lw, 52.0]));
            }

            // ---- right column: tabs ----
            let rx = 402.0;
            let rwid = W - rx - 22.0;
            let tabs = [("History", Hit::Tab(0)), ("Shortcuts", Hit::Tab(1)), ("Recording", Hit::Tab(2)), ("Theme", Hit::Tab(3))];
            segmented(&g, &mut items, rx, 80.0, rwid, 26.0, &tabs, self.tab as usize, hv);
            let py = 114.0;
            let ph = 386.0;
            card(&g, rx, py, rwid, ph);
            match self.tab {
                1 => self.paint_keys(&g, &mut items, cfg, rx, py, rwid, ph),
                2 => self.paint_recording(&g, &mut items, cfg, rx, py, rwid),
                3 => self.paint_theme(&g, &mut items, rx, py, rwid),
                _ => self.paint_history(&g, &mut items, rx, py, rwid, ph),
            }

            g.text(&format!("Aperture {} for Linux", env!("CARGO_PKG_VERSION")), lx, H - 30.0, 200.0, 20.0, 11.5, p.text3, 0);

            // "use a single key?" confirmation on top of everything
            if let Some((_, _, b, never)) = self.confirm {
                items.clear();
                g.rect(0.0, 0.0, W + 2.0, H + 2.0, if p.dark { 0xA0000000 } else { 0x70000000 });
                let (mw, mh) = (400.0, 196.0);
                let (mx, my) = ((W - mw) / 2.0, (H - mh) / 2.0);
                g.fill(&Path::rrect(mx, my + 4.0, mw, mh, 16.0), 0x40000000);
                let m = Path::rrect(mx, my, mw, mh, 16.0);
                g.fill(&m, p.card);
                g.stroke(&m, p.card_line, 1.0);
                let name = b.name();
                g.text(&format!("Use {name} on its own?"), mx + 24.0, my + 20.0, mw - 48.0, 24.0, 16.0, p.text, SEMI);
                g.text(
                    &format!("A shortcut without Ctrl, Alt or Super is caught everywhere, so pressing {name} while typing in other apps will start Aperture instead."),
                    mx + 24.0,
                    my + 52.0,
                    mw - 48.0,
                    54.0,
                    12.0,
                    p.text2,
                    WRAP,
                );
                let cb = [mx + 24.0, my + 112.0, 200.0, 22.0];
                checkbox(&g, cb[0], cb[1] + 2.0, never, hv == Some(Hit::ConfirmNever));
                g.text("Don't ask again", cb[0] + 28.0, cb[1], 170.0, 22.0, 12.0, p.text, 0);
                items.push((Hit::ConfirmNever, cb));
                let yes = [mx + mw - 24.0 - 120.0, my + mh - 50.0, 120.0, 30.0];
                let no = [yes[0] - 10.0 - 90.0, yes[1], 90.0, 30.0];
                pill(&g, no[0], no[1], no[2], no[3], "Cancel", false, hv == Some(Hit::ConfirmNo));
                pill(&g, yes[0], yes[1], yes[2], yes[3], "Use anyway", true, hv == Some(Hit::ConfirmYes));
                items.push((Hit::ConfirmNo, no));
                items.push((Hit::ConfirmYes, yes));
            }
        }
        self.items = items;
        if let Ok(mut out) = self.surf.buffer_mut() {
            if out.len() == back.len() {
                out.copy_from_slice(&back);
                self.win.pre_present_notify();
                let _ = out.present();
            }
        }
        self.back = back;
    }

    fn paint_history(&self, g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, rx: f32, py: f32, rwid: f32, ph: f32) {
        let p = pal();
        let hv = self.hover;
        let pad = 12.0;
        let gap = 10.0;
        let cwid = (rwid - pad * 2.0 - gap) / 2.0;
        let chgt = (ph - pad * 2.0 - gap * 4.0) / 5.0;
        for i in 0..store::MAX {
            let (col, row) = (i % 2, i / 2);
            let x = rx + pad + col as f32 * (cwid + gap);
            let y = py + pad + row as f32 * (chgt + gap);
            let cell = Path::rrect(x, y, cwid, chgt, 10.0);
            if let Some(t) = self.thumbs.get(i) {
                let on = hv == Some(Hit::Thumb(i));
                g.fill(&cell, p.cell);
                g.clip(&cell);
                g.image_cover(&t.px, x, y, cwid, chgt);
                if on {
                    g.rect(x, y, cwid, chgt, 0x401F6FEB);
                }
                g.unclip();
                g.stroke(&cell, if on { 0xFF2D7FF9 } else { p.card_line }, if on { 2.0 } else { 1.0 });
                if on {
                    pill(g, x + cwid / 2.0 - 30.0, y + chgt / 2.0 - 12.0, 60.0, 24.0, "Copy", true, false);
                }
                items.push((Hit::Thumb(i), [x, y, cwid, chgt]));
            } else {
                g.fill(&cell, p.cell);
                g.stroke(&cell, p.divider, 1.0);
            }
        }
        if self.thumbs.is_empty() {
            g.fill(&Path::rrect(rx + 40.0, py + ph / 2.0 - 34.0, rwid - 80.0, 68.0, 14.0), (p.card & 0x00FF_FFFF) | 0xF000_0000);
            g.text("No captures yet", rx, py + ph / 2.0 - 26.0, rwid, 22.0, 14.0, p.text, SEMI | CENTER);
            g.text("Your last 10 screenshots will appear here", rx, py + ph / 2.0 - 2.0, rwid, 18.0, 11.5, p.text3, CENTER);
        } else {
            let clr = [rx + rwid - 60.0, H - 30.0, 60.0, 20.0];
            g.text("Clear all", clr[0], clr[1], clr[2], clr[3], 11.5, if hv == Some(Hit::Clear) { p.link_hover } else { p.link }, SEMI | RIGHT);
            items.push((Hit::Clear, clr));
        }
        g.text("Click a capture to copy it again  ·  right-click to open", rx, H - 30.0, rwid - 70.0, 20.0, 11.0, p.text3, 0);
    }

    fn paint_keys(&self, g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, cfg: &Config, rx: f32, py: f32, rwid: f32, ph: f32) {
        let p = pal();
        let hv = self.hover;
        let ics = [Icon::Region, Icon::Free, Icon::Full, Icon::Copy, Icon::Record, Icon::Undo];
        let clip_sub = format!("Last {}", config::fmt_secs(cfg.replay_secs));
        let subs = ["Drag a box", "Draw any shape", "One monitor", "All screens, instantly", "Start / stop", clip_sub.as_str()];
        let (kw, kh) = (80.0, 28.0);
        let name_w = rwid - 46.0 - 14.0 - kw * 2.0 - 6.0 - 8.0;
        let n = super::hotkeys::ACTIONS;
        for a in 0..n {
            let y = py + 6.0 + a as f32 * 46.0;
            icons::draw(g, ics[a], rx + 14.0, y + 12.0, 22.0);
            g.text(config::ACTION_NAMES[a], rx + 46.0, y + 6.0, name_w, 18.0, 12.5, p.text, SEMI);
            g.text(subs[a], rx + 46.0, y + 24.0, name_w, 15.0, 10.5, p.text3, 0);
            for s in 0..2 {
                let x = rx + rwid - 14.0 - (2 - s) as f32 * kw - (1 - s) as f32 * 6.0;
                let b = cfg.binds[a][s];
                let state = if self.recording == Some((a, s)) {
                    2
                } else if b.is_set() {
                    0
                } else {
                    1
                };
                keycap(g, x, y + 9.0, kw, kh, &b.name(), state, hv == Some(Hit::Bind(a, s)));
                items.push((Hit::Bind(a, s), [x, y + 9.0, kw, kh]));
            }
            if a < n - 1 {
                g.line(rx + 14.0, y + 46.0, rx + rwid - 14.0, y + 46.0, p.divider, 1.0);
            }
        }
        let ty = py + 6.0 + n as f32 * 46.0 + 6.0;
        if cfg.override_keys {
            g.text("Click a key, then press the shortcut. Backspace removes it, Esc cancels. Keys without Ctrl, Alt or Super work too, after a warning.", rx + 16.0, ty, rwid - 32.0, 28.0, 10.5, p.text3, WRAP);
            g.text(&super::hotkeys::status(), rx + 16.0, ty + 28.0, rwid - 32.0, 28.0, 10.5, p.text2, WRAP | SEMI);
        } else {
            g.text("Global shortcuts are turned off. Switch them on under Shortcuts on the left.", rx + 16.0, ty, rwid - 32.0, 48.0, 11.0, p.warn, WRAP | SEMI);
        }
        let rr = [rx + rwid - 14.0 - 132.0, py + ph - 40.0, 132.0, 28.0];
        pill(g, rr[0], rr[1], rr[2], rr[3], "Reset to defaults", false, hv == Some(Hit::ResetKeys));
        items.push((Hit::ResetKeys, rr));
        let wr = [rx + 8.0, py + ph - 42.0, rr[0] - rx - 16.0, 32.0];
        if hv == Some(Hit::WarnPlain) {
            g.fill(&Path::rrect(wr[0], wr[1], wr[2], wr[3], 8.0), p.hover);
        }
        toggle(g, rx + 14.0, py + ph - 38.0, cfg.warn_plain);
        g.text("Warn about single keys", rx + 66.0, py + ph - 40.0, wr[2] - 60.0, 28.0, 11.5, p.text, 0);
        items.push((Hit::WarnPlain, wr));
    }

    fn paint_recording(&self, g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, cfg: &Config, rx: f32, py: f32, rwid: f32) {
        let p = pal();
        let hv = self.hover;
        let right = rx + rwid - 14.0;
        // replay toggle
        let y = py + 8.0;
        if hv == Some(Hit::Replay) {
            g.fill(&Path::rrect4(rx + 1.0, py + 1.0, rwid - 2.0, 62.0, 13.0, 13.0, 0.0, 0.0), p.hover);
        }
        icons::draw(g, Icon::Record, rx + 14.0, y + 14.0, 24.0);
        g.text("Replay clips", rx + 48.0, y + 8.0, 220.0, 18.0, 13.5, p.text, SEMI);
        g.text(&format!("Keeps the last {} ready to save", config::fmt_secs(cfg.replay_secs)), rx + 48.0, y + 28.0, 220.0, 16.0, 11.0, p.text3, 0);
        toggle(g, right - 44.0, y + 16.0, cfg.replay);
        items.push((Hit::Replay, [rx, py, rwid, 64.0]));
        g.line(rx + 14.0, py + 64.0, right, py + 64.0, p.divider, 1.0);

        let edit = self.edit.as_ref();
        // clip length
        let y = py + 64.0;
        g.text("Clip length", rx + 16.0, y, 160.0, 48.0, 13.0, p.text, 0);
        let clip_edit = edit.filter(|e| e.field == Field::Clip);
        stepper(g, items, right, y + 10.0, &config::fmt_secs(cfg.replay_secs), clip_edit, (Hit::ClipMinus, Hit::ClipEdit, Hit::ClipPlus), hv);
        if clip_edit.is_some() {
            g.text("e.g. 90, 2m, 1:30", rx + 16.0, y + 30.0, 120.0, 14.0, 10.0, p.text3, 0);
        }
        g.line(rx + 14.0, y + 48.0, right, y + 48.0, p.divider, 1.0);

        // frame rate
        let y = py + 112.0;
        g.text("Frame rate", rx + 16.0, y, 160.0, 48.0, 13.0, p.text, 0);
        let fps_edit = edit.filter(|e| e.field == Field::Fps);
        stepper(g, items, right, y + 10.0, &format!("{} fps", cfg.fps), fps_edit, (Hit::FpsMinus, Hit::FpsEdit, Hit::FpsPlus), hv);
        if fps_edit.is_some() {
            g.text(&format!("{} to {}", config::FPS_MIN, config::FPS_MAX), rx + 16.0, y + 30.0, 120.0, 14.0, 10.0, p.text3, 0);
        }
        g.line(rx + 14.0, y + 48.0, right, y + 48.0, p.divider, 1.0);

        // resolution
        let y = py + 160.0;
        g.text("Resolution", rx + 16.0, y, 100.0, 48.0, 13.0, p.text, 0);
        let labels: Vec<(String, Hit)> = config::RES_STEPS.iter().map(|r| (if *r == 0 { "Native".to_string() } else { format!("{r}p") }, Hit::Res(*r))).collect();
        let refs: Vec<(&str, Hit)> = labels.iter().map(|(l, h)| (l.as_str(), *h)).collect();
        let sel = config::RES_STEPS.iter().position(|r| *r == cfg.res).unwrap_or(usize::MAX);
        let sw = 226.0;
        segmented(g, items, right - sw, y + 11.0, sw, 26.0, &refs, sel, hv);
        g.line(rx + 14.0, y + 48.0, right, y + 48.0, p.divider, 1.0);

        // video folder
        let y = py + 208.0;
        g.text("Save videos to", rx + 16.0, y + 8.0, 180.0, 18.0, 13.0, p.text, 0);
        g.text(&cfg.video_folder, rx + 16.0, y + 30.0, rwid - 32.0 - 130.0, 18.0, 11.5, p.text2, PATH_TRIM);
        let b1 = [right - 122.0, y + 14.0, 66.0, 28.0];
        let b2 = [right - 50.0, y + 14.0, 50.0, 28.0];
        pill(g, b1[0], b1[1], b1[2], b1[3], "Change", false, hv == Some(Hit::VideoFolder));
        pill(g, b2[0], b2[1], b2[2], b2[3], "Open", false, hv == Some(Hit::OpenVideoFolder));
        items.push((Hit::VideoFolder, b1));
        items.push((Hit::OpenVideoFolder, b2));
        g.line(rx + 14.0, y + 60.0, right, y + 60.0, p.divider, 1.0);

        // info
        let y = py + 280.0;
        let rec = cfg.binds[4].iter().find(|b| b.is_set()).map(|b| b.name()).unwrap_or_else(|| "tray menu".into());
        let clip = cfg.binds[5].iter().find(|b| b.is_set()).map(|b| b.name()).unwrap_or_else(|| "tray menu".into());
        g.text(&format!("Record: {rec}   ·   Save clip: {clip}"), rx + 16.0, y, rwid - 32.0, 18.0, 11.5, p.text, SEMI);
        match super::media::missing() {
            Some(m) => g.text(m, rx + 16.0, y + 22.0, rwid - 32.0, 50.0, 10.5, p.warn, WRAP | SEMI),
            None => {
                let enc = super::media::encoder_label().unwrap_or("no H.264 encoder found");
                let info = format!("Encodes on your {enc} with GStreamer. On Wayland your desktop asks which screen to share the first time. While Replay clips is off, nothing runs in the background.");
                g.text(&info, rx + 16.0, y + 22.0, rwid - 32.0, 50.0, 10.5, p.text3, WRAP);
            }
        }
        if cfg.replay {
            let mb = crate::video::bitrate(1920, 1080, cfg.fps) as u64 * (cfg.replay_secs as u64 + 12) / 8 / 1_000_000;
            g.text(&format!("Buffer: about {mb} MB of temporary disk space at 1080p."), rx + 16.0, y + 74.0, rwid - 32.0, 18.0, 10.5, p.text2, 0);
        }
    }

    /// Apply (or, if it doesn't parse, drop) the number being typed.
    fn commit_edit(&mut self, cfg: &mut Config) -> Option<Cmd> {
        let e = self.edit.take()?;
        let changed = match e.field {
            Field::Clip => config::parse_secs(&e.text).map(|v| v.clamp(config::CLIP_MIN, config::CLIP_MAX)).filter(|v| *v != cfg.replay_secs).map(|v| cfg.replay_secs = v),
            Field::Fps => {
                let t = e.text.trim().to_ascii_lowercase();
                t.trim_end_matches("fps").trim().parse::<f32>().ok().map(|v| (v.round() as u32).clamp(config::FPS_MIN, config::FPS_MAX)).filter(|v| *v != cfg.fps).map(|v| cfg.fps = v)
            }
        };
        cfg.save();
        self.win.request_redraw();
        changed.map(|_| Cmd::ReplayChanged)
    }

    fn edit_key(&mut self, ev: &winit::event::KeyEvent, cfg: &mut Config) -> Option<Cmd> {
        use winit::keyboard::{Key, NamedKey};
        let e = self.edit.as_mut()?;
        match &ev.logical_key {
            Key::Named(NamedKey::Enter) => return self.commit_edit(cfg),
            Key::Named(NamedKey::Escape) => self.edit = None,
            Key::Named(NamedKey::Backspace) => {
                if e.fresh {
                    e.text.clear();
                } else {
                    e.text.pop();
                }
                e.fresh = false;
            }
            _ => {
                for c in ev.text.as_deref().unwrap_or("").chars() {
                    let ok = match e.field {
                        Field::Clip => c.is_ascii_digit() || matches!(c, ':' | '.' | ' ' | 'm' | 'M' | 's' | 'S' | 'i' | 'n' | 'e' | 'c'),
                        Field::Fps => c.is_ascii_digit() || c == '.',
                    };
                    if !ok {
                        continue;
                    }
                    if e.fresh {
                        e.text.clear();
                        e.fresh = false;
                    }
                    if e.text.chars().count() < 8 {
                        e.text.push(c);
                    }
                }
            }
        }
        self.win.request_redraw();
        None
    }

    fn paint_theme(&self, g: &Gfx, items: &mut Vec<(Hit, [f32; 4])>, rx: f32, py: f32, rwid: f32) {
        let p = pal();
        let hv = self.hover;
        let gap = 10.0;
        let cw = (rwid - 28.0 - gap * 2.0) / 3.0;
        for t in 0..3u8 {
            let x = rx + 14.0 + t as f32 * (cw + gap);
            let y = py + 14.0;
            let sel = theme::current() == t;
            let on = hv == Some(Hit::Theme(t));
            let frame = Path::rrect(x, y, cw, 148.0, 12.0);
            g.fill(&frame, if on && !sel { p.hover } else { 0 });
            theme_preview(g, theme::get(t), x + 8.0, y + 8.0, cw - 16.0, 96.0);
            g.text(theme::NAMES[t as usize], x, y + 110.0, cw, 18.0, 12.5, p.text, SEMI | CENTER);
            let sub = ["Default", "Easy on the eyes", "Pure black · OLED"][t as usize];
            g.text(sub, x, y + 128.0, cw, 14.0, 10.0, p.text3, CENTER);
            if sel {
                g.stroke(&frame, 0xFF2D7FF9, 2.0);
                let (bx, by) = (x + cw - 12.0, y + 12.0);
                let badge = Path::circle(bx, by, 9.0);
                g.fill_v(&badge, by - 9.0, by + 9.0, 0xFF5AA2FF, 0xFF1A59CC);
                g.polyline(&[(bx - 4.0, by), (bx - 1.0, by + 3.0), (bx + 4.0, by - 3.0)], 0xFFFFFFFF, 1.8);
            } else {
                g.stroke(&frame, if on { p.btn_line } else { p.divider }, 1.0);
            }
            items.push((Hit::Theme(t), [x, y, cw, 148.0]));
        }
        g.text(
            "Applies to this window and the capture toolbar. Charcoal uses true black, so OLED pixels switch off.",
            rx + 16.0,
            py + 178.0,
            rwid - 32.0,
            40.0,
            11.0,
            p.text3,
            WRAP,
        );
    }

    fn stop_recording(&mut self, cfg: &Config) {
        if self.recording.take().is_some() {
            config::apply_binds(cfg); // grabs were paused while recording
        }
    }

    fn click(&mut self, hit: Hit, cfg: &mut Config) -> Option<Cmd> {
        let was = self.recording;
        self.stop_recording(cfg);
        let editing = self.edit.as_ref().map(|e| e.field);
        let own = matches!((editing, hit), (Some(Field::Clip), Hit::ClipEdit) | (Some(Field::Fps), Hit::FpsEdit));
        let mut out = if own { None } else { self.commit_edit(cfg) };
        let replay_changed = |cfg: &Config| if cfg.replay { Some(Cmd::ReplayChanged) } else { None };
        match hit {
            Hit::Tab(t) => self.tab = t,
            Hit::Bind(i, s) => {
                if was != Some((i, s)) {
                    self.recording = Some((i, s));
                    super::hotkeys::set(&cfg.binds, false); // let the keys reach this window
                }
            }
            Hit::WarnPlain => cfg.warn_plain = !cfg.warn_plain,
            Hit::ConfirmNever => {
                if let Some(c) = &mut self.confirm {
                    c.3 = !c.3;
                }
            }
            Hit::ConfirmNo => self.confirm = None,
            Hit::ConfirmYes => {
                if let Some((act, slot, b, never)) = self.confirm.take() {
                    if never {
                        cfg.warn_plain = false;
                    }
                    set_bind(cfg, act, slot, b);
                    out = Some(Cmd::BindsChanged);
                }
            }
            Hit::ResetKeys => {
                cfg.binds = config::DEFAULT_BINDS;
                out = Some(Cmd::BindsChanged);
            }
            Hit::Close => return Some(Cmd::Close),
            Hit::Cap(i) => return Some(Cmd::Capture([1, 2, 3, 5][i])),
            Hit::Copy => cfg.copy = !cfg.copy,
            Hit::Save => cfg.save = !cfg.save,
            Hit::Folder => out = Some(Cmd::PickFolder(false)),
            Hit::VideoFolder => out = Some(Cmd::PickFolder(true)),
            Hit::OpenVideoFolder => {
                let _ = std::fs::create_dir_all(&cfg.video_folder);
                open_path(&cfg.video_folder);
            }
            Hit::Replay => {
                cfg.replay = !cfg.replay;
                out = Some(Cmd::ReplayChanged);
            }
            Hit::ClipMinus | Hit::ClipPlus => {
                cfg.replay_secs = config::step(&config::CLIP_STEPS, cfg.replay_secs, hit == Hit::ClipPlus);
                out = replay_changed(cfg);
            }
            Hit::FpsMinus | Hit::FpsPlus => {
                cfg.fps = config::step(&config::FPS_STEPS, cfg.fps, hit == Hit::FpsPlus);
                out = replay_changed(cfg);
            }
            Hit::ClipEdit | Hit::FpsEdit => {
                if !own {
                    let (field, text) = if hit == Hit::ClipEdit { (Field::Clip, cfg.replay_secs.to_string()) } else { (Field::Fps, cfg.fps.to_string()) };
                    self.edit = Some(Edit { field, text, fresh: true });
                }
            }
            Hit::Res(r) => {
                if cfg.res != r {
                    cfg.res = r;
                    out = replay_changed(cfg);
                }
            }
            Hit::OpenFolder => {
                let _ = std::fs::create_dir_all(&cfg.folder);
                open_path(&cfg.folder);
            }
            Hit::Override => {
                cfg.override_keys = !cfg.override_keys;
                out = Some(Cmd::BindsChanged);
            }
            Hit::Autostart => {
                self.autostart = !self.autostart;
                config::set_autostart(self.autostart);
            }
            Hit::Theme(t) => {
                cfg.theme = t;
                theme::set(t);
            }
            Hit::Thumb(i) => {
                if let Some(t) = self.thumbs.get(i) {
                    out = Some(Cmd::CopyFile(t.path.clone()));
                }
            }
            Hit::Clear => {
                self.thumbs.clear();
                store::clear();
            }
        }
        cfg.save();
        self.win.request_redraw();
        out
    }

    /// The user pressed a combination for the shortcut being edited.
    fn record_key(&mut self, vk: u16, cfg: &mut Config) -> Option<Cmd> {
        let Some((a, s)) = self.recording.take() else { return None };
        let mut mods = 0;
        if self.mods.super_key() {
            mods |= M_WIN;
        }
        if self.mods.control_key() {
            mods |= M_CTRL;
        }
        if self.mods.alt_key() {
            mods |= M_ALT;
        }
        if self.mods.shift_key() {
            mods |= M_SHIFT;
        }
        let b = Bind::new(mods, vk);
        if b.mods == 0 && b.vk == 0x1B {
            // Esc: cancel
        } else if b.mods == 0 && (b.vk == 0x08 || b.vk == 0x2E) {
            set_bind(cfg, a, s, Bind::NONE);
        } else if !b.valid() && cfg.warn_plain {
            self.confirm = Some((a, s, b, false)); // ask first
        } else {
            set_bind(cfg, a, s, b);
        }
        self.win.request_redraw();
        Some(Cmd::BindsChanged) // re-enables the grabs either way
    }

    pub fn event(&mut self, ev: &WindowEvent, cfg: &mut Config) -> Option<Cmd> {
        let s = self.s;
        match ev {
            WindowEvent::RedrawRequested => self.paint(cfg),
            WindowEvent::CloseRequested => return Some(Cmd::Close),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.s = *scale_factor as f32;
                let _ = self.win.request_inner_size(winit::dpi::PhysicalSize::new((W * self.s) as u32, (H * self.s) as u32));
                self.win.request_redraw();
            }
            WindowEvent::Resized(_) => self.win.request_redraw(),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::Focused(false) => {
                if self.recording.is_some() {
                    self.stop_recording(cfg);
                    self.win.request_redraw();
                }
                return self.commit_edit(cfg);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cur = (position.x as f32 / s, position.y as f32 / s);
                let hv = self.hit(self.cur.0, self.cur.1);
                if hv != self.hover {
                    self.hover = hv;
                    self.win.set_cursor(if hv.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
                    self.win.request_redraw();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if self.hover.take().is_some() {
                    self.win.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (x, y) = self.cur;
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        if y < 76.0 && self.hit(x, y).is_none() {
                            let _ = self.win.drag_window();
                        }
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        if let Some(h) = self.hit(x, y) {
                            return self.click(h, cfg);
                        }
                        return self.commit_edit(cfg);
                    }
                    (MouseButton::Right, ElementState::Released) => {
                        if let Some(Hit::Thumb(i)) = self.hit(x, y) {
                            if let Some(t) = self.thumbs.get(i) {
                                open_path(&t.path.to_string_lossy());
                            }
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && (!event.repeat || self.edit.is_some()) => {
                if self.edit.is_some() {
                    return self.edit_key(event, cfg);
                }
                let PhysicalKey::Code(code) = event.physical_key else { return None };
                if self.recording.is_some() {
                    if let Some(vk) = vk_of(code) {
                        return self.record_key(vk, cfg);
                    }
                    return None; // a modifier on its own: keep waiting
                }
                if self.confirm.is_some() {
                    match code {
                        KeyCode::Escape => {
                            self.confirm = None; // Esc answers the question, not the window
                            self.win.request_redraw();
                        }
                        KeyCode::Enter | KeyCode::NumpadEnter => return self.click(Hit::ConfirmYes, cfg),
                        _ => {}
                    }
                    return None;
                }
                if code == KeyCode::Escape {
                    return Some(Cmd::Close);
                }
            }
            _ => {}
        }
        None
    }
}
