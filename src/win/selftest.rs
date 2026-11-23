//! Developer self-test: `Aperture.exe --test-replay <out.mp4> [fps]`
//! Runs the real replay buffer for a few seconds while a small window animates
//! at 60 fps (so the screen keeps changing), saves a clip, then waits for it.
//! Used to check that clips keep every frame.
use crate::util::wz;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub unsafe fn test_replay(out: &str, fps: u32) {
    let hinst = GetModuleHandleW(null());
    let cls = wz("ApertureSelfTest");
    let wc = WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), hInstance: hinst, lpszClassName: cls.as_ptr(), ..std::mem::zeroed() };
    RegisterClassW(&wc);
    let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, cls.as_ptr(), wz("test").as_ptr(), WS_POPUP | WS_VISIBLE, 40, 40, 320, 200, null_mut(), null_mut(), hinst, null());
    let dir = std::env::temp_dir().join("Aperture").join("replay-test");
    let _ = std::fs::remove_file(out);
    if let Err(e) = crate::media::start_replay(10, fps, 0, dir) {
        eprintln!("replay failed to start: {e}");
        return;
    }
    let t0 = Instant::now();
    let mut n = 0u32;
    let mut saved = false;
    while t0.elapsed() < Duration::from_secs(30) {
        // animate: a bar sweeping across the window, a new color every frame,
        // with a still pause in the middle (exercises the capture watchdog)
        let t = t0.elapsed().as_secs_f32();
        let still = (2.5..6.5).contains(&t);
        let hdc = if still { null_mut() } else { GetDC(hwnd) };
        if !hdc.is_null() {
        let rc = RECT { left: 0, top: 0, right: 320, bottom: 200 };
        let bg = CreateSolidBrush(0x00202020);
        FillRect(hdc, &rc, bg);
        DeleteObject(bg);
        let x = (n * 7 % 300) as i32;
        let br = CreateSolidBrush((n.wrapping_mul(2654435761) & 0x00FF_FFFF) as u32);
        FillRect(hdc, &RECT { left: x, top: 20, right: x + 20, bottom: 180 }, br);
        DeleteObject(br);
        ReleaseDC(hwnd, hdc);
        }
        n += 1;
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            DispatchMessageW(&msg);
        }
        std::thread::sleep(Duration::from_millis(16));
        if !saved && t0.elapsed() > Duration::from_secs(9) {
            crate::media::save_clip(std::path::PathBuf::from(out));
            saved = true;
        }
        // done once the clip exists and has stopped growing
        if saved && t0.elapsed() > Duration::from_secs(12) {
            if let Ok(m) = std::fs::metadata(out) {
                std::thread::sleep(Duration::from_millis(800));
                if std::fs::metadata(out).map(|m2| m2.len()).unwrap_or(0) == m.len() && m.len() > 0 {
                    break;
                }
            }
        }
    }
    crate::media::stop_replay();
    DestroyWindow(hwnd);
    crate::media::shutdown();
}
