//! Linux backend (Wayland + X11): winit windows drawn with tiny-skia through
//! softbuffer, desktop-portal / X11 capture, a StatusNotifierItem tray and
//! global shortcuts (portal on Wayland, XGrabKey on X11).
pub mod capture;
pub mod config_sys;
pub mod gfx;
pub mod hotkeys;
pub mod ipc;
pub mod media;
pub mod output;
pub mod overlay;
pub mod rec;
pub mod settings;
pub mod tray;

use crate::config::{Config, ACTION_NAMES};
use overlay::{Done, Mode, Overlay};
use settings::{Cmd, Settings};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle};
use winit::platform::startup_notify::{EventLoopExtStartupNotify, WindowAttributesExtStartupNotify};
use winit::window::{ActivationToken, Fullscreen, Window, WindowId, WindowLevel};

pub type Surface = softbuffer::Surface<OwnedDisplayHandle, Rc<Window>>;

pub enum UserEvent {
    /// 1 region, 2 freeform, 3 full screen, 4 instant, 5 record (toggle), 6 save clip;
    /// optional activation token
    Action(usize, Option<String>),
    /// same, after menus have had time to close
    Delayed(usize),
    Shot(Result<capture::Shot, String>, usize, Option<String>),
    Settings,
    History,
    /// a folder was picked: (path, is the video folder)
    Folder(String, bool),
    Media(media::MediaEvent),
    Quit,
}

static PROXY: OnceLock<Mutex<EventLoopProxy<UserEvent>>> = OnceLock::new();

/// Wakes the UI thread with an event (callable from any thread).
pub fn post(e: UserEvent) {
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap().send_event(e);
    }
}

const HELP: &str = "Aperture — screenshot tool

Usage: aperture [OPTION]
  (none)        open settings (starts Aperture if it isn't running)
  --tray        start in the background
  --region      capture a region
  --freeform    capture a freeform shape
  --full        capture a whole monitor
  --instant     capture every screen straight away
  --record      start / stop a screen recording
  --clip        save a replay clip (when replay clips are on)
  --help        show this help
";

pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--write-png") {
        // packaging helper: aperture --write-png out.png [size]
        let out = args.get(i + 1).cloned().unwrap_or_else(|| "aperture.png".into());
        let size = args.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(256);
        let _ = std::fs::write(out, gfx::encode_png(&crate::icons::render_camera(size)));
        return;
    }
    let word = ipc::word(&args);
    if ipc::forward(word) {
        return; // the running copy takes it from here
    }
    if !ipc::listen() {
        eprintln!("aperture: couldn't open the control socket");
    }
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("aperture: no graphical session available ({e})");
            return;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
    let cfg = Config::load();
    crate::theme::set(cfg.theme);
    let mut app = App {
        cfg,
        wayland: false,
        ctx: None,
        overlays: Vec::new(),
        settings: None,
        tray: None,
        capturing: false,
        first: Some(word),
        rec: None,
        rec_starting: false,
        pill: None,
        replay: None,
        replay_starting: false,
        next_tick: Instant::now(),
        primary: (0, 0, 1920, 1080),
    };
    let _ = event_loop.run_app(&mut app);
    // finish any recording so the file is playable
    if let Some(r) = app.rec.take() {
        media::stop_record(r);
        std::thread::sleep(Duration::from_secs(2));
    }
    if let Some(r) = app.replay.take() {
        media::stop_replay(r);
    }
    ipc::cleanup();
}

struct App {
    cfg: Config,
    wayland: bool,
    ctx: Option<softbuffer::Context<OwnedDisplayHandle>>,
    overlays: Vec<Overlay>,
    settings: Option<Settings>,
    tray: Option<tray::Handle>,
    capturing: bool,
    first: Option<&'static str>,
    rec: Option<media::Recording>,
    rec_starting: bool,
    pill: Option<rec::Pill>,
    replay: Option<media::Replay>,
    replay_starting: bool,
    next_tick: Instant,
    /// primary monitor in desktop pixels (x, y, w, h), for the replay buffer
    primary: (i32, i32, i32, i32),
}

fn tray_labels(cfg: &Config, recording: bool) -> Vec<(String, usize)> {
    let label = |i: usize, name: &str| match cfg.binds[i].iter().find(|b| b.is_set()) {
        Some(b) if cfg.override_keys => format!("{name}   ({})", b.name()),
        _ => name.to_string(),
    };
    let mut v: Vec<(String, usize)> = (0..4).map(|i| (label(i, ACTION_NAMES[i]), i + 1)).collect();
    v.push((label(4, if recording { "Stop recording" } else { "Record video" }), 5));
    if cfg.replay {
        v.push((label(5, "Save replay clip"), 6));
    }
    v
}

fn fmt_dur(s: f64) -> String {
    let s = s.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

impl App {
    fn surface(&self, w: &Rc<Window>) -> Option<Surface> {
        softbuffer::Surface::new(self.ctx.as_ref()?, w.clone()).ok()
    }

    fn start_capture(&mut self, act: usize, token: Option<String>) {
        if act == 6 {
            return self.save_clip();
        }
        if act == 5 && (self.rec.is_some() || self.rec_starting) {
            return self.stop_recording();
        }
        if self.capturing || !self.overlays.is_empty() {
            return;
        }
        self.capturing = true;
        let wayland = self.wayland;
        std::thread::spawn(move || post(UserEvent::Shot(capture::grab(wayland), act, token)));
    }

    fn open_overlays(&mut self, el: &ActiveEventLoop, shot: capture::Shot, act: usize, token: Option<String>) {
        let mode = match act {
            2 => Mode::Free,
            3 => Mode::Full,
            _ => Mode::Region,
        };
        let record = act == 5;
        let mons: Vec<_> = el.available_monitors().collect();
        if mons.is_empty() {
            return;
        }
        let rects: Vec<(i32, i32, i32, i32)> = mons
            .iter()
            .map(|m| {
                let (p, s) = (m.position(), m.size());
                (p.x, p.y, s.width as i32, s.height as i32)
            })
            .collect();
        let (bx, by) = (rects.iter().map(|r| r.0).min().unwrap(), rects.iter().map(|r| r.1).min().unwrap());
        let bw = rects.iter().map(|r| r.0 + r.2).max().unwrap() - bx;
        let bh = rects.iter().map(|r| r.1 + r.3).max().unwrap() - by;
        // X11 hands us the root window in desktop pixels; the Wayland portal's image
        // covers the same layout but its scale is the compositor's choice.
        let (fx, fy, ox, oy) = if self.wayland {
            (shot.img.w as f32 / bw.max(1) as f32, shot.img.h as f32 / bh.max(1) as f32, bx, by)
        } else {
            (1.0, 1.0, shot.x, shot.y)
        };
        let mut token = token.map(ActivationToken::from_raw).or_else(|| el.read_token_from_env());
        for (m, r) in mons.iter().zip(rects) {
            let cx = (((r.0 - ox) as f32 * fx).round() as i32).clamp(0, shot.img.w);
            let cy = (((r.1 - oy) as f32 * fy).round() as i32).clamp(0, shot.img.h);
            let cw = ((r.2 as f32 * fx).round() as i32).min(shot.img.w - cx);
            let ch = ((r.3 as f32 * fy).round() as i32).min(shot.img.h - cy);
            if cw <= 0 || ch <= 0 {
                continue;
            }
            let mut px = Vec::with_capacity((cw * ch) as usize);
            for y in cy..cy + ch {
                let o = (y * shot.img.w + cx) as usize;
                px.extend_from_slice(&shot.img.px[o..o + cw as usize]);
            }
            let crop = crate::img::Img { w: cw, h: ch, px, alpha: false };
            let mut attrs = Window::default_attributes()
                .with_title("Aperture Capture")
                .with_decorations(false)
                .with_resizable(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_position(PhysicalPosition::new(r.0, r.1))
                .with_inner_size(PhysicalSize::new(r.2 as u32, r.3 as u32))
                .with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
            if let Some(t) = token.take() {
                attrs = attrs.with_activation_token(t);
            }
            let Ok(win) = el.create_window(attrs) else { continue };
            let win = Rc::new(win);
            let Some(surf) = self.surface(&win) else { continue };
            let ov = Overlay::new(win, surf, crop, mode, record, r);
            ov.win.request_redraw();
            self.overlays.push(ov);
        }
        if let Some(o) = self.overlays.first() {
            o.win.focus_window();
        }
    }

    fn refresh_tray(&self) {
        if let Some(t) = &self.tray {
            t.set_labels(tray_labels(&self.cfg, self.rec.is_some() || self.rec_starting));
        }
    }

    /// The overlay chose an area to record.
    fn begin_recording(&mut self, ov_mon: (i32, i32, i32, i32), src: (i32, i32), area: (i32, i32, i32, i32), whole: bool) {
        let a = if self.wayland {
            media::Area::Monitor { crop: if whole { None } else { Some(area) }, mon_w: src.0, mon_h: src.1 }
        } else {
            let (mx, my, mw, mh) = ov_mon;
            if whole { media::Area::X11 { x: mx, y: my, w: mw, h: mh } } else { media::Area::X11 { x: mx + area.0, y: my + area.1, w: area.2, h: area.3 } }
        };
        let path = std::path::Path::new(&self.cfg.video_folder).join(format!("Aperture Recording {}.mp4", output::timestamp()));
        self.rec_starting = true;
        self.refresh_tray();
        let (fps, res) = (self.cfg.fps, self.cfg.res);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250)); // let the overlay vanish first
            media::start_record(a, path, fps, res);
        });
    }

    fn stop_recording(&mut self) {
        self.pill = None;
        if let Some(r) = self.rec.take() {
            media::stop_record(r);
            output::notify("Finishing recording…", "One moment", "aperture", None);
        }
        self.rec_starting = false;
        self.refresh_tray();
    }

    fn save_clip(&mut self) {
        let Some(r) = &mut self.replay else {
            let why = if self.cfg.replay { "The replay buffer is still starting" } else { "Turn them on in Settings → Recording" };
            output::notify(if self.cfg.replay { "Not ready yet" } else { "Replay clips are off" }, why, "aperture", None);
            return;
        };
        if r.ended() {
            self.replay = None;
            output::notify("Replay clips stopped", "Screen sharing ended. Toggle Replay clips to restart it.", "aperture", None);
            return;
        }
        let path = std::path::Path::new(&self.cfg.video_folder).join(format!("Aperture Clip {}.mp4", output::timestamp()));
        media::save_clip(r, path);
        output::notify("Saving clip…", &format!("Last {}", crate::config::fmt_secs(self.cfg.replay_secs)), "aperture", None);
    }

    /// Start, restart or stop the replay buffer to match the settings.
    fn replay_apply(&mut self, restart: bool) {
        if (restart || !self.cfg.replay) && self.replay.is_some() {
            if let Some(r) = self.replay.take() {
                media::stop_replay(r);
            }
        }
        if self.cfg.replay && self.replay.is_none() && !self.replay_starting {
            let (x, y, w, h) = self.primary;
            let area = if self.wayland { media::Area::Monitor { crop: None, mon_w: w, mon_h: h } } else { media::Area::X11 { x, y, w, h } };
            self.replay_starting = true;
            let (secs, fps, res, dir) = (self.cfg.replay_secs, self.cfg.fps, self.cfg.res, Config::replay_dir());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(if restart { 1500 } else { 0 })); // old pipeline lets go of the files
                media::start_replay(area, secs, fps, res, dir);
            });
        }
        self.refresh_tray();
    }

    fn on_media(&mut self, el: &ActiveEventLoop, ev: media::MediaEvent) {
        use media::MediaEvent::*;
        match ev {
            Started(Ok(r)) => {
                self.rec_starting = false;
                let rect = r.rect;
                let start = r.start;
                self.rec = Some(r);
                self.refresh_tray();
                // X11 region: a stop pill just outside the recorded area
                let pill_pos = rect.and_then(|(x, y, w, h)| {
                    let mon = el.available_monitors().find(|m| {
                        let (p, s) = (m.position(), m.size());
                        x >= p.x && x < p.x + s.width as i32 && y >= p.y && y < p.y + s.height as i32
                    })?;
                    let (mp, ms, sc) = (mon.position(), mon.size(), mon.scale_factor());
                    if w >= ms.width as i32 && h >= ms.height as i32 {
                        return None; // full screen: the pill would be in the video
                    }
                    let (pw, ph) = ((rec::W as f64 * sc) as i32, (rec::H as f64 * sc) as i32);
                    let gap = (10.0 * sc) as i32;
                    let px = (x + w / 2 - pw / 2).clamp(mp.x + gap, mp.x + ms.width as i32 - pw - gap);
                    let below = y + h + gap;
                    let py = if below + ph <= mp.y + ms.height as i32 - gap {
                        below
                    } else if y - ph - gap >= mp.y {
                        y - ph - gap
                    } else {
                        return None;
                    };
                    Some((px, py, pw, ph))
                });
                match pill_pos {
                    Some((px, py, pw, ph)) => {
                        let attrs = Window::default_attributes()
                            .with_title("Aperture Recording")
                            .with_decorations(false)
                            .with_resizable(false)
                            .with_window_level(WindowLevel::AlwaysOnTop)
                            .with_position(PhysicalPosition::new(px, py))
                            .with_inner_size(PhysicalSize::new(pw as u32, ph as u32));
                        if let Ok(win) = el.create_window(attrs) {
                            let win = Rc::new(win);
                            if let Some(surf) = self.surface(&win) {
                                win.request_redraw();
                                self.pill = Some(rec::Pill::new(win, surf, start));
                            }
                        }
                    }
                    None => {
                        let key = self.cfg.binds[4].iter().find(|b| b.is_set()).filter(|_| self.cfg.override_keys).map(|b| b.name());
                        let how = match key {
                            Some(k) => format!("Press {k} or use the tray menu to stop"),
                            None => "Run aperture --record or use the tray menu to stop".to_string(),
                        };
                        output::notify("Recording", &how, "aperture", None);
                    }
                }
                self.next_tick = Instant::now();
            }
            Started(Err(e)) => {
                self.rec_starting = false;
                self.refresh_tray();
                output::notify("Couldn't start recording", &e, "aperture", None);
            }
            ReplayStarted(Ok(r)) => {
                self.replay_starting = false;
                if self.cfg.replay {
                    self.replay = Some(r);
                } else {
                    media::stop_replay(r); // switched off while it was starting
                }
                self.next_tick = Instant::now();
            }
            ReplayStarted(Err(e)) => {
                self.replay_starting = false;
                output::notify("Replay clips couldn't start", &e, "aperture", None);
            }
            Saved { path, secs, clip } => {
                let copied = self.cfg.copy && output::copy_file(&path, self.wayland);
                let title = if clip { "Clip saved" } else { "Recording saved" };
                let sub = format!("{}  ·  {}", fmt_dur(secs), if copied { "Copied · paste it anywhere" } else { "In your videos folder" });
                output::notify(title, &sub, "aperture", Some(path.to_string_lossy().into_owned()));
            }
            Error(e) => output::notify("Video problem", &e, "aperture", None),
        }
    }

    fn deliver(&self, img: crate::img::Img, how: output::Out) {
        output::deliver(img, how, &self.cfg, self.wayland, || post(UserEvent::History));
    }

    fn show_settings(&mut self, el: &ActiveEventLoop) {
        if let Some(s) = &self.settings {
            s.win.focus_window();
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Aperture")
            .with_decorations(false)
            .with_resizable(false)
            .with_inner_size(LogicalSize::new(settings::W as f64, settings::H as f64))
            .with_window_icon(window_icon());
        let Ok(win) = el.create_window(attrs) else { return };
        let win = Rc::new(win);
        let Some(surf) = self.surface(&win) else { return };
        let tray = self.tray.as_ref().is_some_and(|t| t.available());
        self.settings = Some(Settings::new(win, surf, tray));
    }

    fn apply_binds(&self) {
        crate::config::apply_binds(&self.cfg);
        self.refresh_tray();
    }

    fn settings_cmd(&mut self, el: &ActiveEventLoop, cmd: Cmd) {
        match cmd {
            Cmd::Close => self.settings = None,
            Cmd::Capture(a) => {
                self.settings = None;
                post_delayed(a);
            }
            Cmd::PickFolder(video) => {
                let start = if video { self.cfg.video_folder.clone() } else { self.cfg.folder.clone() };
                std::thread::spawn(move || {
                    let title = if video { "Choose where Aperture saves videos" } else { "Choose where Aperture saves screenshots" };
                    if let Some(f) = pick_folder(&start, title) {
                        post(UserEvent::Folder(f, video));
                    }
                });
            }
            Cmd::ReplayChanged => self.replay_apply(true),
            Cmd::CopyFile(p) => {
                let Ok(bytes) = std::fs::read(&p) else { return };
                if let Some(img) = gfx::decode_png(&bytes) {
                    let ok = output::copy_image(&img, &bytes, self.wayland);
                    let thumb = crate::store::thumb_of(&p).to_string_lossy().into_owned();
                    output::notify(if ok { "Copied from history" } else { "Couldn't copy" }, &format!("{} × {}  ·  Paste with Ctrl+V", img.w, img.h), &thumb, None);
                }
            }
            Cmd::BindsChanged => self.apply_binds(),
        }
        let _ = el;
    }
}

fn window_icon() -> Option<winit::window::Icon> {
    let img = crate::icons::render_camera(64);
    let rgba: Vec<u8> = img.px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]).collect();
    winit::window::Icon::from_rgba(rgba, 64, 64).ok()
}

fn post_delayed(a: usize) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(250));
        post(UserEvent::Action(a, None));
    });
}

/// Folder picker through the desktop portal (works on GNOME, KDE and others).
fn pick_folder(start: &str, title: &str) -> Option<String> {
    use ashpd::desktop::file_chooser::SelectedFiles;
    let _ = std::fs::create_dir_all(start);
    let uri = pollster::block_on(async {
        let r = SelectedFiles::open_file().title(title).directory(true).modal(true).send().await?.response()?;
        Ok::<Option<String>, ashpd::Error>(r.uris().first().map(|u| u.as_str().to_string()))
    })
    .ok()??;
    let path = uri.strip_prefix("file://")?;
    // %-decode
    let b = path.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&path[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.ctx.is_some() {
            return;
        }
        use winit::platform::wayland::ActiveEventLoopExtWayland;
        self.wayland = el.is_wayland();
        self.ctx = softbuffer::Context::new(el.owned_display_handle()).ok();
        if self.ctx.is_none() {
            eprintln!("aperture: couldn't set up drawing");
            el.exit();
            return;
        }
        hotkeys::start(self.wayland);
        if let Some(m) = el.primary_monitor().or_else(|| el.available_monitors().next()) {
            let (p, s) = (m.position(), m.size());
            self.primary = (p.x, p.y, s.width as i32, s.height as i32);
        }
        self.tray = Some(tray::start(tray_labels(&self.cfg, false)));
        self.apply_binds();
        if self.cfg.replay {
            self.replay_apply(false);
        }
        match self.first.take() {
            Some("tray") => {}
            Some("region") => self.start_capture(1, None),
            Some("freeform") => self.start_capture(2, None),
            Some("full") => self.start_capture(3, None),
            Some("instant") => self.start_capture(4, None),
            Some("record") => self.start_capture(5, None),
            Some("clip") => self.start_capture(6, None),
            _ => self.show_settings(el),
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Action(a, token) => self.start_capture(a, token),
            UserEvent::Delayed(a) => post_delayed(a),
            UserEvent::Shot(res, act, token) => {
                self.capturing = false;
                match res {
                    Ok(shot) if act == 4 => self.deliver(shot.img, output::Out::Default),
                    Ok(shot) => self.open_overlays(el, shot, act, token),
                    Err(e) => output::notify("Couldn't capture the screen", &e, "aperture", None),
                }
            }
            UserEvent::Settings => self.show_settings(el),
            UserEvent::History => {
                if let Some(s) = &mut self.settings {
                    s.refresh();
                }
            }
            UserEvent::Media(m) => self.on_media(el, m),
            UserEvent::Folder(f, video) => {
                if video {
                    self.cfg.video_folder = f;
                } else {
                    self.cfg.folder = f;
                    self.cfg.save = true;
                }
                self.cfg.save();
                if let Some(s) = &self.settings {
                    s.win.request_redraw();
                }
            }
            UserEvent::Quit => el.exit(),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
        if let Some(i) = self.overlays.iter().position(|o| o.win.id() == id) {
            let pressed = matches!(ev, WindowEvent::MouseInput { state: winit::event::ElementState::Pressed, .. });
            let done = self.overlays[i].event(&ev);
            // only one monitor holds a selection at a time
            if pressed && self.overlays[i].editing {
                for (j, o) in self.overlays.iter_mut().enumerate() {
                    if j != i && o.editing {
                        o.back_to_select();
                    }
                }
            }
            if let Some(d) = done {
                let (mon, src) = (self.overlays[i].mon, self.overlays[i].src_size());
                self.overlays.clear(); // closes every overlay window
                match d {
                    Done::Deliver(img, how) => self.deliver(img, how),
                    Done::Record { area, whole } => self.begin_recording(mon, src, area, whole),
                    Done::Cancel => {}
                }
            }
            return;
        }
        if self.pill.as_ref().is_some_and(|p| p.win.id() == id) {
            if self.pill.as_mut().unwrap().event(&ev) {
                self.stop_recording();
            }
            return;
        }
        if self.settings.as_ref().is_some_and(|s| s.win.id() == id) {
            let cmd = self.settings.as_mut().unwrap().event(&ev, &mut self.cfg);
            if let Some(c) = cmd {
                self.settings_cmd(el, c);
            }
        }
    }

    /// While video is running: blink the pill and notice pipelines that ended by themselves.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        // scroll-wheel labels on the overlay fade after a moment
        let hint = self.overlays.iter_mut().filter_map(|o| o.tick()).min();
        if self.rec.is_none() && self.replay.is_none() {
            el.set_control_flow(match hint {
                Some(t) => ControlFlow::WaitUntil(t),
                None => ControlFlow::Wait,
            });
            return;
        }
        let now = Instant::now();
        if now >= self.next_tick {
            self.next_tick = now + Duration::from_millis(500);
            if let Some(p) = &mut self.pill {
                p.tick();
            }
            if self.rec.as_mut().is_some_and(|r| r.ended()) {
                self.stop_recording(); // e.g. sharing was stopped from the desktop's indicator
            }
            if self.replay.as_mut().is_some_and(|r| r.ended()) {
                self.replay = None;
                output::notify("Replay clips stopped", "Screen sharing ended. Toggle Replay clips to restart it.", "aperture", None);
            }
        }
        el.set_control_flow(ControlFlow::WaitUntil(hint.map_or(self.next_tick, |h| h.min(self.next_tick))));
    }
}
