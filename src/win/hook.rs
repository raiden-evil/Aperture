//! Low-level keyboard hook for Aperture's global shortcuts (incl. taking over
//! Win+Shift+S / Print Screen) and for recording new shortcuts in Settings.
//! Runs on its own high-priority thread so captures never delay the system's input.
use crate::app::{WM_ACTION, WM_BIND_RECORDED};
use crate::config::{Bind, M_ALT, M_CTRL, M_SHIFT, M_WIN};
use std::ptr::null_mut;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::*};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub static OVERRIDE: AtomicBool = AtomicBool::new(true);
/// When set, the next key combination is captured for the shortcut editor instead of triggering.
pub static RECORD: AtomicBool = AtomicBool::new(false);
static MAIN: AtomicUsize = AtomicUsize::new(0);
/// Key we swallowed on key-down; its repeats and key-up are swallowed too.
static EATEN: AtomicU32 = AtomicU32::new(0);
static BINDS: RwLock<Vec<(u8, u32, usize)>> = RwLock::new(Vec::new());

pub const ACT_REGION: usize = 1;
pub const ACT_FREE: usize = 2;
pub const ACT_FULL: usize = 3;
pub const ACT_INSTANT_FULL: usize = 4;
pub const ACT_RECORD: usize = 5;
pub const ACT_CLIP: usize = 6;

pub fn set_binds(b: &[[Bind; 2]; 6]) {
    let v: Vec<(u8, u32, usize)> = b
        .iter()
        .enumerate()
        .flat_map(|(i, slots)| slots.iter().filter(|s| s.is_set()).map(move |s| (s.mods, s.vk as u32, i + 1)))
        .collect();
    if let Ok(mut g) = BINDS.write() {
        *g = v;
    }
}

pub fn start(main: HWND) {
    MAIN.store(main as usize, SeqCst);
    std::thread::spawn(|| unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), GetModuleHandleW(std::ptr::null()), 0);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {}
    });
}

unsafe fn down(vk: i32) -> bool {
    (GetAsyncKeyState(vk) as u16 & 0x8000) != 0
}

unsafe fn mods() -> u8 {
    let mut m = 0;
    if down(0x5B) || down(0x5C) {
        m |= M_WIN;
    }
    if down(0x11) {
        m |= M_CTRL;
    }
    if down(0x10) {
        m |= M_SHIFT;
    }
    if down(0x12) {
        m |= M_ALT;
    }
    m
}

fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn find(m: u8, vk: u32) -> Option<usize> {
    BINDS.try_read().ok()?.iter().find(|(bm, bv, _)| *bm == m && *bv == vk).map(|(_, _, a)| *a)
}

/// Inject an unassigned key so releasing Win/Alt doesn't pop the Start menu / menu bar.
unsafe fn mask(m: u8) {
    if m & (M_WIN | M_ALT) == 0 {
        return;
    }
    let mut inp: [INPUT; 2] = std::mem::zeroed();
    for (i, e) in inp.iter_mut().enumerate() {
        e.r#type = INPUT_KEYBOARD;
        e.Anonymous.ki.wVk = 0xE8;
        if i == 1 {
            e.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
        }
    }
    SendInput(2, inp.as_ptr(), size_of::<INPUT>() as i32);
}

fn post(msg: u32, wp: usize) {
    unsafe { PostMessageW(MAIN.load(SeqCst) as HWND, msg, wp, 0) };
}

unsafe extern "system" fn proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(lp as *const KBDLLHOOKSTRUCT);
        let vk = k.vkCode;
        if k.flags & LLKHF_INJECTED == 0 && !is_modifier(vk) {
            let is_down = wp == WM_KEYDOWN as usize || wp == WM_SYSKEYDOWN as usize;
            let recording = RECORD.load(Relaxed);
            let active = OVERRIDE.load(Relaxed);
            if !is_down {
                if EATEN.load(Relaxed) == vk {
                    EATEN.store(0, Relaxed);
                    return 1;
                }
                // some keyboards only ever send key-up for Print Screen
                if vk == 0x2C && active && !recording {
                    if let Some(a) = find(mods(), vk) {
                        post(WM_ACTION, a);
                        return 1;
                    }
                }
            } else {
                if EATEN.load(Relaxed) == vk {
                    return 1; // auto-repeat of a swallowed key
                }
                let m = mods();
                if recording {
                    RECORD.store(false, Relaxed);
                    mask(m);
                    EATEN.store(vk, Relaxed);
                    post(WM_BIND_RECORDED, ((m as usize) << 16) | vk as usize);
                    return 1;
                }
                if active {
                    if let Some(a) = find(m, vk) {
                        mask(m);
                        EATEN.store(vk, Relaxed);
                        post(WM_ACTION, a);
                        return 1;
                    }
                }
            }
        }
    }
    CallNextHookEx(null_mut(), code, wp, lp)
}
