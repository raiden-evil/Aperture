//! Single instance: later launches (`aperture --region`, a desktop shortcut,
//! the app menu) hand their request to the running copy over a Unix socket.
//! The launcher's activation token travels along so the overlay may take focus.
use super::{post, UserEvent};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => PathBuf::from(d).join("aperture.sock"),
        None => std::env::temp_dir().join(format!("aperture-{}.sock", std::env::var("USER").unwrap_or_default())),
    }
}

/// Command-line flag → message word.
pub fn word(args: &[String]) -> &'static str {
    for a in args {
        match a.as_str() {
            "--region" => return "region",
            "--freeform" => return "freeform",
            "--full" => return "full",
            "--instant" => return "instant",
            "--record" => return "record",
            "--clip" => return "clip",
            "--tray" => return "tray",
            _ => {}
        }
    }
    "settings"
}

fn token_from_env() -> String {
    std::env::var("XDG_ACTIVATION_TOKEN").or_else(|_| std::env::var("DESKTOP_STARTUP_ID")).unwrap_or_default()
}

/// Sends the request to a running copy. Returns false when none is running.
pub fn forward(word: &str) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path()) else { return false };
    let _ = writeln!(s, "{word} {}", token_from_env());
    true
}

/// Starts listening; returns false if another copy already owns the socket.
pub fn listen() -> bool {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        return false;
    }
    let _ = std::fs::remove_file(&path); // stale socket from a crash
    let Ok(l) = UnixListener::bind(&path) else { return false };
    std::thread::spawn(move || {
        for conn in l.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(conn).read_line(&mut line).is_err() {
                continue;
            }
            let mut it = line.split_whitespace();
            let w = it.next().unwrap_or("settings");
            let token = it.next().map(str::to_string);
            let act = match w {
                "region" => 1,
                "freeform" => 2,
                "full" => 3,
                "instant" => 4,
                "record" => 5,
                "clip" => 6,
                "tray" => continue,
                _ => 0,
            };
            post(if act == 0 { UserEvent::Settings } else { UserEvent::Action(act, token) });
        }
    });
    true
}

pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}
