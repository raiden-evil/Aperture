//! Small rounded modal used by the installer / uninstaller.
use crate::gfx::*;
use crate::icons;
use crate::overlay::Dib;
use crate::settings::pill;
use crate::util::*;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct Dlg {
    hwnd: HWND,
    s: f32,
    title: String,
    body: String,
    ok: String,
    cancel: Option<String>,
    result: bool,
    hover: i32,
    rects: [[f32; 4]; 2],
}
static D: G<Dlg> = G::new();
const CLASS: &str = "ApertureDialog";
const W: f32 = 460.0;
const H: f32 = 210.0;

pub fn ask(title: &str, body: &str, ok: &str, cancel: Option<&str>) -> bool {
    unsafe {
        crate::theme::set(crate::config::Config::load().theme);
        let hinst = GetModuleHandleW(null());
        let cls = wz(CLASS);
        let mut big = icons::render_camera(48);
        let mut small = icons::render_camera(16);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: hicon_from(&mut big),
            hIconSm: hicon_from(&mut small),
            lpszClassName: cls.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let c = cursor();
        let (_, work, s) = monitor_at(c.x, c.y);
        let (w, h) = ((W * s) as i32, (H * s) as i32);
        let x = work.left + ((work.right - work.left) - w) / 2;
        let y = work.top + ((work.bottom - work.top) - h) / 2;
        D.set(Dlg {
            hwnd: null_mut(),
            s,
            title: title.into(),
            body: body.into(),
            ok: ok.into(),
            cancel: cancel.map(Into::into),
            result: false,
            hover: -1,
            rects: [[0.0; 4]; 2],
        });
        let hwnd = CreateWindowExW(WS_EX_APPWINDOW, cls.as_ptr(), wz(title).as_ptr(), WS_POPUP | WS_CAPTION | WS_SYSMENU, x, y, w, h, null_mut(), null_mut(), hinst, null());
        D.get().hwnd = hwnd;
        round_corners(hwnd);
        crate::theme::apply_window(hwnd);
        SetWindowPos(hwnd, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
        ShowWindow(hwnd, SW_SHOW);
        force_foreground(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        while IsWindow(hwnd) != 0 && GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnregisterClassW(cls.as_ptr(), hinst);
        D.get().result
    }
}

unsafe fn paint(d: &mut Dlg, hdc: HDC) {
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(d.hwnd, &mut rc);
    let Some(back) = Dib::new(rc.right.max(1), rc.bottom.max(1)) else { return };
    {
        let g = Gfx::from_hdc(back.dc);
        g.scale(d.s);
        let p = crate::theme::pal();
        g.rect(0.0, 0.0, W + 2.0, H + 2.0, p.bg);
        g.fill_v(&Path::rect(0.0, 0.0, W + 2.0, 120.0), 0.0, 120.0, p.head0, p.head1);
        icons::camera(&g, 24.0, 26.0, 64.0);
        g.text(&d.title, 106.0, 24.0, W - 130.0, 28.0, 19.0, p.title, SEMI);
        g.text(&d.body, 107.0, 58.0, W - 132.0, 80.0, 12.5, p.text2, WRAP);
        let bh = 34.0;
        let by = H - bh - 22.0;
        let okw = 110.0;
        let okr = [W - 24.0 - okw, by, okw, bh];
        pill(&g, okr[0], okr[1], okr[2], okr[3], &d.ok, true, d.hover == 0);
        d.rects[0] = okr;
        if let Some(c) = &d.cancel {
            let cr = [okr[0] - 10.0 - 96.0, by, 96.0, bh];
            pill(&g, cr[0], cr[1], cr[2], cr[3], c, false, d.hover == 1);
            d.rects[1] = cr;
        }
    }
    BitBlt(hdc, 0, 0, rc.right, rc.bottom, back.dc, 0, 0, SRCCOPY);
}

fn hit(d: &Dlg, x: f32, y: f32) -> i32 {
    for (i, r) in d.rects.iter().enumerate() {
        if r[2] > 0.0 && x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3] {
            return i as i32;
        }
    }
    -1
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_NCCALCSIZE if wp == 1 => return 0,
        WM_NCACTIVATE => return DefWindowProcW(h, m, wp, -1),
        _ => {}
    }
    let Some(d) = D.try_get() else { return DefWindowProcW(h, m, wp, lp) };
    if d.hwnd != h {
        return DefWindowProcW(h, m, wp, lp);
    }
    match m {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(h, &mut ps);
            paint(d, hdc);
            EndPaint(h, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_NCHITTEST => {
            let (sx, sy) = lparam_xy(lp);
            let mut pt = POINT { x: sx, y: sy };
            ScreenToClient(h, &mut pt);
            if hit(d, pt.x as f32 / d.s, pt.y as f32 / d.s) < 0 { HTCAPTION as isize } else { HTCLIENT as isize }
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            let hv = hit(d, x as f32 / d.s, y as f32 / d.s);
            if hv != d.hover {
                d.hover = hv;
                InvalidateRect(h, null(), 0);
            }
            0
        }
        WM_NCMOUSEMOVE => {
            if d.hover != -1 {
                d.hover = -1;
                InvalidateRect(h, null(), 0);
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_SETCURSOR => {
            if d.hover >= 0 {
                SetCursor(LoadCursorW(null_mut(), IDC_HAND));
                return 1;
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            match hit(d, x as f32 / d.s, y as f32 / d.s) {
                0 => {
                    d.result = true;
                    DestroyWindow(h);
                }
                1 => {
                    d.result = false;
                    DestroyWindow(h);
                }
                _ => {}
            }
            0
        }
        WM_KEYDOWN => {
            if wp == 0x0D {
                d.result = true;
                DestroyWindow(h);
            } else if wp == 0x1B {
                d.result = false;
                DestroyWindow(h);
            }
            0
        }
        WM_DPICHANGED => {
            d.s = ((wp >> 16) & 0xFFFF) as f32 / 96.0;
            let r = &*(lp as *const RECT);
            SetWindowPos(h, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}
