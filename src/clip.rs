use crate::util::{wz, Img};
use std::ptr::copy_nonoverlapping;
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Threading::Sleep;

const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;

/// Puts a file on the clipboard (like Explorer's Copy) so a video can be pasted into chats.
pub unsafe fn set_file(hwnd: HWND, path: &str) -> bool {
    let wide: Vec<u16> = path.encode_utf16().chain([0, 0]).collect();
    let size = 20 + wide.len() * 2;
    let hg = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, size);
    if hg.is_null() {
        return false;
    }
    let p = GlobalLock(hg) as *mut u8;
    *(p as *mut u32) = 20; // DROPFILES.pFiles
    *(p.add(16) as *mut u32) = 1; // fWide
    copy_nonoverlapping(wide.as_ptr() as *const u8, p.add(20), wide.len() * 2);
    GlobalUnlock(hg);
    for _ in 0..25 {
        if OpenClipboard(hwnd) != 0 {
            EmptyClipboard();
            SetClipboardData(CF_HDROP, hg as HANDLE);
            CloseClipboard();
            return true;
        }
        Sleep(8);
    }
    GlobalFree(hg);
    false
}

/// Puts an image on the clipboard as CF_DIB (+ "PNG" for images with transparency).
pub unsafe fn set_image(hwnd: HWND, img: &Img, png: Option<&[u8]>) -> bool {
    let mut open = false;
    for _ in 0..25 {
        if OpenClipboard(hwnd) != 0 {
            open = true;
            break;
        }
        Sleep(8);
    }
    if !open {
        return false;
    }
    EmptyClipboard();

    let (w, h) = (img.w as usize, img.h as usize);
    let hdr = size_of::<BITMAPINFOHEADER>();
    let hg = GlobalAlloc(GMEM_MOVEABLE, hdr + w * h * 4);
    if !hg.is_null() {
        let p = GlobalLock(hg) as *mut u8;
        let bih = BITMAPINFOHEADER {
            biSize: hdr as u32,
            biWidth: img.w,
            biHeight: img.h, // bottom-up: the most compatible layout
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: (w * h * 4) as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        copy_nonoverlapping(&bih as *const _ as *const u8, p, hdr);
        let dst = p.add(hdr) as *mut u32;
        for y in 0..h {
            let src = &img.px[(h - 1 - y) * w..(h - y) * w];
            let d = std::slice::from_raw_parts_mut(dst.add(y * w), w);
            if img.alpha {
                for (o, &s) in d.iter_mut().zip(src) {
                    let a = s >> 24;
                    let bl = |c: u32| (c * a + 255 * (255 - a)) / 255;
                    *o = 0xFF00_0000 | (bl((s >> 16) & 255) << 16) | (bl((s >> 8) & 255) << 8) | bl(s & 255);
                }
            } else {
                d.copy_from_slice(src);
            }
        }
        GlobalUnlock(hg);
        SetClipboardData(CF_DIB, hg as HANDLE);
    }
    if let Some(png) = png {
        let fmt = RegisterClipboardFormatW(wz("PNG").as_ptr());
        let hp = GlobalAlloc(GMEM_MOVEABLE, png.len());
        if !hp.is_null() {
            copy_nonoverlapping(png.as_ptr(), GlobalLock(hp) as *mut u8, png.len());
            GlobalUnlock(hp);
            SetClipboardData(fmt, hp as HANDLE);
        }
    }
    CloseClipboard();
    true
}
