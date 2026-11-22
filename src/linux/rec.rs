//! The floating timer / stop pill shown during an X11 region recording. It sits
//! just outside the recorded area so it never appears in the video. (Wayland
//! doesn't let apps place windows, so there you stop from the shortcut, the tray,
//! or the desktop's own screen-sharing indicator.)
use super::Surface;
use crate::gfx::*;
use std::rc::Rc;
use std::time::Instant;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::window::{CursorIcon, Window};

pub const W: f32 = 176.0;
pub const H: f32 = 44.0;

pub struct Pill {
    pub win: Rc<Window>,
    surf: Surface,
    s: f32,
    hover_stop: bool,
    blink: bool,
    start: Instant,
    cur: f32,
    back: Vec<u32>,
}

impl Pill {
    pub fn new(win: Rc<Window>, surf: Surface, start: Instant) -> Pill {
        Pill { s: win.scale_factor() as f32, win, surf, hover_stop: false, blink: true, start, cur: 0.0, back: Vec::new() }
    }

    pub fn tick(&mut self) {
        self.blink = !self.blink;
        self.win.request_redraw();
    }

    fn paint(&mut self) {
        let size = self.win.inner_size();
        let (w, h) = (size.width.max(1) as i32, size.height.max(1) as i32);
        let (Some(nw), Some(nh)) = (std::num::NonZeroU32::new(w as u32), std::num::NonZeroU32::new(h as u32)) else { return };
        let _ = self.surf.resize(nw, nh);
        let mut back = std::mem::take(&mut self.back);
        back.clear();
        back.resize((w * h) as usize, 0xFF141518);
        let secs = self.start.elapsed().as_secs();
        {
            let g = Gfx::over_slice(&mut back, w, h);
            g.scale(self.s);
            let pill = Path::rect(0.0, 0.0, W, H);
            g.fill_v(&pill, 0.0, H, 0xFF303338, 0xFF141518);
            g.stroke(&Path::rect(0.5, 0.5, W - 1.0, H - 1.0), 0x22FFFFFF, 1.0);
            let (cx, cy) = (22.0, H / 2.0);
            if self.blink {
                g.fill_radial(&Path::circle(cx, cy, 10.0), cx, cy, 0x70FF3B30, 0x00FF3B30);
            }
            let dot = Path::circle(cx, cy, 6.0);
            g.fill_v(&dot, cy - 6.0, cy + 6.0, if self.blink { 0xFFFF6B5E } else { 0xFFB8342A }, if self.blink { 0xFFD3261A } else { 0xFF7C1C14 });
            g.fill(&Path::ellipse(cx - 3.5, cy - 5.0, 7.0, 4.0), 0x60FFFFFF);
            g.text(&format!("{}:{:02}", secs / 60, secs % 60), 38.0, 0.0, 70.0, H, 15.0, 0xFFFFFFFF, SEMI);
            g.text("REC", 84.0, 0.0, 40.0, H, 10.5, 0xFFFF8A80, SEMI);
            let (bx, by, bs) = (W - 40.0, (H - 30.0) / 2.0, 30.0);
            let b = Path::circle(bx + bs / 2.0, by + bs / 2.0, bs / 2.0);
            g.fill_v(&b, by, by + bs, if self.hover_stop { 0xFF5B6069 } else { 0xFF464A52 }, 0xFF26282D);
            g.stroke(&b, 0x80000000, 1.0);
            g.fill(&Path::rrect(bx + 10.0, by + 10.0, 10.0, 10.0, 2.0), 0xFFFFFFFF);
        }
        if let Ok(mut out) = self.surf.buffer_mut() {
            if out.len() == back.len() {
                out.copy_from_slice(&back);
                self.win.pre_present_notify();
                let _ = out.present();
            }
        }
        self.back = back;
    }

    /// Returns true when the stop button was clicked.
    pub fn event(&mut self, ev: &WindowEvent) -> bool {
        match ev {
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cur = position.x as f32 / self.s;
                let on = self.cur > W - 44.0;
                if on != self.hover_stop {
                    self.hover_stop = on;
                    self.win.set_cursor(if on { CursorIcon::Pointer } else { CursorIcon::Move });
                    self.win.request_redraw();
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                if self.cur > W - 44.0 {
                    return true;
                }
                let _ = self.win.drag_window();
            }
            _ => {}
        }
        false
    }
}
