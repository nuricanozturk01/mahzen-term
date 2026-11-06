//! Layered, validated configuration with live reload.
//!
//! Layers (later wins): built-in defaults < user file < project file. Tables merge deeply; the `keys` array
//! appends across layers. A broken file never crashes the app: its error is reported with file/line/column and the
//! last valid content of that layer stays in effect.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, watch};

use crate::bars::{self, StatusbarConfig, TabbarConfig, WidgetDef};
use crate::bus::{Bus, Event};
use crate::command::UserBinding;

/// Config schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Debounce window for file-change bursts (editors often write several times).
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// What happens when a program asks (OSC 52) to put text on the clipboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Osc52Policy {
    /// Show a prompt with Allow / Deny (default).
    #[default]
    Ask,
    Allow,
    Deny,
}

/// Terminal behaviour settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    /// Font size in logical pixels (6..=72).
    pub font_size: f32,
    /// Scrollback lines per pane (0..=1_000_000).
    pub scrollback: u32,
    /// Shell executable; `None` uses the platform default.
    pub shell: Option<String>,
    /// Clipboard writes requested by programs (`ask`, `allow` or `deny`). Reading the clipboard is never allowed.
    pub osc52: Osc52Policy,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            font_size: 15.0,
            scrollback: 10_000,
            shell: None,
            osc52: Osc52Policy::Ask,
        }
    }
}

/// Look and feel settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppearanceConfig {
    /// Theme name resolved by the theme engine.
    pub theme: String,
    /// Disable animations.
    pub reduced_motion: bool,
    /// UI scale factor (0.5..=3.0).
    pub ui_scale: f32,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "mahzenterm-dark".into(),
            reduced_motion: false,
            ui_scale: 1.0,
        }
    }
}

/// Window behaviour settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    /// Start maximized (fills the screen). Has no effect without a window manager.
    pub maximized: bool,
    /// Draw the title bar yourself (the tab bar doubles as one). Needs a restart to change.
    pub custom_titlebar: bool,
    /// Keep the terminal out from under the floating widgets: it fills the area left free by the cards on the
    /// right and bottom edges. Off by default (cards float over the terminal).
    pub fit_terminal_around_widgets: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            maximized: true,
            custom_titlebar: true,
            fit_terminal_around_widgets: false,
        }
    }
}

/// Session behaviour settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    /// Reopen the previous tabs and splits (in their directories) at startup.
    pub restore: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self { restore: true }
    }
}

/// The merged, validated configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub terminal: TerminalConfig,
    pub appearance: AppearanceConfig,
    pub session: SessionConfig,
    pub window: WindowConfig,
    pub tabbar: TabbarConfig,
    pub statusbar: StatusbarConfig,
    /// Custom and overridden widgets (`[widgets.<id>]`), placed by the bar lists.
    pub widgets: BTreeMap<String, WidgetDef>,
    /// User key bindings (`[[keys]]`); layers append.
    pub keys: Vec<UserBinding>,
    /// Per-module sections (`[modules.<name>]`), typed by each module via [`Config::module`].
    pub modules: BTreeMap<String, toml::Table>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            terminal: TerminalConfig::default(),
            appearance: AppearanceConfig::default(),
            session: SessionConfig::default(),
            window: WindowConfig::default(),
            tabbar: TabbarConfig::default(),
            statusbar: StatusbarConfig::default(),
            widgets: BTreeMap::new(),
            keys: Vec::new(),
            modules: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Deserialize the `[modules.<name>]` section; a missing section yields `T::default()`.
    pub fn module<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, ConfigError> {
        match self.modules.get(name) {
            None => Ok(T::default()),
            Some(table) => table
                .clone()
                .try_into::<T>()
                .map_err(|e| ConfigError::Invalid {
                    path: None,
                    field: format!("modules.{name}"),
                    message: e.message().to_string(),
                }),
        }
    }

    fn validate(&self) -> Result<(), (String, String)> {
        let bad = |f: &str, m: &str| Err((f.to_string(), m.to_string()));
        if !(6.0..=72.0).contains(&self.terminal.font_size) {
            return bad("terminal.font_size", "must be between 6 and 72");
        }
        if self.terminal.scrollback > 1_000_000 {
            return bad("terminal.scrollback", "must be at most 1000000 lines");
        }
        if !(0.5..=3.0).contains(&self.appearance.ui_scale) {
            return bad("appearance.ui_scale", "must be between 0.5 and 3.0");
        }
        if self.appearance.theme.trim().is_empty() {
            return bad("appearance.theme", "must not be empty");
        }
        for (id, def) in &self.widgets {
            bars::validate_def(id, def)?;
        }
        Ok(())
    }
}

/// A problem with one config layer. Always actionable: says where and what to change.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("{}: cannot read file: {message}", path.display())]
    Read { path: PathBuf, message: String },
    #[error("{}:{line}:{col}: {message}", path.display())]
    Syntax {
        path: PathBuf,
        line: usize,
        col: usize,
        message: String,
    },
    #[error("{}: `{field}` {message}", path.as_deref().map_or_else(|| "config".into(), |p| p.display().to_string()))]
    Invalid {
        path: Option<PathBuf>,
        field: String,
        message: String,
    },
    #[error("{}: schema_version {found} is newer than this build supports ({supported}); update MahzenTerm or lower it", path.display())]
    SchemaTooNew {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
}

/// Where a layer comes from; used for error messages and priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    User,
    Project,
}

impl fmt::Display for LayerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "user",
            Self::Project => "project",
        })
    }
}

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let upto = &text[..offset.min(text.len())];
    let line = upto.bytes().filter(|&b| b == b'\n').count() + 1;
    let col = upto.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

fn syntax_error(path: &Path, text: &str, e: &toml::de::Error) -> ConfigError {
    let (line, col) = e.span().map_or((1, 1), |s| line_col(text, s.start));
    ConfigError::Syntax {
        path: path.to_path_buf(),
        line,
        col,
        message: e.message().to_string(),
    }
}

/// Apply schema migrations. `migrations[i]` upgrades version `i + 1` to `i + 2`.
pub fn apply_migrations(table: &mut toml::Table, from: u32, migrations: &[fn(&mut toml::Table)]) {
    let start = from.saturating_sub(1) as usize;
    for m in migrations.iter().skip(start) {
        m(table);
    }
}

/// Migrations from older schema versions to [`SCHEMA_VERSION`]; empty while version 1 is current.
const MIGRATIONS: &[fn(&mut toml::Table)] = &[];

/// Parse and validate one layer's text. Returns the raw table for merging.
pub fn parse_layer(path: &Path, text: &str) -> Result<toml::Table, ConfigError> {
    let mut table: toml::Table = toml::from_str(text).map_err(|e| syntax_error(path, text, &e))?;
    let version = match table.get("schema_version") {
        None => SCHEMA_VERSION,
        Some(v) => v
            .as_integer()
            .and_then(|i| u32::try_from(i).ok())
            .ok_or_else(|| ConfigError::Invalid {
                path: Some(path.to_path_buf()),
                field: "schema_version".into(),
                message: "must be a positive integer".into(),
            })?,
    };
    if version > SCHEMA_VERSION {
        return Err(ConfigError::SchemaTooNew {
            path: path.to_path_buf(),
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    apply_migrations(&mut table, version, MIGRATIONS);
    // Type/field validation with spans: deserialize the layer text on its own.
    let cfg: Config = toml::from_str(text).map_err(|e| syntax_error(path, text, &e))?;
    cfg.validate()
        .map_err(|(field, message)| ConfigError::Invalid {
            path: Some(path.to_path_buf()),
            field,
            message,
        })?;
    Ok(table)
}

/// Deep-merge `over` into `base`. Tables merge key by key; the top-level `keys` array appends; everything else replaces.
pub fn merge(base: &mut toml::Table, over: toml::Table) {
    merge_at(base, over, true);
}

fn merge_at(base: &mut toml::Table, over: toml::Table, top: bool) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge_at(b, o, false),
            (Some(toml::Value::Array(b)), toml::Value::Array(o)) if top && k == "keys" => {
                b.extend(o)
            }
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// Result of a (re)load: the effective config and any layer problems.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub issues: Vec<ConfigError>,
}

/// Loads and merges the layers, remembering the last valid table of each so errors never lose settings.
#[derive(Debug)]
pub struct Loader {
    user: PathBuf,
    project: Option<PathBuf>,
    last_good: BTreeMap<PathBuf, toml::Table>,
}

impl Loader {
    /// `user` is the user config path; `project` the optional per-project config path.
    pub fn new(user: PathBuf, project: Option<PathBuf>) -> Self {
        Self {
            user,
            project,
            last_good: BTreeMap::new(),
        }
    }

    /// Paths of all layers, in priority order (lowest first).
    pub fn paths(&self) -> Vec<(LayerKind, &Path)> {
        let mut v = vec![(LayerKind::User, self.user.as_path())];
        if let Some(p) = &self.project {
            v.push((LayerKind::Project, p.as_path()));
        }
        v
    }

    /// Read every layer from disk and merge. Missing files are fine (layer skipped, no error).
    pub fn load(&mut self) -> Loaded {
        let mut issues = Vec::new();
        let mut merged = defaults_table();
        let paths: Vec<(LayerKind, PathBuf)> = self
            .paths()
            .into_iter()
            .map(|(k, p)| (k, p.to_path_buf()))
            .collect();
        for (kind, path) in paths {
            match std::fs::read_to_string(&path) {
                Ok(text) => match parse_layer(&path, &text) {
                    Ok(mut table) => {
                        if kind == LayerKind::Project {
                            issues.extend(strip_untrusted(&path, &mut table));
                        }
                        self.last_good.insert(path.clone(), table);
                    }
                    Err(e) => issues.push(e),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.last_good.remove(&path);
                }
                Err(e) => issues.push(ConfigError::Read {
                    path: path.clone(),
                    message: e.to_string(),
                }),
            }
            if let Some(t) = self.last_good.get(&path) {
                merge(&mut merged, t.clone());
            }
        }
        let config = merged.try_into::<Config>().unwrap_or_else(|e| {
            // Layers were validated individually, so this only happens on cross-layer type clashes.
            issues.push(ConfigError::Invalid {
                path: None,
                field: "merged config".into(),
                message: e.message().to_string(),
            });
            Config::default()
        });
        let config = match bars::validate_refs(&config.tabbar, &config.statusbar, &config.widgets) {
            Ok(()) => config,
            Err((field, message)) => {
                // Keep everything else; only the broken bar layout falls back to the defaults.
                issues.push(ConfigError::Invalid {
                    path: None,
                    field,
                    message,
                });
                Config {
                    tabbar: TabbarConfig::default(),
                    statusbar: StatusbarConfig::default(),
                    ..config
                }
            }
        };
        Loaded { config, issues }
    }
}

/// A project file may arrive with a cloned repository, so it must not make MahzenTerm run commands: drop its
/// `command` widgets and say so.
fn strip_untrusted(path: &Path, table: &mut toml::Table) -> Vec<ConfigError> {
    let Some(toml::Value::Table(widgets)) = table.get_mut("widgets") else {
        return Vec::new();
    };
    let doomed: Vec<String> = widgets
        .iter()
        .filter(|(_, v)| v.get("kind").and_then(toml::Value::as_str) == Some("command"))
        .map(|(k, _)| k.clone())
        .collect();
    doomed
        .into_iter()
        .map(|id| {
            widgets.remove(&id);
            ConfigError::Invalid {
                path: Some(path.to_path_buf()),
                field: format!("widgets.{id}"),
                message: "is a command widget; commands only run from your own config.toml, so it was ignored"
                    .into(),
            }
        })
        .collect()
}

fn defaults_table() -> toml::Table {
    toml::Table::try_from(Config::default()).expect("Config::default() always serializes to TOML")
}

/// Keeps a file watcher alive; dropping it stops watching and the reload task.
pub struct WatchGuard {
    _watcher: notify::RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Handles to the live config. Clone the receivers freely.
pub struct LiveConfig {
    /// Latest valid effective config.
    pub config: watch::Receiver<Arc<Config>>,
    /// Problems from the most recent reload (empty when all layers are fine).
    pub issues: watch::Receiver<Arc<Vec<ConfigError>>>,
    pub guard: WatchGuard,
}

/// Load the config now, then watch the layer files and reload on change (debounced).
/// Publishes [`Event::ConfigReloaded`] after each reload. Must be called inside a tokio runtime.
pub fn watch_config(mut loader: Loader, bus: Bus) -> Result<LiveConfig, notify::Error> {
    let first = loader.load();
    let (cfg_tx, cfg_rx) = watch::channel(Arc::new(first.config));
    let (iss_tx, iss_rx) = watch::channel(Arc::new(first.issues));

    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok_and(|e| !e.kind.is_access()) {
            let _ = ev_tx.send(());
        }
    })?;
    // Watch parent directories: editors save by replacing the file, which a file watch would lose.
    let mut dirs: Vec<PathBuf> = loader
        .paths()
        .iter()
        .filter_map(|(_, p)| p.parent().map(Path::to_path_buf))
        .collect();
    dirs.sort();
    dirs.dedup();
    for d in dirs {
        if d.is_dir() {
            watcher.watch(&d, RecursiveMode::NonRecursive)?;
        } else {
            tracing::debug!(dir = %d.display(), "config directory missing; not watched");
        }
    }

    let task = tokio::spawn(async move {
        while ev_rx.recv().await.is_some() {
            // Debounce: swallow further events until quiet.
            loop {
                match tokio::time::timeout(DEBOUNCE, ev_rx.recv()).await {
                    Ok(Some(())) => {}
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            let loaded = loader.load();
            for issue in &loaded.issues {
                tracing::warn!(%issue, "config problem");
            }
            let _ = iss_tx.send(Arc::new(loaded.issues));
            let _ = cfg_tx.send(Arc::new(loaded.config));
            bus.publish(Event::ConfigReloaded);
        }
    });

    Ok(LiveConfig {
        config: cfg_rx,
        issues: iss_rx,
        guard: WatchGuard {
            _watcher: watcher,
            task,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn p() -> PathBuf {
        PathBuf::from("config.toml")
    }

    #[test]
    fn defaults_round_trip_and_validate() {
        let t = defaults_table();
        let c: Config = t.try_into().unwrap();
        assert_eq!(c, Config::default());
        assert!(c.validate().is_ok());
    }

    #[test]
    fn layer_overrides_only_given_keys() {
        let mut base = defaults_table();
        merge(
            &mut base,
            parse_layer(&p(), "[terminal]\nfont_size = 18.0\n").unwrap(),
        );
        let c: Config = base.try_into().unwrap();
        assert_eq!(c.terminal.font_size, 18.0);
        assert_eq!(c.terminal.scrollback, 10_000);
    }

    #[test]
    fn keys_append_across_layers_other_arrays_replace() {
        let mut base = defaults_table();
        let a = "[[keys]]\nkeys = \"Ctrl+1\"\ncommand = \"a.one\"\n";
        let b = "[[keys]]\nkeys = \"Ctrl+2\"\ncommand = \"a.two\"\n";
        merge(&mut base, parse_layer(&p(), a).unwrap());
        merge(&mut base, parse_layer(&p(), b).unwrap());
        let c: Config = base.try_into().unwrap();
        assert_eq!(c.keys.len(), 2);
    }

    #[test]
    fn syntax_error_has_line_and_column() {
        let e = parse_layer(&p(), "[terminal]\nfont_size = \n").unwrap_err();
        let ConfigError::Syntax { line, col, .. } = &e else {
            panic!("syntax error expected, got {e:?}")
        };
        assert_eq!(*line, 2);
        assert!(*col >= 1);
        insta::assert_snapshot!(e.to_string());
    }

    #[test]
    fn unknown_field_is_an_error_with_location() {
        let e = parse_layer(&p(), "[terminal]\nfont_sise = 12\n").unwrap_err();
        assert!(matches!(e, ConfigError::Syntax { line: 2, .. }), "{e:?}");
        assert!(e.to_string().contains("font_sise"));
    }

    #[test]
    fn out_of_range_is_actionable() {
        let e = parse_layer(&p(), "[terminal]\nfont_size = 500.0\n").unwrap_err();
        insta::assert_snapshot!(e.to_string());
    }

    #[test]
    fn newer_schema_is_rejected() {
        let e = parse_layer(&p(), "schema_version = 99\n").unwrap_err();
        assert!(matches!(e, ConfigError::SchemaTooNew { found: 99, .. }));
    }

    #[test]
    fn migrations_apply_from_given_version() {
        fn rename(t: &mut toml::Table) {
            if let Some(v) = t.remove("old") {
                t.insert("new".into(), v);
            }
        }
        let mut t: toml::Table = toml::from_str("old = 1").unwrap();
        apply_migrations(&mut t, 1, &[rename]);
        assert!(t.contains_key("new") && !t.contains_key("old"));
        let mut t2: toml::Table = toml::from_str("old = 1").unwrap();
        apply_migrations(&mut t2, 2, &[rename]);
        assert!(t2.contains_key("old"));
    }

    #[test]
    fn module_sections_are_typed_with_defaults() {
        #[derive(Debug, Default, PartialEq, Deserialize)]
        #[serde(default)]
        struct Explorer {
            show_hidden: bool,
        }
        let mut c = Config::default();
        assert_eq!(
            c.module::<Explorer>("explorer").unwrap(),
            Explorer::default()
        );
        c.modules.insert(
            "explorer".into(),
            toml::from_str("show_hidden = true").unwrap(),
        );
        assert!(c.module::<Explorer>("explorer").unwrap().show_hidden);
        c.modules.insert(
            "explorer".into(),
            toml::from_str("show_hidden = 3").unwrap(),
        );
        assert!(c.module::<Explorer>("explorer").is_err());
    }

    #[test]
    fn loader_merges_user_and_project_and_skips_missing() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let proj = dir.path().join("project.toml");
        fs::write(&user, "[terminal]\nfont_size = 16.0\nscrollback = 500\n").unwrap();
        fs::write(&proj, "[terminal]\nfont_size = 20.0\n").unwrap();
        let mut l = Loader::new(user, Some(proj));
        let out = l.load();
        assert!(out.issues.is_empty());
        assert_eq!(out.config.terminal.font_size, 20.0);
        assert_eq!(out.config.terminal.scrollback, 500);

        let mut missing = Loader::new(dir.path().join("none.toml"), None);
        let out = missing.load();
        assert!(out.issues.is_empty());
        assert_eq!(out.config, Config::default());
    }

    #[test]
    fn project_layer_cannot_define_command_widgets() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let proj = dir.path().join("project.toml");
        fs::write(
            &user,
            "[widgets.mine]\nkind = \"command\"\ncommand = \"echo ok\"\n[statusbar]\nright = [\"mine\"]\n",
        )
        .unwrap();
        fs::write(
            &proj,
            "[widgets.evil]\nkind = \"command\"\ncommand = \"rm -rf ~\"\n[widgets.note]\nkind = \"text\"\ntext = \"hi\"\n",
        )
        .unwrap();
        let out = Loader::new(user, Some(proj)).load();
        assert!(out.config.widgets.contains_key("mine"));
        assert!(out.config.widgets.contains_key("note"));
        assert!(!out.config.widgets.contains_key("evil"));
        assert_eq!(out.issues.len(), 1);
        assert!(out.issues[0].to_string().contains("widgets.evil"));
    }

    #[test]
    fn unknown_widget_in_a_bar_falls_back_to_default_layout() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        fs::write(
            &user,
            "[statusbar]\nright = [\"ghost\"]\n[terminal]\nscrollback = 7\n",
        )
        .unwrap();
        let out = Loader::new(user, None).load();
        assert_eq!(out.config.statusbar, StatusbarConfig::default());
        assert_eq!(out.config.terminal.scrollback, 7);
        assert!(out.issues[0].to_string().contains("ghost"));
    }

    #[test]
    fn broken_edit_keeps_last_valid_layer() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        fs::write(&user, "[terminal]\nfont_size = 17.0\n").unwrap();
        let mut l = Loader::new(user.clone(), None);
        assert_eq!(l.load().config.terminal.font_size, 17.0);
        fs::write(&user, "[terminal]\nfont_size = oops\n").unwrap();
        let out = l.load();
        assert_eq!(out.issues.len(), 1);
        assert_eq!(
            out.config.terminal.font_size, 17.0,
            "last valid value stays"
        );
        fs::write(&user, "[terminal]\nfont_size = 19.0\n").unwrap();
        let out = l.load();
        assert!(out.issues.is_empty());
        assert_eq!(out.config.terminal.font_size, 19.0);
    }

    #[test]
    fn deleting_a_layer_reverts_to_lower_layers() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        fs::write(&user, "[terminal]\nfont_size = 17.0\n").unwrap();
        let mut l = Loader::new(user.clone(), None);
        l.load();
        fs::remove_file(&user).unwrap();
        assert_eq!(l.load().config.terminal.font_size, 15.0);
    }

    async fn wait_for<T: Clone>(rx: &mut watch::Receiver<T>, ok: impl Fn(&T) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if ok(&rx.borrow()) {
                return true;
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                return false;
            }
        }
    }

    #[tokio::test]
    async fn live_reload_applies_edits_and_publishes_event() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("config.toml");
        fs::write(&user, "[terminal]\nfont_size = 14.0\n").unwrap();
        let (bus, _rx) = Bus::new();
        let mut events = bus.subscribe();
        let mut live = watch_config(Loader::new(user.clone(), None), bus).unwrap();
        assert_eq!(live.config.borrow().terminal.font_size, 14.0);

        fs::write(&user, "[terminal]\nfont_size = 21.0\n").unwrap();
        assert!(
            wait_for(&mut live.config, |c| c.terminal.font_size == 21.0).await,
            "edit not applied"
        );
        assert_eq!(events.recv().await, Some(Event::ConfigReloaded));

        fs::write(&user, "[terminal]\nfont_size = nope\n").unwrap();
        assert!(
            wait_for(&mut live.issues, |i| !i.is_empty()).await,
            "error not reported"
        );
        assert_eq!(
            live.config.borrow().terminal.font_size,
            21.0,
            "last valid config stays"
        );
    }
}
