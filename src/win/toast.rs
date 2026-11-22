//! Little per-pixel-alpha notification card with a thumbnail of the capture.
use crate::gfx::*;
use crate::icons;
use crate::overlay::Dib;
use crate::settings;
use crate::util::*;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct Toast {
    hwnd: HWND,
    alpha: i32,
    path: Option<String>,
}
static T: G<Toast> = G::new();
const CLASS: &str = "ApertureToast";

pub unsafe fn init(hinst: HINSTANCE) {
    let cls = wz(CLASS);
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        hCursor: LoadCursorW(null_mut(), IDC_HAND),
        lpszClassName: cls.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassExW(&wc);
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        cls.as_ptr(),
        wz("Aperture").as_ptr(),
        WS_POPUP,
        0,
        0,
        1,
        1,
        null_mut(),
        null_mut(),
        hinst,
        null(),
    );
    T.set(Toast { hwnd, alpha: 0, path: None });
}

pub unsafe fn show(img: Option<&Img>, title: &str, sub: &str, path: Option<String>) {
    show_ex(img, None, title, sub, path)
}

pub unsafe fn show_icon(icon: icons::Icon, title: &str, sub: &str, path: Option<String>) {
    show_ex(None, Some(icon), title, sub, path)
}

unsafe fn show_ex(img: Option<&Img>, icon: Option<icons::Icon>, title: &str, sub: &str, path: Option<String>) {
    let Some(t) = T.try_get() else { return };
    let pl = crate::theme::pal();
    let c = cursor();
    let (_, work, s) = monitor_at(c.x, c.y);
    let m = 16.0 * s;
    let (cw, ch) = (330.0 * s, 78.0 * s);
    let (w, h) = ((cw + m * 2.0).ceil() as i32, (ch + m * 2.0).ceil() as i32);
    let Some(dib) = Dib::new(w, h) else { return };
    let bmp = bitmap_over(w, h, dib.bits, PF_PARGB);
    {
        let g = Gfx::from_image(bmp);
        g.clear(0);
        for i in 0..8 {
            let e = (8 - i) as f32 * 1.6 * s;
            g.fill(&Path::rrect(m - e, m - e + 4.0 * s, cw + e * 2.0, ch + e * 2.0, 18.0 * s + e), 0x07000000);
        }
        let card = Path::rrect(m, m, cw, ch, 18.0 * s);
        g.fill_v(&card, m, m + ch, pl.toast0, pl.toast1);
        g.stroke(&card, pl.toast_line, 1.0 * s);
        g.stroke(&Path::rrect(m + 1.0 * s, m + 1.0 * s, cw - 2.0 * s, ch - 2.0 * s, 17.0 * s), pl.toast_hi, 1.0 * s);

        let (tx, ty, ts) = (m + 11.0 * s, m + 11.0 * s, 56.0 * s);
        let frame = Path::rrect(tx, ty, ts, ts, 12.0 * s);
        g.fill_v(&frame, ty, ty + ts, pl.frame0, pl.frame1);
        if let Some(img) = img {
            let th = img.scaled(((ts * 2.0) / (img.w.min(img.h) as f32)).min(1.0));
            let mut px = th.px.clone();
            let tb = bitmap_over(th.w, th.h, px.as_mut_ptr(), if th.alpha { PF_ARGB } else { PF_RGB });
            g.clip(&frame);
            g.image_cover(tb, th.w as f32, th.h as f32, tx, ty, ts, ts);
            g.unclip();
            dispose(tb);
            // green "done" badge
            let (bx, by, br) = (tx + ts - 6.0 * s, ty + ts - 6.0 * s, 9.0 * s);
            let badge = Path::circle(bx, by, br);
            g.fill(&Path::circle(bx, by, br + 2.0 * s), pl.toast0);
            g.fill_v(&badge, by - br, by + br, 0xFF5EDB6E, 0xFF22A33A);
            g.polyline(&[(bx - 4.0 * s, by), (bx - 1.2 * s, by + 3.0 * s), (bx + 4.2 * s, by - 3.2 * s)], 0xFFFFFFFF, 2.0 * s);
        } else if let Some(ic) = icon {
            icons::draw(&g, ic, tx + 10.0 * s, ty + 10.0 * s, ts - 20.0 * s);
        } else {
            icons::camera(&g, tx + 4.0 * s, ty + 4.0 * s, ts - 8.0 * s);
        }
        g.stroke(&frame, 0x26000000, 1.0 * s);
        let x = tx + ts + 14.0 * s;
        let tw = cw - (x - m) - 14.0 * s;
        g.text(title, x, m + 17.0 * s, tw, 22.0 * s, 15.0 * s, pl.title, SEMI);
        g.text(sub, x, m + 40.0 * s, tw, 20.0 * s, 12.0 * s, pl.text2, 0);
    }
    dispose(bmp);
    let pos = POINT { x: work.right - w + (m * 0.25) as i32, y: work.bottom - h + (m * 0.25) as i32 };
    let size = SIZE { cx: w, cy: h };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 1 };
    let sdc = GetDC(null_mut());
    UpdateLayeredWindow(t.hwnd, sdc, &pos, &size, dib.dc, &src, 0, &blend, ULW_ALPHA);
    ReleaseDC(null_mut(), sdc);
    t.alpha = 255;
    t.path = path;
    SetWindowPos(t.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    KillTimer(t.hwnd, 2);
    SetTimer(t.hwnd, 1, 2200, None);
}

unsafe fn set_alpha(h: HWND, a: u8) {
    let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: a, AlphaFormat: 1 };
    UpdateLayeredWindow(h, null_mut(), null(), null(), null_mut(), null(), 0, &blend, ULW_ALPHA);
}

unsafe fn hide(t: &mut Toast) {
    KillTimer(t.hwnd, 1);
    KillTimer(t.hwnd, 2);
    ShowWindow(t.hwnd, SW_HIDE);
    t.alpha = 0;
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(t) = T.try_get() else { return DefWindowProcW(h, m, wp, lp) };
    match m {
        WM_TIMER => {
            if wp == 1 {
                KillTimer(h, 1);
                SetTimer(h, 2, 15, None);
            } else if wp == 2 {
                t.alpha -= 24;
                if t.alpha <= 0 {
                    hide(t);
                } else {
                    set_alpha(h, t.alpha as u8);
                }
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_MOUSEMOVE => {
            // keep it on screen while hovered
            if t.alpha > 0 && t.alpha < 255 {
                t.alpha = 255;
                set_alpha(h, 255);
            }
            KillTimer(h, 2);
            SetTimer(h, 1, 1500, None);
            0
        }
        WM_LBUTTONUP => {
            let p = t.path.take();
            hide(t);
            match p {
                Some(p) => open_path(&p),
                None => settings::show(),
            }
            0
        }
        WM_RBUTTONUP => {
            hide(t);
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}
