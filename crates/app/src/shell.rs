//! Root Iced state: owns the workspace model, one terminal session per pane, routes input, draws the chrome.
//!
//! The pane tree lives in `mahzenterm-core` (pure data); this module only maps it to sessions and widgets.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc;
use iced::futures::{Stream, StreamExt};
use iced::widget::canvas::Cache;
use iced::widget::text::Wrapping;
use iced::widget::{button, center, column, container, mouse_area, pin, row, stack, text, Space};
use iced::{
    clipboard, event, keyboard, mouse, window, Background, Border, Color, Element, Event, Length,
    Padding, Point, Rectangle, Size, Subscription, Task,
};
use mahzenterm_core::command::{Chord, Keymap, Registry, Resolution};
use mahzenterm_core::config::{Config, Loader, Osc52Policy};
use mahzenterm_core::workspace::{Axis, Direction, PaneId, PaneKind, Tab, TabId, Window};
use mahzenterm_platform::Stats;
use mahzenterm_term::{Palette as TermPalette, Session, Size as TermSize, Snapshot, SpawnOptions};
use mahzenterm_ui_kit::{Metrics, TerminalView, Theme, Themes, Tokens, ViewAction};

use crate::chrome;
use crate::commands::{self, id};
use crate::feed::{self, Feed};
use crate::input::to_term_key;
use crate::layout;
use crate::live;
use crate::palette::{self, Outcome};
use crate::search::{self, SearchBar, SearchKey};
use crate::state::{self, State};

mod bars;
mod dock;
mod usage_card;
use bars::Bars;

const TAB_BAR_H: f32 = 40.0;
const STATUS_H: f32 = 28.0;
const RESIZE_STEP: f32 = 0.05;
pub const INITIAL_SIZE: Size = Size::new(1100.0, 700.0);

/// A pane is addressed by its tab and its id within the tab.
type PaneKey = (TabId, PaneId);

type WakeRx = Arc<Mutex<Option<mpsc::UnboundedReceiver<PaneKey>>>>;

/// Subscription identity wrapper: the receiver is taken exactly once.
struct WakeSource(WakeRx);

impl Hash for WakeSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "mahzenterm-wake".hash(state);
    }
}

type LiveRx = Arc<Mutex<Option<mpsc::UnboundedReceiver<live::Reload>>>>;

/// Subscription identity wrapper for the config reload stream.
struct LiveSource(LiveRx);

impl Hash for LiveSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "mahzenterm-live-config".hash(state);
    }
}

fn live_stream(src: &LiveSource) -> impl Stream<Item = Message> + use<> {
    let rx = src.0.lock().ok().and_then(|mut g| g.take());
    iced::futures::stream::iter(rx)
        .flatten()
        .map(Message::ConfigChanged)
}

type FeedRx = Arc<Mutex<Option<mpsc::UnboundedReceiver<Feed>>>>;

/// Subscription identity wrapper for the widget feed.
struct FeedSource(FeedRx);

impl Hash for FeedSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "mahzenterm-feed".hash(state);
    }
}

fn feed_stream(src: &FeedSource) -> impl Stream<Item = Message> + use<> {
    let rx = src.0.lock().ok().and_then(|mut g| g.take());
    iced::futures::stream::iter(rx).flatten().map(Message::Feed)
}

fn wake_stream(src: &WakeSource) -> impl Stream<Item = Message> + use<> {
    let rx = src.0.lock().ok().and_then(|mut g| g.take());
    iced::futures::stream::iter(rx).flatten().map(Message::Wake)
}

enum Term {
    Running {
        session: Session,
        snapshot: Box<Snapshot>,
    },
    Failed(String),
}

/// Runtime state of one pane: its session and render cache.
struct PaneRuntime {
    term: Term,
    cache: Cache,
    grid: (u16, u16),
    /// Title set by the program (OSC 0/2); empty when it never set one.
    title: String,
    /// Shell working directory, home-abbreviated; shown when the program sets no title.
    cwd_label: Option<String>,
    cwd_checked: Instant,
    /// A directory check was skipped by the throttle; a one-shot timer finishes it (no timer while idle).
    cwd_dirty: bool,
    key: PaneKey,
    wake: mpsc::UnboundedSender<PaneKey>,
}

/// How often the working directory is re-read from the OS (cheap, but not worth doing per output chunk).
const CWD_POLL: Duration = Duration::from_millis(500);

/// `/home/me/code/x` as `~/code/x` when under `home`.
fn short_path(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if let Some(rest) = home.and_then(|h| path.strip_prefix(h).ok()) {
        if rest.as_os_str().is_empty() {
            return "~".to_owned();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

/// Last path component of a [`short_path`] label (`~/code/x` becomes `x`, `~` stays `~`).
fn leaf(label: &str) -> &str {
    if label == "~" || label == "/" {
        return label;
    }
    label.rsplit('/').find(|c| !c.is_empty()).unwrap_or(label)
}

impl PaneRuntime {
    fn spawn(
        kind: &PaneKind,
        grid: (u16, u16),
        metrics: Metrics,
        key: PaneKey,
        wake: mpsc::UnboundedSender<PaneKey>,
        default_shell: Option<&str>,
        palette: TermPalette,
    ) -> Self {
        let (cwd, shell, args) = match kind {
            PaneKind::Terminal { cwd, shell, args } => (cwd.clone(), shell.clone(), args.clone()),
            PaneKind::Panel { .. } => (None, None, Vec::new()),
        };
        let opts = SpawnOptions {
            command: shell.or_else(|| default_shell.map(str::to_owned)).map(|s| {
                let mut argv = vec![s];
                argv.extend(args);
                argv
            }),
            cwd,
            palette,
        };
        let wake_session = wake.clone();
        let term = match Session::spawn(opts, term_size(grid, metrics), move || {
            let _ = wake_session.unbounded_send(key);
        }) {
            Ok(session) => {
                let snapshot = Box::new(session.snapshot());
                Term::Running { session, snapshot }
            }
            Err(e) => Term::Failed(e.to_string()),
        };
        Self {
            term,
            cache: Cache::new(),
            grid,
            title: String::new(),
            cwd_label: None,
            cwd_checked: Instant::now() - CWD_POLL,
            cwd_dirty: false,
            key,
            wake,
        }
    }

    /// What the pane header shows: the program's title, else the working directory, else "shell".
    fn display_title(&self) -> &str {
        if !self.title.is_empty() {
            &self.title
        } else {
            self.cwd_label.as_deref().unwrap_or("shell")
        }
    }

    fn resize(&mut self, grid: (u16, u16), metrics: Metrics) {
        if grid == self.grid {
            return;
        }
        self.grid = grid;
        if let Term::Running { session, snapshot } = &mut self.term {
            session.resize(term_size(grid, metrics));
            **snapshot = session.snapshot();
        }
        self.cache.clear();
    }

    fn refresh(&mut self) {
        if let Term::Running { session, snapshot } = &mut self.term {
            **snapshot = session.snapshot();
            self.title = session.title().unwrap_or_default();
            if self.title.is_empty() {
                if self.cwd_checked.elapsed() >= CWD_POLL {
                    self.poll_cwd();
                } else if !self.cwd_dirty {
                    // Finish the skipped check shortly: a one-shot delayed wake, so nothing runs while idle.
                    self.cwd_dirty = true;
                    let (tx, key) = (self.wake.clone(), self.key);
                    std::thread::spawn(move || {
                        std::thread::sleep(CWD_POLL);
                        let _ = tx.unbounded_send(key);
                    });
                }
            } else {
                // The program set its own title; the directory fallback is not needed.
                self.cwd_dirty = false;
            }
        }
        self.cache.clear();
    }

    /// Re-read the shell's working directory from the OS.
    fn poll_cwd(&mut self) {
        self.cwd_checked = Instant::now();
        self.cwd_dirty = false;
        if let Term::Running { session, .. } = &self.term {
            self.cwd_label = session
                .pid()
                .and_then(mahzenterm_platform::process_cwd)
                .map(|p| short_path(&p, mahzenterm_platform::home_dir().as_deref()));
        }
        self.cache.clear();
    }

    fn session(&self) -> Option<&Session> {
        match &self.term {
            Term::Running { session, .. } => Some(session),
            Term::Failed(_) => None,
        }
    }
}

fn term_size(grid: (u16, u16), m: Metrics) -> TermSize {
    let mut ts = TermSize::new(grid.0, grid.1);
    ts.cell_width = m.cell_width as u16;
    ts.cell_height = m.cell_height as u16;
    ts
}

/// An in-progress separator drag.
#[derive(Debug, Clone, Copy)]
struct Drag {
    a_first: PaneId,
    b_first: PaneId,
    axis: Axis,
    /// Pixel area of the split being dragged, in content coordinates.
    split: Rectangle,
}

/// Root application state.
pub struct Shell {
    window: Window,
    runtimes: HashMap<PaneKey, PaneRuntime>,
    registry: Registry,
    keymap: Keymap,
    pending: Vec<Chord>,
    metrics: Metrics,
    win_size: Size,
    wake_tx: mpsc::UnboundedSender<PaneKey>,
    wake_rx: WakeRx,
    tokens: Tokens,
    drag: Option<Drag>,
    hover: Option<(PaneId, PaneId)>,
    shell: Option<String>,
    themes: Themes,
    /// The theme currently drawn (differs from `theme_id` only while the palette previews another one).
    theme: Theme,
    /// The chosen theme.
    theme_id: String,
    /// Theme command id to theme id.
    theme_cmds: HashMap<String, String>,
    palette: Option<palette::Palette>,
    state: State,
    writer: Option<state::Writer>,
    state_dirty: bool,
    font_size: f32,
    start_maximized: bool,
    fullscreen: bool,
    /// Config values last seen, to tell a real config edit from an unrelated reload.
    config_font: f32,
    config_theme: String,
    /// Latest config/theme/state problem, shown in the status line.
    problem: Option<String>,
    live_rx: Option<LiveRx>,
    bars: Bars,
    search: Option<SearchBar>,
    explorer: Option<mahzenterm_explorer::Explorer>,
    explorer_focus: bool,
    /// `[modules.editor] command`.
    editor_pref: Option<String>,
    usage_settings: mahzenterm_ai_usage::Settings,
    usage_visible: bool,
    usage_snap: mahzenterm_ai_usage::Snapshot,
    /// Where the user dragged the card (window pixels); `None` follows the configured anchor.
    usage_pos: Option<(f32, f32)>,
    /// Pointer position inside the card header, kept to grab the card at that spot.
    usage_grab: Point,
    usage_drag: bool,
    usage_worker: crate::usage::Worker,
    osc52: Osc52Policy,
    /// A program's pending clipboard write, waiting for the user's answer.
    clip_prompt: Option<(PaneKey, String)>,
    stats: Stats,
    /// Latest output line of each command widget.
    outputs: HashMap<String, String>,
    feeder: feed::Feeder,
    feed_rx: FeedRx,
    last_plan: feed::Plan,
    /// Time base of scrolling widgets.
    started: Instant,
}

/// The editor named in `[modules.editor]`; a malformed section falls back to auto-detection.
fn editor_pref(config: &Config) -> Option<String> {
    config
        .module::<mahzenterm_editor_bridge::Settings>("editor")
        .ok()
        .and_then(|s| s.command)
        .filter(|c| !c.trim().is_empty())
}

/// `[modules.ai_usage]`; a malformed section falls back to the defaults.
fn usage_settings(config: &Config) -> mahzenterm_ai_usage::Settings {
    config.module("ai_usage").unwrap_or_default()
}

/// Inputs that only the real application provides (tests leave them empty).
struct Startup {
    state: State,
    writer: Option<state::Writer>,
    live: Option<LiveRx>,
    themes: Themes,
    problem: Option<String>,
}

impl Default for Startup {
    fn default() -> Self {
        Self {
            state: State::default(),
            writer: None,
            live: None,
            themes: Themes::builtin(),
            problem: None,
        }
    }
}

const MIN_FONT: f32 = 6.0;
const MAX_FONT: f32 = 72.0;

/// Drop saved working directories that no longer exist, so a restored pane never fails to start.
fn sanitize_window(window: &mut Window) {
    for tab in &mut window.tabs {
        for pid in tab.pane_ids() {
            if let Some(pane) = tab.pane_mut(pid) {
                if let PaneKind::Terminal { cwd, shell, args } = &mut pane.kind {
                    if cwd.as_deref().is_some_and(|d| !d.is_dir()) {
                        *cwd = None;
                    }
                    // A one-shot program pane (file preview) is not brought back as it was.
                    if !args.is_empty() {
                        args.clear();
                        *shell = None;
                    }
                }
            }
        }
    }
}

/// Messages handled by the shell.
#[derive(Debug, Clone)]
pub enum Message {
    /// New output (or exit) in a pane.
    Wake(PaneKey),
    Key(keyboard::Event),
    Resized(Size),
    View(PaneKey, ViewAction),
    Paste(Option<String>),
    FocusPane(PaneKey),
    SelectTab(TabId),
    CloseTab(TabId),
    NewTab,
    SashPress {
        a_first: PaneId,
        b_first: PaneId,
    },
    MouseMoved(Point),
    MouseReleased,
    /// Header of the AI usage card pressed / pointer over it (card-relative).
    UsagePress,
    UsageHover(Point),
    /// Pointer entered or left a divider.
    SashHover(Option<(PaneId, PaneId)>),
    /// Click on a palette row.
    PaletteRun(usize),
    /// Click outside the palette.
    PaletteClose,
    /// The config files changed on disk.
    ConfigChanged(live::Reload),
    SearchStep(bool),
    SearchClose,
    SearchRegex,
    Explorer(mahzenterm_explorer::Event),
    /// Answer to the clipboard-write prompt.
    ClipboardAllow,
    ClipboardDeny,
    /// A pane's own button: focus that pane, then run the command.
    PaneCommand(PaneKey, &'static str),
    /// Data for a bar widget.
    Feed(Feed),
    /// A widget was clicked: run this command id.
    RunCommand(String),
    /// Press on the title bar: start moving the window.
    DragWindow,
    Minimize,
    ToggleMaximize,
    CloseWindow,
    /// Press on a window edge grip.
    ResizeEdge(window::Direction),
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

/// Load the layered config once at startup; problems are logged and summarised, defaults apply.
fn load_config(loader: &mut Loader) -> (Config, Option<String>) {
    let loaded = loader.load();
    for issue in &loaded.issues {
        tracing::warn!(%issue, "config problem");
    }
    (loaded.config, live::describe(&loaded.issues))
}

impl Shell {
    /// Application entry: the shell plus the task that maximizes the window when configured.
    pub fn boot() -> (Self, Task<Message>) {
        let shell = Self::new();
        let task = if shell.start_maximized {
            window::latest().and_then(|id| window::maximize(id, true))
        } else {
            Task::none()
        };
        (shell, task)
    }

    /// Whether the window should draw its own title bar (read before the window exists).
    pub fn wants_custom_titlebar() -> bool {
        let Some(dir) = mahzenterm_platform::config_dir() else {
            return Config::default().window.custom_titlebar;
        };
        Loader::new(dir.join("config.toml"), None)
            .load()
            .config
            .window
            .custom_titlebar
    }

    pub fn new() -> Self {
        let Some(dir) = mahzenterm_platform::config_dir() else {
            return Self::with_config(Config::default());
        };
        let project = std::env::current_dir()
            .ok()
            .map(|d| d.join(".mahzenterm").join("config.toml"));
        let mut loader = Loader::new(dir.join("config.toml"), project.clone());
        let (config, config_problem) = load_config(&mut loader);
        let mut themes = Themes::builtin();
        let theme_errors = themes.load_user_dir(&dir.join("themes"));
        for e in &theme_errors {
            tracing::warn!(error = %e, "theme problem");
        }
        let (state, state_problem) = state::load(&dir.join("state.toml"));
        let problem = config_problem
            .or_else(|| theme_errors.first().map(ToString::to_string))
            .or(state_problem);
        let live = Arc::new(Mutex::new(Some(live::spawn(Loader::new(
            dir.join("config.toml"),
            project,
        )))));
        Self::build(
            config,
            Startup {
                state,
                writer: Some(state::Writer::spawn(dir.join("state.toml"))),
                live: Some(live),
                themes,
                problem,
            },
        )
    }

    /// Build the shell from an explicit config (tests use defaults, never the user's files).
    pub fn with_config(config: Config) -> Self {
        Self::build(config, Startup::default())
    }

    fn build(config: Config, startup: Startup) -> Self {
        let Startup {
            state,
            writer,
            live,
            themes,
            mut problem,
        } = startup;
        let registry = commands::builtin_registry(&themes);
        let theme_cmds: HashMap<String, String> = themes
            .all()
            .iter()
            .map(|t| (commands::theme_command_id(&t.id), t.id.clone()))
            .collect();
        let (keymap, issues) = Keymap::build(&registry, &config.keys);
        for issue in &issues {
            tracing::warn!(?issue, "keymap problem");
        }
        if problem.is_none() && !issues.is_empty() {
            problem = Some(format!("key binding problem: {:?}", issues[0]));
        }
        let wanted = state
            .theme
            .clone()
            .filter(|t| themes.get(t).is_some())
            .unwrap_or_else(|| config.appearance.theme.clone());
        let theme = themes
            .get(&wanted)
            .or_else(|| themes.default_theme())
            .cloned()
            .unwrap_or_else(|| Themes::builtin().all()[0].clone());
        if themes.get(&config.appearance.theme).is_none() && problem.is_none() {
            problem = Some(format!(
                "appearance.theme \"{}\" not found; choose one in the palette (Color Theme)",
                config.appearance.theme
            ));
        }
        let font_size = state
            .font_size
            .unwrap_or(config.terminal.font_size)
            .clamp(MIN_FONT, MAX_FONT);
        let mut window = Window::new();
        if config.session.restore {
            if let Some(saved) = state.window.clone() {
                window = saved;
                sanitize_window(&mut window);
            }
        }
        let (tx, rx) = mpsc::unbounded::<PaneKey>();
        let (feed_tx, feed_rx) = mpsc::unbounded::<Feed>();
        let usage_settings = usage_settings(&config);
        let (claude_dir, codex_dir) =
            mahzenterm_ai_usage::Scanner::default_dirs(mahzenterm_platform::home_dir());
        let usage_worker = crate::usage::Worker::spawn(
            mahzenterm_ai_usage::Scanner::new(
                usage_settings
                    .claude_dir
                    .clone()
                    .map(PathBuf::from)
                    .or(claude_dir),
                usage_settings
                    .codex_dir
                    .clone()
                    .map(PathBuf::from)
                    .or(codex_dir),
            ),
            feed_tx.clone(),
        );
        let usage_visible = state.usage_visible.unwrap_or(usage_settings.visible);
        let usage_pos = state.usage_pos.map(|[x, y]| (x, y));
        let mut shell = Self {
            window,
            runtimes: HashMap::new(),
            registry,
            keymap,
            pending: Vec::new(),
            metrics: Metrics::for_font_size(font_size),
            win_size: INITIAL_SIZE,
            wake_tx: tx,
            wake_rx: Arc::new(Mutex::new(Some(rx))),
            tokens: theme.tokens,
            drag: None,
            hover: None,
            shell: config.terminal.shell.clone(),
            theme_id: theme.id.clone(),
            theme,
            themes,
            theme_cmds,
            palette: None,
            state,
            writer,
            state_dirty: false,
            font_size,
            start_maximized: config.window.maximized,
            fullscreen: false,
            config_font: config.terminal.font_size,
            config_theme: config.appearance.theme.clone(),
            problem,
            live_rx: live,
            bars: Bars::from_config(&config),
            search: None,
            explorer: None,
            explorer_focus: false,
            editor_pref: editor_pref(&config),
            usage_settings,
            usage_visible,
            usage_snap: mahzenterm_ai_usage::Snapshot::default(),
            usage_pos,
            usage_grab: Point::ORIGIN,
            usage_drag: false,
            usage_worker,
            osc52: config.terminal.osc52,
            clip_prompt: None,
            stats: Stats::default(),
            outputs: HashMap::new(),
            feeder: feed::Feeder::spawn(feed_tx),
            feed_rx: Arc::new(Mutex::new(Some(feed_rx))),
            last_plan: feed::Plan::default(),
            started: Instant::now(),
        };
        shell.refresh_feed();
        if shell.usage_visible {
            shell.usage_worker.show(shell.usage_settings.period());
        }
        shell.sync();
        shell
    }

    fn status_h(&self) -> f32 {
        if self.bars.statusbar.visible {
            STATUS_H
        } else {
            0.0
        }
    }

    fn content_area(&self) -> Size {
        Size::new(
            (self.win_size.width - self.dock_w()).max(1.0),
            (self.win_size.height - TAB_BAR_H - self.status_h()).max(1.0),
        )
    }

    /// Tell the feed thread what the visible widgets need (a no-op when nothing changed).
    fn refresh_feed(&mut self) {
        let plan = feed::plan(&self.bars.tabbar, &self.bars.statusbar, &self.bars.widgets);
        if plan != self.last_plan {
            self.feeder.set(plan.clone());
            self.last_plan = plan;
        }
    }

    fn focused_key(&self) -> PaneKey {
        let tab = self.window.active_tab();
        (tab.id, tab.focus)
    }

    fn focused_runtime(&self) -> Option<&PaneRuntime> {
        self.runtimes.get(&self.focused_key())
    }

    /// Working directory of the focused pane's shell, so new panes open "here".
    fn focused_cwd(&self) -> Option<PathBuf> {
        let pid = self.focused_runtime()?.session()?.pid()?;
        mahzenterm_platform::process_cwd(pid)
    }

    fn new_terminal_kind(&self) -> PaneKind {
        PaneKind::Terminal {
            cwd: self.focused_cwd(),
            shell: None,
            args: Vec::new(),
        }
    }

    /// Bring runtimes in line with the model: drop dead panes, spawn missing ones, resize the active tab's grids.
    fn sync(&mut self) {
        let live: HashSet<PaneKey> = self
            .window
            .tabs
            .iter()
            .flat_map(|t| t.pane_ids().into_iter().map(move |p| (t.id, p)))
            .collect();
        self.runtimes.retain(|k, _| live.contains(k));

        let area = self.content_area();
        let tab = self.window.active_tab();
        for p in layout::panes(tab, area) {
            let key = (tab.id, p.id);
            let grid = self.metrics.grid_for(p.body.size());
            if let Some(rt) = self.runtimes.get_mut(&key) {
                rt.resize(grid, self.metrics);
            } else if let Some(pane) = tab.pane(p.id) {
                let rt = PaneRuntime::spawn(
                    &pane.kind,
                    grid,
                    self.metrics,
                    key,
                    self.wake_tx.clone(),
                    self.shell.as_deref(),
                    self.theme.palette.clone(),
                );
                self.runtimes.insert(key, rt);
            }
        }
    }

    fn invalidate_active(&mut self) {
        let tab_id = self.window.active;
        for ((t, _), rt) in &mut self.runtimes {
            if *t == tab_id {
                rt.cache.clear();
            }
        }
    }

    fn flag(&self, name: &str) -> bool {
        let tab = self.window.active_tab();
        match name {
            "tabs.multiple" => self.window.tabs.len() > 1,
            "panes.multiple" => tab.pane_ids().len() > 1,
            "pane.zoomed" => tab.zoomed.is_some(),
            n => {
                if let Some(k) = n.strip_prefix("panes.count_ge_") {
                    return k.parse::<usize>().is_ok_and(|k| tab.pane_ids().len() >= k);
                }
                n.strip_prefix("tabs.count_ge_")
                    .and_then(|k| k.parse::<usize>().ok())
                    .is_some_and(|k| self.window.tabs.len() >= k)
            }
        }
    }

    fn set_focus(&mut self, key: PaneKey) {
        if key.0 != self.window.active {
            return;
        }
        let tab = self.window.active_tab_mut();
        if tab.focus != key.1 && tab.pane(key.1).is_some() {
            tab.focus = key.1;
            self.state_dirty = true;
            self.invalidate_active();
        }
    }

    /// Close a pane; closing a tab's last pane closes the tab, and the last tab's last pane quits.
    fn close_pane(&mut self, key: PaneKey) -> Task<Message> {
        let Some(tab) = self.window.tabs.iter_mut().find(|t| t.id == key.0) else {
            return Task::none();
        };
        if tab.pane_ids().len() > 1 {
            let _ = tab.close(key.1);
        } else if self.window.tabs.len() > 1 {
            let _ = self.window.close_tab(key.0);
        } else {
            return self.quit();
        }
        self.state_dirty = true;
        self.sync();
        self.invalidate_active();
        Task::none()
    }

    /// Save the layout, wait for the state file to be written, stop all sessions and exit.
    fn quit(&mut self) -> Task<Message> {
        self.persist();
        if let Some(w) = &mut self.writer {
            w.flush();
        }
        self.runtimes.clear();
        iced::exit()
    }

    /// Queue the current state for writing (off the UI thread). Pane directories are read fresh from the OS.
    fn persist(&mut self) {
        let Some(writer) = &self.writer else {
            return;
        };
        let mut window = self.window.clone();
        for tab in &mut window.tabs {
            for pid in tab.pane_ids() {
                let cwd = self
                    .runtimes
                    .get(&(tab.id, pid))
                    .and_then(PaneRuntime::session)
                    .and_then(Session::pid)
                    .and_then(mahzenterm_platform::process_cwd);
                if let (Some(cwd), Some(pane)) = (cwd, tab.pane_mut(pid)) {
                    if let PaneKind::Terminal { cwd: slot, .. } = &mut pane.kind {
                        *slot = Some(cwd);
                    }
                }
            }
        }
        let snapshot = State {
            version: state::VERSION,
            theme: Some(self.theme_id.clone()),
            font_size: Some(self.font_size),
            recent: self.state.recent.clone(),
            window: Some(window),
            usage_visible: Some(self.usage_visible),
            usage_pos: self.usage_pos.map(|(x, y)| [x, y]),
        };
        match snapshot.to_text() {
            Ok(text) => writer.save(text),
            Err(e) => tracing::warn!(error = %e, "cannot serialize state"),
        }
    }

    /// Draw `id` (palette preview or a committed choice) on the chrome and every session.
    fn show_theme(&mut self, id: &str) {
        let Some(theme) = self.themes.get(id).cloned() else {
            return;
        };
        self.tokens = theme.tokens;
        for rt in self.runtimes.values_mut() {
            if let Some(session) = rt.session() {
                session.set_palette(theme.palette.clone());
            }
            rt.refresh();
        }
        self.theme = theme;
    }

    /// Choose a theme for good (remembered across restarts).
    fn commit_theme(&mut self, id: &str) {
        self.show_theme(id);
        self.theme_id = id.to_owned();
        self.state_dirty = true;
    }

    fn set_font_size(&mut self, size: f32) {
        let size = size.clamp(MIN_FONT, MAX_FONT);
        if (size - self.font_size).abs() < f32::EPSILON {
            return;
        }
        self.font_size = size;
        self.metrics = Metrics::for_font_size(size);
        for rt in self.runtimes.values_mut() {
            rt.grid = (0, 0); // force a resize with the new cell size, also for background tabs on activation
            rt.cache.clear();
        }
        self.sync();
        self.state_dirty = true;
    }

    fn open_palette(&mut self) {
        self.pending.clear();
        let results = self.palette_results("");
        self.palette = Some(palette::Palette::new(results, self.theme_id.clone()));
    }

    fn palette_results(&self, query: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .registry
            .search(query, &|n| self.flag(n), &self.state.recent)
            .into_iter()
            .map(|c| c.id.clone())
            .collect();
        // Among theme commands the chosen theme comes first, so the live preview never starts by flashing
        // some other (possibly light) theme just because it sorts first alphabetically.
        let current = commands::theme_command_id(&self.theme_id);
        let first_theme = ids.iter().position(|c| self.theme_cmds.contains_key(c));
        if let (Some(at), Some(cur)) = (first_theme, ids.iter().position(|c| *c == current)) {
            if cur > at {
                let c = ids.remove(cur);
                ids.insert(at, c);
            }
        }
        ids
    }

    /// Show the highlighted theme command live; anything else shows the theme the palette opened with.
    fn preview_selected(&mut self) {
        let Some(p) = &self.palette else {
            return;
        };
        let wanted = p
            .current()
            .and_then(|c| self.theme_cmds.get(c))
            .cloned()
            .unwrap_or_else(|| p.origin_theme.clone());
        if wanted != self.theme.id {
            self.show_theme(&wanted);
        }
    }

    fn close_palette(&mut self, revert: bool) {
        if let Some(p) = self.palette.take() {
            if revert && self.theme.id != p.origin_theme {
                self.show_theme(&p.origin_theme);
            }
        }
    }

    fn run_from_palette(&mut self, command: &str) -> Task<Message> {
        self.close_palette(false);
        self.run_command(command)
    }

    fn palette_key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
    ) -> Task<Message> {
        // The palette shortcut toggles it closed.
        if let Some(chord) = commands::chord_from(key, modifiers) {
            if self.keymap.resolve(&[chord], &|n| self.flag(n))
                == Resolution::Matched(id::PALETTE_OPEN.into())
            {
                self.close_palette(true);
                return Task::none();
            }
        }
        let Some(mut p) = self.palette.take() else {
            return Task::none();
        };
        let outcome = p.on_key(palette::classify(key, modifiers, text));
        match outcome {
            Outcome::Idle => self.palette = Some(p),
            Outcome::Changed { query_changed } => {
                if query_changed {
                    let results = self.palette_results(&p.query);
                    p.set_results(results);
                }
                self.palette = Some(p);
                self.preview_selected();
            }
            Outcome::Close => {
                self.palette = Some(p);
                self.close_palette(true);
            }
            Outcome::Run(cmd) => {
                self.palette = Some(p);
                return self.run_from_palette(&cmd);
            }
        }
        Task::none()
    }

    /// Apply a config reload: key bindings, font, shell and theme follow real edits to the file.
    fn apply_config(&mut self, config: &Config, problem: Option<String>) {
        let (keymap, issues) = Keymap::build(&self.registry, &config.keys);
        self.keymap = keymap;
        self.shell.clone_from(&config.terminal.shell);
        self.osc52 = config.terminal.osc52;
        self.editor_pref = editor_pref(config);
        self.usage_settings = usage_settings(config);
        if self.usage_visible {
            self.usage_worker.show(self.usage_settings.period());
        }
        let sized = self.bars.statusbar.visible != config.statusbar.visible;
        let custom = self.bars.custom_titlebar;
        self.bars = Bars::from_config(config);
        self.bars.custom_titlebar = custom; // the window frame cannot change while running
        self.refresh_feed();
        self.problem = problem.or_else(|| {
            issues
                .first()
                .map(|i| format!("key binding problem: {i:?}"))
        });
        if sized {
            self.sync();
            self.invalidate_active();
        }
        if (config.terminal.font_size - self.config_font).abs() > f32::EPSILON {
            self.config_font = config.terminal.font_size;
            self.set_font_size(config.terminal.font_size);
        }
        if config.appearance.theme != self.config_theme {
            self.config_theme.clone_from(&config.appearance.theme);
            if self.themes.get(&config.appearance.theme).is_some() {
                self.commit_theme(&config.appearance.theme);
            } else if self.problem.is_none() {
                self.problem = Some(format!(
                    "appearance.theme \"{}\" not found; choose one in the palette (Color Theme)",
                    config.appearance.theme
                ));
            }
        }
    }

    fn close_tab(&mut self, id: TabId) {
        if self.window.close_tab(id).is_ok() {
            self.sync();
            self.invalidate_active();
        }
    }

    fn split_focused(&mut self, axis: Axis) {
        let kind = self.new_terminal_kind();
        let tab = self.window.active_tab_mut();
        let target = tab.focus;
        if tab.split(target, axis, kind).is_ok() {
            self.sync();
            self.invalidate_active();
        }
    }

    /// Focus the pane at `index` (0-based, in tab order); a zoomed pane hands its zoom to the new one.
    fn focus_index(&mut self, index: usize) {
        let tab = self.window.active_tab_mut();
        let Some(&target) = tab.pane_ids().get(index) else {
            return;
        };
        if tab.focus == target {
            return;
        }
        tab.focus = target;
        if tab.zoomed.is_some() {
            tab.zoomed = Some(target);
        }
        self.state_dirty = true;
        self.sync();
        self.invalidate_active();
    }

    /// Focus the pane `step` places from the current one, wrapping around.
    fn focus_step(&mut self, step: isize) {
        let tab = self.window.active_tab();
        let ids = tab.pane_ids();
        let Some(at) = ids.iter().position(|p| *p == tab.focus) else {
            return;
        };
        let next = (at as isize + step).rem_euclid(ids.len() as isize) as usize;
        self.focus_index(next);
    }

    fn focus_dir(&mut self, dir: Direction) {
        if self.window.active_tab_mut().focus_dir(dir).is_some() {
            self.sync();
            self.invalidate_active();
        }
    }

    fn move_border(&mut self, axis: Axis, delta: f32) {
        let tab = self.window.active_tab_mut();
        let target = tab.focus;
        let _ = tab.move_border(target, axis, delta);
        self.sync();
    }

    fn run_command(&mut self, command: &str) -> Task<Message> {
        if self.registry.get(command).is_none() {
            tracing::warn!(command, "ignoring unregistered command");
            return Task::none();
        }
        if command != id::PALETTE_OPEN {
            self.state.touch_recent(command);
        }
        self.state_dirty = true;
        match command {
            id::PALETTE_OPEN => self.open_palette(),
            id::THEME_NEXT | id::THEME_PREV => {
                let step = if command == id::THEME_NEXT { 1 } else { -1 };
                if let Some(next) = self
                    .themes
                    .cycle(&self.theme_id, step)
                    .map(|t| t.id.clone())
                {
                    self.commit_theme(&next);
                }
            }
            id::PROMPT_PREV | id::PROMPT_NEXT => {
                if let Some(session) = self.focused_runtime().and_then(PaneRuntime::session) {
                    if !session.jump_prompt(command == id::PROMPT_PREV) && !session.has_prompts() {
                        self.problem = Some(
                            "no prompt marks yet: enable shell integration (docs/shell-integration.md)"
                                .into(),
                        );
                    }
                }
            }
            mahzenterm_explorer::id::TOGGLE => self.toggle_explorer(),
            mahzenterm_explorer::id::FOCUS => self.focus_explorer(),
            mahzenterm_editor_bridge::id::OPEN_HERE => self.open_editor_here(),
            mahzenterm_ai_usage::id::TOGGLE => self.toggle_usage(),
            mahzenterm_ai_usage::id::RESET_POSITION => self.reset_usage_position(),
            id::SEARCH_OPEN => self.open_search(),
            id::SEARCH_NEXT => self.search_step(true),
            id::SEARCH_PREV => self.search_step(false),
            id::FONT_LARGER => self.set_font_size(self.font_size + 1.0),
            id::FONT_SMALLER => self.set_font_size(self.font_size - 1.0),
            id::FONT_RESET => self.set_font_size(self.config_font),
            id::APP_QUIT => return self.quit(),
            id::WINDOW_FULLSCREEN => {
                self.fullscreen = !self.fullscreen;
                let mode = if self.fullscreen {
                    window::Mode::Fullscreen
                } else {
                    window::Mode::Windowed
                };
                return window::latest().and_then(move |id| window::set_mode(id, mode));
            }
            id::WINDOW_MAXIMIZE => {
                return window::latest().and_then(window::toggle_maximize);
            }
            id::TAB_NEW => {
                let kind = self.new_terminal_kind();
                self.window.new_tab(kind);
                self.sync();
            }
            id::TAB_CLOSE => self.close_tab(self.window.active),
            id::TAB_NEXT | id::TAB_PREV => {
                self.window
                    .cycle(if command == id::TAB_NEXT { 1 } else { -1 });
                self.sync();
                self.invalidate_active();
            }
            id::PANE_SPLIT_RIGHT => self.split_focused(Axis::Horizontal),
            id::PANE_SPLIT_DOWN => self.split_focused(Axis::Vertical),
            id::PANE_CLOSE => return self.close_pane(self.focused_key()),
            id::PANE_ZOOM => {
                let tab = self.window.active_tab_mut();
                let target = tab.zoomed.unwrap_or(tab.focus);
                let _ = tab.toggle_zoom(target);
                self.sync();
                self.invalidate_active();
            }
            id::PANE_NEXT => self.focus_step(1),
            id::PANE_PREV => self.focus_step(-1),
            id::PANE_FOCUS_LEFT => self.focus_dir(Direction::Left),
            id::PANE_FOCUS_RIGHT => self.focus_dir(Direction::Right),
            id::PANE_FOCUS_UP => self.focus_dir(Direction::Up),
            id::PANE_FOCUS_DOWN => self.focus_dir(Direction::Down),
            id::PANE_GROW_LEFT => self.move_border(Axis::Horizontal, -RESIZE_STEP),
            id::PANE_GROW_RIGHT => self.move_border(Axis::Horizontal, RESIZE_STEP),
            id::PANE_GROW_UP => self.move_border(Axis::Vertical, -RESIZE_STEP),
            id::PANE_GROW_DOWN => self.move_border(Axis::Vertical, RESIZE_STEP),
            id::PANE_SWAP_NEXT => {
                let tab = self.window.active_tab_mut();
                let ids = tab.pane_ids();
                if let Some(i) = ids.iter().position(|p| *p == tab.focus) {
                    let next = ids[(i + 1) % ids.len()];
                    let _ = tab.swap(tab.focus, next);
                }
                self.sync();
                self.invalidate_active();
            }
            id::EDIT_COPY => {
                return self
                    .focused_runtime()
                    .and_then(PaneRuntime::session)
                    .and_then(Session::selection_text)
                    .map_or_else(Task::none, clipboard::write);
            }
            id::EDIT_PASTE => return clipboard::read().map(Message::Paste),
            other => {
                if let Some(n) = other
                    .strip_prefix("pane.goto_")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    self.focus_index(n.saturating_sub(1));
                } else if let Some(n) = other
                    .strip_prefix("tab.goto_")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    if let Some(tab) = self.window.tabs.get(n.saturating_sub(1)) {
                        let tid = tab.id;
                        let _ = self.window.activate(tid);
                        self.sync();
                        self.invalidate_active();
                    }
                } else if let Some(theme) = self.theme_cmds.get(other).cloned() {
                    self.commit_theme(&theme);
                } else {
                    tracing::warn!(command = other, "unhandled command");
                }
            }
        }
        Task::none()
    }

    pub fn title(&self) -> String {
        match self.focused_runtime() {
            Some(rt) if !rt.title.is_empty() => format!("{} - {}", rt.title, mahzenterm_core::NAME),
            Some(rt) if rt.cwd_label.is_some() => {
                format!("{} - {}", rt.display_title(), mahzenterm_core::NAME)
            }
            _ => format!("{} {}", mahzenterm_core::NAME, mahzenterm_core::VERSION),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            keyboard::listen().map(Message::Key),
            window::resize_events().map(|(_, size)| Message::Resized(size)),
            Subscription::run_with(WakeSource(self.wake_rx.clone()), wake_stream),
        ];
        if self.drag.is_some() || self.usage_drag {
            subs.push(event::listen_with(drag_event));
        }
        subs.push(Subscription::run_with(
            FeedSource(self.feed_rx.clone()),
            feed_stream,
        ));
        if let Some(rx) = &self.live_rx {
            subs.push(Subscription::run_with(LiveSource(rx.clone()), live_stream));
        }
        Subscription::batch(subs)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.update_inner(message);
        self.sync_search();
        if self.state_dirty {
            self.state_dirty = false;
            self.persist();
        }
        task
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Wake(key) => {
                let exited = self
                    .runtimes
                    .get(&key)
                    .and_then(PaneRuntime::session)
                    .is_some_and(Session::exited);
                if exited {
                    return self.close_pane(key);
                }
                if let Some(rt) = self.runtimes.get_mut(&key) {
                    let before = rt.cwd_label.clone();
                    rt.refresh();
                    if rt.cwd_label != before {
                        self.state_dirty = true; // a pane changed directory: remember it for restore
                    }
                    if let Some(text) = rt.session().and_then(Session::take_clipboard_request) {
                        return self.clipboard_request(key, text);
                    }
                }
            }
            Message::ClipboardAllow => {
                if let Some((_, text)) = self.clip_prompt.take() {
                    return clipboard::write(text);
                }
            }
            Message::ClipboardDeny => self.clip_prompt = None,
            Message::Resized(size) => {
                self.win_size = size;
                self.sync();
            }
            Message::FocusPane(key) => {
                self.explorer_focus = false;
                self.set_focus(key);
            }
            Message::SelectTab(tab) => {
                if self.window.activate(tab).is_ok() {
                    self.state_dirty = true;
                    self.sync();
                    self.invalidate_active();
                }
            }
            Message::CloseTab(tab) => self.close_tab(tab),
            Message::NewTab => return self.run_command(id::TAB_NEW),
            Message::SashPress { a_first, b_first } => {
                let area = self.content_area();
                let found = layout::sashes(self.window.active_tab(), area)
                    .into_iter()
                    .find(|s| s.a_first == a_first && s.b_first == b_first);
                self.drag = found.map(|s| Drag {
                    a_first,
                    b_first,
                    axis: s.axis,
                    split: s.split,
                });
            }
            Message::UsagePress => self.usage_press(),
            Message::UsageHover(p) => self.usage_hover(p),
            Message::MouseMoved(pos) => {
                if self.usage_drag {
                    self.usage_drag_to(pos);
                    return Task::none();
                }
                if let Some(d) = self.drag {
                    let p = Point::new(pos.x - self.dock_w(), pos.y - TAB_BAR_H);
                    let ratio = layout::ratio_at(d.axis, d.split, p);
                    self.window
                        .active_tab_mut()
                        .set_split_ratio(d.a_first, d.b_first, ratio);
                    self.sync();
                }
            }
            Message::MouseReleased => {
                if std::mem::take(&mut self.usage_drag) {
                    self.state_dirty = true;
                    return Task::none();
                }
                if self.drag.take().is_some() {
                    self.state_dirty = true;
                }
            }
            Message::PaletteRun(i) => {
                let cmd = self.palette.as_mut().and_then(|p| {
                    p.selected = i;
                    p.current().map(str::to_owned)
                });
                if let Some(cmd) = cmd {
                    return self.run_from_palette(&cmd);
                }
            }
            Message::PaletteClose => self.close_palette(true),
            Message::ConfigChanged((config, problem)) => self.apply_config(&config, problem),
            Message::SashHover(h) => self.hover = h,
            Message::Feed(Feed::Redraw) => {}
            Message::Feed(Feed::Stats(s)) => self.stats = s,
            Message::Feed(Feed::Usage(u)) => self.set_usage_snapshot(u),
            Message::Feed(Feed::Output(id, text)) => {
                self.outputs.insert(id, text);
            }
            Message::SearchStep(forward) => self.search_step(forward),
            Message::SearchClose => self.close_search(),
            Message::SearchRegex => self.toggle_regex(),
            Message::Explorer(e) => self.on_explorer(e),
            Message::PaneCommand(key, cmd) => {
                self.set_focus(key);
                return self.run_command(cmd);
            }
            Message::RunCommand(cmd) => return self.run_command(&cmd),
            Message::DragWindow => return window::latest().and_then(window::drag),
            Message::Minimize => return window::latest().and_then(|id| window::minimize(id, true)),
            Message::ToggleMaximize => return window::latest().and_then(window::toggle_maximize),
            Message::CloseWindow => return self.quit(),
            Message::ResizeEdge(dir) => {
                return window::latest().and_then(move |id| window::drag_resize(id, dir));
            }
            Message::View(key, action) => self.on_view_action(key, action),
            Message::Paste(Some(text)) => {
                if let Some(s) = self.focused_runtime().and_then(PaneRuntime::session) {
                    s.paste(&text);
                }
            }
            Message::Paste(None) => {}
            Message::Key(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            }) => return self.on_key(&key, modifiers, text.as_deref()),
            Message::Key(_) => {}
        }
        Task::none()
    }

    /// A program asked to put `text` on the clipboard: honour the policy, or ask.
    fn clipboard_request(&mut self, key: PaneKey, text: String) -> Task<Message> {
        match self.osc52 {
            Osc52Policy::Deny => Task::none(),
            Osc52Policy::Allow => clipboard::write(text),
            Osc52Policy::Ask => {
                self.clip_prompt = Some((key, text));
                Task::none()
            }
        }
    }

    fn search_session(&self) -> Option<&Session> {
        let bar = self.search.as_ref()?;
        self.runtimes.get(&bar.pane).and_then(PaneRuntime::session)
    }

    fn open_search(&mut self) {
        if self.search.is_some() {
            return;
        }
        let pane = self.focused_key();
        // A one-line selection becomes the first query.
        let query = self
            .focused_runtime()
            .and_then(PaneRuntime::session)
            .and_then(Session::selection_text)
            .filter(|t| !t.contains('\n') && t.chars().count() <= 80)
            .unwrap_or_default();
        self.search = Some(SearchBar {
            query,
            regex: false,
            pane,
        });
        if let Some(bar) = &self.search {
            if let Some(s) = self.search_session() {
                s.search(&bar.query, bar.regex);
            }
        }
    }

    fn toggle_regex(&mut self) {
        if let Some(bar) = self.search.as_mut() {
            bar.regex = !bar.regex;
        }
        self.edit_search(|_| {});
    }

    fn edit_search(&mut self, edit: impl FnOnce(&mut String)) {
        if let Some(bar) = self.search.as_mut() {
            edit(&mut bar.query);
        }
        if let (Some(bar), Some(s)) = (&self.search, self.search_session()) {
            s.search(&bar.query, bar.regex);
        }
    }

    /// F3 / buttons: step through matches; with no search open, F3 opens one.
    fn search_step(&mut self, forward: bool) {
        match self.search_session() {
            Some(s) => {
                s.search_step(forward);
            }
            None => self.open_search(),
        }
    }

    fn close_search(&mut self) {
        if let Some(s) = self.search_session() {
            s.search_clear();
        }
        self.search = None;
    }

    /// The search belongs to one pane: close it when focus moves elsewhere.
    fn sync_search(&mut self) {
        if self
            .search
            .as_ref()
            .is_some_and(|b| b.pane != self.focused_key())
        {
            self.close_search();
        }
    }

    fn on_view_action(&mut self, key: PaneKey, action: ViewAction) {
        if matches!(action, ViewAction::Press { .. }) {
            self.set_focus(key);
        }
        if let ViewAction::OpenLink(url) = action {
            if let Err(e) = mahzenterm_platform::open_url(&url) {
                self.problem = Some(format!("cannot open {}: {e}", truncate(&url, 40)));
            }
            return;
        }
        let action = match action {
            ViewAction::OpenToken { token, col, row } => {
                if self.open_token(&token) {
                    return;
                }
                ViewAction::Press {
                    col,
                    row,
                    clicks: 1,
                }
            }
            other => other,
        };
        let Some(session) = self.runtimes.get(&key).and_then(PaneRuntime::session) else {
            return;
        };
        match action {
            ViewAction::Scroll(lines) => session.scroll(lines),
            ViewAction::Press { col, row, clicks } => match clicks {
                2 => session.select_word(col, row),
                3 => session.select_line(row),
                _ => {
                    session.select_clear();
                    session.select_start(col, row, false);
                }
            },
            ViewAction::Drag { col, row } => session.select_update(col, row),
            ViewAction::Release | ViewAction::OpenLink(_) | ViewAction::OpenToken { .. } => {}
        }
    }

    fn on_key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
    ) -> Task<Message> {
        if self.palette.is_some() {
            return self.palette_key(key, modifiers, text);
        }
        if self.explorer_focus && self.search.is_none() && self.explorer_key(key, modifiers, text) {
            return Task::none();
        }
        if self.search.is_some() {
            match search::classify(key, modifiers, text) {
                SearchKey::Close => self.close_search(),
                SearchKey::Next => self.search_step(true),
                SearchKey::Prev => self.search_step(false),
                SearchKey::Backspace => self.edit_search(|q| {
                    q.pop();
                }),
                SearchKey::ToggleRegex => self.toggle_regex(),
                SearchKey::Insert(t) => self.edit_search(|q| q.push_str(&t)),
                SearchKey::Other => {}
            }
            if !matches!(search::classify(key, modifiers, text), SearchKey::Other) {
                return Task::none();
            }
        }
        if let Some(chord) = commands::chord_from(key, modifiers) {
            let mut seq = std::mem::take(&mut self.pending);
            seq.push(chord);
            match self.keymap.resolve(&seq, &|n| self.flag(n)) {
                Resolution::Matched(cmd) => return self.run_command(&cmd),
                Resolution::Pending => {
                    self.pending = seq;
                    return Task::none();
                }
                // A failed multi-chord sequence is swallowed rather than leaking half a shortcut into the shell.
                Resolution::NoMatch if seq.len() > 1 => return Task::none(),
                Resolution::NoMatch => {}
            }
        }
        if let Some((k, m)) = to_term_key(key, text, modifiers) {
            if let Some(session) = self.focused_runtime().and_then(PaneRuntime::session) {
                session.select_clear();
                session.send_key(k, m);
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let t = self.tokens;
        let tab = self.window.active_tab();
        let area = self.content_area();
        let strips = layout::show_strips(tab);
        let mut layers: Vec<Element<'_, Message>> = Vec::new();
        let panes = layout::panes(tab, area);
        for p in &panes {
            layers.push(
                pin(self.pane_view(tab, p, strips))
                    .x(p.outer.x)
                    .y(p.outer.y)
                    .into(),
            );
        }
        // Several panes: tint the inactive ones and bracket the focused one's corners in the accent colour.
        if strips {
            let bar = |x: f32, y: f32, w: f32, h: f32| -> Element<'_, Message> {
                pin(container(Space::new())
                    .width(Length::Fixed(w))
                    .height(Length::Fixed(h))
                    .style(move |_| container::background(t.accent)))
                .x(x)
                .y(y)
                .into()
            };
            for p in &panes {
                let key = (tab.id, p.id);
                if tab.focus == p.id {
                    let (o, n, w) = (p.outer, 12.0, 2.0);
                    layers.push(bar(o.x, o.y, n, w));
                    layers.push(bar(o.x, o.y, w, n));
                    layers.push(bar(o.x + o.width - n, o.y + o.height - w, n, w));
                    layers.push(bar(o.x + o.width - w, o.y + o.height - n, w, n));
                } else if self.runtimes.contains_key(&key) {
                    layers.push(
                        pin(container(Space::new())
                            .width(Length::Fixed(p.outer.width))
                            .height(Length::Fixed(p.outer.height))
                            .style(move |_| container::background(with_alpha(t.surface, 0.30))))
                        .x(p.outer.x)
                        .y(p.outer.y)
                        .into(),
                    );
                }
            }
        }
        for s in layout::sashes(tab, area) {
            let id = (s.a_first, s.b_first);
            let lit =
                self.drag.is_some_and(|d| (d.a_first, d.b_first) == id) || self.hover == Some(id);
            layers.push(
                pin(container(Space::new())
                    .width(Length::Fixed(s.line.width))
                    .height(Length::Fixed(s.line.height))
                    .style(move |_| container::background(if lit { t.accent } else { t.border })))
                .x(s.line.x)
                .y(s.line.y)
                .into(),
            );
        }
        for s in layout::sashes(tab, area) {
            let id = (s.a_first, s.b_first);
            let cursor = match s.axis {
                Axis::Horizontal => mouse::Interaction::ResizingHorizontally,
                Axis::Vertical => mouse::Interaction::ResizingVertically,
            };
            let handle = Space::new()
                .width(Length::Fixed(s.hit.width))
                .height(Length::Fixed(s.hit.height));
            layers.push(
                pin(mouse_area(handle)
                    .on_press(Message::SashPress {
                        a_first: s.a_first,
                        b_first: s.b_first,
                    })
                    .on_enter(Message::SashHover(Some(id)))
                    .on_exit(Message::SashHover(None))
                    .interaction(cursor))
                .x(s.hit.x)
                .y(s.hit.y)
                .into(),
            );
        }
        if let Some(bar) = &self.search {
            if let Some(p) = panes.iter().find(|p| (tab.id, p.id) == bar.pane) {
                let status = self
                    .search_session()
                    .and_then(Session::search_status)
                    .unwrap_or((0, 0));
                let x = (p.outer.x + p.outer.width - search::WIDTH - 14.0).max(p.outer.x + 4.0);
                let y = p.outer.y + if strips { layout::STRIP_H + 6.0 } else { 10.0 };
                layers.push(pin(search::view(bar, status, t)).x(x).y(y).into());
            }
        }
        if let Some((key, text)) = &self.clip_prompt {
            if let Some(p) = panes.iter().find(|p| (tab.id, p.id) == *key) {
                let w = 460.0_f32.min((p.outer.width - 16.0).max(200.0));
                let x = p.outer.x + (p.outer.width - w) / 2.0;
                let y = p.outer.y + p.outer.height - 64.0;
                layers.push(
                    pin(clipboard_prompt(text, w, t))
                        .x(x)
                        .y(y.max(p.outer.y))
                        .into(),
                );
            }
        }
        let content = container(stack(layers).width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::background(t.bg));
        let middle: Element<'_, Message> = match self.dock_view() {
            Some(dock) => row![dock, content].into(),
            None => content.into(),
        };
        let mut base: Element<'_, Message> = if self.bars.statusbar.visible {
            column![self.tab_bar(), middle, self.status_bar()].into()
        } else {
            column![self.tab_bar(), middle].into()
        };
        if let Some(grips) = self.resize_grips() {
            base = stack![base, grips].into();
        }
        if let Some(card) = self.usage_view() {
            base = stack![base, card].into();
        }
        match &self.palette {
            Some(p) => stack![
                base,
                palette::view(
                    p,
                    &self.registry,
                    &self.keymap,
                    &self.themes,
                    &self.theme_cmds,
                    t
                )
            ]
            .into(),
            None => base,
        }
    }

    /// The thin title strip of a pane: index tag, title and grid size, closed by a hairline. Same background as
    /// the terminal, so it reads as part of the surface rather than a window title bar.
    fn pane_strip<'a>(
        &'a self,
        tab: &'a Tab,
        p: &layout::PanePx,
        rt: &'a PaneRuntime,
    ) -> Element<'a, Message> {
        let t = self.tokens;
        let focused = tab.focus == p.id;
        let key = (tab.id, p.id);
        let idx = tab
            .pane_ids()
            .iter()
            .position(|x| *x == p.id)
            .map_or(1, |i| i + 1);
        let title = tab
            .pane(p.id)
            .and_then(|pane| pane.title.clone())
            .unwrap_or_else(|| rt.display_title().to_owned());
        let mut line = row![
            chrome::tag(format!("{idx:02}"), t, focused),
            text(truncate(&title, 48))
                .size(11)
                .wrapping(Wrapping::None)
                .color(if focused { t.text } else { t.text_dim })
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        if tab.zoomed == Some(p.id) {
            line = line.push(chrome::tag("ZOOM", t, true));
        }
        line = line
            .push(Space::new().width(Length::Fill))
            .push(
                text(format!("{}×{}", rt.grid.0, rt.grid.1))
                    .size(10)
                    .color(t.text_dim),
            )
            .push(icon_button(
                "⤢",
                t,
                false,
                Message::PaneCommand(key, id::PANE_ZOOM),
            ))
            .push(icon_button(
                "×",
                t,
                true,
                Message::PaneCommand(key, id::PANE_CLOSE),
            ));
        column![
            container(line)
                .width(Length::Fill)
                .height(Length::Fixed(layout::STRIP_H - 5.0))
                .padding(Padding::from([0.0, layout::PAD_X]))
                .align_y(iced::Alignment::Center),
            chrome::hairline(Length::Fill, t.border),
        ]
        .into()
    }

    fn pane_view<'a>(
        &'a self,
        tab: &'a Tab,
        p: &layout::PanePx,
        strips: bool,
    ) -> Element<'a, Message> {
        let t = self.tokens;
        let key = (tab.id, p.id);
        let Some(rt) = self.runtimes.get(&key) else {
            return Space::new().into();
        };
        let focused = tab.focus == p.id;
        let (body, bg): (Element<'a, Message>, Color) = match &rt.term {
            Term::Running { snapshot, .. } => {
                let bg = Color::from_rgb8(snapshot.bg.0, snapshot.bg.1, snapshot.bg.2);
                let view = TerminalView::new(snapshot, &rt.cache, self.metrics, move |a| {
                    Message::View(key, a)
                })
                .focused(focused)
                .selection(self.theme.selection)
                .highlights(
                    rgb8(mahzenterm_ui_kit::mix(t.bg, t.warn, 0.35)),
                    rgb8(t.warn),
                    rgb8(t.accent),
                )
                .into_element();
                (view, bg)
            }
            Term::Failed(err) => (
                center(
                    column![
                        text("Could not start the shell").size(16).color(t.text),
                        text(err.as_str()).size(12).color(t.text_dim),
                        text("Check the shell path in config.toml, then open a new pane.")
                            .size(12)
                            .color(t.text_dim),
                    ]
                    .spacing(8),
                )
                .into(),
                t.bg,
            ),
        };
        let body = container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding {
                top: if strips { 0.0 } else { layout::PAD_Y },
                right: layout::PAD_X,
                bottom: layout::PAD_Y,
                left: layout::PAD_X,
            });
        let inner: Element<'a, Message> = if strips {
            column![
                self.pane_strip(tab, p, rt),
                Space::new().height(Length::Fixed(4.0)),
                body
            ]
            .into()
        } else {
            body.into()
        };
        let framed = container(inner)
            .width(Length::Fixed(p.outer.width))
            .height(Length::Fixed(p.outer.height))
            .style(move |_| container::background(bg));
        mouse_area(framed).on_press(Message::FocusPane(key)).into()
    }

    fn tab_label(&self, tab: &Tab, index: usize) -> String {
        let raw =
            tab.name
                .clone()
                .unwrap_or_else(|| match self.runtimes.get(&(tab.id, tab.focus)) {
                    Some(rt) if !rt.title.is_empty() => rt.title.clone(),
                    Some(rt) if rt.cwd_label.is_some() => leaf(rt.display_title()).to_owned(),
                    _ => format!("Terminal {}", index + 1),
                });
        truncate(&raw, 18)
    }
}

fn drag_event(event: Event, _status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::MouseMoved(position)),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
            Some(Message::MouseReleased)
        }
        _ => None,
    }
}

/// Banner asking whether a program may write to the clipboard.
fn clipboard_prompt<'a>(request: &str, width: f32, t: Tokens) -> Element<'a, Message> {
    let preview: String = request
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(36)
        .collect();
    let btn = |label: &'static str, msg: Message, primary: bool| {
        button(text(label).size(12).center())
            .padding(Padding::from([3.0, 12.0]))
            .on_press(msg)
            .style(move |_, status| {
                let mut s = flat_button(t, status, false, primary);
                if primary {
                    s.background = Some(Background::Color(t.accent));
                    s.text_color = t.bg;
                }
                s
            })
    };
    container(
        row![
            column![
                text(format!(
                    "A program wants to copy {} characters",
                    request.chars().count()
                ))
                .size(12)
                .color(t.text),
                text(format!("\"{preview}\""))
                    .size(11)
                    .color(t.text_dim)
                    .wrapping(Wrapping::None),
            ]
            .spacing(2)
            .width(Length::Fill),
            btn("Deny", Message::ClipboardDeny, false),
            btn("Allow", Message::ClipboardAllow, true),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fixed(width))
    .padding(Padding::from([8.0, 12.0]))
    .style(move |_| container::Style {
        background: Some(Background::Color(t.surface)),
        border: Border {
            color: t.warn,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn rgb8(c: Color) -> mahzenterm_term::Rgb {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    mahzenterm_term::Rgb(b(c.r), b(c.g), b(c.b))
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn flat_button(t: Tokens, status: button::Status, danger: bool, filled: bool) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: (hovered || filled).then_some(Background::Color(t.surface_hi)),
        text_color: if hovered && danger { t.danger } else { t.text },
        border: Border::default().rounded(6.0),
        ..button::Style::default()
    }
}

fn icon_button<'a>(
    glyph: &'static str,
    t: Tokens,
    danger: bool,
    on_press: Message,
) -> Element<'a, Message> {
    button(text(glyph).size(15).center())
        .padding(Padding::from([2.0, 8.0]))
        .on_press(on_press)
        .style(move |_, status| flat_button(t, status, danger, false))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn shell() -> Shell {
        Shell::with_config(Config::default())
    }

    fn panes(s: &Shell) -> usize {
        s.window.active_tab().pane_ids().len()
    }

    #[test]
    fn explorer_toggles_and_shrinks_the_content_area() {
        let mut s = shell();
        let full = s.content_area().width;
        let _ = s.run_command(mahzenterm_explorer::id::TOGGLE);
        assert!(s.explorer.is_some() && s.explorer_focus);
        assert_eq!(s.content_area().width, full - dock::DOCK_W);
        let _ = s.run_command(mahzenterm_explorer::id::TOGGLE);
        assert!(s.explorer.is_none());
        assert_eq!(s.content_area().width, full);
    }

    #[test]
    fn explorer_keys_are_swallowed_while_focused_and_enter_previews_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "# hi").unwrap();
        let mut s = shell();
        s.explorer = Some(mahzenterm_explorer::Explorer::new(dir.path().to_path_buf()));
        s.explorer_focus = true;
        let key = keyboard::Key::Character("x".into());
        assert!(
            s.explorer_key(&key, keyboard::Modifiers::empty(), Some("x")),
            "typing is swallowed"
        );
        assert!(
            !s.explorer_key(&key, keyboard::Modifiers::CTRL, Some("x")),
            "shortcuts pass through"
        );
        let before = panes(&s);
        let enter = keyboard::Key::Named(keyboard::key::Named::Enter);
        assert!(s.explorer_key(&enter, keyboard::Modifiers::empty(), None));
        assert_eq!(panes(&s), before + 1, "a preview pane opened");
        let tab = s.window.active_tab();
        let args = match &tab.pane(tab.focus).unwrap().kind {
            PaneKind::Terminal { args, .. } => args.clone(),
            PaneKind::Panel { .. } => Vec::new(),
        };
        assert_eq!(args[0], "view");
        assert!(args[1].ends_with("a.md"));
        assert!(!s.explorer_focus, "focus goes to the preview");
    }

    #[test]
    fn clipboard_requests_follow_the_policy() {
        let mut s = shell();
        let key = s.focused_key();
        let _ = s.clipboard_request(key, "secret".into());
        assert_eq!(
            s.clip_prompt.as_ref().map(|(_, t)| t.as_str()),
            Some("secret")
        );
        let _ = s.update(Message::ClipboardDeny);
        assert!(s.clip_prompt.is_none());
        s.osc52 = Osc52Policy::Deny;
        let _ = s.clipboard_request(key, "x".into());
        assert!(s.clip_prompt.is_none());
        s.osc52 = Osc52Policy::Allow;
        let _ = s.clipboard_request(key, "x".into());
        assert!(s.clip_prompt.is_none());
    }

    #[test]
    fn search_opens_on_the_focused_pane_and_closes_when_focus_moves() {
        let mut s = shell();
        let _ = s.run_command(id::SEARCH_OPEN);
        assert_eq!(s.search.as_ref().map(|b| b.pane), Some(s.focused_key()));
        let _ = s.update(Message::SearchClose);
        assert!(s.search.is_none());
        let _ = s.run_command(id::SEARCH_OPEN);
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let _ = s.update(Message::SearchStep(true));
        assert!(s.search.is_none(), "a new focused pane ends the old search");
    }

    #[test]
    fn short_path_abbreviates_home() {
        let home = Path::new("/home/me");
        assert_eq!(short_path(Path::new("/home/me"), Some(home)), "~");
        assert_eq!(
            short_path(Path::new("/home/me/code/x"), Some(home)),
            "~/code/x"
        );
        assert_eq!(short_path(Path::new("/etc"), Some(home)), "/etc");
        assert_eq!(short_path(Path::new("/home/me/x"), None), "/home/me/x");
    }

    #[test]
    fn leaf_takes_last_component() {
        assert_eq!(leaf("~/code/x"), "x");
        assert_eq!(leaf("~"), "~");
        assert_eq!(leaf("/"), "/");
        assert_eq!(leaf("/etc"), "etc");
    }

    #[test]
    fn truncate_adds_ellipsis_on_char_boundaries() {
        assert_eq!(truncate("short", 20), "short");
        assert_eq!(truncate("çok-uzun-bir-klasör-adı-burada", 10), "çok-uzun-…");
    }

    #[test]
    fn starts_with_one_tab_one_running_pane() {
        let s = shell();
        assert_eq!(s.window.tabs.len(), 1);
        assert_eq!(panes(&s), 1);
        assert_eq!(s.runtimes.len(), 1);
    }

    #[test]
    fn splits_create_sessions_with_smaller_grids() {
        let mut s = shell();
        let full = s.runtimes.values().next().map(|r| r.grid).unwrap();
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        assert_eq!(panes(&s), 2);
        assert_eq!(s.runtimes.len(), 2);
        assert!(s.runtimes.values().all(|r| r.grid.0 < full.0));
        let _ = s.run_command(id::PANE_SPLIT_DOWN);
        assert_eq!(panes(&s), 3);
        assert_eq!(s.runtimes.len(), 3);
    }

    #[test]
    fn closing_a_pane_drops_its_session_and_keeps_the_rest() {
        let mut s = shell();
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let _ = s.run_command(id::PANE_CLOSE);
        assert_eq!(panes(&s), 1);
        assert_eq!(s.runtimes.len(), 1);
        let (tab, focus) = s.focused_key();
        assert!(s.runtimes.contains_key(&(tab, focus)));
    }

    #[test]
    fn tabs_open_cycle_goto_and_close() {
        let mut s = shell();
        let first = s.window.active;
        let _ = s.run_command(id::TAB_NEW);
        let second = s.window.active;
        assert_ne!(first, second);
        assert_eq!(s.runtimes.len(), 2);
        let _ = s.run_command(id::TAB_PREV);
        assert_eq!(s.window.active, first);
        let _ = s.run_command(&id::tab_goto(2));
        assert_eq!(s.window.active, second);
        let _ = s.run_command(id::TAB_CLOSE);
        assert_eq!(s.window.tabs.len(), 1);
        assert_eq!(s.runtimes.len(), 1);
        assert_eq!(s.window.active, first);
    }

    #[test]
    fn usage_card_toggles_and_follows_the_pointer_inside_the_window() {
        let mut s = shell();
        assert!(!s.usage_visible);
        let _ = s.run_command(mahzenterm_ai_usage::id::TOGGLE);
        assert!(s.usage_visible);
        let _ = s.update(Message::UsageHover(Point::new(10.0, 5.0)));
        let _ = s.update(Message::UsagePress);
        let _ = s.update(Message::MouseMoved(Point::new(310.0, 205.0)));
        assert_eq!(
            s.usage_origin(),
            (300.0, 200.0),
            "grabbed at the pointer spot"
        );
        let _ = s.update(Message::MouseMoved(Point::new(-500.0, -500.0)));
        assert_eq!(s.usage_origin(), (0.0, 0.0), "cannot leave the window");
        let _ = s.update(Message::MouseReleased);
        assert!(!s.usage_drag && s.usage_pos.is_some());
        let _ = s.run_command(mahzenterm_ai_usage::id::RESET_POSITION);
        assert!(s.usage_pos.is_none());
        let _ = s.run_command(mahzenterm_ai_usage::id::TOGGLE);
        assert!(!s.usage_visible);
    }

    #[test]
    fn panes_switch_by_number_and_in_order() {
        let mut s = shell();
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let ids = s.window.active_tab().pane_ids();
        assert_eq!(ids.len(), 3);
        let _ = s.run_command(&id::pane_goto(1));
        assert_eq!(s.window.active_tab().focus, ids[0]);
        let _ = s.run_command(id::PANE_NEXT);
        assert_eq!(s.window.active_tab().focus, ids[1]);
        let _ = s.run_command(id::PANE_PREV);
        let _ = s.run_command(id::PANE_PREV);
        assert_eq!(s.window.active_tab().focus, ids[2], "wraps around");
        assert!(!s.flag("panes.count_ge_4") && s.flag("panes.count_ge_3"));
    }

    #[test]
    fn zoom_hides_siblings_and_restores_them() {
        let mut s = shell();
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let _ = s.run_command(id::PANE_ZOOM);
        assert!(s.window.active_tab().zoomed.is_some());
        let zoomed_grid = s.runtimes[&s.focused_key()].grid;
        let _ = s.run_command(id::PANE_ZOOM);
        assert!(s.window.active_tab().zoomed.is_none());
        assert!(s.runtimes[&s.focused_key()].grid.0 < zoomed_grid.0);
    }

    #[test]
    fn focus_moves_between_split_panes() {
        let mut s = shell();
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let right = s.window.active_tab().focus;
        let _ = s.run_command(id::PANE_FOCUS_LEFT);
        assert_ne!(s.window.active_tab().focus, right);
        let _ = s.run_command(id::PANE_FOCUS_RIGHT);
        assert_eq!(s.window.active_tab().focus, right);
    }

    #[test]
    fn dragging_a_sash_changes_the_ratio_and_resizes_sessions() {
        let mut s = shell();
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let area = s.content_area();
        let sash = layout::sashes(s.window.active_tab(), area)[0];
        let _ = s.update(Message::SashPress {
            a_first: sash.a_first,
            b_first: sash.b_first,
        });
        assert!(s.drag.is_some());
        let before = s.runtimes.values().map(|r| r.grid.0).collect::<Vec<_>>();
        // Pointer at 25% of the width (content coordinates are offset by the tab bar).
        let _ = s.update(Message::MouseMoved(Point::new(
            area.width * 0.25,
            TAB_BAR_H + 100.0,
        )));
        let ratio = s.window.active_tab().splits()[0].ratio;
        assert!((ratio - 0.25).abs() < 0.01, "ratio {ratio}");
        let after = s.runtimes.values().map(|r| r.grid.0).collect::<Vec<_>>();
        assert_ne!(before, after);
        let _ = s.update(Message::MouseReleased);
        assert!(s.drag.is_none());
    }

    #[test]
    fn keyboard_resize_moves_the_border() {
        let mut s = shell();
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let _ = s.run_command(id::PANE_GROW_LEFT);
        assert!((s.window.active_tab().splits()[0].ratio - 0.45).abs() < 1e-5);
    }

    /// A pane whose program has set no title, with a pending (throttled) directory check.
    fn pane_with_pending_cwd_check(s: &mut Shell) -> PaneKey {
        let key = s.focused_key();
        let rt = s.runtimes.get_mut(&key).unwrap();
        rt.title.clear();
        rt.cwd_dirty = true;
        key
    }

    #[test]
    fn pending_cwd_check_completes_once_the_throttle_window_passed() {
        let mut s = shell();
        let key = pane_with_pending_cwd_check(&mut s);
        let rt = s.runtimes.get_mut(&key).unwrap();
        rt.cwd_checked = Instant::now() - CWD_POLL;
        rt.refresh();
        // Only holds while the shell has not set its own title in the meantime.
        if rt.title.is_empty() {
            assert!(!rt.cwd_dirty);
            #[cfg(target_os = "linux")]
            assert!(rt.cwd_label.is_some());
        }
    }

    #[test]
    fn program_title_clears_the_pending_flag() {
        let mut s = shell();
        let key = pane_with_pending_cwd_check(&mut s);
        let rt = s.runtimes.get_mut(&key).unwrap();
        // Emulate a title arriving: refresh reads it from the session, so wait for the shell to set one.
        let deadline = Instant::now() + Duration::from_secs(3);
        while rt.title.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            rt.refresh();
        }
        if !rt.title.is_empty() {
            assert!(!rt.cwd_dirty);
        }
    }

    #[test]
    fn divider_hover_is_tracked() {
        let mut s = shell();
        let _ = s.update(Message::SashHover(Some((PaneId(0), PaneId(1)))));
        assert_eq!(s.hover, Some((PaneId(0), PaneId(1))));
        let _ = s.update(Message::SashHover(None));
        assert_eq!(s.hover, None);
    }

    #[test]
    fn unregistered_commands_are_ignored() {
        let mut s = shell();
        let _ = s.run_command("does.not_exist");
        assert_eq!(panes(&s), 1);
    }

    #[test]
    fn broken_shell_shows_failure_instead_of_crashing() {
        let mut cfg = Config::default();
        cfg.terminal.shell = Some("/definitely/not/a/shell".into());
        let s = Shell::with_config(cfg);
        assert!(s
            .runtimes
            .values()
            .all(|r| matches!(r.term, Term::Failed(_))));
    }

    fn type_into_palette(s: &mut Shell, text: &str) {
        let _ = s.palette_key(
            &keyboard::Key::Character(text.into()),
            keyboard::Modifiers::empty(),
            Some(text),
        );
    }

    fn press(s: &mut Shell, named: keyboard::key::Named) -> Task<Message> {
        s.palette_key(
            &keyboard::Key::Named(named),
            keyboard::Modifiers::empty(),
            None,
        )
    }

    fn selected(s: &Shell) -> Option<String> {
        s.palette
            .as_ref()
            .and_then(|p| p.current().map(str::to_owned))
    }

    #[test]
    fn palette_filters_runs_and_closes() {
        let mut s = shell();
        let _ = s.run_command(id::PALETTE_OPEN);
        assert!(s.palette.is_some());
        let all = s.palette.as_ref().unwrap().results.len();
        for c in "split r".chars() {
            type_into_palette(&mut s, &c.to_string());
        }
        assert!(s.palette.as_ref().unwrap().results.len() < all);
        assert_eq!(selected(&s).as_deref(), Some(id::PANE_SPLIT_RIGHT));
        let _ = press(&mut s, keyboard::key::Named::Enter);
        assert!(s.palette.is_none());
        assert_eq!(panes(&s), 2, "the selected command ran");
        assert_eq!(
            s.state.recent.first().map(String::as_str),
            Some(id::PANE_SPLIT_RIGHT)
        );
    }

    #[test]
    fn palette_hides_commands_whose_condition_fails() {
        let mut s = shell();
        let _ = s.run_command(id::PALETTE_OPEN);
        let ids = &s.palette.as_ref().unwrap().results;
        assert!(
            !ids.iter().any(|i| i == id::TAB_CLOSE),
            "needs more than one tab"
        );
        assert!(
            !ids.iter().any(|i| i == id::PANE_FOCUS_LEFT),
            "needs more than one pane"
        );
        assert!(ids.iter().any(|i| i == id::TAB_NEW));
    }

    #[test]
    fn current_theme_is_first_among_theme_commands() {
        let mut s = shell();
        let _ = s.run_command(id::THEME_NEXT); // some non-default theme becomes current
        let current = commands::theme_command_id(&s.theme_id);
        let _ = s.run_command(id::PALETTE_OPEN);
        for c in "theme".chars() {
            type_into_palette(&mut s, &c.to_string());
        }
        let first_theme_row = s
            .palette
            .as_ref()
            .unwrap()
            .results
            .iter()
            .find(|c| s.theme_cmds.contains_key(*c))
            .cloned();
        assert_eq!(first_theme_row.as_deref(), Some(current.as_str()));
        assert_eq!(
            s.theme.id, s.theme_id,
            "typing alone never previews another theme"
        );
    }

    #[test]
    fn escape_closes_without_running() {
        let mut s = shell();
        let _ = s.run_command(id::PALETTE_OPEN);
        let _ = press(&mut s, keyboard::key::Named::Escape);
        assert!(s.palette.is_none());
        assert_eq!(panes(&s), 1);
    }

    #[test]
    fn palette_shortcut_toggles() {
        let mut s = shell();
        let _ = s.run_command(id::PALETTE_OPEN);
        let _ = s.palette_key(
            &keyboard::Key::Character("P".into()),
            keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
            None,
        );
        assert!(s.palette.is_none());
    }

    #[test]
    fn theme_is_previewed_live_and_reverted_on_escape() {
        let mut s = shell();
        let original = s.theme_id.clone();
        let _ = s.run_command(id::PALETTE_OPEN);
        for c in "nord".chars() {
            type_into_palette(&mut s, &c.to_string());
        }
        assert_eq!(selected(&s).as_deref(), Some("theme.set.nord"));
        assert_eq!(s.theme.id, "nord", "previewed on the chrome");
        assert_eq!(s.theme_id, original, "but not chosen yet");
        let bg = s.themes.get("nord").unwrap().palette.bg;
        let snap_bg = match &s.focused_runtime().unwrap().term {
            Term::Running { snapshot, .. } => snapshot.bg,
            Term::Failed(_) => panic!("shell failed to start"),
        };
        assert_eq!(snap_bg, bg, "sessions use the previewed palette");
        let _ = press(&mut s, keyboard::key::Named::Escape);
        assert_eq!(s.theme.id, original, "escape restores the previous theme");
        assert_eq!(s.tokens, s.themes.get(&original).unwrap().tokens);
    }

    #[test]
    fn choosing_a_theme_commits_it() {
        let mut s = shell();
        let _ = s.run_command(id::PALETTE_OPEN);
        for c in "dracula".chars() {
            type_into_palette(&mut s, &c.to_string());
        }
        let _ = press(&mut s, keyboard::key::Named::Enter);
        assert_eq!(s.theme_id, "dracula");
        assert_eq!(s.theme.id, "dracula");
        assert!(s.palette.is_none());
    }

    #[test]
    fn moving_off_a_theme_row_reverts_the_preview() {
        let mut s = shell();
        let original = s.theme.id.clone();
        let _ = s.run_command(id::PALETTE_OPEN);
        for c in "theme".chars() {
            type_into_palette(&mut s, &c.to_string());
        }
        let first = selected(&s).unwrap();
        assert!(first.starts_with("theme."), "{first}");
        // Type a query that selects a non-theme command.
        let _ = s.palette_key(
            &keyboard::Key::Character("u".into()),
            keyboard::Modifiers::CTRL,
            None,
        );
        for c in "new tab".chars() {
            type_into_palette(&mut s, &c.to_string());
        }
        assert_eq!(s.theme.id, original);
    }

    #[test]
    fn next_and_previous_theme_cycle_and_commit() {
        let mut s = shell();
        let first = s.theme_id.clone();
        let _ = s.run_command(id::THEME_NEXT);
        assert_ne!(s.theme_id, first);
        let _ = s.run_command(id::THEME_PREV);
        assert_eq!(s.theme_id, first);
    }

    #[test]
    fn font_zoom_changes_grids_and_reset_returns_to_config() {
        let mut s = shell();
        let before = s.runtimes.values().next().unwrap().grid;
        let _ = s.run_command(id::FONT_LARGER);
        let bigger = s.runtimes.values().next().unwrap().grid;
        assert!(
            bigger.0 < before.0 && bigger.1 < before.1,
            "{before:?} -> {bigger:?}"
        );
        let _ = s.run_command(id::FONT_SMALLER);
        let _ = s.run_command(id::FONT_SMALLER);
        assert!(s.runtimes.values().next().unwrap().grid.0 > before.0);
        let _ = s.run_command(id::FONT_RESET);
        assert_eq!(s.runtimes.values().next().unwrap().grid, before);
        for _ in 0..100 {
            let _ = s.run_command(id::FONT_SMALLER);
        }
        assert_eq!(s.font_size, MIN_FONT, "clamped");
    }

    fn start_with(config: Config, state: State) -> Shell {
        Shell::build(
            config,
            Startup {
                state,
                ..Startup::default()
            },
        )
    }

    #[test]
    fn restore_rebuilds_layout_and_drops_missing_directories() {
        let mut w = Window::new();
        let first = w.active_tab().focus;
        w.active_tab_mut()
            .split(
                first,
                Axis::Horizontal,
                PaneKind::Terminal {
                    cwd: Some("/definitely/not/a/dir".into()),
                    shell: None,
                    args: Vec::new(),
                },
            )
            .unwrap();
        w.new_tab(PaneKind::Terminal {
            cwd: Some(std::env::temp_dir()),
            shell: None,
            args: Vec::new(),
        });
        let s = start_with(
            Config::default(),
            State {
                window: Some(w),
                ..State::default()
            },
        );
        assert_eq!(s.window.tabs.len(), 2);
        assert_eq!(
            s.runtimes.len(),
            1,
            "only the active tab starts sessions at first"
        );
        assert!(s
            .runtimes
            .values()
            .all(|r| matches!(r.term, Term::Running { .. })));
        let missing_dir_pane = &s.window.tabs[0].pane_ids();
        let kinds: Vec<_> = missing_dir_pane
            .iter()
            .filter_map(|p| s.window.tabs[0].pane(*p))
            .collect();
        assert!(kinds.iter().all(|p| matches!(&p.kind, PaneKind::Terminal { cwd, .. } if cwd.as_deref().is_none_or(std::path::Path::is_dir))));
    }

    #[test]
    fn restore_can_be_turned_off() {
        let mut w = Window::new();
        w.new_tab(PaneKind::terminal());
        let mut cfg = Config::default();
        cfg.session.restore = false;
        let s = start_with(
            cfg,
            State {
                window: Some(w),
                ..State::default()
            },
        );
        assert_eq!(s.window.tabs.len(), 1);
    }

    #[test]
    fn saved_theme_and_font_win_over_config_defaults() {
        let s = start_with(
            Config::default(),
            State {
                theme: Some("tokyo-night".into()),
                font_size: Some(19.0),
                ..State::default()
            },
        );
        assert_eq!(s.theme.id, "tokyo-night");
        assert_eq!(s.font_size, 19.0);
        let unknown = start_with(
            Config::default(),
            State {
                theme: Some("no-such-theme".into()),
                ..State::default()
            },
        );
        assert_eq!(
            unknown.theme.id, "mahzenterm-dark",
            "unknown saved theme falls back"
        );
    }

    #[test]
    fn unknown_config_theme_is_reported_not_fatal() {
        let mut cfg = Config::default();
        cfg.appearance.theme = "nope".into();
        let s = shell_with(cfg);
        assert!(s.problem.as_deref().unwrap().contains("nope"));
        assert_eq!(s.theme.id, "mahzenterm-dark");
    }

    fn shell_with(cfg: Config) -> Shell {
        Shell::with_config(cfg)
    }

    #[test]
    fn state_is_saved_in_the_background_and_restores_the_same_layout() {
        let dir = std::env::temp_dir().join(format!("mahzenterm-test-{}", std::process::id()));
        let path = dir.join("state.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Shell::build(
            Config::default(),
            Startup {
                writer: Some(state::Writer::spawn(path.clone())),
                ..Startup::default()
            },
        );
        let _ = s.update(Message::NewTab);
        let _ = s.run_command(id::PANE_SPLIT_RIGHT);
        let _ = s.run_command(id::THEME_NEXT);
        let _ = s.update(Message::MouseReleased); // any update flushes the dirty flag
        s.writer.as_mut().unwrap().flush();

        let (saved, problem) = state::load(&path);
        assert_eq!(problem, None);
        assert_eq!(saved.theme.as_deref(), Some(s.theme_id.as_str()));
        let w = saved.window.unwrap();
        assert_eq!(w.tabs.len(), 2);
        assert_eq!(w.active_tab().pane_ids().len(), 2);
        // Pane directories were read from the OS, not left unset.
        #[cfg(target_os = "linux")]
        assert!(w
            .tabs
            .iter()
            .flat_map(|t| t
                .pane_ids()
                .into_iter()
                .filter_map(move |p| t.pane(p).cloned()))
            .any(|p| matches!(p.kind, PaneKind::Terminal { cwd: Some(_), .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_reload_follows_real_edits_but_not_unrelated_ones() {
        let mut s = shell();
        let _ = s.run_command(id::FONT_LARGER);
        let zoomed = s.font_size;
        // A reload where nothing relevant changed keeps the user's zoom.
        s.apply_config(&Config::default(), None);
        assert_eq!(s.font_size, zoomed);
        // A real font edit applies.
        let mut edited = Config::default();
        edited.terminal.font_size = 21.0;
        edited.appearance.theme = "gruvbox-dark".into();
        s.apply_config(&edited, None);
        assert_eq!(s.font_size, 21.0);
        assert_eq!(s.theme_id, "gruvbox-dark");
        // Problems are surfaced and cleared on the next good reload.
        s.apply_config(&edited, Some("config.toml:2:1: oops".into()));
        assert!(s.problem.as_deref().unwrap().contains("oops"));
        s.apply_config(&edited, None);
        assert_eq!(s.problem, None);
    }

    #[test]
    fn user_key_bindings_from_a_reload_take_effect() {
        let mut s = shell();
        let mut cfg = Config::default();
        cfg.keys.push(mahzenterm_core::command::UserBinding {
            keys: "Ctrl+Shift+N".into(),
            command: id::TAB_NEW.into(),
            when: None,
        });
        s.apply_config(&cfg, None);
        let chord = commands::chord_from(
            &keyboard::Key::Character("N".into()),
            keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
        )
        .unwrap();
        assert_eq!(
            s.keymap.resolve(&[chord], &|_| false),
            Resolution::Matched(id::TAB_NEW.into())
        );
    }

    #[test]
    fn sanitize_drops_only_missing_directories() {
        let mut w = Window::new();
        let id0 = w.active_tab().focus;
        w.active_tab_mut()
            .split(
                id0,
                Axis::Vertical,
                PaneKind::Terminal {
                    cwd: Some(std::env::temp_dir()),
                    shell: Some("/bin/sh".into()),
                    args: Vec::new(),
                },
            )
            .unwrap();
        w.active_tab_mut().pane_mut(id0).unwrap().kind = PaneKind::Terminal {
            cwd: Some("/definitely/not/a/dir".into()),
            shell: None,
            args: Vec::new(),
        };
        sanitize_window(&mut w);
        let tab = w.active_tab();
        let ids = tab.pane_ids();
        assert!(matches!(
            &tab.pane(ids[0]).unwrap().kind,
            PaneKind::Terminal { cwd: None, .. }
        ));
        assert!(matches!(
            &tab.pane(ids[1]).unwrap().kind,
            PaneKind::Terminal {
                cwd: Some(_),
                shell: Some(_),
                ..
            }
        ));
    }
}
