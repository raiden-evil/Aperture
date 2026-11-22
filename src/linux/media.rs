//! Screen recording and the replay buffer on Linux, done by a GStreamer
//! pipeline in a child process (`gst-launch-1.0`), so nothing has to be linked
//! at build time and a crash in a driver can't take Aperture down.
//! - X11: `ximagesrc` grabs the area directly.
//! - Wayland: the ScreenCast portal hands over a PipeWire stream of a monitor
//!   (the desktop asks which one the first time and remembers the choice).
//!
//! H.264 is encoded on the graphics card through VA-API when possible, else with
//! x264 / OpenH264. Replay clips are rolling 4-second MPEG-TS segments on disk
//! that get joined (without re-encoding) into an MP4 when a clip is saved.
use super::{post, UserEvent};
use crate::video::{bitrate, scaled_size};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const SEG_SECS: u32 = 4;

pub enum MediaEvent {
    Started(Result<Recording, String>),
    ReplayStarted(Result<Replay, String>),
    Saved { path: PathBuf, secs: f64, clip: bool },
    Error(String),
}

fn report(e: MediaEvent) {
    post(UserEvent::Media(e));
}

// ---------------------------------------------------------------- tools

fn has(cmd: &str) -> bool {
    super::output::has_cmd(cmd)
}

/// None when recording can work, otherwise what to install.
pub fn missing() -> Option<&'static str> {
    if !has("gst-launch-1.0") || !has("gst-inspect-1.0") {
        return Some("GStreamer isn't installed. Run: sudo apt install gstreamer1.0-tools gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-pipewire");
    }
    None
}

fn element_exists(name: &str) -> bool {
    Command::new("gst-inspect-1.0").args(["--exists", name]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Encoder {
    Va,
    Vaapi,
    X264,
    OpenH264,
}

impl Encoder {
    fn element(self, kbps: u32, gop: u32) -> String {
        match self {
            Encoder::Va => format!("vah264enc rate-control=cbr bitrate={kbps} key-int-max={gop}"),
            Encoder::Vaapi => format!("vaapih264enc rate-control=cbr bitrate={kbps} keyframe-period={gop}"),
            Encoder::X264 => format!("x264enc speed-preset=veryfast tune=zerolatency bitrate={kbps} key-int-max={gop} threads=0"),
            Encoder::OpenH264 => format!("openh264enc bitrate={} gop-size={gop}", kbps * 1000),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Encoder::Va | Encoder::Vaapi => "graphics card (VA-API)",
            Encoder::X264 => "processor (x264)",
            Encoder::OpenH264 => "processor (OpenH264)",
        }
    }
}

/// Installed H.264 encoders, best first (checked once).
fn encoders() -> &'static [Encoder] {
    static E: OnceLock<Vec<Encoder>> = OnceLock::new();
    E.get_or_init(|| {
        if missing().is_some() {
            return Vec::new();
        }
        [("vah264enc", Encoder::Va), ("vaapih264enc", Encoder::Vaapi), ("x264enc", Encoder::X264), ("openh264enc", Encoder::OpenH264)]
            .into_iter()
            .filter(|(n, _)| element_exists(n))
            .map(|(_, e)| e)
            .collect()
    })
}

/// Which encoder is in use, for the settings window.
pub fn encoder_label() -> Option<&'static str> {
    if let Some(e) = WORKING.get() {
        return Some(e.label());
    }
    encoders().first().map(|e| e.label())
}

/// The first encoder that actually started (a VA element can exist without a working driver).
static WORKING: OnceLock<Encoder> = OnceLock::new();

// ---------------------------------------------------------------- portal

/// A live ScreenCast session; closing it (drop) ends the stream.
pub struct Cast {
    node: u32,
    fd: OwnedFd,
    stop: Sender<()>,
}
impl Drop for Cast {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn token_file(kind: &str) -> PathBuf {
    Path::new(&crate::config::Config::dir()).join(format!("{kind}.token"))
}

/// Asks the portal for a monitor stream. `kind` keeps separate remembered
/// choices for recordings and the replay buffer.
fn open_cast(kind: &'static str) -> Result<Cast, String> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;
    let token = std::fs::read_to_string(token_file(kind)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let (ready_tx, ready_rx) = channel::<Result<(u32, OwnedFd), String>>();
    let (stop_tx, stop_rx) = channel::<()>();
    std::thread::spawn(move || {
        pollster::block_on(async move {
            let res = async {
                let e = |e: ashpd::Error| e.to_string();
                let sc = Screencast::new().await.map_err(e)?;
                let session = sc.create_session(Default::default()).await.map_err(e)?;
                let opts = SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::EMBEDDED)
                    .set_sources(SourceType::MONITOR)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(token.as_deref());
                sc.select_sources(&session, opts).await.map_err(e)?.response().map_err(e)?;
                let streams = sc.start(&session, None, Default::default()).await.map_err(e)?.response().map_err(|err| match err {
                    ashpd::Error::Response(_) => "Screen sharing was cancelled".to_string(),
                    err => err.to_string(),
                })?;
                if let Some(t) = streams.restore_token() {
                    let _ = std::fs::create_dir_all(crate::config::Config::dir());
                    let _ = std::fs::write(token_file(kind), t);
                }
                let node = streams.streams().first().ok_or("No screen was shared")?.pipe_wire_node_id();
                let fd = sc.open_pipe_wire_remote(&session, Default::default()).await.map_err(e)?;
                Ok::<_, String>((sc, session, node, fd))
            }
            .await;
            match res {
                Ok((sc, session, node, fd)) => {
                    let _ = ready_tx.send(Ok((node, fd)));
                    let _ = stop_rx.recv(); // keep the session open until the Cast is dropped
                    let _ = session.close().await;
                    drop(sc);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
    });
    // the user may be looking at the "choose a screen" dialog
    let (node, fd) = ready_rx.recv_timeout(Duration::from_secs(180)).map_err(|_| "Screen sharing didn't start".to_string())??;
    Ok(Cast { node, fd, stop: stop_tx })
}

// ---------------------------------------------------------------- pipelines

/// What to record. Coordinates are physical pixels.
#[derive(Clone, Copy, Debug)]
pub enum Area {
    /// X11: a rectangle of the root window.
    X11 { x: i32, y: i32, w: i32, h: i32 },
    /// Wayland: part of a monitor (`crop` within a `mon_w`×`mon_h` stream), or all of it.
    Monitor { crop: Option<(i32, i32, i32, i32)>, mon_w: i32, mon_h: i32 },
}

/// Placeholder for the output file in a pipeline description. gst-launch keeps
/// an argument with spaces in one piece, so the path is passed as its own argument.
const OUT: &str = "location=@OUT@";

/// gst-launch description: source → crop → scale → fixed fps → NV12 → H.264 → `sink`.
fn describe(area: Area, cast: Option<&Cast>, fps: u32, res: u32, enc: Encoder, sink: &str) -> (String, (u32, u32)) {
    let (src, crop, (sw, sh)) = match area {
        Area::X11 { x, y, w, h } => {
            let (w, h) = (w / 2 * 2, h / 2 * 2);
            (
                format!("ximagesrc startx={x} starty={y} endx={} endy={} use-damage=false show-pointer=true ! video/x-raw,framerate={fps}/1", x + w - 1, y + h - 1),
                String::new(),
                (w as u32, h as u32),
            )
        }
        Area::Monitor { crop, mon_w, mon_h } => {
            let c = cast.expect("a portal stream");
            let src = format!("pipewiresrc fd={} path={} do-timestamp=true", c.fd.as_raw_fd(), c.node);
            match crop {
                Some((l, t, w, h)) => {
                    let (w, h) = ((w / 2 * 2).max(64).min(mon_w), (h / 2 * 2).max(64).min(mon_h));
                    let (l, t) = (l.clamp(0, mon_w - w), t.clamp(0, mon_h - h));
                    let crop = format!(" ! videocrop left={l} top={t} right={} bottom={}", mon_w - l - w, mon_h - t - h);
                    (src, crop, (w as u32, h as u32))
                }
                None => (src, String::new(), (mon_w as u32, mon_h as u32)),
            }
        }
    };
    let (w, h) = scaled_size(sw, sh, res);
    let kbps = bitrate(w, h, fps) / 1000;
    let d = format!(
        "{src} ! queue max-size-buffers=8 leaky=downstream ! videoconvert{crop} ! videoscale ! video/x-raw,width={w},height={h},pixel-aspect-ratio=1/1 ! videorate ! video/x-raw,framerate={fps}/1 ! videoconvert ! video/x-raw,format=NV12 ! queue ! {} ! h264parse config-interval=-1 ! {sink}",
        enc.element(kbps, fps)
    );
    (d, (w, h))
}

fn log_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("aperture-gst.log")
}

fn last_error() -> String {
    let log = std::fs::read_to_string(log_path()).unwrap_or_default();
    log.lines().rev().find(|l| l.contains("ERROR") || l.contains("rror")).map(|l| l.trim().to_string()).unwrap_or_else(|| "The video pipeline stopped".into())
}

fn spawn(desc: &str, out: &Path, cast: Option<&Cast>) -> Result<Child, String> {
    if let Some(c) = cast {
        // let the child inherit the PipeWire connection
        unsafe {
            let fd = c.fd.as_raw_fd();
            let fl = libc::fcntl(fd, libc::F_GETFD);
            libc::fcntl(fd, libc::F_SETFD, fl & !libc::FD_CLOEXEC);
        }
    }
    let log = std::fs::File::create(log_path()).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("gst-launch-1.0");
    cmd.arg("-e").arg("-q");
    // gst-launch joins its arguments and parses them itself
    for tok in desc.split(' ').filter(|s| !s.is_empty()) {
        if tok == OUT {
            cmd.arg(format!("location={}", out.to_string_lossy()));
        } else {
            cmd.arg(tok);
        }
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
    cmd.spawn().map_err(|e| format!("Couldn't run GStreamer: {e}"))
}

/// Starts a pipeline, trying encoders until one keeps running.
fn launch(area: Area, cast: Option<&Cast>, fps: u32, res: u32, sink: &str, out: &Path) -> Result<(Child, (u32, u32)), String> {
    if let Some(m) = missing() {
        return Err(m.into());
    }
    let list: Vec<Encoder> = match WORKING.get() {
        Some(e) => vec![*e],
        None => encoders().to_vec(),
    };
    if list.is_empty() {
        return Err("No H.264 encoder found. Run: sudo apt install gstreamer1.0-plugins-ugly".into());
    }
    let mut err = String::new();
    for enc in list {
        let (desc, size) = describe(area, cast, fps, res, enc, sink);
        let mut child = spawn(&desc, out, cast)?;
        // a pipeline that can't start exits within a moment
        let t = Instant::now();
        let mut alive = true;
        while t.elapsed() < Duration::from_millis(1500) {
            if let Ok(Some(_)) = child.try_wait() {
                alive = false;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if alive {
            let _ = WORKING.set(enc);
            return Ok((child, size));
        }
        err = last_error();
    }
    Err(format!("Recording couldn't start: {err}"))
}

/// Ask gst-launch to finish (EOS, so the MP4 gets its index), then wait.
fn finish(mut child: Child) -> bool {
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(15) {
        if let Ok(Some(st)) = child.try_wait() {
            return st.success() || st.code().is_none();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

// ---------------------------------------------------------------- recordings

pub struct Recording {
    child: Child,
    _cast: Option<Cast>,
    pub path: PathBuf,
    pub start: Instant,
    /// the recorded rectangle in desktop pixels (X11 only, for the stop pill)
    pub rect: Option<(i32, i32, i32, i32)>,
}

impl Recording {
    /// The pipeline ended by itself (e.g. screen sharing was stopped from the desktop).
    pub fn ended(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

/// Starts recording in the background; the result arrives as `MediaEvent::Started`.
pub fn start_record(area: Area, path: PathBuf, fps: u32, res: u32) {
    std::thread::spawn(move || {
        let r = (|| {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            let cast = match area {
                Area::Monitor { .. } => Some(open_cast("record")?),
                Area::X11 { .. } => None,
            };
            let (child, _) = launch(area, cast.as_ref(), fps, res, &format!("mp4mux faststart=true ! filesink {OUT}"), &path)?;
            let rect = match area {
                Area::X11 { x, y, w, h } => Some((x, y, w, h)),
                _ => None,
            };
            Ok(Recording { child, _cast: cast, path: path.clone(), start: Instant::now(), rect })
        })();
        report(MediaEvent::Started(r));
    });
}

/// Finishes the file in the background; `MediaEvent::Saved` follows.
pub fn stop_record(r: Recording) {
    std::thread::spawn(move || {
        let secs = r.start.elapsed().as_secs_f64();
        let Recording { child, _cast, path, .. } = r;
        let ok = finish(child);
        drop(_cast);
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if ok && size > 0 || size > 4096 {
            report(MediaEvent::Saved { path, secs, clip: false });
        } else {
            report(MediaEvent::Error(format!("The recording couldn't be finished: {}", last_error())));
        }
    });
}

// ---------------------------------------------------------------- replay buffer

pub struct Replay {
    child: Child,
    _cast: Option<Cast>,
    dir: PathBuf,
    secs: u32,
    start: Instant,
}

impl Replay {
    pub fn ended(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

/// Starts the replay buffer; `MediaEvent::ReplayStarted` follows.
pub fn start_replay(area: Area, secs: u32, fps: u32, res: u32, dir: PathBuf) {
    std::thread::spawn(move || {
        let r = (|| {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let cast = match area {
                Area::Monitor { .. } => Some(open_cast("replay")?),
                Area::X11 { .. } => None,
            };
            let keep = secs.div_ceil(SEG_SECS) + 3;
            let sink = format!("splitmuxsink muxer-factory=mpegtsmux max-size-time={} max-files={keep} {OUT}", SEG_SECS as u64 * 1_000_000_000);
            let (child, _) = launch(area, cast.as_ref(), fps, res, &sink, &dir.join("seg_%06d.ts"))?;
            Ok(Replay { child, _cast: cast, dir: dir.clone(), secs, start: Instant::now() })
        })();
        report(MediaEvent::ReplayStarted(r));
    });
}

pub fn stop_replay(r: Replay) {
    std::thread::spawn(move || {
        let Replay { child, _cast, dir, .. } = r;
        finish(child);
        drop(_cast);
        let _ = std::fs::remove_dir_all(dir);
    });
}

/// Joins the newest segments covering the last `secs` into `out` (no re-encode).
pub fn save_clip(r: &Replay, out: PathBuf) {
    let mut segs: Vec<PathBuf> = std::fs::read_dir(&r.dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "ts")).collect())
        .unwrap_or_default();
    segs.sort();
    let n = r.secs.div_ceil(SEG_SECS) as usize + 1; // +1: the segment being written is partial
    let take: Vec<PathBuf> = segs.iter().rev().take(n).rev().cloned().collect();
    let secs = (r.secs as f64).min(r.start.elapsed().as_secs_f64());
    let dir = r.dir.clone();
    std::thread::spawn(move || {
        let res = (|| {
            if take.is_empty() {
                return Err("Nothing has been recorded yet".to_string());
            }
            // MPEG-TS segments can simply be appended to one another
            let joined = dir.join("clip.ts");
            let mut data = Vec::new();
            for p in &take {
                data.extend(std::fs::read(p).map_err(|e| e.to_string())?);
            }
            std::fs::write(&joined, data).map_err(|e| e.to_string())?;
            if let Some(d) = out.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let st = Command::new("gst-launch-1.0")
                .args(["-q", "-e", "filesrc", &format!("location={}", joined.to_string_lossy()), "!", "tsdemux", "!", "h264parse", "!", "mp4mux", "faststart=true", "!", "filesink"])
                .arg(format!("location={}", out.to_string_lossy()))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&joined);
            if st.success() && std::fs::metadata(&out).is_ok_and(|m| m.len() > 0) {
                Ok(())
            } else {
                Err("GStreamer couldn't write the MP4".to_string())
            }
        })();
        match res {
            Ok(()) => report(MediaEvent::Saved { path: out, secs, clip: true }),
            Err(e) => report(MediaEvent::Error(format!("Couldn't save the clip: {e}"))),
        }
    });
}
