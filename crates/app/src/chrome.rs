//! Small shared chrome pieces: tags, status dots, hairlines and the spaced brand text.
//!
//! The look is deliberate HUD styling: tiny bold uppercase tags, 1 px hairlines, accent used sparingly.

use iced::widget::{container, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding};
use mahzenterm_ui_kit::{fonts, Tokens};

/// A small uppercase label. `lit` fills it with the accent colour (dark text on accent); otherwise it is outlined.
pub fn tag<'a, M: 'a>(label: impl Into<String>, t: Tokens, lit: bool) -> Element<'a, M> {
    container(
        text(label.into())
            .size(10)
            .font(fonts::MONO_BOLD)
            .color(if lit { t.bg } else { t.text_dim }),
    )
    .padding(Padding::from([1.0, 6.0]))
    .style(move |_| container::Style {
        background: lit.then_some(Background::Color(t.accent)),
        border: Border {
            color: if lit { t.accent } else { t.border },
            width: 1.0,
            radius: 3.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// A filled status dot.
pub fn dot<'a, M: 'a>(color: Color) -> Element<'a, M> {
    container(Space::new())
        .width(Length::Fixed(7.0))
        .height(Length::Fixed(7.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                radius: 3.5.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// A 1 px line across `width` (or a fixed pixel width).
pub fn hairline<'a, M: 'a>(width: Length, color: Color) -> Element<'a, M> {
    container(Space::new())
        .width(width)
        .height(Length::Fixed(1.0))
        .style(move |_| container::background(color))
        .into()
}

/// Letter-spaced text: hair spaces between characters (the UI toolkit has no tracking control).
pub fn spaced(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 4);
    for (i, c) in s.chars().enumerate() {
        if i > 0 {
            out.push('\u{200A}');
            out.push('\u{200A}');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaced_inserts_hair_spaces_between_characters_only() {
        assert_eq!(spaced("AB"), "A\u{200A}\u{200A}B");
        assert_eq!(spaced("A"), "A");
        assert_eq!(spaced(""), "");
        assert_eq!(spaced("ÇŞ").chars().filter(|c| *c == '\u{200A}').count(), 2);
    }
}
