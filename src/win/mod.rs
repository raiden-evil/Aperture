//! Windows backend: Win32 windows, GDI+ drawing, DXGI capture and Media Foundation video.
pub mod app;
pub mod clip;
pub mod config_sys;
pub mod dialog;
pub mod gfx;
pub mod hook;
pub mod install;
pub mod media;
pub mod overlay;
pub mod rec;
pub mod selftest;
pub mod settings;
pub mod toast;
pub mod util;

use crate::icons;
use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};

pub fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        gfx::startup();

        let args: Vec<String> = std::env::args().skip(1).collect();
        let a = args.first().map(String::as_str).unwrap_or("");
        match a {
            "--write-icon" => {
                icons::write_ico(args.get(1).map(String::as_str).unwrap_or("assets/aperture.ico"));
                return;
            }
            "--probe" => {
                if let (Some(i), Some(o)) = (args.get(1), args.get(2)) {
                    media::probe(i, o);
                }
                return;
            }
            "--test-replay" => {
                let out = args.get(1).cloned().unwrap_or_else(|| "replay-test.mp4".into());
                let fps = args.get(2).and_then(|f| f.parse().ok()).unwrap_or(30);
                selftest::test_replay(&out, fps);
                return;
            }
            "--hdr-info" => {
                let dir = std::env::temp_dir();
                let path = dir.join("aperture-hdr.txt");
                let mut report = media::hdr_report();
                if let Some((plain, fixed, log)) = overlay::capture_compare() {
                    let (a, b) = (dir.join("aperture-hdr-windows.png"), dir.join("aperture-hdr-aperture.png"));
                    let _ = std::fs::write(&a, gfx::encode_png(&plain));
                    let _ = std::fs::write(&b, gfx::encode_png(&fixed));
                    report += "\r\nTest capture just now:\r\n";
                    for l in &log {
                        report += &format!("  {l}\r\n");
                    }
                    if log.is_empty() {
                        report += "  no HDR monitor, nothing to correct\r\n";
                    }
                    report += &format!(
                        "\r\nCompare these with what you see on screen:\r\n  {}  (plain Windows capture)\r\n  {}  (Aperture's HDR correction)\r\n",
                        a.display(),
                        b.display()
                    );
                }
                let _ = std::fs::write(&path, report);
                util::open_path(&path.to_string_lossy());
                return;
            }
            "--uninstall" => {
                install::uninstall();
                return;
            }
            _ => {}
        }
        // Running the downloaded exe (anywhere but the install folder) = installer.
        if a != "--portable" && !install::running_from_install_dir() {
            install::run_installer();
            return;
        }
        // Single instance: forward the request to the running copy.
        let existing = FindWindowW(util::wz(app::MAIN_CLASS).as_ptr(), std::ptr::null());
        if !existing.is_null() {
            let act = args
                .iter()
                .map(|s| match s.as_str() {
                    "--region" => hook::ACT_REGION,
                    "--freeform" => hook::ACT_FREE,
                    "--full" => hook::ACT_FULL,
                    "--record" => hook::ACT_RECORD,
                    "--clip" => hook::ACT_CLIP,
                    _ => 0,
                })
                .max()
                .unwrap_or(0);
            if act == 0 {
                if !args.iter().any(|s| s == "--tray") {
                    PostMessageW(existing, app::WM_SHOW_SETTINGS, 0, 0);
                }
            } else {
                PostMessageW(existing, app::WM_ACTION, act, 0);
            }
            return;
        }
        let mode = if args.iter().any(|s| s == "--tray") {
            "--tray"
        } else if args.iter().any(|s| s == "--welcome") {
            "--welcome"
        } else {
            ""
        };
        app::run(mode);
    }
}
