//! Tray icon, hidden message window, and capture delivery (clipboard / file / history).
use crate::config::Config;
use crate::gfx::{self, hicon_from};
use crate::hook::{self, *};
use crate::overlay::{self, Mode};
use crate::util::*;
use crate::{clip, icons, settings, store, toast};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const MAIN_CLASS: &str = "ApertureMain";
pub const WM_TRAY: u32 = WM_APP + 1;
pub const WM_ACTION: u32 = WM_APP + 2;
pub const WM_HISTORY: u32 = WM_APP + 3;
pub const WM_SHOW_SETTINGS: u32 = WM_APP + 4;
pub const WM_QUIT_APP: u32 = WM_APP + 5;
pub const WM_BIND_RECORDED: u32 = WM_APP + 6;
const TIMER_ACTION: usize = 100;

pub struct App {
    pub main: HWND,
    pub hinst: HINSTANCE,
    pub cfg: Config,
    pub icon: HICON,
    pub icon_sm: HICON,
    taskbar_created: u32,
    pending: usize,
}

static APP: G<App> = G::new();
pub fn app() -> &'static mut App {
    APP.get()
}

#[derive(Clone, Copy, PartialEq)]
pub enum Out {
    Default,
    Copy,
    Save,
}

unsafe fn load_icon(hinst: HINSTANCE, size: i32) -> HICON {
    let h = LoadImageW(hinst, 1usize as *const u16, IMAGE_ICON, size, size, LR_DEFAULTCOLOR);
    if !h.is_null() {
        return h as HICON;
    }
    hicon_from(&mut icons::render_camera(size))
}

pub unsafe fn run(arg: &str) {
    let hinst = GetModuleHandleW(null());
    let cfg = Config::load();
    crate::theme::set(cfg.theme);
    let icon = load_icon(hinst, GetSystemMetrics(SM_CXICON));
    let icon_sm = load_icon(hinst, GetSystemMetrics(SM_CXSMICON));
    let cls = wz(MAIN_CLASS);
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        hIcon: icon,
        hIconSm: icon_sm,
        lpszClassName: cls.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassExW(&wc);
    let main = CreateWindowExW(WS_EX_TOOLWINDOW, cls.as_ptr(), wz("Aperture").as_ptr(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), hinst, null());
    APP.set(App { main, hinst, cfg, icon, icon_sm, taskbar_created: RegisterWindowMessageW(wz("TaskbarCreated").as_ptr()), pending: 0 });
    tray(NIM_ADD);
    overlay::init(hinst);
    toast::init(hinst);
    crate::rec::init(hinst);
    crate::media::set_notify(main as usize);
    crate::config::apply_binds(&app().cfg);
    if app().cfg.replay {
        replay_apply();
    }
    hook::start(main);
    match arg {
        "--tray" => {}
        "--welcome" => toast::show(None, "Aperture is ready", "Press Win+Shift+S to capture  ·  lives in your tray", None),
        _ => settings::show(),
    }
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    tray(NIM_DELETE);
    crate::media::shutdown();
}

unsafe fn tray(op: NOTIFY_ICON_MESSAGE) {
    let a = app();
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = a.main;
    nid.uID = 1;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = a.icon_sm;
    let key = a.cfg.binds[0].iter().find(|b| b.is_set()).map(|b| format!("  ·  {}", b.name())).unwrap_or_default();
    let tip = wz(&format!("Aperture{key}"));
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    Shell_NotifyIconW(op, &nid);
}

/// Update the tray tooltip after shortcuts change.
pub unsafe fn refresh_tray() {
    tray(NIM_MODIFY);
}

unsafe fn tray_menu(h: HWND) {
    let m = CreatePopupMenu();
    let item = |id: usize, s: &str| {
        AppendMenuW(m, MF_STRING, id, wz(s).as_ptr());
    };
    let key = |i: usize| app().cfg.binds[i].iter().find(|b| b.is_set()).map(|b| b.name()).unwrap_or_default();
    for i in 0..3 {
        item(i + 1, &format!("{}\t{}", crate::config::ACTION_NAMES[i], key(i)));
    }
    AppendMenuW(m, MF_SEPARATOR, 0, null());
    if crate::rec::active() {
        item(5, &format!("Stop recording\t{}", key(4)));
    } else {
        item(5, &format!("Record video\t{}", key(4)));
    }
    if app().cfg.replay {
        item(6, &format!("Save replay clip\t{}", key(5)));
    }
    AppendMenuW(m, MF_SEPARATOR, 0, null());
    item(10, "Settings && history");
    AppendMenuW(m, MF_SEPARATOR, 0, null());
    item(99, "Quit Aperture");
    SetForegroundWindow(h);
    let p = cursor();
    TrackPopupMenu(m, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN, p.x, p.y, 0, h, null());
    PostMessageW(h, WM_NULL, 0, 0);
    DestroyMenu(m);
}

/// Run a capture after menus/windows have finished animating away.
pub unsafe fn delayed_action(a: usize) {
    app().pending = a;
    SetTimer(app().main, TIMER_ACTION, 180, None);
}

pub unsafe fn action(a: usize) {
    match a {
        ACT_REGION => overlay::open(Mode::Region, false),
        ACT_FREE => overlay::open(Mode::Free, false),
        ACT_FULL => overlay::open(Mode::Full, false),
        ACT_RECORD => {
            if crate::rec::active() {
                crate::rec::stop();
            } else {
                overlay::open(Mode::Region, true);
            }
        }
        ACT_CLIP => save_clip(),
        ACT_INSTANT_FULL => {
            if let Some(img) = overlay::capture_virtual() {
                deliver(img, Out::Default);
            }
        }
        _ => {}
    }
}

/// Start, retune or stop the replay buffer to match the settings.
pub unsafe fn replay_apply() {
    let c = &app().cfg;
    if !c.replay {
        crate::media::stop_replay();
        return;
    }
    let (secs, fps, res, dir) = (c.replay_secs, c.fps, c.res, Config::replay_dir());
    std::thread::spawn(move || {
        if let Err(e) = crate::media::start_replay(secs, fps, res, dir) {
            crate::media::report(crate::media::MediaEvent::Error(format!("Replay clips couldn't start: {e}")));
        }
    });
}

unsafe fn save_clip() {
    let c = &app().cfg;
    if !c.replay {
        toast::show_icon(crate::icons::Icon::Record, "Replay clips are off", "Turn them on in Settings → Recording", None);
        return;
    }
    let _ = std::fs::create_dir_all(&c.video_folder);
    let path = std::path::PathBuf::from(format!("{}\\Aperture Clip {}.mp4", c.video_folder.trim_end_matches('\\'), timestamp()));
    crate::media::save_clip(path);
    toast::show_icon(crate::icons::Icon::Record, "Saving clip…", &format!("Last {}", crate::config::fmt_secs(c.replay_secs)), None);
}

fn fmt_dur(s: f64) -> String {
    let s = s.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

unsafe fn on_media(ev: crate::media::MediaEvent) {
    use crate::media::MediaEvent::*;
    match ev {
        Saved { path, secs, clip } => {
            let p = path.to_string_lossy().into_owned();
            let copied = app().cfg.copy && clip::set_file(app().main, &p);
            let title = if clip { "Clip saved" } else { "Recording saved" };
            let sub = format!("{}  ·  {}", fmt_dur(secs), if copied { "Copied · click to open" } else { "Click to open" });
            toast::show_icon(crate::icons::Icon::Record, title, &sub, Some(p));
        }
        Error(e) => toast::show_icon(crate::icons::Icon::Record, "Video problem", &e, None),
    }
}

fn unique_file(folder: &str) -> String {
    let base = format!("{}\\Aperture {}", folder.trim_end_matches('\\'), timestamp());
    let mut p = format!("{base}.png");
    let mut n = 2;
    while std::path::Path::new(&p).exists() {
        p = format!("{base} ({n}).png");
        n += 1;
    }
    p
}

/// Hand a finished capture to the clipboard, the save folder and the history.
pub unsafe fn deliver(img: Img, how: Out) {
    let a = app();
    let copy = a.cfg.copy || how == Out::Copy;
    let save = a.cfg.save || how == Out::Save;
    let png_clip = if copy && img.alpha { Some(gfx::encode_png(&img)) } else { None };
    if copy {
        clip::set_image(a.main, &img, png_clip.as_deref());
    }
    let file = if save { Some(unique_file(&a.cfg.folder)) } else { None };
    let title = match (copy, save) {
        (true, true) => "Copied & saved",
        (true, false) => "Copied to clipboard",
        (false, true) => "Saved",
        _ => "Captured",
    };
    let sub = format!("{} × {}  ·  {}", img.w, img.h, if save { "Click to open" } else { "Paste with Ctrl+V" });
    toast::show(Some(&img), title, &sub, file.clone());
    let main = a.main as usize;
    let folder = a.cfg.folder.clone();
    std::thread::spawn(move || {
        let png = png_clip.unwrap_or_else(|| gfx::encode_png(&img));
        if let Some(f) = &file {
            let _ = std::fs::create_dir_all(&folder);
            let _ = std::fs::write(f, &png);
        }
        let th = gfx::encode_png(&img.fit(320, 240));
        store::add(&png, &th);
        unsafe { PostMessageW(main as HWND, WM_HISTORY, 0, 0) };
    });
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_TRAY => {
            match (lp & 0xFFFF) as u32 {
                WM_LBUTTONUP => settings::show(),
                WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(h),
                _ => {}
            }
            0
        }
        WM_ACTION => {
            action(wp);
            0
        }
        WM_TIMER => {
            if wp == TIMER_ACTION {
                KillTimer(h, wp);
                action(app().pending);
            }
            0
        }
        WM_HISTORY => {
            settings::refresh();
            0
        }
        crate::media::WM_MEDIA => {
            if lp != 0 {
                let ev = Box::from_raw(lp as *mut crate::media::MediaEvent);
                on_media(*ev);
            }
            0
        }
        WM_BIND_RECORDED => {
            settings::on_recorded(wp);
            0
        }
        WM_SHOW_SETTINGS => {
            settings::show();
            0
        }
        WM_COMMAND => {
            match wp & 0xFFFF {
                id @ 1..=3 => delayed_action(id),
                5 => {
                    if crate::rec::active() {
                        crate::rec::stop();
                    } else {
                        delayed_action(5);
                    }
                }
                6 => save_clip(),
                10 => settings::show(),
                99 => PostQuitMessage(0),
                _ => {}
            }
            0
        }
        WM_QUIT_APP | WM_CLOSE => {
            settings::close();
            crate::rec::stop();
            crate::media::stop_replay();
            PostQuitMessage(0);
            0
        }
        _ => {
            if let Some(a) = APP.try_get() {
                if m == a.taskbar_created && m != 0 {
                    tray(NIM_ADD);
                    return 0;
                }
            }
            DefWindowProcW(h, m, wp, lp)
        }
    }
}
