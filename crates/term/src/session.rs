//! A running terminal session: child process on a PTY plus VT emulation state.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Direction as SearchDirection;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, Processor};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use thiserror::Error;

use crate::gfx::{self, Action as GfxAction, CellPx, Image, Kitty};
use crate::keys::{encode_key, Key, Mods};
use crate::osc133::{self, CwdReport, Scanner};
use crate::palette::{Palette, Rgb};

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to open pty: {0}")]
    Pty(String),
    #[error("failed to start `{command}`: {reason}")]
    Spawn { command: String, reason: String },
}

/// Terminal size in cells plus cell pixel size (reported to programs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Size {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            cell_width: 8,
            cell_height: 16,
        }
    }
}

struct Dims {
    cols: usize,
    rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// How to start a session.
#[derive(Default)]
pub struct SpawnOptions {
    /// Program and arguments; `None` starts the platform default shell.
    pub command: Option<Vec<String>>,
    pub cwd: Option<PathBuf>,
    pub palette: Palette,
    /// Extra environment variables for the child (a profile's), applied after the terminal's own.
    pub env: Vec<(String, String)>,
}

/// Display attributes of a cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CellFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub wide: bool,
}

/// One resolved screen cell. Wide-char spacer cells have `ch == '\0'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub flags: CellFlags,
    /// Part of the current mouse selection.
    pub selected: bool,
    /// Search highlight state.
    pub mark: Mark,
    /// 0 for none, else 1 + index into [`Snapshot::links`] (an OSC 8 hyperlink).
    pub link: u16,
}

/// How a cell relates to the active search.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mark {
    #[default]
    None,
    /// Part of some match.
    Found,
    /// Part of the match the user is on.
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShapeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShapeKind {
    Block,
    Underline,
    Beam,
    Hollow,
}

/// An image anchored to the text: `line` counts from the top of the scrollback, so it scrolls with the text.
#[derive(Debug, Clone)]
struct Placement {
    image: Arc<Image>,
    /// The program's image id (0 when it has none).
    client_id: u32,
    line: i64,
    col: u16,
    cols: u16,
    rows: u16,
    /// Placed on the alternate screen (dropped when that screen is left).
    alt: bool,
}

/// An image to draw: its top-left cell on screen (the row may be above the screen when it is partly scrolled out)
/// and its size in cells.
#[derive(Debug, Clone)]
pub struct ImageView {
    pub image: Arc<Image>,
    pub col: u16,
    pub row: i32,
    pub cols: u16,
    pub rows: u16,
}

/// Most images kept on screen at once.
const MAX_PLACEMENTS: usize = 256;

/// Immutable copy of what is on screen, ready for rendering.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    /// Row-major, `rows * cols` cells.
    pub cells: Vec<Cell>,
    pub cursor: Option<Cursor>,
    pub bg: Rgb,
    /// Cursor colour from the active palette.
    pub cursor_color: Rgb,
    pub display_offset: usize,
    /// Targets of OSC 8 hyperlinks on screen, indexed by `Cell::link - 1`.
    pub links: Vec<String>,
    /// Images to draw over the cells they cover.
    pub images: Vec<ImageView>,
}

/// A link found on screen: its cells and target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    pub row: u16,
    pub start: u16,
    /// Last column, inclusive.
    pub end: u16,
    pub url: String,
}

/// Schemes that may be opened from the terminal.
pub const OPENABLE_SCHEMES: &[&str] = &["https://", "http://", "ftp://", "mailto:"];

/// Whether `url` starts with a scheme MahzenTerm is willing to open.
pub fn is_openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    OPENABLE_SCHEMES.iter().any(|s| lower.starts_with(s))
}

/// Find a plain-text URL covering `at` in `chars`: returns its inclusive column range.
fn scan_url(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let stop = |c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`' | '\\' | '|');
    if at >= chars.len() || stop(chars[at]) {
        return None;
    }
    let (mut a, mut b) = (at, at);
    while a > 0 && !stop(chars[a - 1]) {
        a -= 1;
    }
    while b + 1 < chars.len() && !stop(chars[b + 1]) {
        b += 1;
    }
    while a <= b && matches!(chars[a], '(' | '[' | '{' | '\'') {
        a += 1;
    }
    let token = |a: usize, b: usize| chars[a..=b].iter().collect::<String>();
    loop {
        if a > b {
            return None;
        }
        let last = chars[b];
        let t = token(a, b);
        let unbalanced = |open: char, close: char| {
            last == close && t.matches(close).count() > t.matches(open).count()
        };
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*')
            || unbalanced('(', ')')
            || unbalanced('[', ']')
            || unbalanced('{', '}')
        {
            if b == 0 {
                return None;
            }
            b -= 1;
        } else {
            break;
        }
    }
    let t = token(a, b);
    // Longest scheme prefix must start the token and leave something after it.
    let lower = t.to_ascii_lowercase();
    let ok = OPENABLE_SCHEMES
        .iter()
        .any(|s| lower.starts_with(s) && lower.len() > s.len());
    (ok && (a..=b).contains(&at)).then_some((a, b))
}

impl Snapshot {
    /// The link under a cell: an OSC 8 hyperlink if the program set one, else a URL in the visible text.
    pub fn link_at(&self, col: u16, row: u16) -> Option<LinkSpan> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let id = self.cell(col, row).link;
        if id != 0 {
            let same = |c: u16| self.cell(c, row).link == id;
            let (mut a, mut b) = (col, col);
            while a > 0 && same(a - 1) {
                a -= 1;
            }
            while b + 1 < self.cols && same(b + 1) {
                b += 1;
            }
            let url = self.links.get(usize::from(id) - 1)?.clone();
            return is_openable(&url).then_some(LinkSpan {
                row,
                start: a,
                end: b,
                url,
            });
        }
        let chars: Vec<char> = (0..self.cols)
            .map(|c| match self.cell(c, row).ch {
                '\0' => ' ',
                ch => ch,
            })
            .collect();
        let (a, b) = scan_url(&chars, usize::from(col))?;
        Some(LinkSpan {
            row,
            start: a as u16,
            end: b as u16,
            url: chars[a..=b].iter().collect(),
        })
    }
}

impl Snapshot {
    /// The whitespace-delimited word under a cell (quotes and angle brackets also end a word), for callers
    /// that interpret it themselves, such as `file:line` references.
    pub fn token_at(&self, col: u16, row: u16) -> Option<String> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let stop =
            |c: char| c.is_whitespace() || c == '\0' || matches!(c, '<' | '>' | '"' | '`' | '|');
        let ch = |c: u16| self.cell(c, row).ch;
        if stop(ch(col)) {
            return None;
        }
        let (mut a, mut b) = (col, col);
        while a > 0 && !stop(ch(a - 1)) {
            a -= 1;
        }
        while b + 1 < self.cols && !stop(ch(b + 1)) {
            b += 1;
        }
        Some((a..=b).map(ch).collect())
    }

    pub fn cell(&self, col: u16, row: u16) -> &Cell {
        &self.cells[usize::from(row) * usize::from(self.cols) + usize::from(col)]
    }

    /// Text of one row with trailing blanks trimmed (test and copy helper).
    pub fn row_text(&self, row: u16) -> String {
        let s: String = (0..self.cols)
            .map(|c| self.cell(c, row).ch)
            .filter(|&c| c != '\0')
            .collect();
        s.trim_end().to_string()
    }
}

struct Shared {
    writer: Mutex<Box<dyn Write + Send>>,
    title: Mutex<Option<String>>,
    size: Mutex<Size>,
    palette: Mutex<Palette>,
    /// Text a program asked to put on the clipboard (OSC 52); the UI decides whether to honour it.
    clipboard: Mutex<Option<String>>,
    /// Shell-integration state (OSC 133).
    prompts: Mutex<Prompts>,
    /// Latest working directory the shell reported (OSC 7).
    cwd: Mutex<Option<CwdReport>>,
    /// Pixel size of one cell (set by the UI), to size images.
    cell_px: Mutex<CellPx>,
    /// Images on screen (Kitty graphics, iTerm2 inline images).
    placements: Mutex<Vec<Placement>>,
    pending: AtomicBool,
    exited: AtomicBool,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn record_mark(&self, term: &Term<Listener>, mark: osc133::Mark) {
        let Ok(mut p) = self.prompts.lock() else {
            return;
        };
        match mark {
            osc133::Mark::PromptStart if !term.mode().contains(TermMode::ALT_SCREEN) => {
                let row = term.history_size() + term.grid().cursor.point.line.0.max(0) as usize;
                if p.rows.last() != Some(&row) {
                    p.rows.push(row);
                }
            }
            osc133::Mark::CommandDone(code) => p.last_exit = code,
            _ => {}
        }
    }

    fn forget_graphics(&self) {
        if let Ok(mut g) = self.placements.lock() {
            g.clear();
        }
    }

    /// Carry out a graphics command: place an image at the cursor, or delete placements.
    fn apply_graphics(&self, term: &mut Term<Listener>, parser: &mut Processor, action: GfxAction) {
        match action {
            GfxAction::None => {}
            GfxAction::Delete { id, free: _ } => {
                if let Ok(mut g) = self.placements.lock() {
                    g.retain(|p| id.is_some_and(|i| p.client_id != i));
                }
            }
            GfxAction::Place {
                image,
                id,
                cols,
                rows,
                stay,
            } => {
                let cell = self.cell_px.lock().map(|c| *c).unwrap_or_default();
                let max_cols = term.columns() as u32;
                let (cols, rows) =
                    gfx::layout(image.width, image.height, cell, cols, rows, max_cols);
                let cursor = term.grid().cursor.point;
                let line = term.history_size() as i64 + i64::from(cursor.line.0);
                let alt = term.mode().contains(TermMode::ALT_SCREEN);
                if let Ok(mut g) = self.placements.lock() {
                    g.push(Placement {
                        image,
                        client_id: id,
                        line,
                        col: u16::try_from(cursor.column.0).unwrap_or(0),
                        cols,
                        rows,
                        alt,
                    });
                    let extra = g.len().saturating_sub(MAX_PLACEMENTS);
                    g.drain(..extra);
                }
                if !stay {
                    // Leave the cursor on the line below the image, like the protocol describes.
                    let lines = "\n".repeat(usize::from(rows));
                    parser.advance(&mut *term, lines.as_bytes());
                }
            }
        }
    }

    fn forget_prompts(&self) {
        if let Ok(mut p) = self.prompts.lock() {
            p.rows.clear();
        }
    }

    /// Current palette (cloned: it is small and read rarely).
    fn palette(&self) -> Palette {
        self.palette
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn write(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}

#[derive(Clone)]
struct Listener(Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let sh = &self.0;
        match event {
            Event::PtyWrite(s) => sh.write(s.as_bytes()),
            Event::Title(t) => {
                if let Ok(mut g) = sh.title.lock() {
                    *g = Some(t);
                }
                sh.notify();
            }
            Event::ClipboardStore(_, text) => {
                if let Ok(mut g) = sh.clipboard.lock() {
                    *g = Some(text.chars().take(MAX_CLIPBOARD_CHARS).collect());
                }
                sh.notify();
            }
            Event::ResetTitle => {
                if let Ok(mut g) = sh.title.lock() {
                    *g = None;
                }
                sh.notify();
            }
            Event::ColorRequest(idx, fmt) => {
                let palette = sh.palette();
                let rgb = match idx {
                    256 => palette.fg,
                    257 => palette.bg,
                    258 => palette.cursor,
                    i if i < 256 => palette.indexed(i as u8),
                    _ => palette.fg,
                };
                let v = alacritty_terminal::vte::ansi::Rgb {
                    r: rgb.0,
                    g: rgb.1,
                    b: rgb.2,
                };
                sh.write(fmt(v).as_bytes());
            }
            Event::TextAreaSizeRequest(fmt) => {
                if let Ok(s) = sh.size.lock() {
                    let ws = WindowSize {
                        num_lines: s.rows,
                        num_cols: s.cols,
                        cell_width: s.cell_width,
                        cell_height: s.cell_height,
                    };
                    sh.write(fmt(ws).as_bytes());
                }
            }
            Event::Exit | Event::ChildExit(_) => {
                sh.exited.store(true, Ordering::Release);
                sh.notify();
            }
            // Clipboard requests are ignored until the clipboard policy UI exists (PLAN.md section 7).
            _ => {}
        }
    }
}

/// A live terminal.
pub struct Session {
    term: Arc<Mutex<Term<Listener>>>,
    shared: Arc<Shared>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    search: Mutex<Option<Box<Search>>>,
}

/// Where prompts are, as rows counted from the top of the history, plus the last exit status.
#[derive(Debug, Default)]
struct Prompts {
    rows: Vec<usize>,
    last_exit: Option<i32>,
}

/// Longest clipboard request kept (characters).
const MAX_CLIPBOARD_CHARS: usize = 1_000_000;

/// Most matches kept for one search (keeps a huge scrollback responsive).
const MAX_MATCHES: usize = 20_000;

/// Active scrollback search.
struct Search {
    regex: RegexSearch,
    /// Keep only matches that are not part of a longer word.
    whole_word: bool,
    matches: Vec<Match>,
    current: usize,
}

/// Escape regex metacharacters: the user types plain text, not a pattern.
fn escape(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 2);
    for c in query.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether the match is delimited by non-word cells (or the line edges) on both sides.
fn is_whole_word(term: &Term<Listener>, m: &Match) -> bool {
    let (a, b) = (m.start(), m.end());
    let grid = term.grid();
    let before =
        a.column.0 == 0 || !is_word_char(grid[Point::new(a.line, Column(a.column.0 - 1))].c);
    let after = b.column.0 + 1 >= term.columns()
        || !is_word_char(grid[Point::new(b.line, Column(b.column.0 + 1))].c);
    before && after
}

fn find_all(term: &Term<Listener>, regex: &mut RegexSearch, whole_word: bool) -> Vec<Match> {
    let start = Point::new(term.topmost_line(), Column(0));
    let end = Point::new(
        term.bottommost_line(),
        Column(term.columns().saturating_sub(1)),
    );
    RegexIter::new(start, end, SearchDirection::Right, term, regex)
        .filter(|m| !whole_word || is_whole_word(term, m))
        .take(MAX_MATCHES)
        .collect()
}

impl Session {
    /// Start a session. `wake` is invoked (coalesced) whenever new output should be redrawn.
    pub fn spawn(
        opts: SpawnOptions,
        size: Size,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Session, SessionError> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(|e| SessionError::Pty(e.to_string()))?;

        let argv = opts
            .command
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| vec![mahzenterm_platform::default_shell()]);
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "MahzenTerm");
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        if let Some(dir) = opts.cwd.or_else(mahzenterm_platform::home_dir) {
            cmd.cwd(dir);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SessionError::Spawn {
                command: argv[0].clone(),
                reason: e.to_string(),
            })?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| SessionError::Pty(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| SessionError::Pty(e.to_string()))?;

        let shared = Arc::new(Shared {
            writer: Mutex::new(writer),
            title: Mutex::new(None),
            size: Mutex::new(size),
            palette: Mutex::new(opts.palette),
            clipboard: Mutex::new(None),
            prompts: Mutex::new(Prompts::default()),
            cwd: Mutex::new(None),
            cell_px: Mutex::new(CellPx::default()),
            placements: Mutex::new(Vec::new()),
            pending: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            wake: Arc::new(wake),
        });
        let config = Config {
            // Programs may ask to write the clipboard (the UI asks the user); reading it is never possible.
            osc52: Osc52::OnlyCopy,
            ..Config::default()
        };
        let history_cap = config.scrolling_history;
        let dims = Dims {
            cols: usize::from(size.cols),
            rows: usize::from(size.rows),
        };
        let term = Arc::new(Mutex::new(Term::new(
            config,
            &dims,
            Listener(shared.clone()),
        )));

        spawn_reader(reader, term.clone(), shared.clone(), history_cap);

        Ok(Session {
            term,
            shared,
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            search: Mutex::new(None),
        })
    }

    fn term(&self) -> MutexGuard<'_, Term<Listener>> {
        self.term.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Switch the colour theme of this session; the next [`Session::snapshot`] uses it.
    pub fn set_palette(&self, palette: Palette) {
        *self
            .shared
            .palette
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = palette;
    }

    /// OS process id of the shell, if the platform reports one.
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().ok().and_then(|c| c.process_id())
    }

    /// Write raw bytes to the child's stdin.
    pub fn write(&self, bytes: &[u8]) {
        self.shared.write(bytes);
    }

    /// Encode and send a key press.
    pub fn send_key(&self, key: Key, mods: Mods) {
        let app = self.term().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = encode_key(key, mods, app) {
            self.scroll_to_bottom();
            self.write(&bytes);
        }
    }

    /// Paste text, using bracketed paste when the program asked for it. ESC is stripped.
    pub fn paste(&self, text: &str) {
        let clean: String = text.chars().filter(|&c| c != '\x1b').collect();
        let bracketed = self.term().mode().contains(TermMode::BRACKETED_PASTE);
        self.scroll_to_bottom();
        if bracketed {
            self.write(format!("\x1b[200~{clean}\x1b[201~").as_bytes());
        } else {
            self.write(clean.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        }
    }

    pub fn resize(&self, size: Size) {
        {
            let mut cur = self.shared.size.lock().unwrap_or_else(|p| p.into_inner());
            if *cur == size {
                return;
            }
            *cur = size;
        }
        if let Ok(m) = self.master.lock() {
            let _ = m.resize(pty_size(size));
        }
        let dims = Dims {
            cols: usize::from(size.cols),
            rows: usize::from(size.rows),
        };
        self.term().resize(dims);
        // Reflow changes line counts, so stored prompt rows and image anchors no longer match.
        self.shared.forget_prompts();
        self.shared.forget_graphics();
        self.shared.notify();
    }

    /// Tell the session how big a cell is in pixels (images are sized with it).
    pub fn set_cell_size(&self, width: u16, height: u16) {
        if let Ok(mut c) = self.shared.cell_px.lock() {
            *c = CellPx {
                w: width.max(1),
                h: height.max(1),
            };
        }
    }

    /// Scroll the viewport: positive lines go back in history.
    pub fn scroll(&self, lines: i32) {
        self.term().scroll_display(Scroll::Delta(lines));
        self.shared.notify();
    }

    pub fn scroll_to_bottom(&self) {
        let mut t = self.term();
        if t.grid().display_offset() != 0 {
            t.scroll_display(Scroll::Bottom);
            drop(t);
            self.shared.notify();
        }
    }

    fn grid_point(&self, col: u16, row: u16) -> Point {
        let off = self.term().grid().display_offset() as i32;
        Point::new(Line(i32::from(row) - off), Column(usize::from(col)))
    }

    /// Begin a selection at a visible cell. `block` selects a rectangle (Alt+drag).
    pub fn select_start(&self, col: u16, row: u16, block: bool) {
        let point = self.grid_point(col, row);
        let ty = if block {
            SelectionType::Block
        } else {
            SelectionType::Simple
        };
        self.term().selection = Some(Selection::new(ty, point, Side::Left));
        self.shared.notify();
    }

    /// Extend the selection to a visible cell (no-op without an active selection).
    pub fn select_update(&self, col: u16, row: u16) {
        let point = self.grid_point(col, row);
        if let Some(sel) = self.term().selection.as_mut() {
            sel.update(point, Side::Right);
        }
        self.shared.notify();
    }

    /// Select the word under a visible cell (double click).
    pub fn select_word(&self, col: u16, row: u16) {
        let point = self.grid_point(col, row);
        self.term().selection = Some(Selection::new(SelectionType::Semantic, point, Side::Left));
        self.shared.notify();
    }

    /// Select the whole line under a visible row (triple click).
    pub fn select_line(&self, row: u16) {
        let point = self.grid_point(0, row);
        self.term().selection = Some(Selection::new(SelectionType::Lines, point, Side::Left));
        self.shared.notify();
    }

    pub fn select_clear(&self) {
        self.term().selection = None;
        self.shared.notify();
    }

    /// Selected text, if any (trailing blanks per line are trimmed by the VT layer).
    pub fn selection_text(&self) -> Option<String> {
        self.term().selection_to_string().filter(|s| !s.is_empty())
    }

    fn search_lock(&self) -> MutexGuard<'_, Option<Box<Search>>> {
        self.search.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Start or update a plain-text search (smart case: lowercase queries ignore case). Jumps to the match
    /// nearest the bottom of the screen and returns `(current, total)` as 1-based position and count.
    ///
    /// With `regex` the query is a regular expression; a pattern that does not compile finds nothing.
    pub fn search(&self, query: &str, regex: bool) -> (usize, usize) {
        self.search_opts(query, regex, false)
    }

    /// Like [`Session::search`]; with `whole_word` a match inside a longer word (`foo` in `foobar`) is skipped.
    pub fn search_opts(&self, query: &str, regex: bool, whole_word: bool) -> (usize, usize) {
        if query.is_empty() {
            self.search_clear();
            return (0, 0);
        }
        let pattern = if regex {
            query.to_owned()
        } else {
            escape(query)
        };
        let Ok(mut regex) = RegexSearch::new(&pattern) else {
            self.search_clear();
            return (0, 0);
        };
        let matches = find_all(&self.term(), &mut regex, whole_word);
        let total = matches.len();
        let current = total.saturating_sub(1);
        *self.search_lock() = Some(Box::new(Search {
            regex,
            whole_word,
            matches,
            current,
        }));
        self.reveal_current();
        self.shared.notify();
        (if total == 0 { 0 } else { current + 1 }, total)
    }

    /// Move to the next (`forward`) or previous match, wrapping around.
    pub fn search_step(&self, forward: bool) -> (usize, usize) {
        let status = {
            let mut guard = self.search_lock();
            let Some(s) = guard.as_mut() else {
                return (0, 0);
            };
            let n = s.matches.len();
            if n > 0 {
                s.current = if forward {
                    (s.current + 1) % n
                } else {
                    (s.current + n - 1) % n
                };
            }
            (if n == 0 { 0 } else { s.current + 1 }, n)
        };
        self.reveal_current();
        self.shared.notify();
        status
    }

    /// Stop searching and remove the highlights.
    pub fn search_clear(&self) {
        if self.search_lock().take().is_some() {
            self.shared.notify();
        }
    }

    /// `(current, total)` of the active search, if any.
    pub fn search_status(&self) -> Option<(usize, usize)> {
        self.search_lock().as_ref().map(|s| {
            (
                if s.matches.is_empty() {
                    0
                } else {
                    s.current + 1
                },
                s.matches.len(),
            )
        })
    }

    /// Scroll so the current match sits mid-screen.
    fn reveal_current(&self) {
        let line = {
            let guard = self.search_lock();
            match guard.as_ref().and_then(|s| s.matches.get(s.current)) {
                Some(m) => m.start().line.0,
                None => return,
            }
        };
        let mut term = self.term();
        let rows = term.screen_lines() as i32;
        let history = term.history_size() as i32;
        let wanted = (rows / 2 - line).clamp(0, history);
        let now = term.grid().display_offset() as i32;
        if wanted != now {
            term.scroll_display(Scroll::Delta(wanted - now));
        }
    }

    /// Exit status the shell reported for the last finished command (needs shell integration).
    pub fn last_exit(&self) -> Option<i32> {
        self.shared.prompts.lock().ok().and_then(|p| p.last_exit)
    }

    /// The directory the shell last reported with OSC 7, when it is on this machine (`local_host` is this
    /// machine's host name). A report from an ssh session names a path elsewhere and yields `None`.
    pub fn reported_cwd(&self, local_host: &str) -> Option<String> {
        let c = self.shared.cwd.lock().ok()?.clone()?;
        let local = c.host.is_empty()
            || c.host.eq_ignore_ascii_case("localhost")
            || c.host.eq_ignore_ascii_case(local_host);
        local.then_some(c.path)
    }

    /// Whether the shell has announced any prompt (so jumping can work).
    pub fn has_prompts(&self) -> bool {
        self.shared.prompts.lock().is_ok_and(|p| !p.rows.is_empty())
    }

    /// Scroll to the previous (`back`) or next prompt relative to the top of the screen. Returns false when
    /// there is none that way (forward past the last prompt returns to the bottom and reports true).
    pub fn jump_prompt(&self, back: bool) -> bool {
        let rows = match self.shared.prompts.lock() {
            Ok(p) => p.rows.clone(),
            Err(_) => return false,
        };
        let mut term = self.term();
        let history = term.history_size();
        let offset = term.grid().display_offset();
        let top = history - offset.min(history);
        let target = if back {
            rows.iter().rev().find(|&&r| r < top).copied()
        } else {
            rows.iter().find(|&&r| r > top).copied()
        };
        let wanted = match target {
            Some(r) => history.saturating_sub(r),
            None if !back => 0,
            None => return false,
        };
        term.scroll_display(Scroll::Delta(wanted as i32 - offset as i32));
        drop(term);
        self.shared.notify();
        true
    }

    /// Take the pending OSC 52 clipboard-write request, if a program made one.
    pub fn take_clipboard_request(&self) -> Option<String> {
        self.shared.clipboard.lock().ok().and_then(|mut g| g.take())
    }

    pub fn title(&self) -> Option<String> {
        self.shared.title.lock().ok().and_then(|t| t.clone())
    }

    /// True once the child process has exited.
    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }

    /// Copy the visible screen. Clears the pending-redraw flag so the next change wakes the UI again.
    pub fn snapshot(&self) -> Snapshot {
        self.shared.pending.store(false, Ordering::Release);
        let palette = self.shared.palette();
        let palette = &palette;
        let term = self.term();
        let cols = term.columns();
        let rows = term.screen_lines();
        let content = term.renderable_content();
        let offset = content.display_offset;
        let blank = Cell {
            ch: ' ',
            fg: palette.fg,
            bg: palette.bg,
            flags: CellFlags::default(),
            selected: false,
            mark: Mark::None,
            link: 0,
        };
        let mut cells = vec![blank; cols * rows];
        let mut links: Vec<String> = Vec::new();
        for item in content.display_iter {
            let row = item.point.line.0 + offset as i32;
            let col = item.point.column.0;
            if row < 0 || row as usize >= rows || col >= cols {
                continue;
            }
            let c = &item.cell;
            let mut fg = palette.resolve(c.fg, content.colors);
            let mut bg = palette.resolve(c.bg, content.colors);
            if c.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if c.flags.contains(Flags::DIM) && !c.flags.contains(Flags::BOLD) {
                fg = fg.scaled(2, 3);
            }
            let ch = if c.flags.contains(Flags::WIDE_CHAR_SPACER) {
                '\0'
            } else if c.flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                c.c
            };
            cells[row as usize * cols + col] = Cell {
                ch,
                fg,
                bg,
                flags: CellFlags {
                    bold: c.flags.contains(Flags::BOLD),
                    italic: c.flags.contains(Flags::ITALIC),
                    underline: c.flags.intersects(Flags::ALL_UNDERLINES),
                    strike: c.flags.contains(Flags::STRIKEOUT),
                    wide: c.flags.contains(Flags::WIDE_CHAR),
                },
                selected: content.selection.is_some_and(|r| r.contains(item.point)),
                mark: Mark::None,
                link: c.hyperlink().map_or(0, |h| {
                    let uri = h.uri();
                    let at = links.iter().position(|l| l == uri).unwrap_or_else(|| {
                        links.push(uri.to_owned());
                        links.len() - 1
                    });
                    u16::try_from(at + 1).unwrap_or(0)
                }),
            };
        }
        if let Some(search) = self.search_lock().as_mut() {
            // Output keeps arriving, so the match list is rebuilt for every frame while a search is open.
            let was = search.current;
            search.matches = find_all(&term, &mut search.regex, search.whole_word);
            search.current = was.min(search.matches.len().saturating_sub(1));
            for (i, m) in search.matches.iter().enumerate() {
                let kind = if i == search.current {
                    Mark::Current
                } else {
                    Mark::Found
                };
                let (a, b) = (m.start(), m.end());
                for line in
                    a.line.0.max(-(offset as i32))..=b.line.0.min(rows as i32 - 1 - offset as i32)
                {
                    let row = (line + offset as i32) as usize;
                    let from = if line == a.line.0 { a.column.0 } else { 0 };
                    let to = if line == b.line.0 {
                        b.column.0
                    } else {
                        cols - 1
                    };
                    for col in from..=to.min(cols - 1) {
                        cells[row * cols + col].mark = kind;
                    }
                }
            }
        }
        let cur = content.cursor;
        let cur_row = cur.point.line.0 + offset as i32;
        let visible = content.mode.contains(TermMode::SHOW_CURSOR)
            && cur.shape != CursorShape::Hidden
            && cur_row >= 0
            && (cur_row as usize) < rows;
        let cursor = visible.then_some(Cursor {
            col: cur.point.column.0 as u16,
            row: cur_row as u16,
            shape: match cur.shape {
                CursorShape::Underline => CursorShapeKind::Underline,
                CursorShape::Beam => CursorShapeKind::Beam,
                CursorShape::HollowBlock => CursorShapeKind::Hollow,
                _ => CursorShapeKind::Block,
            },
        });
        Snapshot {
            cols: cols as u16,
            rows: rows as u16,
            cells,
            cursor,
            bg: palette.bg,
            cursor_color: palette.cursor,
            display_offset: offset,
            links,
            images: self.visible_images(&term, offset),
        }
    }

    /// Images that touch the screen now. Alternate-screen images vanish once that screen is left.
    fn visible_images(&self, term: &Term<Listener>, offset: usize) -> Vec<ImageView> {
        let Ok(mut all) = self.shared.placements.lock() else {
            return Vec::new();
        };
        if all.is_empty() {
            return Vec::new();
        }
        let in_alt = term.mode().contains(TermMode::ALT_SCREEN);
        if !in_alt {
            all.retain(|p| !p.alt);
        }
        let history = term.history_size() as i64;
        let screen_rows = term.screen_lines() as i64;
        all.iter()
            .filter(|p| p.alt == in_alt)
            .filter_map(|p| {
                let row = p.line - history + offset as i64;
                (row + i64::from(p.rows) > 0 && row < screen_rows).then(|| ImageView {
                    image: p.image.clone(),
                    col: p.col,
                    row: i32::try_from(row).unwrap_or(0),
                    cols: p.cols,
                    rows: p.rows,
                })
            })
            .collect()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }
}

fn pty_size(s: Size) -> PtySize {
    PtySize {
        rows: s.rows,
        cols: s.cols,
        pixel_width: s.cols.saturating_mul(s.cell_width),
        pixel_height: s.rows.saturating_mul(s.cell_height),
    }
}

/// Something found in the output stream that needs the cursor position.
enum StreamEvent {
    Mark(osc133::Mark),
    Gfx(gfx::Raw),
}

fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    term: Arc<Mutex<Term<Listener>>>,
    shared: Arc<Shared>,
    history_cap: usize,
) {
    std::thread::Builder::new()
        .name("mahzenterm-pty-reader".into())
        .spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut scanner = Scanner::default();
            let mut gfx_scanner = gfx::Scanner::default();
            let mut kitty = Kitty::default();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let marks = scanner.scan(&buf[..n]);
                        if let Some(report) = scanner.take_cwd() {
                            if let Ok(mut c) = shared.cwd.lock() {
                                *c = Some(report);
                            }
                        }
                        // Marks and graphics commands are applied in stream order, each just after its terminator.
                        let mut events: Vec<(usize, StreamEvent)> = marks
                            .into_iter()
                            .map(|(end, m)| (end, StreamEvent::Mark(m)))
                            .chain(
                                gfx_scanner
                                    .scan(&buf[..n])
                                    .into_iter()
                                    .map(|(end, g)| (end, StreamEvent::Gfx(g))),
                            )
                            .collect();
                        events.sort_by_key(|(end, _)| *end);
                        {
                            let mut t = term.lock().unwrap_or_else(|p| p.into_inner());
                            let mut from = 0;
                            for (end, ev) in events {
                                // Feed the parser up to the event so the cursor is where the program meant it.
                                parser.advance(&mut *t, &buf[from..end]);
                                from = end;
                                match ev {
                                    StreamEvent::Mark(mark) => shared.record_mark(&t, mark),
                                    StreamEvent::Gfx(raw) => {
                                        let out = match raw {
                                            gfx::Raw::Kitty(text) => kitty.handle(&text),
                                            gfx::Raw::Iterm(text) => gfx::iterm(
                                                &text,
                                                t.columns() as u32,
                                                t.screen_lines() as u32,
                                            ),
                                            gfx::Raw::Sixel(text) => gfx::sixel_outcome(&text),
                                        };
                                        if let Some(reply) = out.reply {
                                            shared.write(&reply);
                                        }
                                        shared.apply_graphics(&mut t, &mut parser, out.action);
                                    }
                                }
                            }
                            parser.advance(&mut *t, &buf[from..n]);
                            // Lines that scrolled out of a full history shift every stored row: forget them.
                            if t.history_size() >= history_cap {
                                shared.forget_prompts();
                                shared.forget_graphics();
                            }
                        }
                        shared.notify();
                    }
                }
            }
            shared.exited.store(true, Ordering::Release);
            shared.notify();
        })
        .ok();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn run(script: &str, size: Size) -> Session {
        let opts = SpawnOptions {
            command: Some(vec!["/bin/sh".into(), "-c".into(), script.into()]),
            ..SpawnOptions::default()
        };
        Session::spawn(opts, size, || {}).expect("spawn")
    }

    fn wait_for(s: &Session, pred: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let snap = s.snapshot();
            if pred(&snap) || Instant::now() > end {
                return snap;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn plain_urls_are_found_and_trimmed() {
        let line = chars("see (https://example.com/a_(b)). ok");
        let (a, b) = scan_url(&line, 12).expect("url");
        assert_eq!(
            line[a..=b].iter().collect::<String>(),
            "https://example.com/a_(b)"
        );
        let line = chars("go to http://x.org/p?q=1, then");
        let (a, b) = scan_url(&line, 8).expect("url");
        assert_eq!(line[a..=b].iter().collect::<String>(), "http://x.org/p?q=1");
        assert!(scan_url(&line, 0).is_none());
        assert!(scan_url(&chars("javascript:alert(1)"), 3).is_none());
        assert!(scan_url(&chars("file:///etc/passwd"), 3).is_none());
        assert!(scan_url(&chars("https://"), 3).is_none());
    }

    #[test]
    fn only_safe_schemes_open() {
        assert!(is_openable("HTTPS://a.b"));
        assert!(is_openable("mailto:me@x.org"));
        assert!(!is_openable("file:///etc/passwd"));
        assert!(!is_openable("javascript:1"));
    }

    /// A 2x2 RGB image (red, green / blue, white) as a Kitty transmit-and-display command.
    const KITTY_2X2: &str = "\\033_Ga=T,f=24,s=2,v=2,i=5;/wAAAP8AAAD/////\\033\\\\";

    #[test]
    fn kitty_images_are_placed_at_the_cursor_and_scroll_with_the_text() {
        // The test sets the cell size after spawn, so the image is sized by the 9x18 default: one cell.
        let s = run(
            &format!("printf 'ab'; printf '{KITTY_2X2}'; printf 'X'; sleep 3"),
            Size::new(20, 4),
        );
        let snap = wait_for(&s, |p| !p.images.is_empty() && p.row_text(1).contains('X'));
        assert_eq!(snap.images.len(), 1, "one image placed");
        let img = &snap.images[0];
        assert_eq!((img.image.width, img.image.height), (2, 2));
        assert_eq!(&img.image.rgba[..4], &[255, 0, 0, 255], "red first");
        assert_eq!(
            (img.col, img.row),
            (2, 0),
            "where the cursor was, after `ab`"
        );
        assert_eq!((img.cols, img.rows), (1, 1));
        // The cursor moved to the line below the image before `X` was printed.
        assert!(snap.row_text(1).contains('X'), "{}", snap.row_text(1));
    }

    #[test]
    fn sixel_images_are_placed_and_the_text_after_them_survives() {
        // 4x6 green block, then text on the line below it.
        let s = run(
            "printf 'ab'; printf '\\033P0;1q\"1;1;4;6#1;2;0;100;0#1~~~~\\033\\\\'; printf 'Z'; sleep 3",
            Size::new(20, 4),
        );
        let snap = wait_for(&s, |p| !p.images.is_empty() && p.row_text(1).contains('Z'));
        assert_eq!(snap.images.len(), 1);
        let img = &snap.images[0];
        assert_eq!((img.image.width, img.image.height), (4, 6));
        assert_eq!(&img.image.rgba[..4], &[0, 255, 0, 255]);
        assert_eq!((img.col, img.row), (2, 0));
        assert!(snap.row_text(1).contains('Z'), "{}", snap.row_text(1));
    }

    #[test]
    fn kitty_queries_are_answered_on_the_pty() {
        // The terminal is put in raw mode so the reply (no newline) reaches `dd`; `od` makes it visible.
        let s = run(
            "stty raw -echo; printf '\\033_Ga=q,i=31,f=32,s=1,v=1;AAAAAA==\\033\\\\'; dd bs=1 count=12 2>/dev/null | od -An -c; sleep 3",
            Size::new(60, 6),
        );
        let snap = wait_for(&s, |p| (0..6).any(|r| p.row_text(r).contains("O   K")));
        let text: Vec<String> = (0..6).map(|r| snap.row_text(r)).collect();
        assert!(text.iter().any(|l| l.contains("O   K")), "{text:?}");
        assert!(snap.images.is_empty(), "a query places nothing");
    }

    #[test]
    fn osc7_cwd_is_kept_for_local_hosts_only() {
        let s = run(
            "printf '\\033]7;file://thishost/tmp/a%%20b\\007ok'; sleep 3",
            Size::new(40, 5),
        );
        wait_for(&s, |p| p.row_text(0).contains("ok"));
        assert_eq!(s.reported_cwd("thishost").as_deref(), Some("/tmp/a b"));
        assert_eq!(s.reported_cwd("THISHOST").as_deref(), Some("/tmp/a b"));
        assert_eq!(
            s.reported_cwd("other"),
            None,
            "a report from another host is a remote path"
        );
    }

    #[test]
    fn extra_environment_reaches_the_child() {
        let opts = SpawnOptions {
            command: Some(vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf 'v=%s' \"$MZ_PROFILE_TEST\"; sleep 3".into(),
            ]),
            env: vec![("MZ_PROFILE_TEST".into(), "work".into())],
            ..SpawnOptions::default()
        };
        let s = Session::spawn(opts, Size::new(30, 3), || {}).expect("spawn");
        let snap = wait_for(&s, |p| p.row_text(0).contains("v="));
        assert!(
            snap.row_text(0).starts_with("v=work"),
            "{}",
            snap.row_text(0)
        );
    }

    #[test]
    fn link_at_reads_the_screen_and_osc8() {
        let s = run(
            "printf 'a https://rust-lang.org b\\n\\033]8;;https://osc8.example\\033\\\\click\\033]8;;\\033\\\\ end'; sleep 3",
            Size::new(40, 5),
        );
        let snap = wait_for(&s, |p| p.row_text(1).contains("end"));
        let plain = snap.link_at(6, 0).expect("plain url");
        assert_eq!((plain.start, plain.end), (2, 22));
        assert_eq!(plain.url, "https://rust-lang.org");
        let osc = snap.link_at(1, 1).expect("osc 8 link");
        assert_eq!(
            (osc.start, osc.end, osc.url.as_str()),
            (0, 4, "https://osc8.example")
        );
        assert!(snap.link_at(30, 0).is_none());
    }

    #[test]
    fn token_at_returns_the_word_under_a_cell() {
        let s = run(
            "printf 'error at src/main.rs:12:5: bad\n'; sleep 3",
            Size::new(40, 5),
        );
        let snap = wait_for(&s, |p| p.row_text(0).contains("bad"));
        assert_eq!(snap.token_at(12, 0).as_deref(), Some("src/main.rs:12:5:"));
        assert_eq!(snap.token_at(0, 0).as_deref(), Some("error"));
        assert!(snap.token_at(8, 0).is_none(), "a gap between words");
        assert!(snap.token_at(39, 4).is_none());
    }

    #[test]
    fn search_marks_matches_and_steps_through_them() {
        let s = run(
            "printf 'foo bar\\nbaz Foo\\nfoo.x'; sleep 3",
            Size::new(20, 5),
        );
        wait_for(&s, |p| p.row_text(2) == "foo.x");
        // Smart case: lowercase query ignores case; the '.' is literal.
        assert_eq!(s.search("foo", false), (3, 3));
        let snap = s.snapshot();
        assert_eq!(snap.cell(0, 2).mark, Mark::Current);
        assert_eq!(snap.cell(4, 1).mark, Mark::Found);
        assert_eq!(snap.cell(4, 0).mark, Mark::None);
        assert_eq!(s.search_step(true), (1, 3));
        assert_eq!(s.snapshot().cell(0, 0).mark, Mark::Current);
        assert_eq!(s.search_step(false), (3, 3));
        assert_eq!(s.search("Foo", false), (1, 1));
        assert_eq!(s.search("foo.", false), (1, 1));
        assert_eq!(s.search("zzz", false), (0, 0));
        assert_eq!(s.search("fo+\\.x", true), (1, 1), "regex mode");
        assert_eq!(
            s.search("fo(", true),
            (0, 0),
            "a broken pattern finds nothing"
        );
        assert!(s.search_status().is_none());
        s.search_clear();
        assert!(s.search_status().is_none());
        assert_eq!(s.snapshot().cell(0, 0).mark, Mark::None);
        // Whole words: `foo` in `foo.x` still counts (a dot ends a word), `fo` never does.
        assert_eq!(s.search_opts("foo", false, true), (3, 3));
        assert_eq!(s.search_opts("fo", false, true), (0, 0));
        assert_eq!(
            s.search("fo", false),
            (3, 3),
            "without the option fragments match"
        );
        s.search_clear();
    }

    #[test]
    fn osc52_copy_is_held_for_the_ui() {
        // "hello" in base64 is aGVsbG8=
        let s = run(
            "printf '\\033]52;c;aGVsbG8=\\007'; sleep 3",
            Size::new(20, 3),
        );
        let end = Instant::now() + Duration::from_secs(5);
        let mut got = None;
        while got.is_none() && Instant::now() < end {
            got = s.take_clipboard_request();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.as_deref(), Some("hello"));
        assert!(s.take_clipboard_request().is_none());
    }

    #[test]
    fn prompt_marks_allow_jumping_and_report_exit_status() {
        let s = run(
            "for i in 1 2 3; do printf '\\033]133;A\\007prompt%s\\n' $i; seq 1 20; printf '\\033]133;D;%s\\007' $i; done; printf end; sleep 3",
            Size::new(20, 6),
        );
        wait_for(&s, |p| {
            p.row_text(5).contains("end") || p.row_text(4).contains("end")
        });
        assert!(s.has_prompts());
        assert_eq!(s.last_exit(), Some(3));
        assert!(s.jump_prompt(true));
        assert_eq!(s.snapshot().row_text(0), "prompt3");
        assert!(s.jump_prompt(true));
        assert_eq!(s.snapshot().row_text(0), "prompt2");
        assert!(s.jump_prompt(false));
        assert_eq!(s.snapshot().row_text(0), "prompt3");
        assert!(
            s.jump_prompt(false),
            "past the last prompt returns to the bottom"
        );
        assert_eq!(s.snapshot().display_offset, 0);
        assert!(s.jump_prompt(true) && s.jump_prompt(true) && s.jump_prompt(true));
        assert!(!s.jump_prompt(true), "nothing above the first prompt");
    }

    #[test]
    fn prints_child_output() {
        let s = run("printf 'hello'", Size::new(40, 5));
        let snap = wait_for(&s, |p| p.row_text(0) == "hello");
        assert_eq!(snap.row_text(0), "hello");
    }

    #[test]
    fn ansi_colors_are_resolved() {
        let s = run("printf '\\033[31mR\\033[0m'", Size::new(40, 5));
        let snap = wait_for(&s, |p| p.cell(0, 0).ch == 'R');
        assert_eq!(snap.cell(0, 0).fg, Palette::default().ansi[1]);
    }

    #[test]
    fn palette_can_be_switched_while_running() {
        let s = run("printf '\\033[31mR'; sleep 3", Size::new(40, 5));
        wait_for(&s, |p| p.cell(0, 0).ch == 'R');
        let mut pal = Palette::default();
        pal.ansi[1] = Rgb(1, 2, 3);
        pal.bg = Rgb(9, 8, 7);
        pal.cursor = Rgb(4, 5, 6);
        s.set_palette(pal);
        let snap = s.snapshot();
        assert_eq!(snap.cell(0, 0).fg, Rgb(1, 2, 3));
        assert_eq!(snap.bg, Rgb(9, 8, 7));
        assert_eq!(snap.cursor_color, Rgb(4, 5, 6));
    }

    fn screen_text(snap: &Snapshot) -> String {
        (0..snap.rows)
            .map(|r| snap.row_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn input_round_trips_through_the_pty() {
        let s = run("read x; printf \"got:$x\"", Size::new(40, 5));
        s.write(b"abc\r");
        let snap = wait_for(&s, |p| screen_text(p).contains("got:abc"));
        assert!(
            screen_text(&snap).contains("got:abc"),
            "screen: {:?}",
            screen_text(&snap)
        );
    }

    #[test]
    fn resize_changes_snapshot_dimensions() {
        let s = run("sleep 5", Size::new(40, 5));
        s.resize(Size::new(80, 10));
        let snap = s.snapshot();
        assert_eq!((snap.cols, snap.rows), (80, 10));
    }

    #[test]
    fn exit_is_reported() {
        let s = run("true", Size::new(40, 5));
        let end = Instant::now() + Duration::from_secs(5);
        while !s.exited() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(s.exited());
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        let opts = SpawnOptions {
            command: Some(vec!["/nonexistent/shell".into()]),
            ..SpawnOptions::default()
        };
        assert!(Session::spawn(opts, Size::new(10, 3), || {}).is_err());
    }

    #[test]
    fn paste_strips_escape_characters() {
        let s = run("cat", Size::new(40, 5));
        s.paste("a\x1bb");
        let snap = wait_for(&s, |p| screen_text(p).contains("ab"));
        assert!(screen_text(&snap).contains("ab"));
    }

    #[test]
    fn selection_copies_text_and_marks_cells() {
        let s = run("printf 'hello world'; sleep 3", Size::new(40, 5));
        wait_for(&s, |p| p.row_text(0) == "hello world");
        s.select_start(0, 0, false);
        s.select_update(4, 0);
        assert_eq!(s.selection_text().as_deref(), Some("hello"));
        let snap = s.snapshot();
        assert!(snap.cell(0, 0).selected && snap.cell(4, 0).selected);
        assert!(!snap.cell(5, 0).selected);
        s.select_clear();
        assert_eq!(s.selection_text(), None);
    }

    #[test]
    fn double_click_selects_a_word() {
        let s = run("printf 'foo bar baz'; sleep 3", Size::new(40, 5));
        wait_for(&s, |p| p.row_text(0) == "foo bar baz");
        s.select_word(5, 0);
        assert_eq!(s.selection_text().as_deref(), Some("bar"));
    }

    #[test]
    fn scrollback_is_reachable_and_snapshot_follows() {
        let s = run("seq 1 100; sleep 3", Size::new(20, 5));
        wait_for(&s, |p| screen_text(p).contains("100"));
        let bottom = s.snapshot();
        assert_eq!(bottom.display_offset, 0);
        s.scroll(10);
        let up = s.snapshot();
        assert_eq!(up.display_offset, 10);
        assert_ne!(screen_text(&up), screen_text(&bottom));
        s.scroll_to_bottom();
        assert_eq!(s.snapshot().display_offset, 0);
    }

    #[test]
    fn wide_characters_occupy_two_cells() {
        let s = run("printf '日本x'; sleep 3", Size::new(20, 3));
        let snap = wait_for(&s, |p| p.cell(0, 0).ch == '日');
        assert!(snap.cell(0, 0).flags.wide);
        assert_eq!(snap.cell(1, 0).ch, '\0');
        assert_eq!(snap.cell(2, 0).ch, '本');
        assert_eq!(snap.cell(4, 0).ch, 'x');
    }
}
