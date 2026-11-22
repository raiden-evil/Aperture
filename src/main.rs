//! Aperture — ultra lightweight screenshot tool (pure Rust).
//!
//! Shared code lives at the top level (settings, icons, palettes, history, image
//! maths). Each platform has its own backend, and only the backend for the
//! target being built is compiled: `win/` (Win32, GDI+, DXGI, Media Foundation)
//! or `linux/` (Wayland + X11).
#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn, clippy::missing_safety_doc, clippy::too_many_arguments)]

// Recording, replay clips and the custom toast are Windows-only for now, so the
// Linux build doesn't use every shared setting, icon and palette entry yet.
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod config;
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod icons;
mod img;
mod store;
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod theme;
mod video;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win::{app, clip, config_sys, dialog, gfx, hook, install, media, overlay, rec, settings, toast, util};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::{config_sys, gfx};

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("Aperture supports Windows and Linux.");

fn main() {
    #[cfg(windows)]
    win::main();
    #[cfg(target_os = "linux")]
    linux::main();
}
