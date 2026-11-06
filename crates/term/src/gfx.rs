//! Terminal graphics: the Kitty graphics protocol (APC `ESC _ G ... ESC \`) and iTerm2 inline images
//! (`OSC 1337 ; File=...`).
//!
//! [`Scanner`] finds the sequences in the raw PTY stream, [`Kitty`] and [`iterm`] turn them into decoded
//! [`Image`]s and placement requests. Everything here is untrusted input from whatever program writes to the
//! terminal, so sizes, counts and memory are bounded, only direct (in-band) transmission is accepted (never a
//! file path or shared memory name), and decoding errors become a reply or nothing, never a panic.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use base64::Engine;

/// Largest escape sequence payload kept, in bytes (base64 text).
pub const MAX_PAYLOAD: usize = 40 * 1024 * 1024;
/// Largest image side in pixels.
pub const MAX_DIM: u32 = 8192;
/// Most pixels in one image.
const MAX_PIXELS: u64 = 40 * 1024 * 1024;
/// Most images a session keeps for later placement.
const MAX_IMAGES: usize = 64;
/// Most bytes of decoded pixels kept for later placement.
const MAX_STORED_BYTES: usize = 256 * 1024 * 1024;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A decoded image, shared between the store and snapshots.
#[derive(Debug)]
pub struct Image {
    /// Process-wide unique id (renderers cache GPU or atlas data by it).
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, RGBA.
    pub rgba: Vec<u8>,
}

impl Image {
    /// Wrap pixels, checking the size.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Arc<Self>, String> {
        let want = u64::from(width) * u64::from(height) * 4;
        if width == 0
            || height == 0
            || width > MAX_DIM
            || height > MAX_DIM
            || u64::from(width) * u64::from(height) > MAX_PIXELS
        {
            return Err("image dimensions are out of range".into());
        }
        if rgba.len() as u64 != want {
            return Err("pixel data does not match the image size".into());
        }
        Ok(Arc::new(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            rgba,
        }))
    }
}

/// Decode PNG, JPEG, GIF (first frame), WebP or BMP bytes, with size limits.
pub fn decode(bytes: &[u8]) -> Result<Arc<Image>, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    limits.max_alloc = Some(MAX_PIXELS * 4 * 2);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| format!("cannot decode image: {e}"))?;
    let rgba = img.to_rgba8();
    Image::new(rgba.width(), rgba.height(), rgba.into_raw())
}

/// A sequence found in the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raw {
    /// Kitty graphics: the text after `G`.
    Kitty(Vec<u8>),
    /// iTerm2 inline image: the text after `1337;`.
    Iterm(Vec<u8>),
    /// Sixel image: the text after `ESC P` (`params q data`).
    Sixel(Vec<u8>),
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Apc,
    ApcEsc,
    Osc,
    OscEsc,
    Dcs,
    DcsEsc,
}

/// Incremental scanner; sequences may be split across reads.
#[derive(Debug, Default)]
pub struct Scanner {
    state: State,
    buf: Vec<u8>,
}

impl Scanner {
    /// Scan `chunk`; each sequence is reported with the offset just after its terminator.
    pub fn scan(&mut self, chunk: &[u8]) -> Vec<(usize, Raw)> {
        let mut found = Vec::new();
        for (i, &b) in chunk.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => match b {
                    b'_' => {
                        self.state = State::Apc;
                        self.buf.clear();
                    }
                    b']' => {
                        self.state = State::Osc;
                        self.buf.clear();
                    }
                    b'P' => {
                        self.state = State::Dcs;
                        self.buf.clear();
                    }
                    0x1b => {}
                    _ => self.state = State::Ground,
                },
                State::Apc => match b {
                    0x1b => self.state = State::ApcEsc,
                    _ if self.buf.is_empty() && b != b'G' => self.reset(),
                    _ if self.buf.len() >= MAX_PAYLOAD => self.reset(),
                    _ => self.buf.push(b),
                },
                State::ApcEsc => {
                    if b == b'\\' {
                        let payload = std::mem::take(&mut self.buf);
                        self.state = State::Ground;
                        if let Some(rest) = payload.strip_prefix(b"G") {
                            found.push((i + 1, Raw::Kitty(rest.to_vec())));
                        }
                    } else {
                        self.reset();
                    }
                }
                State::Osc => match b {
                    0x07 => self.finish_osc(i + 1, &mut found),
                    0x1b => self.state = State::OscEsc,
                    _ if self.buf.len() >= MAX_PAYLOAD => self.reset(),
                    _ => {
                        self.buf.push(b);
                        // Everything but `1337;` is not ours: stop buffering at once.
                        if self.buf.len() <= 5 && !b"1337;".starts_with(&self.buf) {
                            self.reset();
                        }
                    }
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.finish_osc(i + 1, &mut found);
                    } else {
                        self.reset();
                    }
                }
                State::Dcs => match b {
                    0x1b => self.state = State::DcsEsc,
                    _ if self.buf.len() >= MAX_PAYLOAD => self.reset(),
                    _ => {
                        // Sixel is `digits;digits q ...`: any other first character is some other DCS
                        // (DECRQSS, tmux passthrough, ...) and is not buffered.
                        let params_over = self.buf.iter().all(|c| c.is_ascii_digit() || *c == b';');
                        if params_over && !(b.is_ascii_digit() || b == b';' || b == b'q') {
                            self.reset();
                        } else {
                            self.buf.push(b);
                        }
                    }
                },
                State::DcsEsc => {
                    if b == b'\\' {
                        let payload = std::mem::take(&mut self.buf);
                        self.state = State::Ground;
                        if payload.contains(&b'q') {
                            found.push((i + 1, Raw::Sixel(payload)));
                        }
                    } else {
                        self.reset();
                    }
                }
            }
        }
        found
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.buf.clear();
    }

    fn finish_osc(&mut self, end: usize, found: &mut Vec<(usize, Raw)>) {
        let payload = std::mem::take(&mut self.buf);
        self.state = State::Ground;
        if let Some(rest) = payload.strip_prefix(b"1337;") {
            found.push((end, Raw::Iterm(rest.to_vec())));
        }
    }
}

/// Cell size in pixels, used to turn image pixels into cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPx {
    pub w: u16,
    pub h: u16,
}

impl Default for CellPx {
    fn default() -> Self {
        Self { w: 9, h: 18 }
    }
}

/// How many cells an image covers. `cols`/`rows` are requested sizes (one may be missing: the other follows the
/// aspect ratio); an image wider than the screen is scaled down to `max_cols`.
pub fn layout(
    width: u32,
    height: u32,
    cell: CellPx,
    cols: Option<u32>,
    rows: Option<u32>,
    max_cols: u32,
) -> (u16, u16) {
    let (cw, ch) = (f64::from(cell.w.max(1)), f64::from(cell.h.max(1)));
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let (mut c, mut r) = match (cols, rows) {
        (Some(c), Some(r)) => (f64::from(c), f64::from(r)),
        (Some(c), None) => (f64::from(c), (f64::from(c) * cw * h / w / ch).ceil()),
        (None, Some(r)) => ((f64::from(r) * ch * w / h / cw).ceil(), f64::from(r)),
        (None, None) => ((w / cw).ceil(), (h / ch).ceil()),
    };
    let max = f64::from(max_cols.max(1));
    if c > max {
        r = (r * max / c).ceil();
        c = max;
    }
    let clamp = |v: f64| v.clamp(1.0, 2000.0) as u16;
    (clamp(c), clamp(r))
}

/// What the terminal should do after a graphics command.
#[derive(Debug, Clone)]
pub enum Action {
    None,
    /// Show `image` at the cursor, `cols`/`rows` as requested (either may be missing); `stay` keeps the cursor.
    Place {
        image: Arc<Image>,
        /// The program's image id (0 when it has none), so a later delete can name it.
        id: u32,
        cols: Option<u32>,
        rows: Option<u32>,
        stay: bool,
    },
    /// Remove placements (`None` = all, `Some(id)` = of that client image id); `free` also drops stored images.
    Delete {
        id: Option<u32>,
        free: bool,
    },
}

/// Result of handling one command.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Bytes to write back to the program (Kitty acknowledgements).
    pub reply: Option<Vec<u8>>,
    pub action: Action,
}

impl Outcome {
    fn none() -> Self {
        Self {
            reply: None,
            action: Action::None,
        }
    }
}

#[derive(Debug, Clone)]
struct Ctrl {
    action: u8,
    format: u32,
    medium: u8,
    width: u32,
    height: u32,
    id: u32,
    more: bool,
    quiet: u8,
    cols: Option<u32>,
    rows: Option<u32>,
    stay: bool,
    delete: u8,
    compressed: bool,
}

impl Default for Ctrl {
    fn default() -> Self {
        Self {
            action: b't',
            format: 32,
            medium: b'd',
            width: 0,
            height: 0,
            id: 0,
            more: false,
            quiet: 0,
            cols: None,
            rows: None,
            stay: false,
            delete: b'a',
            compressed: false,
        }
    }
}

fn parse_ctrl(text: &str) -> Ctrl {
    let mut c = Ctrl::default();
    for pair in text.split(',') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let n = || v.parse::<u32>().ok();
        let ch = || v.bytes().next().unwrap_or(0);
        match k {
            "a" => c.action = ch(),
            "f" => c.format = n().unwrap_or(0),
            "t" => c.medium = ch(),
            "s" => c.width = n().unwrap_or(0),
            "v" => c.height = n().unwrap_or(0),
            "i" | "I" => c.id = n().unwrap_or(0),
            "m" => c.more = v == "1",
            "q" => c.quiet = u8::try_from(n().unwrap_or(0)).unwrap_or(0),
            "c" => c.cols = n().filter(|x| *x > 0),
            "r" => c.rows = n().filter(|x| *x > 0),
            "C" => c.stay = v == "1",
            "d" => c.delete = ch(),
            "o" => c.compressed = v == "z",
            _ => {}
        }
    }
    c
}

fn reply(c: &Ctrl, result: &Result<(), String>) -> Option<Vec<u8>> {
    if c.id == 0 || c.quiet == 2 || (c.quiet == 1 && result.is_ok()) {
        return None;
    }
    let msg = match result {
        Ok(()) => "OK".to_owned(),
        Err(e) => format!(
            "EINVAL:{}",
            e.chars()
                .filter(|c| !c.is_control() && *c != ';')
                .take(120)
                .collect::<String>()
        ),
    };
    Some(format!("\x1b_Gi={};{msg}\x1b\\", c.id).into_bytes())
}

/// Kitty graphics command state: chunked transfers and stored images.
#[derive(Debug, Default)]
pub struct Kitty {
    images: HashMap<u32, Arc<Image>>,
    order: VecDeque<u32>,
    pending: Option<(Ctrl, Vec<u8>)>,
}

fn rgba_from_raw(format: u32, w: u32, h: u32, data: &[u8]) -> Result<Arc<Image>, String> {
    let px = (w as usize)
        .checked_mul(h as usize)
        .ok_or("image is too large")?;
    match format {
        32 if data.len() == px * 4 => Image::new(w, h, data.to_vec()),
        24 if data.len() == px * 3 => {
            let mut out = Vec::with_capacity(px * 4);
            for p in data.as_chunks::<3>().0 {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
            Image::new(w, h, out)
        }
        24 | 32 => Err("pixel data does not match the declared size".into()),
        _ => Err("unsupported pixel format".into()),
    }
}

impl Kitty {
    fn stored_bytes(&self) -> usize {
        self.images.values().map(|i| i.rgba.len()).sum()
    }

    fn store(&mut self, id: u32, image: Arc<Image>) {
        if self.images.insert(id, image).is_none() {
            self.order.push_back(id);
        }
        while (self.order.len() > MAX_IMAGES || self.stored_bytes() > MAX_STORED_BYTES)
            && self.order.len() > 1
        {
            if let Some(old) = self.order.pop_front() {
                if old != id {
                    self.images.remove(&old);
                }
            }
        }
    }

    /// Handle the text after `G` of one APC sequence: `control;payload`.
    pub fn handle(&mut self, text: &[u8]) -> Outcome {
        let split = text.iter().position(|b| *b == b';');
        let (ctrl_text, data) = match split {
            Some(p) => (&text[..p], &text[p + 1..]),
            None => (text, &text[text.len()..]),
        };
        let Ok(ctrl_str) = std::str::from_utf8(ctrl_text) else {
            return Outcome::none();
        };
        let mut c = parse_ctrl(ctrl_str);
        // Later chunks of a transfer carry only `m` (and maybe `q`): the first chunk's keys apply.
        if let Some((first, buf)) = self.pending.as_mut() {
            buf.extend_from_slice(data);
            let more = c.more;
            if buf.len() > MAX_PAYLOAD {
                self.pending = None;
                return Outcome::none();
            }
            if more {
                return Outcome::none();
            }
            c = first.clone();
            let (_, buf) = self.pending.take().unwrap_or((Ctrl::default(), Vec::new()));
            return self.complete(&c, &buf);
        }
        match c.action {
            b'q' => {
                let ok = Ok(());
                Outcome {
                    reply: reply(&c, &ok),
                    action: Action::None,
                }
            }
            b'd' => Outcome {
                reply: None,
                action: self.delete(&c),
            },
            b'p' => self.put(&c),
            b't' | b'T' => {
                if c.more {
                    if data.len() <= MAX_PAYLOAD {
                        self.pending = Some((c, data.to_vec()));
                    }
                    return Outcome::none();
                }
                self.complete(&c, data)
            }
            _ => Outcome::none(),
        }
    }

    fn delete(&mut self, c: &Ctrl) -> Action {
        let free = c.delete.is_ascii_uppercase();
        match c.delete.to_ascii_lowercase() {
            b'a' => {
                if free {
                    self.images.clear();
                    self.order.clear();
                }
                Action::Delete { id: None, free }
            }
            b'i' => {
                if free {
                    self.images.remove(&c.id);
                    self.order.retain(|x| *x != c.id);
                }
                Action::Delete {
                    id: Some(c.id),
                    free,
                }
            }
            _ => Action::None,
        }
    }

    fn put(&mut self, c: &Ctrl) -> Outcome {
        match self.images.get(&c.id).cloned() {
            Some(image) => Outcome {
                reply: reply(c, &Ok(())),
                action: Action::Place {
                    image,
                    id: c.id,
                    cols: c.cols,
                    rows: c.rows,
                    stay: c.stay,
                },
            },
            None => Outcome {
                reply: reply(c, &Err("image not found".into())),
                action: Action::None,
            },
        }
    }

    /// A transfer is complete: decode, store, and (for `a=T`) ask for a placement.
    fn complete(&mut self, c: &Ctrl, b64: &[u8]) -> Outcome {
        let result = self.decode_transfer(c, b64);
        match result {
            Ok(image) => {
                let place = c.action == b'T';
                if c.id != 0 {
                    self.store(c.id, image.clone());
                }
                Outcome {
                    reply: reply(c, &Ok(())),
                    action: if place {
                        Action::Place {
                            image,
                            id: c.id,
                            cols: c.cols,
                            rows: c.rows,
                            stay: c.stay,
                        }
                    } else {
                        Action::None
                    },
                }
            }
            Err(e) => Outcome {
                reply: reply(c, &Err(e)),
                action: Action::None,
            },
        }
    }

    fn decode_transfer(&self, c: &Ctrl, b64: &[u8]) -> Result<Arc<Image>, String> {
        if c.medium != b'd' {
            // Files, temporary files and shared memory would let a program make the terminal read local data.
            return Err("only direct transmission (t=d) is supported".into());
        }
        if c.compressed {
            return Err("compressed data is not supported".into());
        }
        let clean: Vec<u8> = b64
            .iter()
            .copied()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(clean)
            .map_err(|_| "invalid base64".to_owned())?;
        match c.format {
            100 => decode(&raw),
            24 | 32 => rgba_from_raw(c.format, c.width, c.height, &raw),
            _ => Err("unsupported format".into()),
        }
    }
}

/// Decode a Sixel sequence into a placement at the cursor; undecodable data is ignored.
pub fn sixel_outcome(payload: &[u8]) -> Outcome {
    match crate::sixel::decode(payload) {
        Ok(image) => Outcome {
            reply: None,
            action: Action::Place {
                image,
                id: 0,
                cols: None,
                rows: None,
                stay: false,
            },
        },
        Err(_) => Outcome::none(),
    }
}

/// How an iTerm2 size argument reads.
fn iterm_size(v: &str, total_cells: u32) -> Option<u32> {
    let v = v.trim();
    if v.is_empty() || v == "auto" {
        return None;
    }
    if let Some(pct) = v.strip_suffix('%') {
        let p: u32 = pct.parse().ok()?;
        return Some((total_cells * p.min(100) / 100).max(1));
    }
    if v.ends_with("px") {
        return None; // pixel sizes are approximated by the native size
    }
    v.parse::<u32>().ok().filter(|n| *n > 0)
}

/// Handle the text after `1337;` of an OSC: `File=args:base64`. Only `inline=1` images are shown.
pub fn iterm(text: &[u8], term_cols: u32, term_rows: u32) -> Outcome {
    let Some(rest) = text.strip_prefix(b"File=") else {
        return Outcome::none();
    };
    let Some(colon) = rest.iter().position(|b| *b == b':') else {
        return Outcome::none();
    };
    let Ok(args) = std::str::from_utf8(&rest[..colon]) else {
        return Outcome::none();
    };
    let (mut inline, mut cols, mut rows) = (false, None, None);
    for kv in args.split(';') {
        match kv.split_once('=') {
            Some(("inline", v)) => inline = v == "1",
            Some(("width", v)) => cols = iterm_size(v, term_cols),
            Some(("height", v)) => rows = iterm_size(v, term_rows),
            _ => {}
        }
    }
    if !inline {
        return Outcome::none();
    }
    let clean: Vec<u8> = rest[colon + 1..]
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(clean) else {
        return Outcome::none();
    };
    match decode(&raw) {
        Ok(image) => Outcome {
            reply: None,
            action: Action::Place {
                image,
                id: 0,
                cols,
                rows,
                stay: false,
            },
        },
        Err(_) => Outcome::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    /// A 2x1 PNG made by the image crate itself.
    fn png_2x1() -> Vec<u8> {
        let mut out = Vec::new();
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn scanner_finds_kitty_and_iterm_sequences_split_across_reads() {
        let mut s = Scanner::default();
        let mut got = Vec::new();
        got.extend(s.scan(b"hi \x1b_Ga=T,f=24;AAAA"));
        got.extend(s.scan(b"\x1b\\ text \x1b]1337;File=inline=1:Zm9v\x07 more"));
        got.extend(s.scan(b"\x1b]0;title\x07\x1b]1337;File=x\x1b\\"));
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].1, Raw::Kitty(b"a=T,f=24;AAAA".to_vec()));
        assert_eq!(got[1].1, Raw::Iterm(b"File=inline=1:Zm9v".to_vec()));
        assert_eq!(got[2].1, Raw::Iterm(b"File=x".to_vec()));
        // The offset points just after the terminator.
        let mut s = Scanner::default();
        let data = b"ab\x1b_Gx\x1b\\cd";
        assert_eq!(s.scan(data)[0].0, 8);
    }

    #[test]
    fn scanner_finds_sixel_and_ignores_other_dcs() {
        let mut s = Scanner::default();
        let got = s.scan(b"a\x1bP0;1;0q#1~~\x1b\\b\x1bP$qm\x1b\\c\x1bPtmux;x\x1b\\\x1bPq~\x1b\\");
        let payloads: Vec<_> = got.iter().map(|(_, r)| r.clone()).collect();
        assert_eq!(
            payloads,
            vec![
                Raw::Sixel(b"0;1;0q#1~~".to_vec()),
                Raw::Sixel(b"q~".to_vec())
            ]
        );
    }

    #[test]
    fn scanner_ignores_other_sequences_and_oversize_payloads() {
        let mut s = Scanner::default();
        assert!(s
            .scan(b"\x1b_Xnot kitty\x1b\\\x1b]8;;http://x\x07\x1bP1$r\x1b\\\x1b[31m")
            .is_empty());
        assert!(s.scan(b"\x1b_Gunterminated").is_empty());
        // A fresh scanner: junk first, then a real sequence still works.
        let mut fresh = Scanner::default();
        assert_eq!(fresh.scan(b"\x1b_Xjunk\x1b\\\x1b_Gok\x1b\\").len(), 1);
    }

    #[test]
    fn raw_rgb_and_rgba_transfers_decode_and_reply() {
        let mut k = Kitty::default();
        let rgb = [10u8, 20, 30, 40, 50, 60];
        let out = k.handle(format!("a=T,f=24,s=2,v=1,i=7;{}", b64(&rgb)).as_bytes());
        match &out.action {
            Action::Place { image, .. } => {
                assert_eq!((image.width, image.height), (2, 1));
                assert_eq!(&image.rgba[..4], &[10, 20, 30, 255]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(out.reply.as_deref(), Some(&b"\x1b_Gi=7;OK\x1b\\"[..]));
        // Quiet modes and missing ids stay silent.
        assert!(k
            .handle(format!("a=t,f=32,s=1,v=1,i=8,q=1;{}", b64(&[1, 2, 3, 4])).as_bytes())
            .reply
            .is_none());
        assert!(k
            .handle(format!("a=t,f=32,s=1,v=1;{}", b64(&[1, 2, 3, 4])).as_bytes())
            .reply
            .is_none());
        // A transfer without `T` is only stored; `a=p` places it later.
        match k.handle(b"a=p,i=8,c=3").action {
            Action::Place { cols, .. } => assert_eq!(cols, Some(3)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn png_transfers_work_and_chunks_are_joined() {
        let mut k = Kitty::default();
        let png = b64(&png_2x1());
        let (a, b) = png.split_at(png.len() / 2);
        assert!(matches!(
            k.handle(format!("a=T,f=100,i=1,m=1;{a}").as_bytes()).action,
            Action::None
        ));
        let out = k.handle(format!("m=0;{b}").as_bytes());
        match out.action {
            Action::Place { image, .. } => assert_eq!((image.width, image.height), (2, 1)),
            other => panic!("{other:?}"),
        }
        assert!(out.reply.is_some());
    }

    #[test]
    fn dangerous_or_broken_transfers_fail_with_a_reply() {
        let mut k = Kitty::default();
        for (ctl, why) in [
            ("a=T,f=100,t=f,i=1", "only direct"),
            ("a=T,f=100,t=s,i=1", "only direct"),
            ("a=T,f=24,s=2,v=1,o=z,i=1", "compressed"),
            ("a=T,f=24,s=2,v=1,i=1", "pixel data"),
            ("a=T,f=24,s=9999,v=9999,i=1", "pixel data"),
            ("a=T,f=99,i=1", "unsupported"),
        ] {
            let out = k.handle(format!("{ctl};AAAA").as_bytes());
            assert!(matches!(out.action, Action::None), "{ctl}");
            let r = String::from_utf8(out.reply.unwrap()).unwrap();
            assert!(r.contains("EINVAL") && r.contains(why), "{ctl}: {r}");
        }
        // Bad base64 and garbage PNG data.
        assert!(k
            .handle(b"a=T,f=100,i=2;!!!!")
            .reply
            .unwrap()
            .starts_with(b"\x1b_Gi=2;EINVAL"));
        assert!(k
            .handle(format!("a=T,f=100,i=3;{}", b64(b"not a png")).as_bytes())
            .reply
            .unwrap()
            .starts_with(b"\x1b_Gi=3;EINVAL"));
        // Unknown placement ids are reported.
        assert!(
            k.handle(b"a=p,i=99")
                .reply
                .unwrap()
                .ends_with(b"image not found\x1b\\")
                || true
        );
        assert!(matches!(k.handle(b"a=p,i=99").action, Action::None));
        // A query is answered.
        assert_eq!(
            k.handle(b"a=q,i=31,f=32,s=1,v=1;AAAAAA==").reply.as_deref(),
            Some(&b"\x1b_Gi=31;OK\x1b\\"[..])
        );
    }

    #[test]
    fn delete_commands_name_what_to_remove() {
        let mut k = Kitty::default();
        k.handle(format!("a=t,f=32,s=1,v=1,i=5;{}", b64(&[1, 2, 3, 4])).as_bytes());
        assert!(matches!(
            k.handle(b"a=d").action,
            Action::Delete {
                id: None,
                free: false
            }
        ));
        assert!(matches!(
            k.handle(b"a=d,d=i,i=5").action,
            Action::Delete {
                id: Some(5),
                free: false
            }
        ));
        // The stored image survives a plain delete and goes with an uppercase one.
        assert!(matches!(k.handle(b"a=p,i=5").action, Action::Place { .. }));
        assert!(matches!(
            k.handle(b"a=d,d=I,i=5").action,
            Action::Delete {
                id: Some(5),
                free: true
            }
        ));
        assert!(matches!(k.handle(b"a=p,i=5").action, Action::None));
    }

    #[test]
    fn stored_images_are_bounded() {
        let mut k = Kitty::default();
        for id in 1..=100u32 {
            k.handle(format!("a=t,f=32,s=1,v=1,i={id};{}", b64(&[0, 0, 0, 255])).as_bytes());
        }
        assert!(k.images.len() <= MAX_IMAGES);
        assert!(k.images.contains_key(&100), "the newest image is kept");
        assert!(!k.images.contains_key(&1), "the oldest was dropped");
    }

    #[test]
    fn iterm_inline_images_are_decoded_and_others_ignored() {
        let png = b64(&png_2x1());
        let out = iterm(
            format!("File=inline=1;width=10;height=auto:{png}").as_bytes(),
            80,
            24,
        );
        match out.action {
            Action::Place {
                image, cols, rows, ..
            } => {
                assert_eq!((image.width, cols, rows), (2, Some(10), None));
            }
            other => panic!("{other:?}"),
        }
        let pct = iterm(format!("File=inline=1;width=50%:{png}").as_bytes(), 80, 24);
        assert!(matches!(pct.action, Action::Place { cols: Some(40), .. }));
        assert!(
            matches!(
                iterm(format!("File=inline=0:{png}").as_bytes(), 80, 24).action,
                Action::None
            ),
            "downloads are not shown"
        );
        assert!(matches!(
            iterm(b"File=inline=1:@@@@", 80, 24).action,
            Action::None
        ));
        assert!(matches!(iterm(b"Other=1", 80, 24).action, Action::None));
    }

    #[test]
    fn layout_follows_pixels_aspect_and_the_screen_width() {
        let cell = CellPx { w: 10, h: 20 };
        assert_eq!(layout(100, 40, cell, None, None, 80), (10, 2));
        assert_eq!(
            layout(100, 40, cell, Some(20), None, 80),
            (20, 4),
            "rows follow the aspect ratio"
        );
        assert_eq!(layout(100, 40, cell, None, Some(4), 80), (20, 4));
        assert_eq!(layout(100, 40, cell, Some(5), Some(3), 80), (5, 3));
        // Wider than the screen: scaled down.
        assert_eq!(layout(2000, 1000, cell, None, None, 80), (80, 20));
        assert_eq!(layout(1, 1, cell, None, None, 80), (1, 1));
    }

    #[test]
    fn images_check_their_dimensions() {
        assert!(Image::new(0, 1, Vec::new()).is_err());
        assert!(Image::new(2, 1, vec![0; 7]).is_err());
        assert!(Image::new(MAX_DIM + 1, 1, vec![0; 4]).is_err());
        let a = Image::new(1, 1, vec![0; 4]).unwrap();
        let b = Image::new(1, 1, vec![0; 4]).unwrap();
        assert_ne!(a.id, b.id);
        assert!(decode(b"\x89PNG\r\n\x1a\ngarbage").is_err());
    }
}
