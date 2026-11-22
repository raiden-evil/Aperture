//! Video sizing shared by both recording backends.

/// Encoder bitrate (bits/s) for a `w`×`h` video at `fps`.
pub fn bitrate(w: u32, h: u32, fps: u32) -> u32 {
    ((w as f64 * h as f64 * fps as f64 * 0.11) as u32).clamp(2_000_000, 60_000_000)
}

/// Output size for a `sw` x `sh` source capped at `res` pixels tall (0 = native).
/// Both sides stay even, as encoders require. Never upscales.
pub fn scaled_size(sw: u32, sh: u32, res: u32) -> (u32, u32) {
    let even = |v: u32| (v / 2 * 2).max(64);
    if res == 0 || sh <= res {
        return (even(sw), even(sh));
    }
    let k = sh as f64 / res as f64;
    (even((sw as f64 / k).round() as u32), even((sh as f64 / k).round() as u32))
}
