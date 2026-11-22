//! Per-user installer / uninstaller (no admin rights needed).
//! Installs to %LOCALAPPDATA%\Programs\Aperture, adds a Start-menu shortcut,
//! autostart entry and an "Apps & features" uninstall entry.
use crate::app;
use crate::config::{self, Config};
use crate::dialog::ask;
use crate::gfx::com_release;
use crate::util::*;
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::ptr::null_mut;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, FOLDERID_Programs};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Aperture";
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn install_dir() -> String {
    format!("{}\\Programs\\Aperture", known_folder(&FOLDERID_LocalAppData))
}
pub fn install_exe() -> String {
    format!("{}\\Aperture.exe", install_dir())
}
fn shortcut_path() -> String {
    format!("{}\\Aperture.lnk", known_folder(&FOLDERID_Programs))
}

pub fn running_from_install_dir() -> bool {
    let me = exe_path().to_lowercase();
    me == install_exe().to_lowercase()
}

/// Ask a running Aperture to quit and wait for it.
unsafe fn close_running() {
    let h = FindWindowW(wz(app::MAIN_CLASS).as_ptr(), std::ptr::null());
    if h.is_null() {
        return;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, &mut pid);
    let ph = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
    PostMessageW(h, app::WM_QUIT_APP, 0, 0);
    if !ph.is_null() {
        WaitForSingleObject(ph, 4000);
        CloseHandle(ph);
    } else {
        Sleep(800);
    }
}

type SetStr = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;

unsafe fn create_shortcut(lnk: &str, target: &str, args: &str, desc: &str) -> bool {
    let clsid = GUID::from_u128(0x00021401_0000_0000_C000_000000000046);
    let iid_link = GUID::from_u128(0x000214F9_0000_0000_C000_000000000046);
    let iid_pf = GUID::from_u128(0x0000010B_0000_0000_C000_000000000046);
    let mut sl: *mut c_void = null_mut();
    if CoCreateInstance(&clsid, null_mut(), CLSCTX_INPROC_SERVER, &iid_link, &mut sl) < 0 || sl.is_null() {
        return false;
    }
    let vt = *(sl as *const *const usize);
    let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32 = std::mem::transmute(*vt.add(0));
    let set_desc: SetStr = std::mem::transmute(*vt.add(7));
    let set_wd: SetStr = std::mem::transmute(*vt.add(9));
    let set_args: SetStr = std::mem::transmute(*vt.add(11));
    let set_icon: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32 = std::mem::transmute(*vt.add(17));
    let set_path: SetStr = std::mem::transmute(*vt.add(20));
    set_path(sl, wz(target).as_ptr());
    set_args(sl, wz(args).as_ptr());
    set_desc(sl, wz(desc).as_ptr());
    set_wd(sl, wz(&install_dir()).as_ptr());
    set_icon(sl, wz(target).as_ptr(), 0);
    let mut pf: *mut c_void = null_mut();
    let mut ok = false;
    if qi(sl, &iid_pf, &mut pf) >= 0 && !pf.is_null() {
        let pvt = *(pf as *const *const usize);
        let save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32 = std::mem::transmute(*pvt.add(6));
        ok = save(pf, wz(lnk).as_ptr(), 1) >= 0;
        com_release(pf);
    }
    com_release(sl);
    ok
}

fn do_install() -> Result<(), String> {
    unsafe { close_running() };
    let dir = install_dir();
    let exe = install_exe();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {dir}: {e}"))?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut last = String::new();
    let mut copied = false;
    for _ in 0..20 {
        match std::fs::copy(&me, &exe) {
            Ok(_) => {
                copied = true;
                break;
            }
            Err(e) => {
                last = e.to_string();
                unsafe { Sleep(150) };
            }
        }
    }
    if !copied {
        return Err(format!("Couldn't copy Aperture.exe: {last}"));
    }
    let size_kb = std::fs::metadata(&exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    reg_set_str(UNINSTALL_KEY, "DisplayName", "Aperture");
    reg_set_str(UNINSTALL_KEY, "DisplayVersion", VERSION);
    reg_set_str(UNINSTALL_KEY, "Publisher", "Aperture");
    reg_set_str(UNINSTALL_KEY, "DisplayIcon", &format!("{exe},0"));
    reg_set_str(UNINSTALL_KEY, "InstallLocation", &dir);
    reg_set_str(UNINSTALL_KEY, "UninstallString", &format!("\"{exe}\" --uninstall"));
    reg_set_str(UNINSTALL_KEY, "QuietUninstallString", &format!("\"{exe}\" --uninstall"));
    reg_set_dword(UNINSTALL_KEY, "NoModify", 1);
    reg_set_dword(UNINSTALL_KEY, "NoRepair", 1);
    reg_set_dword(UNINSTALL_KEY, "EstimatedSize", size_kb);
    reg_set_str(config::RUN_KEY, "Aperture", &format!("\"{exe}\" --tray"));
    unsafe { create_shortcut(&shortcut_path(), &exe, "", "Aperture - lightning-fast screenshots") };
    config::set_windows_prtsc(!config::windows_prtsc_wanted_off(&Config::load()));
    Ok(())
}

pub fn run_installer() {
    let update = std::path::Path::new(&install_exe()).exists();
    let (title, body, btn) = if update {
        ("Update Aperture", "Aperture is already installed. Replace it with this version? Your settings and history are kept.", "Update")
    } else {
        (
            "Install Aperture",
            "Aperture lives in your system tray and takes over Win+Shift+S and Print Screen. It installs just for you, with no admin rights needed.",
            "Install",
        )
    };
    if !ask(title, body, btn, Some("Cancel")) {
        return;
    }
    match do_install() {
        Ok(()) => {
            let _ = std::process::Command::new(install_exe()).arg("--welcome").spawn();
        }
        Err(e) => {
            ask("Couldn't install Aperture", &e, "OK", None);
        }
    }
}

pub fn uninstall() {
    if !ask(
        "Uninstall Aperture?",
        "This removes Aperture, its shortcuts and its clipboard history. Screenshots you saved to your folder are kept.",
        "Uninstall",
        Some("Cancel"),
    ) {
        return;
    }
    unsafe { close_running() };
    reg_del_value(config::RUN_KEY, "Aperture");
    reg_del_tree(UNINSTALL_KEY);
    config::set_windows_prtsc(true);
    let _ = std::fs::remove_file(shortcut_path());
    let _ = std::fs::remove_dir_all(Config::dir());
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("Aperture"));
    let dir = install_dir();
    let inside = exe_path().to_lowercase().starts_with(&dir.to_lowercase());
    if !inside {
        let _ = std::fs::remove_dir_all(&dir);
    }
    ask("Aperture was removed", "Thanks for trying Aperture! Windows' own screenshot shortcuts are back.", "Done", None);
    if inside {
        // The running exe can't delete itself: let a hidden cmd do it once we've exited.
        let _ = std::process::Command::new("cmd.exe")
            .raw_arg(format!("/c ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{dir}\""))
            .current_dir(std::env::temp_dir())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
}
