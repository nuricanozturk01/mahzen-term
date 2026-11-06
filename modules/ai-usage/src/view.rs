//! The usage card: a draggable header and one block per tool, in theme tokens.

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{mouse, Alignment, Element, Length, Padding};
use mahzenterm_ui_kit::{fonts, Tokens};

use crate::{Snapshot, Tool};

/// Card width in pixels.
pub const CARD_W: f32 = 236.0;
/// Height of the drag handle at the top.
pub const HEADER_H: f32 = 26.0;
const BLOCK_H: f32 = 58.0;

/// Total card height (header + two tool blocks + padding).
pub fn card_height() -> f32 {
    HEADER_H + BLOCK_H * 2.0 + 10.0
}

/// `1234` -> `1.2k`, `2_500_000` -> `2.5M`.
pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

fn block<'a, M: 'a>(tool: Tool, snap: &Snapshot, t: Tokens) -> Element<'a, M> {
    let s = snap.get(tool);
    let live = s.per_min > 0.0;
    let dot = text("\u{25cf}")
        .size(9)
        .color(if live { t.ok } else { t.text_dim });
    let rate = if live {
        format!("{}/min", fmt_tokens(s.per_min.round() as u64))
    } else {
        "idle".to_owned()
    };
    let big = text(fmt_tokens(s.today.billable()))
        .size(18)
        .font(fonts::MONO_BOLD)
        .color(t.text);
    container(
        column![
            row![
                dot,
                text(tool.name()).size(11).color(t.accent),
                Space::new().width(Length::Fill),
                text(rate).size(11).color(t.text_dim)
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                big,
                text("today").size(11).color(t.text_dim),
                Space::new().width(Length::Fill),
                text(format!("1h {}", fmt_tokens(s.last_hour.billable())))
                    .size(11)
                    .color(t.text)
            ]
            .spacing(6)
            .align_y(Alignment::End),
            text(format!(
                "in {}  out {}  cache {}",
                fmt_tokens(s.today.input),
                fmt_tokens(s.today.output),
                fmt_tokens(s.today.cache_read)
            ))
            .size(10)
            .color(t.text_dim),
        ]
        .spacing(2),
    )
    .height(BLOCK_H)
    .padding(Padding::from([4.0, 10.0]))
    .width(Length::Fill)
    .into()
}

/// The card. The header reports the pointer (`on_move`, header-relative) and presses (`on_press`), which the
/// app turns into a drag.
pub fn view<'a, M: Clone + 'a>(
    snap: &Snapshot,
    t: Tokens,
    on_press: M,
    on_move: impl Fn(iced::Point) -> M + 'a,
) -> Element<'a, M> {
    let header = mouse_area(
        container(
            row![
                text("TOKENS")
                    .size(10)
                    .font(fonts::MONO_BOLD)
                    .color(t.text_dim),
                Space::new().width(Length::Fill),
                text("\u{2261}").size(12).color(t.text_dim),
            ]
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([0.0, 10.0]))
        .width(Length::Fill)
        .height(HEADER_H)
        .align_y(Alignment::Center)
        .style(move |_| container::background(t.surface_hi)),
    )
    .on_press(on_press)
    .on_move(on_move)
    .interaction(mouse::Interaction::Grab);
    container(column![
        header,
        block(Tool::Claude, snap, t),
        block(Tool::Codex, snap, t)
    ])
    .width(CARD_W)
    .style(move |_| container::Style {
        background: Some(iced::Background::Color(t.surface)),
        border: iced::Border {
            color: t.border,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .clip(true)
    .into()
}

#[cfg(test)]
mod tests {
    use super::fmt_tokens;

    #[test]
    fn tokens_are_compact() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1_234), "1.2k");
        assert_eq!(fmt_tokens(2_500_000), "2.5M");
        assert_eq!(fmt_tokens(3_000_000_000), "3.0B");
    }
}
