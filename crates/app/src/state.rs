//! Persisted UI state (`state.toml`): chosen theme, recent commands and the workspace layout.
//!
//! Reading happens once at startup. Writing is handed to a background thread (never blocks the UI thread), is
//! coalesced (only the latest state is written) and atomic (temp file plus rename), so a crash cannot leave a half file.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;

use mahzenterm_core::workspace::Window;
use serde::{Deserialize, Serialize};

/// State file schema version.
pub const VERSION: u32 = 1;

/// How many recently used commands are remembered.
pub const MAX_RECENT: usize = 8;

/// Everything worth restoring on the next start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub version: u32,
    /// Theme chosen in the palette; wins over the config file's theme until the config theme changes.
    pub theme: Option<String>,
    /// Font size chosen with the zoom commands.
    pub font_size: Option<f32>,
    /// Most recent command ids first.
    pub recent: Vec<String>,
    /// Tabs and splits, with each pane's last known directory.
    pub window: Option<Window>,
    /// AI usage card: shown or hidden, and where the user dragged it (window pixels).
    pub usage_visible: Option<bool>,
    pub usage_pos: Option<[f32; 2]>,
}

impl State {
    /// Remember a command as most recent.
    pub fn touch_recent(&mut self, id: &str) {
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_owned());
        self.recent.truncate(MAX_RECENT);
    }

    /// Serialize to TOML text.
    pub fn to_text(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }
}

/// Load the state file. A missing file is a normal first start; a corrupt one yields defaults plus a message
/// that tells the user what happened (the corrupt file is left in place until the next save replaces it).
pub fn load(path: &Path) -> (State, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (State::default(), None),
        Err(e) => {
            return (
                State::default(),
                Some(format!("{}: cannot read state: {e}", path.display())),
            )
        }
    };
    match toml::from_str::<State>(&text) {
        Ok(mut s) => {
            if s.version > VERSION {
                return (
                    State::default(),
                    Some(format!(
                        "{}: state version {} is newer than this build ({VERSION}); starting fresh",
                        path.display(),
                        s.version
                    )),
                );
            }
            let mut problem = None;
            if let Some(w) = &s.window {
                if let Err(why) = w.validate() {
                    problem = Some(format!(
                        "{}: saved layout ignored ({why}); starting with a single tab",
                        path.display()
                    ));
                    s.window = None;
                }
            }
            (s, problem)
        }
        Err(e) => (
            State::default(),
            Some(format!(
                "{}: {} - state ignored and will be rewritten",
                path.display(),
                e.message()
            )),
        ),
    }
}

/// Background writer. Dropping it flushes pending writes.
pub struct Writer {
    tx: Option<mpsc::Sender<String>>,
    handle: Option<JoinHandle<()>>,
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp: PathBuf = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

impl Writer {
    /// Start the writer thread for `path`.
    pub fn spawn(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<String>();
        let handle = std::thread::Builder::new()
            .name("mahzenterm-state".into())
            .spawn(move || {
                while let Ok(mut latest) = rx.recv() {
                    // Coalesce: only the newest snapshot matters.
                    while let Ok(newer) = rx.try_recv() {
                        latest = newer;
                    }
                    if let Err(e) = write_atomic(&path, &latest) {
                        tracing::warn!(error = %e, path = %path.display(), "could not save state");
                    }
                }
            })
            .ok();
        Self {
            tx: handle.as_ref().map(|_| tx),
            handle,
        }
    }

    /// Queue `text` for writing; returns immediately.
    pub fn save(&self, text: String) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(text);
        }
    }

    /// Write everything queued and stop the thread.
    pub fn flush(&mut self) {
        self.tx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mahzenterm_core::workspace::{Axis, PaneKind};

    #[test]
    fn recent_commands_are_unique_newest_first_and_capped() {
        let mut s = State::default();
        for i in 0..12 {
            s.touch_recent(&format!("a.cmd{i}"));
        }
        s.touch_recent("a.cmd5");
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], "a.cmd5");
        assert_eq!(s.recent.iter().filter(|r| *r == "a.cmd5").count(), 1);
    }

    #[test]
    fn round_trips_through_a_file_with_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/state.toml");
        let mut w = Window::new();
        let id = w.active_tab().focus;
        w.active_tab_mut()
            .split(id, Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        let s = State {
            version: VERSION,
            theme: Some("nord".into()),
            font_size: Some(17.0),
            recent: vec!["tab.new".into()],
            window: Some(w),
            usage_visible: Some(true),
            usage_pos: Some([120.0, 80.0]),
        };
        let mut writer = Writer::spawn(path.clone());
        writer.save(s.to_text().unwrap());
        writer.flush();
        let (back, problem) = load(&path);
        assert_eq!(problem, None);
        assert_eq!(back, s);
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "temp file is renamed away"
        );
    }

    #[test]
    fn missing_file_is_a_normal_first_start() {
        let (s, problem) = load(Path::new("/definitely/missing/state.toml"));
        assert_eq!(s, State::default());
        assert_eq!(problem, None);
    }

    #[test]
    fn corrupt_file_falls_back_with_an_explanation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(&path, "theme = [oops").unwrap();
        let (s, problem) = load(&path);
        assert_eq!(s, State::default());
        assert!(problem.unwrap().contains("rewritten"));
    }

    #[test]
    fn invalid_layout_is_dropped_but_the_rest_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let mut w = Window::new();
        w.tabs.clear();
        let s = State {
            version: VERSION,
            theme: Some("dracula".into()),
            window: Some(w),
            ..State::default()
        };
        std::fs::write(&path, toml::to_string(&s).unwrap()).unwrap();
        let (back, problem) = load(&path);
        assert!(problem.unwrap().contains("layout ignored"));
        assert_eq!(back.window, None);
        assert_eq!(back.theme.as_deref(), Some("dracula"));
    }

    #[test]
    fn newer_state_version_is_not_misread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(
            &path,
            format!("version = {}\ntheme = \"nord\"\n", VERSION + 1),
        )
        .unwrap();
        let (s, problem) = load(&path);
        assert_eq!(s, State::default());
        assert!(problem.unwrap().contains("newer"));
    }

    #[test]
    fn writer_coalesces_to_the_latest_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let mut writer = Writer::spawn(path.clone());
        for i in 0..50 {
            writer.save(format!("theme = \"t{i}\"\n"));
        }
        writer.flush();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = \"t49\"\n");
    }
}
