use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::core::{GUID, PWSTR};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub unsafe fn from_wide(p: *const u16) -> String {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

/// Single-threaded global cell (all UI lives on one thread).
pub struct G<T>(UnsafeCell<Option<T>>);
unsafe impl<T> Sync for G<T> {}
impl<T> G<T> {
    pub const fn new() -> Self {
        G(UnsafeCell::new(None))
    }
    pub fn set(&self, v: T) {
        unsafe { *self.0.get() = Some(v) }
    }
    #[allow(clippy::mut_from_ref)]
    pub fn get(&self) -> &mut T {
        unsafe { (*self.0.get()).as_mut().unwrap() }
    }
    #[allow(clippy::mut_from_ref)]
    pub fn try_get(&self) -> Option<&mut T> {
        unsafe { (*self.0.get()).as_mut() }
    }
}

pub use crate::img::Img;

pub fn known_folder(id: &GUID) -> String {
    unsafe {
        let mut p: PWSTR = null_mut();
        if SHGetKnownFolderPath(id, 0, null_mut(), &mut p) >= 0 && !p.is_null() {
            let s = from_wide(p);
            CoTaskMemFree(p as *const c_void);
            return s;
        }
        String::new()
    }
}

pub fn exe_path() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

// ---------- registry (HKCU) ----------
pub fn reg_set_str(sub: &str, name: &str, val: &str) {
    let v = wz(val);
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, wz(sub).as_ptr(), wz(name).as_ptr(), REG_SZ, v.as_ptr() as *const c_void, (v.len() * 2) as u32);
    }
}
pub fn reg_set_dword(sub: &str, name: &str, val: u32) {
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, wz(sub).as_ptr(), wz(name).as_ptr(), REG_DWORD, &val as *const u32 as *const c_void, 4);
    }
}
pub fn reg_get_str(sub: &str, name: &str) -> Option<String> {
    unsafe {
        let mut buf = [0u16; 1024];
        let mut cb = (buf.len() * 2) as u32;
        let r = RegGetValueW(HKEY_CURRENT_USER, wz(sub).as_ptr(), wz(name).as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut cb);
        if r == 0 { Some(from_wide(buf.as_ptr())) } else { None }
    }
}
pub fn reg_del_value(sub: &str, name: &str) {
    unsafe {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, wz(sub).as_ptr(), wz(name).as_ptr());
    }
}
pub fn reg_del_tree(sub: &str) {
    unsafe {
        RegDeleteTreeW(HKEY_CURRENT_USER, wz(sub).as_ptr());
    }
}

pub fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32)
}

pub fn timestamp() -> String {
    unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!("{:04}-{:02}-{:02} at {:02}.{:02}.{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
}

/// Monitor rect (screen coords), work area and UI scale for a screen point.
pub unsafe fn monitor_at(x: i32, y: i32) -> (RECT, RECT, f32) {
    let hm = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(hm, &mut mi);
    let (mut dx, mut dy) = (96u32, 96u32);
    GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    (mi.rcMonitor, mi.rcWork, dx as f32 / 96.0)
}

pub unsafe fn cursor() -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    GetCursorPos(&mut p);
    p
}

/// Reliably bring a window to the foreground (works when triggered from a global hook).
pub unsafe fn force_foreground(h: HWND) {
    let fg = GetForegroundWindow();
    let ft = GetWindowThreadProcessId(fg, null_mut());
    let me = GetCurrentThreadId();
    let attach = ft != 0 && ft != me;
    if attach {
        AttachThreadInput(me, ft, 1);
    }
    SetForegroundWindow(h);
    BringWindowToTop(h);
    SetFocus(h);
    if attach {
        AttachThreadInput(me, ft, 0);
    }
}

/// Win11 rounded corners for our custom-chrome windows.
pub unsafe fn round_corners(h: HWND) {
    let pref: u32 = 2; // DWMWCP_ROUND
    DwmSetWindowAttribute(h, 33, &pref as *const u32 as *const c_void, 4);
}

pub unsafe fn open_path(p: &str) {
    windows_sys::Win32::UI::Shell::ShellExecuteW(null_mut(), wz("open").as_ptr(), wz(p).as_ptr(), null(), null(), SW_SHOWNORMAL);
}

pub fn rect_has(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}
