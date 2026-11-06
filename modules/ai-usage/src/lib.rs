//! AI token usage monitor for Claude Code and Codex.
//!
//! Reads the tools' own local session logs (best effort: formats are not a stable API) and sums tokens for
//! today, the last hour and the last five minutes. [`Scanner`] reads logs incrementally and de-duplicates
//! messages; [`view`] draws a small card the app floats anywhere over the terminal. No network, no telemetry.

mod scan;
mod view;

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use serde::Deserialize;

pub use scan::{parse_claude_line, parse_codex_line, Scanner};
pub use view::{
    card_height, card_width, clamp_scale, fmt_tokens, view, CARD_W, HEADER_H, MAX_SCALE, MIN_SCALE,
};

/// Command ids of this module.
pub mod id {
    pub const TOGGLE: &str = "ai_usage.toggle";
    pub const RESET_POSITION: &str = "ai_usage.reset_position";
    pub const OPEN_TAB: &str = "ai_usage.open_tab";
    pub const OPEN_WINDOW: &str = "ai_usage.open_window";
    pub const LARGER: &str = "ai_usage.larger";
    pub const SMALLER: &str = "ai_usage.smaller";
    pub const RESET_SIZE: &str = "ai_usage.reset_size";
}

/// Which tool a record belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Claude,
    Codex,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

/// Token counts of one or many responses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Input that was not served from cache.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// Tokens that were actually processed or generated, without cache hits (what people mean by "spent").
    pub fn billable(&self) -> u64 {
        self.input + self.output + self.cache_write
    }

    pub fn add(&mut self, o: &Usage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
}

/// One response found in a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub tool: Tool,
    /// Unique id of the response, used to drop duplicates (resumed sessions repeat lines).
    pub id: String,
    /// Unix seconds.
    pub at: i64,
    pub usage: Usage,
}

/// Sums for one tool.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ToolStats {
    pub today: Usage,
    pub last_hour: Usage,
    /// Billable tokens per minute over the last five minutes.
    pub per_min: f64,
    pub responses_today: usize,
    /// Unix seconds of the newest response.
    pub last_at: Option<i64>,
}

/// What the card shows.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Snapshot {
    pub claude: ToolStats,
    pub codex: ToolStats,
}

impl Snapshot {
    pub fn get(&self, tool: Tool) -> &ToolStats {
        match tool {
            Tool::Claude => &self.claude,
            Tool::Codex => &self.codex,
        }
    }
}

/// Corner the card is placed relative to until the user drags it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

/// `[modules.ai_usage]` in the config.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Show the card at startup (the command `ai_usage.toggle` also shows it).
    pub visible: bool,
    pub anchor: Anchor,
    /// Distance from the anchored corner, in pixels.
    pub offset_x: f32,
    pub offset_y: f32,
    /// Card size multiplier (0.6 to 2.5); resizing with the mouse or keys overrides it and is remembered.
    pub scale: f32,
    /// Seconds between log scans while the card is visible.
    pub interval: f32,
    /// Override `~/.claude` (or `CLAUDE_CONFIG_DIR`).
    pub claude_dir: Option<String>,
    /// Override `~/.codex`.
    pub codex_dir: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            visible: false,
            anchor: Anchor::TopRight,
            offset_x: 16.0,
            offset_y: 56.0,
            scale: 1.0,
            interval: 5.0,
            claude_dir: None,
            codex_dir: None,
        }
    }
}

impl Settings {
    /// Top-left pixel of the card at `scale` for a window of `win` size, from the anchor and offsets.
    pub fn origin(&self, win: (f32, f32), scale: f32) -> (f32, f32) {
        let x = match self.anchor {
            Anchor::TopLeft | Anchor::BottomLeft => self.offset_x,
            Anchor::TopRight | Anchor::BottomRight => win.0 - card_width(scale) - self.offset_x,
        };
        let y = match self.anchor {
            Anchor::TopLeft | Anchor::TopRight => self.offset_y,
            Anchor::BottomLeft | Anchor::BottomRight => win.1 - card_height(scale) - self.offset_y,
        };
        (x, y)
    }

    /// Scan period, kept within 1..=60 seconds.
    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f32(self.interval.clamp(1.0, 60.0))
    }
}

/// Keep a card at `scale` fully inside a window of `win`.
pub fn clamp_origin(pos: (f32, f32), win: (f32, f32), scale: f32) -> (f32, f32) {
    let max_x = (win.0 - card_width(scale)).max(0.0);
    let max_y = (win.1 - card_height(scale)).max(0.0);
    (pos.0.clamp(0.0, max_x), pos.1.clamp(0.0, max_y))
}

/// The module registration.
pub struct AiUsageModule;

impl Module for AiUsageModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "ai_usage",
            name: "AI token usage",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::FsRead],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(id::TOGGLE, "Widgets: Token Usage", "Widgets").key("Ctrl+Alt+U"),
            CommandSpec::new(id::RESET_POSITION, "Reset AI Usage Card Position", "AI"),
            CommandSpec::new(id::OPEN_TAB, "Token Usage: Open in a Tab", "AI"),
            CommandSpec::new(id::OPEN_WINDOW, "Token Usage: Open in a Window", "AI"),
            CommandSpec::new(id::LARGER, "Enlarge AI Usage Card", "AI").key("Ctrl+Alt+="),
            CommandSpec::new(id::SMALLER, "Shrink AI Usage Card", "AI").key("Ctrl+Alt+-"),
            CommandSpec::new(id::RESET_SIZE, "Reset AI Usage Card Size", "AI"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_is_valid() {
        mahzenterm_core::module::validate(&AiUsageModule).unwrap();
    }

    #[test]
    fn anchors_place_the_card_in_the_right_corner() {
        let win = (1000.0, 800.0);
        let mut s = Settings::default();
        assert_eq!(s.origin(win, 1.0), (1000.0 - CARD_W - 16.0, 56.0));
        s.anchor = Anchor::BottomLeft;
        let (x, y) = s.origin(win, 1.0);
        assert_eq!(x, 16.0);
        assert!((y + card_height(1.0) + 56.0 - 800.0).abs() < 0.01);
    }

    #[test]
    fn dragging_cannot_push_the_card_out_of_the_window() {
        let win = (1000.0, 800.0);
        assert_eq!(clamp_origin((-50.0, -5.0), win, 1.0), (0.0, 0.0));
        let (x, y) = clamp_origin((5000.0, 5000.0), win, 1.0);
        assert_eq!(x, 1000.0 - CARD_W);
        assert!((y - (800.0 - card_height(1.0))).abs() < 0.01);
    }

    #[test]
    fn a_bigger_card_is_kept_inside_too() {
        let win = (1000.0, 800.0);
        let (x, y) = clamp_origin((5000.0, 5000.0), win, 2.0);
        assert_eq!(x, 1000.0 - card_width(2.0));
        assert!((y - (800.0 - card_height(2.0))).abs() < 0.01);
    }

    #[test]
    fn settings_reject_unknown_keys_and_clamp_the_period() {
        let ok: Settings = toml_like(r#"{"visible":true,"anchor":"bottom_right"}"#);
        assert!(ok.visible && ok.anchor == Anchor::BottomRight);
        assert!(serde_json::from_str::<Settings>(r#"{"bogus":1}"#).is_err());
        let s = Settings {
            interval: 0.0,
            ..Settings::default()
        };
        assert_eq!(s.period().as_secs(), 1);
    }

    fn toml_like(json: &str) -> Settings {
        serde_json::from_str(json).unwrap()
    }
}
