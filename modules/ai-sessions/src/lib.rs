//! AI sessions: a card listing running Claude Code sessions with their live status, and a way to jump to the pane
//! each one runs in.
//!
//! Status comes from Claude Code's own local session records (read-only, best effort). The app finds which pane a
//! session belongs to by walking the process tree (`Session::lineage`), so no hook is written into any tool's config.

pub mod codex;
mod read;
mod view;

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use serde::Deserialize;

pub use read::{parse_session, read_sessions};
pub use view::{
    card_height, card_width, clamp_scale, view, CARD_W, MAX_ROWS, MAX_SCALE, MIN_SCALE,
};

/// Command ids of this module.
pub mod id {
    pub const TOGGLE: &str = "ai_sessions.toggle";
    pub const FOCUS_WAITING: &str = "ai_sessions.focus_waiting";
    pub const RESET_POSITION: &str = "ai_sessions.reset_position";
    pub const OPEN_TAB: &str = "ai_sessions.open_tab";
    pub const OPEN_WINDOW: &str = "ai_sessions.open_window";
    pub const LARGER: &str = "ai_sessions.larger";
    pub const SMALLER: &str = "ai_sessions.smaller";
}

/// What a session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
    /// Asking the user for something (a permission, an answer).
    Waiting,
    /// Dropped to a shell command.
    Shell,
    Unknown,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Idle => "idle",
            Self::Waiting => "needs you",
            Self::Shell => "shell",
            Self::Unknown => "?",
        }
    }
}

/// Which tool a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Claude,
    Codex,
}

/// One live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub tool: Tool,
    pub pid: u32,
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub status: Status,
    /// What it waits for, when it says so.
    pub waiting_for: Option<String>,
    /// Unix milliseconds of the last status change.
    pub updated_ms: u64,
    /// `pid` and its ancestors, filled in by the app from the process table.
    pub lineage: Vec<u32>,
}

impl Session {
    /// The pane this session runs in: the one whose shell is the nearest ancestor of the session's process.
    pub fn owner_pane<K: Copy>(&self, panes: &[(K, u32)]) -> Option<K> {
        self.lineage
            .iter()
            .find_map(|p| panes.iter().find(|(_, shell)| shell == p).map(|(k, _)| *k))
    }
}

/// Counts for a summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub busy: usize,
    pub waiting: usize,
    pub idle: usize,
}

/// Count sessions by status.
pub fn summarize(sessions: &[Session]) -> Summary {
    let mut s = Summary::default();
    for x in sessions {
        match x.status {
            Status::Busy => s.busy += 1,
            Status::Waiting => s.waiting += 1,
            _ => s.idle += 1,
        }
    }
    s
}

/// Corner the card is placed relative to until the user drags it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    TopRight,
    #[default]
    BottomLeft,
    BottomRight,
}

/// `[modules.ai_sessions]` in the config.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub visible: bool,
    pub anchor: Anchor,
    pub offset_x: f32,
    pub offset_y: f32,
    pub scale: f32,
    /// Seconds between scans while the card is visible.
    pub interval: f32,
    /// Override `~/.claude` (or `CLAUDE_CONFIG_DIR`).
    pub claude_dir: Option<String>,
    /// Override `~/.codex` (or `CODEX_HOME`).
    pub codex_dir: Option<String>,
    /// Send a desktop notification when a session starts waiting for you. Needs the background scan to run even
    /// while the card is hidden (a small constant cost), so it is off by default.
    pub notify: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            visible: false,
            anchor: Anchor::BottomLeft,
            offset_x: 16.0,
            offset_y: 44.0,
            scale: 1.0,
            interval: 2.0,
            claude_dir: None,
            codex_dir: None,
            notify: false,
        }
    }
}

impl Settings {
    /// Top-left of the card for `rows` rows at `scale` in a window of `win`.
    pub fn origin(&self, win: (f32, f32), rows: usize, scale: f32) -> (f32, f32) {
        let x = match self.anchor {
            Anchor::TopLeft | Anchor::BottomLeft => self.offset_x,
            Anchor::TopRight | Anchor::BottomRight => win.0 - card_width(scale) - self.offset_x,
        };
        let y = match self.anchor {
            Anchor::TopLeft | Anchor::TopRight => self.offset_y,
            Anchor::BottomLeft | Anchor::BottomRight => {
                win.1 - card_height(rows, scale) - self.offset_y
            }
        };
        (x, y)
    }

    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f32(self.interval.clamp(1.0, 60.0))
    }
}

/// The module registration.
pub struct AiSessionsModule;

impl Module for AiSessionsModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "ai_sessions",
            name: "AI sessions",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::FsRead],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(id::TOGGLE, "Widgets: AI Sessions", "Widgets").key("Ctrl+Alt+A"),
            CommandSpec::new(id::FOCUS_WAITING, "Go To AI Session That Needs Me", "AI")
                .key("Ctrl+Alt+J"),
            CommandSpec::new(id::RESET_POSITION, "Reset AI Sessions Card Position", "AI"),
            CommandSpec::new(id::OPEN_TAB, "AI Sessions: Open in a Tab", "AI"),
            CommandSpec::new(id::OPEN_WINDOW, "AI Sessions: Open in a Window", "AI"),
            CommandSpec::new(id::LARGER, "Enlarge AI Sessions Card", "AI"),
            CommandSpec::new(id::SMALLER, "Shrink AI Sessions Card", "AI"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(pid: u32, status: Status, lineage: &[u32]) -> Session {
        Session {
            pid,
            id: String::new(),
            name: format!("s{pid}"),
            cwd: String::new(),
            status,
            waiting_for: None,
            updated_ms: 0,
            lineage: lineage.to_vec(),
            tool: Tool::Claude,
        }
    }

    #[test]
    fn the_nearest_pane_shell_owns_the_session() {
        let panes = [("a", 100), ("b", 200), ("outer", 50)];
        // claude (pid 9) runs under shell 200, which runs under shell 50 (a nested terminal).
        assert_eq!(
            s(9, Status::Busy, &[9, 200, 50, 1]).owner_pane(&panes),
            Some("b")
        );
        assert_eq!(s(9, Status::Busy, &[9, 77, 1]).owner_pane(&panes), None);
        assert_eq!(
            s(100, Status::Idle, &[100, 1]).owner_pane(&panes),
            Some("a"),
            "the shell itself"
        );
    }

    #[test]
    fn summary_counts_by_status() {
        let v = [
            s(1, Status::Busy, &[]),
            s(2, Status::Waiting, &[]),
            s(3, Status::Idle, &[]),
            s(4, Status::Unknown, &[]),
        ];
        assert_eq!(
            summarize(&v),
            Summary {
                busy: 1,
                waiting: 1,
                idle: 2
            }
        );
    }

    #[test]
    fn module_is_valid_and_settings_are_strict() {
        mahzenterm_core::module::validate(&AiSessionsModule).unwrap();
        assert!(serde_json::from_str::<Settings>(r#"{"bogus":1}"#).is_err());
        assert_eq!(
            serde_json::from_str::<Settings>("{}").unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn bottom_anchors_grow_upward() {
        let st = Settings::default();
        let win = (1000.0, 800.0);
        let (x, y1) = st.origin(win, 1, 1.0);
        let (_, y3) = st.origin(win, 3, 1.0);
        assert_eq!(x, 16.0);
        assert!(y3 < y1, "more rows need more room above the bottom edge");
    }
}
