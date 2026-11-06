//! `tf play`: play video and audio in the terminal, any format ffmpeg reads, from a file or a link.
//!
//! Video frames are decoded by `ffmpeg` (in its own process) as raw RGB at a modest size and fed to the terminal with the
//! Kitty graphics protocol, each frame replacing the last. Sound comes from `ffplay` (no window). Nothing is downloaded to
//! disk. A web link is only opened after the user confirmed it (the address is shown), ffmpeg is limited to the protocols
//! the source needs, and every external program is a plain argument list, never a shell line.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::img;

/// Widest decoded video frame, in pixels.
const MAX_WIDTH: u32 = 640;
/// Frames per second shown (the terminal is not a cinema: smooth enough, cheap enough).
const DEFAULT_FPS: u32 = 15;
/// Seek step of the arrow keys, in seconds.
const SEEK_STEP: f64 = 10.0;
/// Image id used for the video frames.
const FRAME_ID: u32 = 90_001;

const INSTALL_HINT: &str = "install ffmpeg: Debian/Ubuntu `sudo apt install ffmpeg`, Fedora `sudo dnf install ffmpeg`, macOS `brew install ffmpeg`";

/// What `ffprobe` found out about a source.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub duration: Option<f64>,
    pub video: Option<(u32, u32)>,
    pub has_audio: bool,
    pub fps: f64,
}

/// Is `s` a web link?
pub fn is_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Protocols ffmpeg may use for this source: just `file` for a local file, the web ones for a link.
pub fn protocol_whitelist(src: &str) -> &'static str {
    if is_url(src) {
        "file,http,https,tcp,tls,crypto"
    } else {
        "file"
    }
}

/// Parse an `ffprobe -of json` answer.
pub fn parse_probe(json: &str) -> Option<Info> {
    let v: Value = serde_json::from_str(json).ok()?;
    let duration = v
        .pointer("/format/duration")
        .and_then(|d| {
            d.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| d.as_f64())
        })
        .filter(|d| d.is_finite() && *d > 0.0);
    let streams = v.get("streams")?.as_array()?;
    let mut video = None;
    let mut fps = f64::from(DEFAULT_FPS);
    let mut has_audio = false;
    for s in streams {
        match s.get("codec_type").and_then(Value::as_str) {
            Some("video") if video.is_none() => {
                let w = u32::try_from(s.get("width")?.as_u64()?).ok()?;
                let h = u32::try_from(s.get("height")?.as_u64()?).ok()?;
                // Cover art in audio files is a "video" stream of one frame: it has no frame rate.
                let rate = s
                    .get("avg_frame_rate")
                    .and_then(Value::as_str)
                    .unwrap_or("0/0");
                let (n, d) = rate.split_once('/').unwrap_or((rate, "1"));
                let r = n.parse::<f64>().unwrap_or(0.0) / d.parse::<f64>().unwrap_or(1.0).max(1.0);
                if w > 0 && h > 0 && r > 0.5 {
                    video = Some((w, h));
                    fps = r;
                }
            }
            Some("audio") => has_audio = true,
            _ => {}
        }
    }
    if video.is_none() && !has_audio {
        return None;
    }
    Some(Info {
        duration,
        video,
        has_audio,
        fps,
    })
}

/// Frame size to decode: at most [`MAX_WIDTH`] wide, even numbers, aspect ratio kept.
pub fn frame_size(w: u32, h: u32) -> (u32, u32) {
    let fw = w.clamp(2, MAX_WIDTH) & !1;
    let fh = ((u64::from(h) * u64::from(fw) / u64::from(w.max(1))) as u32).max(2) & !1;
    (fw, fh)
}

/// `65` becomes `1:05`, `3725` becomes `1:02:05`.
pub fn clock_text(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A key that controls playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Pause,
    Quit,
    Back,
    Forward,
    /// Fill the pane with the picture (and back).
    Fullscreen,
}

/// Columns for a picture of `frame` pixels in a pane of `pane` cells (columns, rows): never wider than `wanted`, and
/// tall enough to fit `rows_free` rows at the usual cell proportions (a cell is about half as wide as it is tall).
/// `full` lifts the usual width limit and uses the whole pane.
pub fn fit_cols(
    pane: (u32, u32),
    rows_free: u32,
    frame: (u32, u32),
    wanted: u32,
    full: bool,
) -> u32 {
    let width_limit = if full { pane.0 } else { wanted.min(pane.0) };
    // rows = cols * 0.5 * h / w  =>  cols = rows * 2 * w / h
    let by_height =
        (u64::from(rows_free) * 2 * u64::from(frame.0) / u64::from(frame.1.max(1))) as u32;
    width_limit.min(by_height.max(8)).max(8)
}

/// Turn the bytes of a terminal key press (`q`, space, `ESC [ C`) into a control key.
pub fn parse_keys(bytes: &[u8]) -> Vec<Key> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'p' | b'k' => out.push(Key::Pause),
            b'q' | 3 => out.push(Key::Quit),
            b'h' | b'j' => out.push(Key::Back),
            b'l' => out.push(Key::Forward),
            b'f' | b'F' => out.push(Key::Fullscreen),
            0x1b if bytes.get(i + 1) == Some(&b'[') => {
                match bytes.get(i + 2) {
                    Some(b'D') => out.push(Key::Back),
                    Some(b'C') => out.push(Key::Forward),
                    _ => {}
                }
                i += 2;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// How the terminal is put in raw mode and back, through `stty` (the terminal keeps its own settings otherwise).
struct RawMode {
    saved: Option<String>,
}

impl RawMode {
    fn enter() -> Self {
        let saved = Command::new("stty")
            .arg("-g")
            .stdin(Stdio::inherit())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
        if saved.is_some() {
            let _ = Command::new("stty")
                .args(["-icanon", "-echo", "min", "1"])
                .stdin(Stdio::inherit())
                .status();
        }
        Self { saved }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some(s) = &self.saved {
            let _ = Command::new("stty").arg(s).stdin(Stdio::inherit()).status();
        }
    }
}

fn which(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Ask ffprobe about the source.
pub fn probe(src: &str) -> Result<Info, String> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            protocol_whitelist(src),
        ])
        .args([
            "-show_entries",
            "format=duration:stream=codec_type,width,height,avg_frame_rate",
            "-of",
            "json",
        ])
        .arg("-i")
        .arg(src)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("ffprobe was not found; {INSTALL_HINT}")
            } else {
                e.to_string()
            }
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("not a media file");
        return Err(format!(
            "cannot play {src}: {}",
            first
                .chars()
                .filter(|c| !c.is_control())
                .take(160)
                .collect::<String>()
        ));
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| format!("{src} has no audio or video stream"))
}

/// Playback options.
#[derive(Debug, Clone)]
pub struct Options {
    pub start: f64,
    pub fps: Option<u32>,
    pub cols: u32,
    pub audio: bool,
    pub yes: bool,
    /// Stop after this many frames (tests and thumbnails).
    pub max_frames: Option<u64>,
    pub keys: bool,
}

struct Running {
    video: Option<Child>,
    audio: Option<Child>,
}

impl Running {
    fn stop(&mut self) {
        for c in [self.video.as_mut(), self.audio.as_mut()]
            .into_iter()
            .flatten()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.video = None;
        self.audio = None;
    }

    fn signal_audio(&self, sig: &str) {
        if let Some(a) = &self.audio {
            let _ = Command::new("kill")
                .args([sig, &a.id().to_string()])
                .status();
        }
    }
}

fn spawn_video(src: &str, start: f64, fps: u32, size: (u32, u32)) -> Result<Child, String> {
    Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            protocol_whitelist(src),
        ])
        .args(["-re", "-ss", &format!("{start:.2}"), "-i"])
        .arg(src)
        .args([
            "-an",
            "-vf",
            &format!("fps={fps},scale={}:{}", size.0, size.1),
        ])
        .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("ffmpeg was not found; {INSTALL_HINT}")
            } else {
                e.to_string()
            }
        })
}

fn spawn_audio(src: &str, start: f64) -> Option<Child> {
    Command::new("ffplay")
        .args([
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "quiet",
            "-protocol_whitelist",
            protocol_whitelist(src),
        ])
        .args(["-ss", &format!("{start:.2}"), "-i"])
        .arg(src)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

/// Read exactly one frame; false at the end of the stream.
fn read_frame(from: &mut impl Read, buf: &mut [u8]) -> bool {
    from.read_exact(buf).is_ok()
}

/// Where the playback is: the source position where the current ffmpeg run started plus what it has shown since.
struct Clock {
    run_start: f64,
    frames: u64,
    audio_secs: f64,
    fps: u32,
    video: bool,
}

impl Clock {
    fn now(&self) -> f64 {
        self.run_start
            + if self.video {
                self.frames as f64 / f64::from(self.fps.max(1))
            } else {
                self.audio_secs
            }
    }

    fn restart_at(&mut self, pos: f64) {
        self.run_start = pos;
        self.frames = 0;
        self.audio_secs = 0.0;
    }
}

/// Play `src`. `keys` delivers control keys (a thread reading the terminal, or a test). Returns the frames shown.
pub fn play(
    src: &str,
    info: &Info,
    opts: &Options,
    out: &mut impl Write,
    keys: &Receiver<Key>,
) -> Result<u64, String> {
    let fps = opts
        .fps
        .unwrap_or_else(|| (info.fps.round() as u32).clamp(5, DEFAULT_FPS));
    let size = info.video.map(|(w, h)| frame_size(w, h));
    let mut buf = size
        .map(|(w, h)| vec![0u8; (w * h * 3) as usize])
        .unwrap_or_default();
    // Leave room for the picture (its rows, estimated from a typical cell) and put the cursor back at its top: frames
    // are drawn there without moving the cursor, the status line goes under the picture.
    let pane = terminal_size::terminal_size()
        .map_or((opts.cols, 40), |(w, h)| (u32::from(w.0), u32::from(h.0)));
    let rows_for = |c: u32, f: (u32, u32)| {
        (f64::from(c) * 9.0 * f64::from(f.1) / f64::from(f.0) / 18.0).ceil() as u32
    };
    let mut full = false;
    let mut cols = opts.cols;
    let mut rows = 0usize;
    if let Some(sz) = size {
        // A tall picture must not be taller than the pane: the status line and the prompt stay visible.
        cols = fit_cols(pane, pane.1.saturating_sub(3).max(4), sz, opts.cols, false);
        rows = rows_for(cols, sz) as usize + 1;
        write!(out, "{}\x1b[{rows}A", "\n".repeat(rows)).map_err(|e| e.to_string())?;
    }
    let mut clock = Clock {
        run_start: opts.start.max(0.0),
        frames: 0,
        audio_secs: 0.0,
        fps,
        video: size.is_some(),
    };
    let (mut shown, mut paused, mut restart) = (0u64, false, true);
    let mut run = Running {
        video: None,
        audio: None,
    };
    let mut last_status = Instant::now() - Duration::from_secs(1);
    loop {
        if restart {
            run.stop();
            run.video = match size {
                Some(sz) => Some(spawn_video(src, clock.run_start, fps, sz)?),
                None => None,
            };
            if opts.audio && info.has_audio {
                run.audio = spawn_audio(src, clock.run_start);
            }
            clock.restart_at(clock.run_start);
            restart = false;
        }
        // Controls.
        loop {
            match keys.try_recv() {
                Ok(Key::Quit) | Err(TryRecvError::Disconnected) => {
                    run.stop();
                    return Ok(shown);
                }
                Ok(Key::Pause) => {
                    paused = !paused;
                    run.signal_audio(if paused { "-STOP" } else { "-CONT" });
                }
                Ok(Key::Fullscreen) => {
                    if let Some(sz) = size {
                        full = !full;
                        // Wipe the pane and lay the picture out again: centred over the whole pane, or back at the
                        // top of the scrolling layout.
                        write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\\x1b[2J\x1b[3J\x1b[H")
                            .map_err(|e| e.to_string())?;
                        if full {
                            cols = fit_cols(
                                pane,
                                pane.1.saturating_sub(1).max(4),
                                sz,
                                opts.cols,
                                true,
                            );
                            rows = pane.1.saturating_sub(1).max(1) as usize;
                            let left = pane.0.saturating_sub(cols) / 2;
                            write!(out, "\x1b[1;{}H", left + 1).map_err(|e| e.to_string())?;
                        } else {
                            cols = fit_cols(
                                pane,
                                pane.1.saturating_sub(3).max(4),
                                sz,
                                opts.cols,
                                false,
                            );
                            rows = rows_for(cols, sz) as usize + 1;
                            write!(out, "{}\x1b[{rows}A", "\n".repeat(rows))
                                .map_err(|e| e.to_string())?;
                        }
                        out.flush().map_err(|e| e.to_string())?;
                    }
                }
                Ok(k @ (Key::Back | Key::Forward)) => {
                    let step = if k == Key::Forward {
                        SEEK_STEP
                    } else {
                        -SEEK_STEP
                    };
                    let end = info.duration.map_or(f64::MAX, |d| (d - 1.0).max(0.0));
                    clock.run_start = (clock.now() + step).clamp(0.0, end);
                    paused = false;
                    restart = true;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if restart {
            continue;
        }
        if paused {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        // A frame, or the end.
        let ended = match run.video.as_mut().and_then(|c| c.stdout.as_mut()) {
            Some(stdout) => {
                if read_frame(stdout, &mut buf) {
                    let (w, h) = size.unwrap_or((2, 2));
                    out.write_all(&img::kitty_rgb_frame(w, h, &buf, cols, FRAME_ID))
                        .map_err(|e| e.to_string())?;
                    shown += 1;
                    clock.frames += 1;
                    false
                } else {
                    true
                }
            }
            None => {
                // Audio only: time passes by itself until the sound ends.
                std::thread::sleep(Duration::from_millis(100));
                clock.audio_secs += 0.1;
                run.audio
                    .as_mut()
                    .is_none_or(|a| a.try_wait().ok().flatten().is_some())
                    || info.duration.is_some_and(|d| clock.now() >= d)
            }
        };
        if last_status.elapsed() >= Duration::from_millis(250) || ended {
            let total = info
                .duration
                .map_or_else(String::new, |d| format!(" / {}", clock_text(d)));
            write!(
                out,
                "\x1b7\x1b[{}B\r\x1b[2K{} {}{total}   space pause \u{b7} \u{2190}/\u{2192} seek \u{b7} f fullscreen \u{b7} q quit\x1b8",
                rows.max(1),
                if paused { "\u{23f8}" } else { "\u{25b6}" },
                self::clock_text(clock.now()),
            )
            .map_err(|e| e.to_string())?;
            last_status = Instant::now();
        }
        out.flush().map_err(|e| e.to_string())?;
        if ended || opts.max_frames.is_some_and(|m| shown >= m) {
            run.stop();
            return Ok(shown);
        }
    }
}

/// Resolve a web link to a direct stream address with `yt-dlp` when the page is not itself media.
fn resolve_link(url: &str) -> Result<String, String> {
    let direct = [
        "mp4", "mkv", "webm", "mov", "avi", "mp3", "flac", "ogg", "m4a", "wav", "m3u8", "ts", "gif",
    ];
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if direct.iter().any(|e| path.ends_with(&format!(".{e}"))) {
        return Ok(url.to_owned());
    }
    let out = Command::new("yt-dlp")
        .args(["--no-playlist", "-g", "-f", "best[height<=720]/best", "--"])
        .arg(url)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "this page needs yt-dlp to find the video (install it, or give a direct media link)"
                    .to_owned()
            } else {
                e.to_string()
            }
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "yt-dlp: {}",
            err.lines()
                .last()
                .unwrap_or("failed")
                .chars()
                .take(160)
                .collect::<String>()
        ));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| is_url(l))
        .map(str::to_owned)
        .ok_or_else(|| "yt-dlp found no playable address".to_owned())
}

/// The entry point of `tf play`.
pub fn run(args: &[String]) -> Result<(), String> {
    let mut o = Options {
        start: 0.0,
        fps: None,
        cols: terminal_size::terminal_size()
            .map_or(80, |(w, _)| u32::from(w.0).clamp(20, 120))
            .min(100),
        audio: true,
        yes: false,
        max_frames: None,
        keys: true,
    };
    let mut src = None::<String>;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--start" => {
                o.start = it
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|s| *s >= 0.0)
                    .ok_or("--start needs seconds")?
            }
            "--fps" => {
                o.fps = Some(
                    it.next()
                        .and_then(|v| v.parse::<u32>().ok())
                        .filter(|f| (1..=60).contains(f))
                        .ok_or("--fps must be 1 to 60")?,
                )
            }
            "--width" => {
                o.cols = it
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .filter(|c| (10..=300).contains(c))
                    .ok_or("--width must be 10 to 300 columns")?
            }
            "--no-audio" => o.audio = false,
            "--yes" => o.yes = true,
            "--frames" => {
                o.max_frames = Some(
                    it.next()
                        .and_then(|v| v.parse::<u64>().ok())
                        .filter(|n| *n > 0)
                        .ok_or("--frames needs a number")?,
                )
            }
            "--no-keys" => o.keys = false,
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f if src.is_none() => src = Some(f.to_owned()),
            _ => return Err("play takes one file or link".into()),
        }
    }
    let src = src.ok_or("play needs a media file or a link")?;
    if !std::io::stdout().is_terminal() && o.max_frames.is_none() {
        return Err("play draws into a terminal".into());
    }
    if !which("ffmpeg") {
        return Err(format!("ffmpeg was not found; {INSTALL_HINT}"));
    }
    let src = if is_url(&src) {
        if !o.yes {
            eprintln!(
                "This will open {src}\n(through ffmpeg{}). Continue? [y/N] ",
                if src.contains("youtube") || src.contains("youtu.be") {
                    " and yt-dlp"
                } else {
                    ""
                }
            );
            let mut ans = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut ans)
                .map_err(|e| e.to_string())?;
            if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                return Err("cancelled".into());
            }
        }
        resolve_link(&src)?
    } else {
        if !Path::new(&src).is_file() {
            return Err(format!("{src}: no such file"));
        }
        src
    };
    let info = probe(&src)?;
    let (tx, rx) = mpsc::channel();
    let _raw = (o.keys && std::io::stdin().is_terminal()).then(RawMode::enter);
    if o.keys && std::io::stdin().is_terminal() {
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut b = [0u8; 8];
            while let Ok(n) = stdin.read(&mut b) {
                if n == 0 {
                    break;
                }
                for k in parse_keys(&b[..n]) {
                    if tx.send(k).is_err() {
                        return;
                    }
                }
            }
        });
    } else {
        // No keyboard: never ends by itself unless the media does; keep the sender alive for the whole run.
        std::mem::forget(tx);
    }
    let mut out = std::io::stdout().lock();
    let frames = play(&src, &info, &o, &mut out, &rx)?;
    writeln!(out, "\n").ok();
    let _ = frames;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_answers_are_understood() {
        let video = r#"{"streams":[{"codec_type":"video","width":1920,"height":1080,"avg_frame_rate":"30000/1001"},{"codec_type":"audio"}],"format":{"duration":"125.5"}}"#;
        let i = parse_probe(video).unwrap();
        assert_eq!(
            (i.video, i.has_audio, i.duration),
            (Some((1920, 1080)), true, Some(125.5))
        );
        assert!((i.fps - 29.97).abs() < 0.01);
        // Cover art in an audio file is not video.
        let art = r#"{"streams":[{"codec_type":"audio"},{"codec_type":"video","width":500,"height":500,"avg_frame_rate":"0/0"}],"format":{"duration":"10"}}"#;
        assert_eq!(parse_probe(art).unwrap().video, None);
        assert!(parse_probe(r#"{"streams":[{"codec_type":"subtitle"}],"format":{}}"#).is_none());
        assert!(parse_probe("not json").is_none());
        assert!(
            parse_probe(r#"{"streams":[{"codec_type":"audio"}],"format":{}}"#)
                .unwrap()
                .duration
                .is_none()
        );
    }

    #[test]
    fn sizes_clocks_keys_and_links() {
        assert_eq!(frame_size(1920, 1080), (640, 360));
        assert_eq!(
            frame_size(320, 240),
            (320, 240),
            "small videos are not enlarged"
        );
        assert_eq!(frame_size(641, 481).0 % 2, 0);
        assert_eq!(
            (clock_text(65.0), clock_text(3725.0), clock_text(-5.0)),
            ("1:05".to_owned(), "1:02:05".to_owned(), "0:00".to_owned())
        );
        assert_eq!(parse_keys(b" q"), [Key::Pause, Key::Quit]);
        assert_eq!(parse_keys(b"\x1b[C\x1b[D"), [Key::Forward, Key::Back]);
        assert_eq!(parse_keys(b"\x03"), [Key::Quit]);
        assert!(parse_keys(b"zzz\x1b[Z").is_empty());
        assert!(
            is_url("HTTPS://example.com/a.mp4")
                && !is_url("/home/me/a.mp4")
                && !is_url("file:///etc/passwd")
        );
        assert_eq!(
            protocol_whitelist("/tmp/a.mp4"),
            "file",
            "a local file may not pull in the network"
        );
        assert!(protocol_whitelist("https://x.y/z").contains("https"));
    }

    fn ffmpeg_here() -> bool {
        which("ffmpeg") && which("ffprobe")
    }

    /// A 2 second test video made by ffmpeg itself.
    fn sample() -> Option<std::path::PathBuf> {
        if !ffmpeg_here() {
            return None;
        }
        let p = std::env::temp_dir().join(format!(
            "mz-play-{}-{:?}.mp4",
            std::process::id(),
            std::thread::current().id()
        ));
        let ok = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x90:rate=10:duration=2",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-shortest",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(p)
    }

    #[test]
    fn a_real_video_plays_frames_into_the_terminal_stream() {
        let Some(file) = sample() else { return };
        let src = file.display().to_string();
        let info = probe(&src).unwrap();
        assert_eq!(info.video, Some((160, 90)));
        assert!(info.has_audio && info.duration.is_some());
        let (_tx, rx) = mpsc::channel();
        let opts = Options {
            start: 0.0,
            fps: Some(10),
            cols: 40,
            audio: false,
            yes: true,
            max_frames: Some(4),
            keys: false,
        };
        let mut out = Vec::new();
        let n = play(&src, &info, &opts, &mut out, &rx).unwrap();
        assert_eq!(n, 4);
        // The stream holds four Kitty frames, all with the same image id (each replaces the last).
        let mut scanner = mahzenterm_term::gfx::Scanner::default();
        let mut kitty = mahzenterm_term::gfx::Kitty::default();
        let mut placed = 0;
        for (_, raw) in scanner.scan(&out) {
            if let mahzenterm_term::gfx::Raw::Kitty(t) = raw {
                if let mahzenterm_term::gfx::Action::Place { image, id, .. } =
                    kitty.handle(&t).action
                {
                    assert_eq!((image.width, image.height, id), (160, 90, FRAME_ID));
                    placed += 1;
                }
            }
        }
        assert_eq!(placed, 4);
        // Quit stops it at once.
        let (qtx, qrx) = mpsc::channel();
        qtx.send(Key::Quit).unwrap();
        let mut sink = Vec::new();
        let opts = Options {
            max_frames: None,
            ..opts
        };
        assert_eq!(play(&src, &info, &opts, &mut sink, &qrx).unwrap(), 0);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn bad_sources_are_explained() {
        assert!(probe("/definitely/not/here.mp4").is_err());
        let junk = std::env::temp_dir().join(format!("mz-play-junk-{}.mp4", std::process::id()));
        std::fs::write(&junk, "this is not a video").unwrap();
        if ffmpeg_here() {
            assert!(probe(&junk.display().to_string())
                .unwrap_err()
                .contains("cannot play"));
        }
        let _ = std::fs::remove_file(junk);
        assert!(run(&["--bogus".into()])
            .unwrap_err()
            .contains("unknown option"));
        assert!(run(&[]).unwrap_err().contains("needs a media file"));
    }

    #[test]
    fn f_toggles_fullscreen_and_the_picture_fills_the_pane_without_overflowing() {
        assert_eq!(parse_keys(b"f"), vec![Key::Fullscreen]);
        assert_eq!(parse_keys(b"Fq"), vec![Key::Fullscreen, Key::Quit]);
        // 16:9 in a 112x58 pane. Normal playback keeps its width limit; fullscreen uses the pane width...
        let (pane, frame) = ((112, 58), (640, 360));
        assert_eq!(fit_cols(pane, 55, frame, 100, false), 100);
        assert_eq!(fit_cols(pane, 56, frame, 100, true), 112);
        // ...but never more rows than there are: a short pane narrows the picture instead.
        let wide = fit_cols((200, 20), 19, frame, 100, true);
        assert!(wide <= 19 * 2 * 640 / 360, "{wide}");
        // A tall (portrait) video is narrowed to fit the height.
        let tall = fit_cols((112, 58), 55, (360, 640), 100, true);
        assert!(tall < 70, "{tall}");
        assert!(
            fit_cols((10, 4), 1, frame, 100, true) >= 8,
            "never absurdly small"
        );
    }
}
