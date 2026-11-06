//! Command palette: fuzzy search over the command registry with live theme preview.
//!
//! State and key handling live here as plain data/functions (unit-tested); the shell owns the instance and applies
//! the effects (running a command, previewing a theme).

use std::collections::HashMap;

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding};
use mahzenterm_core::command::{Keymap, Registry};
use mahzenterm_ui_kit::{Theme, Themes, Tokens};

use crate::shell::Message;

/// Rows shown at once.
pub const VISIBLE_ROWS: usize = 10;
/// Most results kept per query.
const MAX_RESULTS: usize = 60;

/// An open palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub query: String,
    pub selected: usize,
    /// Command ids matching the query, best first.
    pub results: Vec<String>,
    /// Theme shown when the palette opened; restored when it closes without choosing a theme.
    pub origin_theme: String,
}

/// What a key press did to the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing visible changed (key swallowed).
    Idle,
    /// Query or selection changed; the shell should refresh results and preview.
    Changed { query_changed: bool },
    /// Close without running anything.
    Close,
    /// Run the selected command and close.
    Run(String),
}

/// A key press reduced to what the palette cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteKey<'a> {
    Escape,
    Enter,
    Up,
    Down,
    PageUp,
    PageDown,
    Backspace,
    /// Ctrl+U: clear the query.
    ClearLine,
    Text(&'a str),
    Other,
}

impl Palette {
    /// Open with `results` for the empty query.
    pub fn new(results: Vec<String>, origin_theme: String) -> Self {
        Self {
            query: String::new(),
            selected: 0,
            results,
            origin_theme,
        }
    }

    /// The highlighted command id.
    pub fn current(&self) -> Option<&str> {
        self.results.get(self.selected).map(String::as_str)
    }

    /// Replace results after a query change; selection returns to the top.
    pub fn set_results(&mut self, mut results: Vec<String>) {
        results.truncate(MAX_RESULTS);
        self.results = results;
        self.selected = 0;
    }

    fn move_by(&mut self, delta: isize) {
        let n = self.results.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(n as isize) as usize;
    }

    /// Apply a key; typing edits the query, arrows move the selection.
    pub fn on_key(&mut self, key: PaletteKey<'_>) -> Outcome {
        match key {
            PaletteKey::Escape => Outcome::Close,
            PaletteKey::Enter => self
                .current()
                .map_or(Outcome::Idle, |c| Outcome::Run(c.to_owned())),
            PaletteKey::Up => {
                self.move_by(-1);
                Outcome::Changed {
                    query_changed: false,
                }
            }
            PaletteKey::Down => {
                self.move_by(1);
                Outcome::Changed {
                    query_changed: false,
                }
            }
            PaletteKey::PageUp => {
                self.move_by(-(VISIBLE_ROWS as isize));
                Outcome::Changed {
                    query_changed: false,
                }
            }
            PaletteKey::PageDown => {
                self.move_by(VISIBLE_ROWS as isize);
                Outcome::Changed {
                    query_changed: false,
                }
            }
            PaletteKey::Backspace => {
                if self.query.pop().is_some() {
                    Outcome::Changed {
                        query_changed: true,
                    }
                } else {
                    Outcome::Idle
                }
            }
            PaletteKey::ClearLine => {
                if self.query.is_empty() {
                    Outcome::Idle
                } else {
                    self.query.clear();
                    Outcome::Changed {
                        query_changed: true,
                    }
                }
            }
            PaletteKey::Text(t) => {
                let clean: String = t.chars().filter(|c| !c.is_control()).collect();
                if clean.is_empty() {
                    Outcome::Idle
                } else {
                    self.query.push_str(&clean);
                    Outcome::Changed {
                        query_changed: true,
                    }
                }
            }
            PaletteKey::Other => Outcome::Idle,
        }
    }

    /// First visible row index so the selection stays on screen.
    pub fn first_visible(&self) -> usize {
        self.selected.saturating_sub(VISIBLE_ROWS - 1)
    }
}

fn swatch<'a>(c: Color, t: Tokens) -> Element<'a, Message> {
    container(Space::new())
        .width(Length::Fixed(10.0))
        .height(Length::Fixed(10.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(c)),
            border: Border {
                color: t.border,
                width: 1.0,
                radius: 2.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn rgb(c: mahzenterm_term::Rgb) -> Color {
    Color::from_rgb8(c.0, c.1, c.2)
}

fn theme_swatches<'a>(theme: &Theme, t: Tokens) -> Element<'a, Message> {
    let p = &theme.palette;
    row![
        swatch(rgb(p.bg), t),
        swatch(theme.tokens.accent, t),
        swatch(rgb(p.ansi[1]), t),
        swatch(rgb(p.ansi[2]), t),
        swatch(rgb(p.ansi[4]), t),
    ]
    .spacing(3)
    .into()
}

/// The overlay: an invisible backdrop (click closes) and the palette card near the top.
/// No translucent layers: partial repaints of the CPU renderer would blend them twice.
pub fn view<'a>(
    palette: &'a Palette,
    registry: &'a Registry,
    keymap: &'a Keymap,
    themes: &'a Themes,
    theme_cmds: &'a HashMap<String, String>,
    t: Tokens,
) -> Element<'a, Message> {
    let query = if palette.query.is_empty() {
        row![
            text("›").size(16).color(t.accent),
            text("Type a command, or a theme name…")
                .size(15)
                .color(t.text_dim)
        ]
    } else {
        row![
            text("›").size(16).color(t.accent),
            text(format!("{}▏", palette.query)).size(15).color(t.text)
        ]
    }
    .spacing(10)
    .align_y(iced::Alignment::Center);

    let mut list = column![].spacing(1);
    let start = palette.first_visible();
    for (i, cmd) in palette
        .results
        .iter()
        .enumerate()
        .skip(start)
        .take(VISIBLE_ROWS)
    {
        let Some(spec) = registry.get(cmd) else {
            continue;
        };
        let selected = i == palette.selected;
        let mut line = row![text(spec.title.clone()).size(14).color(if selected {
            t.text
        } else {
            t.text_dim
        })]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if let Some(theme) = theme_cmds.get(cmd).and_then(|tid| themes.get(tid)) {
            line = line.push(theme_swatches(theme, t));
        }
        line = line.push(Space::new().width(Length::Fill));
        if let Some(keys) = keymap.shortcut_for(cmd) {
            line = line.push(text(keys).size(11).color(t.text_dim));
        } else {
            line = line.push(text(spec.category.clone()).size(11).color(t.text_dim));
        }
        let bar = container(Space::new())
            .width(Length::Fixed(2.0))
            .height(Length::Fixed(18.0))
            .style(move |_| {
                container::background(if selected {
                    t.accent
                } else {
                    Color::TRANSPARENT
                })
            });
        let rowbox = container(row![bar, line].spacing(10).align_y(iced::Alignment::Center))
            .width(Length::Fill)
            .padding(Padding::from([7.0, 12.0]))
            .style(move |_| container::Style {
                background: selected.then_some(Background::Color(t.surface_hi)),
                border: Border::default().rounded(6.0),
                ..container::Style::default()
            });
        list = list.push(mouse_area(rowbox).on_press(Message::PaletteRun(i)));
    }
    if palette.results.is_empty() {
        list = list.push(
            container(text("No matching command").size(13).color(t.text_dim))
                .padding(Padding::from([10.0, 14.0])),
        );
    }

    let footer = row![
        text("↑↓ navigate   ⏎ run   esc close")
            .size(11)
            .color(t.text_dim),
        Space::new().width(Length::Fill),
        text(format!("{} results", palette.results.len()))
            .size(11)
            .color(t.text_dim),
    ];
    let card = container(
        column![
            container(
                row![
                    crate::chrome::tag("COMMANDS", t, true),
                    Space::new().width(Length::Fill),
                    text("themes preview live").size(10).color(t.text_dim),
                ]
                .align_y(iced::Alignment::Center)
            )
            .padding(Padding::from([2.0, 8.0])),
            container(query).padding(Padding::from([4.0, 8.0])),
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(move |_| container::background(t.border)),
            list,
            container(footer).padding(Padding::from([4.0, 8.0])),
        ]
        .spacing(8),
    )
    .width(Length::Fixed(620.0))
    .padding(10)
    .style(move |_| container::Style {
        background: Some(Background::Color(t.surface)),
        border: Border {
            color: t.border,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    });

    let backdrop = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style::default()),
    )
    .on_press(Message::PaletteClose);
    iced::widget::stack![
        backdrop,
        container(card)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center)
            .padding(Padding {
                top: 64.0,
                ..Padding::ZERO
            })
    ]
    .into()
}

/// Reduce an Iced key press to a [`PaletteKey`]. `text` is the layout-aware text of the press.
pub fn classify<'a>(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    text: Option<&'a str>,
) -> PaletteKey<'a> {
    use iced::keyboard::key::Named;
    use iced::keyboard::Key;
    match key {
        Key::Named(Named::Escape) => PaletteKey::Escape,
        Key::Named(Named::Enter) => PaletteKey::Enter,
        Key::Named(Named::ArrowUp) => PaletteKey::Up,
        Key::Named(Named::ArrowDown) => PaletteKey::Down,
        Key::Named(Named::PageUp) => PaletteKey::PageUp,
        Key::Named(Named::PageDown) => PaletteKey::PageDown,
        Key::Named(Named::Backspace) => PaletteKey::Backspace,
        Key::Character(c) if modifiers.control() => match c.to_lowercase().as_str() {
            "u" => PaletteKey::ClearLine,
            "n" => PaletteKey::Down,
            "p" => PaletteKey::Up,
            "j" => PaletteKey::Down,
            "k" => PaletteKey::Up,
            _ => PaletteKey::Other,
        },
        _ if modifiers.control() || modifiers.alt() || modifiers.logo() => PaletteKey::Other,
        _ => text.map_or(PaletteKey::Other, PaletteKey::Text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::Named;
    use iced::keyboard::{Key, Modifiers};

    fn pal() -> Palette {
        Palette::new(
            vec!["a.one".into(), "a.two".into(), "a.three".into()],
            "dark".into(),
        )
    }

    #[test]
    fn typing_edits_the_query_and_backspace_removes() {
        let mut p = pal();
        assert_eq!(
            p.on_key(PaletteKey::Text("sp")),
            Outcome::Changed {
                query_changed: true
            }
        );
        assert_eq!(p.query, "sp");
        assert_eq!(
            p.on_key(PaletteKey::Backspace),
            Outcome::Changed {
                query_changed: true
            }
        );
        assert_eq!(p.query, "s");
        p.on_key(PaletteKey::Backspace);
        assert_eq!(
            p.on_key(PaletteKey::Backspace),
            Outcome::Idle,
            "nothing left to delete"
        );
    }

    #[test]
    fn control_characters_never_enter_the_query() {
        let mut p = pal();
        assert_eq!(p.on_key(PaletteKey::Text("\u{7f}\r")), Outcome::Idle);
        assert!(p.query.is_empty());
    }

    #[test]
    fn selection_wraps_both_ways() {
        let mut p = pal();
        p.on_key(PaletteKey::Up);
        assert_eq!(p.selected, 2);
        p.on_key(PaletteKey::Down);
        assert_eq!(p.selected, 0);
        p.on_key(PaletteKey::PageDown);
        assert_eq!(
            p.selected, 1,
            "a page of 10 over 3 items wraps around (10 mod 3 = 1)"
        );
    }

    #[test]
    fn enter_runs_the_selection_and_escape_closes() {
        let mut p = pal();
        p.on_key(PaletteKey::Down);
        assert_eq!(p.on_key(PaletteKey::Enter), Outcome::Run("a.two".into()));
        assert_eq!(p.on_key(PaletteKey::Escape), Outcome::Close);
        let mut empty = Palette::new(vec![], "x".into());
        assert_eq!(empty.on_key(PaletteKey::Enter), Outcome::Idle);
        assert_eq!(
            empty.on_key(PaletteKey::Down),
            Outcome::Changed {
                query_changed: false
            }
        );
    }

    #[test]
    fn new_results_reset_the_selection_and_are_capped() {
        let mut p = pal();
        p.on_key(PaletteKey::Down);
        p.set_results((0..200).map(|i| format!("a.c{i}")).collect());
        assert_eq!(p.selected, 0);
        assert_eq!(p.results.len(), MAX_RESULTS);
    }

    #[test]
    fn clear_line_empties_the_query() {
        let mut p = pal();
        p.on_key(PaletteKey::Text("abc"));
        assert_eq!(
            p.on_key(PaletteKey::ClearLine),
            Outcome::Changed {
                query_changed: true
            }
        );
        assert!(p.query.is_empty());
        assert_eq!(p.on_key(PaletteKey::ClearLine), Outcome::Idle);
    }

    #[test]
    fn window_follows_the_selection() {
        let mut p = Palette::new((0..30).map(|i| format!("a.c{i}")).collect(), "x".into());
        assert_eq!(p.first_visible(), 0);
        p.selected = 25;
        assert_eq!(p.first_visible(), 25 - (VISIBLE_ROWS - 1));
        assert!(p.selected < p.first_visible() + VISIBLE_ROWS);
    }

    #[test]
    fn classify_maps_keys() {
        let none = Modifiers::empty();
        assert_eq!(
            classify(&Key::Named(Named::Escape), none, None),
            PaletteKey::Escape
        );
        assert_eq!(
            classify(&Key::Named(Named::ArrowDown), none, None),
            PaletteKey::Down
        );
        assert_eq!(
            classify(&Key::Character("a".into()), none, Some("a")),
            PaletteKey::Text("a")
        );
        assert_eq!(
            classify(&Key::Character("u".into()), Modifiers::CTRL, Some("\u{15}")),
            PaletteKey::ClearLine
        );
        assert_eq!(
            classify(&Key::Character("n".into()), Modifiers::CTRL, None),
            PaletteKey::Down
        );
        assert_eq!(
            classify(&Key::Character("x".into()), Modifiers::ALT, Some("x")),
            PaletteKey::Other
        );
        // Layout-aware text: Turkish letters reach the query.
        assert_eq!(
            classify(&Key::Character("ş".into()), none, Some("ş")),
            PaletteKey::Text("ş")
        );
    }
}
