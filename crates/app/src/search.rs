//! Find-in-scrollback bar: state, key handling and the small floating view.

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use iced::widget::text::Wrapping;
use iced::widget::{button, container, row, text, Space};
use iced::{Alignment, Background, Border, Element, Length, Padding};
use mahzenterm_core::workspace::{PaneId, TabId};
use mahzenterm_ui_kit::{fonts, Tokens};

use crate::shell::Message;

/// Width of the bar in pixels.
pub const WIDTH: f32 = 380.0;

/// An open search on one pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchBar {
    pub query: String,
    /// Treat the query as a regular expression.
    pub regex: bool,
    pub pane: (TabId, PaneId),
}

/// What a key press means while the bar is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchKey {
    Close,
    Next,
    Prev,
    Backspace,
    ToggleRegex,
    Insert(String),
    /// Not for the bar: let the normal shortcuts handle it (the terminal never sees it).
    Other,
}

/// Classify a key press for the search bar.
pub fn classify(key: &Key, modifiers: Modifiers, text: Option<&str>) -> SearchKey {
    match key {
        Key::Named(Named::Escape) => SearchKey::Close,
        Key::Named(Named::Enter) if modifiers.shift() => SearchKey::Prev,
        Key::Named(Named::Enter) => SearchKey::Next,
        Key::Named(Named::F3) if modifiers.shift() => SearchKey::Prev,
        Key::Named(Named::F3) => SearchKey::Next,
        Key::Named(Named::Backspace) => SearchKey::Backspace,
        Key::Character(c)
            if modifiers.alt() && !modifiers.control() && c.eq_ignore_ascii_case("r") =>
        {
            SearchKey::ToggleRegex
        }
        _ if modifiers.control() || modifiers.alt() || modifiers.logo() => SearchKey::Other,
        _ => match text {
            Some(t) if !t.is_empty() && t.chars().all(|c| !c.is_control()) => {
                SearchKey::Insert(t.to_owned())
            }
            _ => SearchKey::Other,
        },
    }
}

/// The bar: query with caret, match counter, previous/next/close buttons. Opaque, so it never blends.
pub fn view<'a>(bar: &SearchBar, status: (usize, usize), t: Tokens) -> Element<'a, Message> {
    let (at, total) = status;
    let regex = bar.regex;
    let counter = if bar.query.is_empty() {
        String::new()
    } else if total == 0 {
        "no match".to_owned()
    } else {
        format!("{at}/{total}")
    };
    let shown: Element<'a, Message> = if bar.query.is_empty() {
        text("Find in scrollback")
            .size(13)
            .color(t.text_dim)
            .wrapping(Wrapping::None)
            .into()
    } else {
        text(format!("{}\u{258F}", tail(&bar.query, 26)))
            .size(13)
            .color(t.text)
            .wrapping(Wrapping::None)
            .into()
    };
    let step = |glyph: &'static str, forward: bool| {
        button(text(glyph).size(13).center())
            .padding(Padding::from([2.0, 7.0]))
            .on_press(Message::SearchStep(forward))
            .style(move |_, status| flat(t, status))
    };
    container(
        row![
            text("\u{2315}").size(15).color(t.accent),
            container(shown).width(Length::Fill),
            text(counter).size(11).font(fonts::MONO_BOLD).color(
                if total == 0 && !bar.query.is_empty() {
                    t.danger
                } else {
                    t.text_dim
                }
            ),
            button(text(".*").size(12).font(fonts::MONO_BOLD).center())
                .padding(Padding::from([2.0, 6.0]))
                .on_press(Message::SearchRegex)
                .style(move |_, status| {
                    let mut s = flat(t, status);
                    if regex {
                        s.background = Some(Background::Color(t.accent));
                        s.text_color = t.bg;
                    }
                    s
                }),
            step("\u{2191}", false),
            step("\u{2193}", true),
            button(text("\u{00D7}").size(15).center())
                .padding(Padding::from([1.0, 7.0]))
                .on_press(Message::SearchClose)
                .style(move |_, status| flat(t, status)),
            Space::new().width(Length::Fixed(0.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Length::Fixed(WIDTH))
    .padding(Padding::from([7.0, 12.0]))
    .style(move |_| container::Style {
        background: Some(Background::Color(t.surface)),
        border: Border {
            color: t.accent,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn flat(t: Tokens, status: button::Status) -> button::Style {
    let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hot.then_some(Background::Color(t.surface_hi)),
        text_color: t.text,
        border: Border::default().rounded(5.0),
        ..button::Style::default()
    }
}

/// The last `max` characters of `s` (the caret end stays visible in a long query).
fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_owned()
    } else {
        s.chars().skip(n - max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(n: Named) -> Key {
        Key::Named(n)
    }

    #[test]
    fn keys_map_to_search_actions() {
        let none = Modifiers::empty();
        assert_eq!(classify(&k(Named::Escape), none, None), SearchKey::Close);
        assert_eq!(classify(&k(Named::Enter), none, None), SearchKey::Next);
        assert_eq!(
            classify(&k(Named::Enter), Modifiers::SHIFT, None),
            SearchKey::Prev
        );
        assert_eq!(
            classify(&k(Named::F3), Modifiers::SHIFT, None),
            SearchKey::Prev
        );
        assert_eq!(
            classify(&k(Named::Backspace), none, None),
            SearchKey::Backspace
        );
        assert_eq!(
            classify(&Key::Character("a".into()), none, Some("a")),
            SearchKey::Insert("a".into())
        );
        assert_eq!(
            classify(&Key::Character("c".into()), Modifiers::CTRL, Some("c")),
            SearchKey::Other
        );
        assert_eq!(classify(&k(Named::ArrowUp), none, None), SearchKey::Other);
        assert_eq!(
            classify(&Key::Character("r".into()), Modifiers::ALT, Some("r")),
            SearchKey::ToggleRegex
        );
    }

    #[test]
    fn long_queries_show_their_end() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
    }
}
