//! Declarative layout of the tab bar and status bar: which widgets appear where, and what custom widgets do.
//!
//! Lists name widgets by id. An id is either a built-in widget (`cpu`, `clock`, ...) or a custom one defined in
//! `[widgets.<id>]`; a definition with a built-in id overrides that built-in's look.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What a widget shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    /// Fixed text (`text`), optionally scrolling (`marquee`).
    Text,
    /// First line of a shell command's output, refreshed every `interval` seconds.
    Command,
    /// Logo and product name.
    Brand,
    /// Liveness dot and the focused pane's title or directory.
    Status,
    /// `PANE 2/3`, only with several panes.
    Pane,
    /// `TAB 1/2`, only with several tabs.
    Tab,
    /// `ZOOM`, only while a pane is zoomed.
    Zoom,
    /// `EXIT 1`: status of the last command, only when it failed (needs shell integration).
    Exit,
    /// Config/theme problem, only while there is one.
    Problem,
    Cpu,
    Memory,
    Load,
    Uptime,
    Clock,
    /// Dot and the active theme's name.
    Theme,
    /// Dot and the active profile's name, only while a profile is active.
    Profile,
    /// Tokens Claude Code and Codex used today (click: the token card).
    Tokens,
    /// Running AI sessions: busy and idle counts, or how many need you (click: go to the one that does).
    Ai,
    /// `context/namespace` of the current Kubernetes context (red for production), only when known.
    Kube,
    Slogan,
    /// Flexible empty space.
    Spacer,
    /// Thin vertical line.
    Separator,
    /// Minimize, maximize and close buttons (custom title bar only).
    Controls,
}

impl WidgetKind {
    /// The built-in widget with this id, if any. `text` and `command` have no built-in form.
    pub fn builtin(id: &str) -> Option<Self> {
        Some(match id {
            "brand" => Self::Brand,
            "status" => Self::Status,
            "pane" => Self::Pane,
            "tab" => Self::Tab,
            "zoom" => Self::Zoom,
            "exit" => Self::Exit,
            "problem" => Self::Problem,
            "cpu" => Self::Cpu,
            "memory" => Self::Memory,
            "load" => Self::Load,
            "uptime" => Self::Uptime,
            "clock" => Self::Clock,
            "theme" => Self::Theme,
            "profile" => Self::Profile,
            "kube" => Self::Kube,
            "tokens" => Self::Tokens,
            "ai" => Self::Ai,
            "slogan" => Self::Slogan,
            "spacer" => Self::Spacer,
            "separator" => Self::Separator,
            "controls" => Self::Controls,
            _ => return None,
        })
    }
}

/// How a widget is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetStyle {
    /// Plain text (the default; `cpu` and `memory` default to `bar`).
    #[default]
    Auto,
    Text,
    /// Small bordered tag.
    Chip,
    /// Value with a mini gauge (`cpu`, `memory`).
    Bar,
}

/// Theme token names a widget may use as its colour.
pub const COLOR_TOKENS: &[&str] = &["accent", "text", "text_dim", "ok", "warn", "danger"];

/// Definition of one widget (`[widgets.<id>]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetDef {
    pub kind: WidgetKind,
    /// Content of a `text` widget.
    #[serde(default)]
    pub text: Option<String>,
    /// Shell line of a `command` widget; runs only from your own config file, never a project's.
    #[serde(default)]
    pub command: Option<String>,
    /// `strftime` pattern of a `clock` (default `%H:%M`).
    #[serde(default)]
    pub format: Option<String>,
    /// Small dim prefix, e.g. `GPU`.
    #[serde(default)]
    pub label: Option<String>,
    /// Glyph shown before the value, in the accent colour.
    #[serde(default)]
    pub icon: Option<String>,
    /// Theme token: accent, text, text_dim, ok, warn or danger.
    #[serde(default)]
    pub color: Option<String>,
    /// Refresh period in seconds (1..=86400) of `command` and `cpu`/`memory`/`load`.
    #[serde(default)]
    pub interval: Option<f32>,
    /// Scroll the text sideways when it is wider than `width`.
    #[serde(default)]
    pub marquee: bool,
    /// Width in characters of the text area (4..=200); with `marquee` the visible window.
    #[serde(default)]
    pub width: Option<u16>,
    /// Marquee speed in characters per second (1..=40, default 8).
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub style: WidgetStyle,
    /// Command id (see the palette) run when the widget is clicked.
    #[serde(default)]
    pub on_click: Option<String>,
}

impl WidgetDef {
    /// A widget of `kind` with every option at its default.
    pub fn of(kind: WidgetKind) -> Self {
        Self {
            kind,
            text: None,
            command: None,
            format: None,
            label: None,
            icon: None,
            color: None,
            interval: None,
            marquee: false,
            width: None,
            speed: None,
            style: WidgetStyle::Auto,
            on_click: None,
        }
    }
}

/// The tab bar: tabs sit between `left` and `right`; empty space drags the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TabbarConfig {
    pub left: Vec<String>,
    pub right: Vec<String>,
    /// Width of one tab in pixels (90..=400).
    pub tab_width: f32,
    /// Show the `×` on each tab.
    pub close_buttons: bool,
    /// Show the `+` after the last tab.
    pub new_tab_button: bool,
}

impl Default for TabbarConfig {
    fn default() -> Self {
        Self {
            left: vec!["brand".into()],
            right: vec!["controls".into()],
            tab_width: 176.0,
            close_buttons: true,
            new_tab_button: true,
        }
    }
}

/// The status bar: three groups; the centre one stays centred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatusbarConfig {
    pub visible: bool,
    pub left: Vec<String>,
    pub center: Vec<String>,
    pub right: Vec<String>,
}

impl Default for StatusbarConfig {
    fn default() -> Self {
        let v = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect();
        Self {
            visible: true,
            left: v(&["status", "exit", "pane", "tab", "zoom", "problem"]),
            center: Vec::new(),
            right: v(&["kube", "profile", "cpu", "memory", "clock", "theme"]),
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Check one definition on its own (what a single config file can know).
pub fn validate_def(id: &str, def: &WidgetDef) -> Result<(), (String, String)> {
    let at = |f: &str| format!("widgets.{id}.{f}");
    let bad = |f: &str, m: &str| Err((at(f), m.to_owned()));
    if !valid_id(id) {
        return Err((
            format!("widgets.{id}"),
            "id must be 1-32 characters of a-z, 0-9 and _".into(),
        ));
    }
    if def.kind == WidgetKind::Text && def.text.is_none() {
        return bad("text", "is required for kind = \"text\"");
    }
    if def.kind == WidgetKind::Command && def.command.as_deref().is_none_or(|c| c.trim().is_empty())
    {
        return bad("command", "is required for kind = \"command\"");
    }
    if let Some(i) = def.interval {
        if !(1.0..=86_400.0).contains(&i) {
            return bad("interval", "must be between 1 and 86400 seconds");
        }
    }
    if let Some(w) = def.width {
        if !(4..=200).contains(&w) {
            return bad("width", "must be between 4 and 200 characters");
        }
    }
    if let Some(s) = def.speed {
        if !(1.0..=40.0).contains(&s) {
            return bad("speed", "must be between 1 and 40 characters per second");
        }
    }
    if let Some(c) = &def.color {
        if !COLOR_TOKENS.contains(&c.as_str()) {
            return bad(
                "color",
                "must be one of: accent, text, text_dim, ok, warn, danger",
            );
        }
    }
    Ok(())
}

/// Check that every id in the bar lists is a built-in or defined (needs the merged config).
pub fn validate_refs(
    tabbar: &TabbarConfig,
    statusbar: &StatusbarConfig,
    widgets: &BTreeMap<String, WidgetDef>,
) -> Result<(), (String, String)> {
    let lists: [(&str, &Vec<String>); 5] = [
        ("tabbar.left", &tabbar.left),
        ("tabbar.right", &tabbar.right),
        ("statusbar.left", &statusbar.left),
        ("statusbar.center", &statusbar.center),
        ("statusbar.right", &statusbar.right),
    ];
    for (field, ids) in lists {
        for id in ids {
            if !widgets.contains_key(id) && WidgetKind::builtin(id).is_none() {
                return Err((
                    field.to_owned(),
                    format!(
                        "names unknown widget \"{id}\"; define it in [widgets.{id}] or use a built-in \
                         (brand, status, exit, pane, tab, zoom, problem, cpu, memory, load, uptime, clock, theme, \
                         slogan, spacer, separator, controls)"
                    ),
                ));
            }
        }
    }
    if !(90.0..=400.0).contains(&tabbar.tab_width) {
        return Err((
            "tabbar.tab_width".into(),
            "must be between 90 and 400".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_reference_only_builtins() {
        assert!(validate_refs(
            &TabbarConfig::default(),
            &StatusbarConfig::default(),
            &BTreeMap::new()
        )
        .is_ok());
    }

    #[test]
    fn unknown_widget_is_reported_with_its_list() {
        let bars = StatusbarConfig {
            right: vec!["nope".into()],
            ..StatusbarConfig::default()
        };
        let (field, msg) =
            validate_refs(&TabbarConfig::default(), &bars, &BTreeMap::new()).unwrap_err();
        assert_eq!(field, "statusbar.right");
        assert!(msg.contains("nope"));
    }

    #[test]
    fn custom_widget_satisfies_a_reference() {
        let mut defs = BTreeMap::new();
        defs.insert("news".to_owned(), WidgetDef::of(WidgetKind::Text));
        let bars = StatusbarConfig {
            center: vec!["news".into()],
            ..StatusbarConfig::default()
        };
        assert!(validate_refs(&TabbarConfig::default(), &bars, &defs).is_ok());
    }

    #[test]
    fn definitions_are_validated() {
        let mut d = WidgetDef::of(WidgetKind::Command);
        assert!(validate_def("gpu", &d).is_err());
        d.command = Some("echo 1".into());
        assert!(validate_def("gpu", &d).is_ok());
        d.interval = Some(0.1);
        assert_eq!(
            validate_def("gpu", &d).unwrap_err().0,
            "widgets.gpu.interval"
        );
        d.interval = Some(5.0);
        d.color = Some("#ff0000".into());
        assert_eq!(validate_def("gpu", &d).unwrap_err().0, "widgets.gpu.color");
        assert!(validate_def("Bad Id", &WidgetDef::of(WidgetKind::Cpu)).is_err());
        assert!(validate_def("t", &WidgetDef::of(WidgetKind::Text)).is_err());
    }

    #[test]
    fn builtin_ids_resolve() {
        assert_eq!(WidgetKind::builtin("cpu"), Some(WidgetKind::Cpu));
        assert_eq!(WidgetKind::builtin("profile"), Some(WidgetKind::Profile));
        assert_eq!(WidgetKind::builtin("kube"), Some(WidgetKind::Kube));
        assert_eq!(WidgetKind::builtin("tokens"), Some(WidgetKind::Tokens));
        assert_eq!(WidgetKind::builtin("ai"), Some(WidgetKind::Ai));
        assert_eq!(WidgetKind::builtin("text"), None);
    }
}
