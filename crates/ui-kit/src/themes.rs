//! Theme engine: declarative TOML themes (terminal palette plus accent) and a registry of built-in and user themes.
//!
//! A theme only lists the terminal colours and one accent; the application chrome is derived from them
//! ([`Tokens::from_terminal`]) so panes, tabs and dividers always match the terminal and stay readable.

use std::collections::BTreeMap;
use std::path::Path;

use iced::Color;
use mahzenterm_term::{Palette, Rgb};
use serde::Deserialize;
use thiserror::Error;

use crate::theme::{contrast, Tokens};

/// Whether a theme is meant for a dark or a light surrounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Dark,
    Light,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Colors {
    bg: String,
    fg: String,
    cursor: String,
    selection: String,
    accent: String,
    ansi: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    id: String,
    name: String,
    kind: Kind,
    colors: Colors,
}

/// A fully resolved theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Stable lowercase id used in config (`appearance.theme`).
    pub id: String,
    /// Display name for the palette and UI.
    pub name: String,
    pub kind: Kind,
    /// Terminal colours handed to sessions.
    pub palette: Palette,
    /// Selected-cell background.
    pub selection: Rgb,
    /// Application chrome colours derived from the palette.
    pub tokens: Tokens,
    /// Non-fatal quality problems (for example low text contrast).
    pub warnings: Vec<String>,
}

/// Error loading a theme. Says which theme and what to fix.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("theme {source_name}: {message}")]
pub struct ThemeError {
    pub source_name: String,
    pub message: String,
}

fn err(source: &str, message: impl Into<String>) -> ThemeError {
    ThemeError {
        source_name: source.to_owned(),
        message: message.into(),
    }
}

fn hex(src: &str, field: &str, origin: &str) -> Result<Rgb, ThemeError> {
    let h = src.strip_prefix('#').unwrap_or(src);
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err(
            origin,
            format!("`{field}` must look like \"#rrggbb\", got \"{src}\""),
        ));
    }
    let v = u32::from_str_radix(h, 16).map_err(|e| err(origin, e.to_string()))?;
    Ok(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn color(c: Rgb) -> Color {
    Color::from_rgb8(c.0, c.1, c.2)
}

/// Parse and validate one theme. `origin` names the file for error messages.
pub fn parse_theme(text: &str, origin: &str) -> Result<Theme, ThemeError> {
    let spec: Spec =
        toml::from_str(text).map_err(|e| err(origin, e.to_string().replace('\n', " ")))?;
    let valid_id = !spec.id.is_empty()
        && spec
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_id {
        return Err(err(
            origin,
            format!(
                "`id` \"{}\" must use lowercase letters, digits and `-`",
                spec.id
            ),
        ));
    }
    if spec.colors.ansi.len() != 16 {
        return Err(err(
            origin,
            format!(
                "`ansi` needs exactly 16 colours, found {}",
                spec.colors.ansi.len()
            ),
        ));
    }
    let c = &spec.colors;
    let mut ansi = [Rgb(0, 0, 0); 16];
    for (i, v) in c.ansi.iter().enumerate() {
        ansi[i] = hex(v, &format!("ansi[{i}]"), origin)?;
    }
    let palette = Palette {
        fg: hex(&c.fg, "fg", origin)?,
        bg: hex(&c.bg, "bg", origin)?,
        cursor: hex(&c.cursor, "cursor", origin)?,
        ansi,
    };
    let selection = hex(&c.selection, "selection", origin)?;
    let accent = hex(&c.accent, "accent", origin)?;
    let tokens = Tokens::from_terminal(
        color(palette.bg),
        color(palette.fg),
        color(accent),
        color(palette.ansi[1]),
        color(palette.ansi[2]),
        color(palette.ansi[3]),
    );
    let mut warnings = Vec::new();
    let text_contrast = contrast(color(palette.fg), color(palette.bg));
    if text_contrast < 4.5 {
        warnings.push(format!(
            "text contrast {text_contrast:.1}:1 is below 4.5:1; brighten `fg` or darken `bg`"
        ));
    }
    Ok(Theme {
        id: spec.id,
        name: spec.name,
        kind: spec.kind,
        palette,
        selection,
        tokens,
        warnings,
    })
}

/// Built-in theme sources, the default first.
const BUILTIN: [(&str, &str); 11] = [
    (
        "mahzenterm-dark",
        include_str!("../../../themes/mahzenterm-dark.toml"),
    ),
    (
        "mahzenterm-abyss",
        include_str!("../../../themes/mahzenterm-abyss.toml"),
    ),
    (
        "mahzenterm-light",
        include_str!("../../../themes/mahzenterm-light.toml"),
    ),
    (
        "tokyo-night",
        include_str!("../../../themes/tokyo-night.toml"),
    ),
    ("dracula", include_str!("../../../themes/dracula.toml")),
    ("nord", include_str!("../../../themes/nord.toml")),
    (
        "gruvbox-dark",
        include_str!("../../../themes/gruvbox-dark.toml"),
    ),
    (
        "solarized-dark",
        include_str!("../../../themes/solarized-dark.toml"),
    ),
    (
        "synthwave-84",
        include_str!("../../../themes/synthwave-84.toml"),
    ),
    (
        "catppuccin-mocha",
        include_str!("../../../themes/catppuccin-mocha.toml"),
    ),
    (
        "catppuccin-latte",
        include_str!("../../../themes/catppuccin-latte.toml"),
    ),
];

/// Id of the default theme.
pub const DEFAULT_ID: &str = "mahzenterm-dark";

/// All available themes: built-in first (fixed order), then user themes sorted by id.
#[derive(Debug, Clone)]
pub struct Themes {
    list: Vec<Theme>,
}

impl Themes {
    /// The built-in set. A broken built-in is a build-time bug caught by tests; at runtime it is skipped and logged.
    pub fn builtin() -> Self {
        let mut list = Vec::new();
        for (name, src) in BUILTIN {
            match parse_theme(src, name) {
                Ok(t) => list.push(t),
                Err(e) => tracing::error!(error = %e, "invalid built-in theme"),
            }
        }
        Self { list }
    }

    /// Add every `*.toml` in `dir` as a user theme. A user theme with a built-in id replaces it.
    /// Returns the problems found; the other files still load.
    pub fn load_user_dir(&mut self, dir: &Path) -> Vec<ThemeError> {
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return errors;
        };
        let mut found: BTreeMap<String, Theme> = BTreeMap::new();
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let origin = path.display().to_string();
            match std::fs::read_to_string(&path) {
                Ok(text) => match parse_theme(&text, &origin) {
                    Ok(t) => {
                        found.insert(t.id.clone(), t);
                    }
                    Err(e) => errors.push(e),
                },
                Err(e) => errors.push(err(&origin, format!("cannot read file: {e}"))),
            }
        }
        for (_, t) in found {
            match self.list.iter().position(|x| x.id == t.id) {
                Some(i) => self.list[i] = t,
                None => self.list.push(t),
            }
        }
        errors
    }

    /// Every theme, in display order.
    pub fn all(&self) -> &[Theme] {
        &self.list
    }

    /// Look up by id.
    pub fn get(&self, id: &str) -> Option<&Theme> {
        self.list.iter().find(|t| t.id == id)
    }

    /// The default theme (falls back to the first available one).
    pub fn default_theme(&self) -> Option<&Theme> {
        self.get(DEFAULT_ID).or_else(|| self.list.first())
    }

    /// Id of the theme `step` places after `current` (negative goes back), wrapping around.
    pub fn cycle(&self, current: &str, step: i32) -> Option<&Theme> {
        if self.list.is_empty() {
            return None;
        }
        let len = self.list.len() as i32;
        let at = self.list.iter().position(|t| t.id == current).unwrap_or(0) as i32;
        self.list.get((at + step).rem_euclid(len) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r##"
id = "x"
name = "X"
kind = "dark"
[colors]
bg = "#000000"
fg = "#ffffff"
cursor = "#ffffff"
selection = "#333333"
accent = "#00ff00"
ansi = ["#000000","#111111","#222222","#333333","#444444","#555555","#666666","#777777",
        "#888888","#999999","#aaaaaa","#bbbbbb","#cccccc","#dddddd","#eeeeee","#ffffff"]
"##;

    #[test]
    fn parses_a_valid_theme() {
        let t = parse_theme(GOOD, "x.toml").unwrap();
        assert_eq!(t.palette.ansi[15], Rgb(255, 255, 255));
        assert_eq!(t.palette.bg, Rgb(0, 0, 0));
        assert!(t.warnings.is_empty());
    }

    #[test]
    fn rejects_bad_colours_and_counts_with_actionable_messages() {
        let e = parse_theme(&GOOD.replace("#00ff00", "green"), "x.toml").unwrap_err();
        assert!(
            e.message.contains("accent") && e.message.contains("#rrggbb"),
            "{e}"
        );
        let e = parse_theme(&GOOD.replace("\"#ffffff\"]", "]"), "x.toml").unwrap_err();
        assert!(e.message.contains("16"), "{e}");
        let e = parse_theme(&GOOD.replace("id = \"x\"", "id = \"Bad Id\""), "x.toml").unwrap_err();
        assert!(e.message.contains("id"), "{e}");
        let e = parse_theme(
            &GOOD.replace("kind = \"dark\"", "kind = \"blue\""),
            "x.toml",
        )
        .unwrap_err();
        assert!(e.source_name == "x.toml");
        let e = parse_theme(&format!("{GOOD}\nsurprise = 1"), "x.toml").unwrap_err();
        assert!(e.message.contains("surprise"), "{e}");
    }

    #[test]
    fn low_contrast_is_a_warning_not_an_error() {
        let t = parse_theme(
            &GOOD.replace("fg = \"#ffffff\"", "fg = \"#111111\""),
            "x.toml",
        )
        .unwrap();
        assert_eq!(t.warnings.len(), 1);
    }

    #[test]
    fn every_builtin_theme_loads_cleanly() {
        let themes = Themes::builtin();
        assert_eq!(themes.all().len(), BUILTIN.len());
        let mut ids: Vec<_> = themes.all().iter().map(|t| t.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BUILTIN.len(), "ids must be unique");
        for t in themes.all() {
            assert!(t.warnings.is_empty(), "{}: {:?}", t.id, t.warnings);
            // Chrome text is readable on every chrome surface (UX rule: contrast >= 4.5:1).
            for bg in [t.tokens.bg, t.tokens.surface, t.tokens.surface_hi] {
                assert!(contrast(t.tokens.text, bg) >= 4.5, "{} text", t.id);
                assert!(contrast(t.tokens.text_dim, bg) >= 4.5, "{} dim text", t.id);
            }
        }
    }

    #[test]
    fn default_theme_matches_the_default_palette_and_tokens() {
        let themes = Themes::builtin();
        let d = themes.default_theme().unwrap();
        assert_eq!(d.id, DEFAULT_ID);
        assert_eq!(d.palette, Palette::default());
        assert_eq!(d.tokens.bg, Tokens::dark().bg);
        assert_eq!(d.tokens.accent, Tokens::dark().accent);
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        let themes = Themes::builtin();
        let n = themes.all().len() as i32;
        let first = themes.all()[0].id.clone();
        assert_eq!(
            themes.cycle(&first, -1).unwrap().id,
            themes.all()[(n - 1) as usize].id
        );
        assert_eq!(themes.cycle(&first, n).unwrap().id, first);
        assert_eq!(themes.cycle("unknown", 1).unwrap().id, themes.all()[1].id);
    }

    #[test]
    fn user_dir_adds_replaces_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mine.toml"), GOOD).unwrap();
        std::fs::write(
            dir.path().join("override.toml"),
            GOOD.replace("id = \"x\"", "id = \"nord\"")
                .replace("name = \"X\"", "name = \"My Nord\""),
        )
        .unwrap();
        std::fs::write(dir.path().join("broken.toml"), "id = ").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let mut themes = Themes::builtin();
        let before = themes.all().len();
        let errors = themes.load_user_dir(dir.path());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].source_name.ends_with("broken.toml"));
        assert_eq!(themes.all().len(), before + 1);
        assert_eq!(themes.get("nord").unwrap().name, "My Nord");
        assert!(themes.get("x").is_some());
        assert!(Themes::builtin()
            .load_user_dir(Path::new("/definitely/missing"))
            .is_empty());
    }
}
