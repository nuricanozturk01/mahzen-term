//! The usage card: a draggable header and one block per tool, in theme tokens.

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{mouse, Alignment, Element, Length, Padding};
use mahzenterm_ui_kit::{fonts, Tokens};

use crate::{Snapshot, Tool};

/// Card width in pixels at scale 1.
pub const CARD_W: f32 = 236.0;
/// Height of the drag handle at the top at scale 1.
pub const HEADER_H: f32 = 26.0;
const BLOCK_H: f32 = 58.0;
const FOOTER_H: f32 = 14.0;
/// Smallest and largest card scale.
pub const MIN_SCALE: f32 = 0.6;
pub const MAX_SCALE: f32 = 2.5;

/// Keep a scale inside the allowed range (NaN becomes 1).
pub fn clamp_scale(s: f32) -> f32 {
    if s.is_nan() {
        1.0
    } else {
        s.clamp(MIN_SCALE, MAX_SCALE)
    }
}

/// Card width in pixels at `scale`.
pub fn card_width(scale: f32) -> f32 {
    CARD_W * scale
}

/// Total card height at `scale` (header + two tool blocks + resize grip).
pub fn card_height(scale: f32) -> f32 {
    (HEADER_H + BLOCK_H * 2.0 + FOOTER_H) * scale
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

fn block<'a, M: 'a>(tool: Tool, snap: &Snapshot, t: Tokens, k: f32) -> Element<'a, M> {
    let s = snap.get(tool);
    let live = s.per_min > 0.0;
    let dot = text("\u{25cf}")
        .size(9.0 * k)
        .color(if live { t.ok } else { t.text_dim });
    let rate = if live {
        format!("{}/min", fmt_tokens(s.per_min.round() as u64))
    } else {
        "idle".to_owned()
    };
    let big = text(fmt_tokens(s.today.billable()))
        .size(18.0 * k)
        .font(fonts::MONO_BOLD)
        .color(t.text);
    container(
        column![
            row![
                dot,
                text(tool.name()).size(11.0 * k).color(t.accent),
                Space::new().width(Length::Fill),
                text(rate).size(11.0 * k).color(t.text_dim)
            ]
            .spacing(6.0 * k)
            .align_y(Alignment::Center),
            row![
                big,
                text("today").size(11.0 * k).color(t.text_dim),
                Space::new().width(Length::Fill),
                text(format!("1h {}", fmt_tokens(s.last_hour.billable())))
                    .size(11.0 * k)
                    .color(t.text)
            ]
            .spacing(6.0 * k)
            .align_y(Alignment::End),
            text(format!(
                "in {}  out {}  cache {}",
                fmt_tokens(s.today.input),
                fmt_tokens(s.today.output),
                fmt_tokens(s.today.cache_read)
            ))
            .size(10.0 * k)
            .color(t.text_dim),
        ]
        .spacing(2.0 * k),
    )
    .height(BLOCK_H * k)
    .padding(Padding::from([4.0 * k, 10.0 * k]))
    .width(Length::Fill)
    .into()
}

/// The card. The header reports the pointer (`on_move`, header-relative) and presses (`on_press`), which the
/// app turns into a drag.
pub fn view<'a, M: Clone + 'a>(
    snap: &Snapshot,
    t: Tokens,
    scale: f32,
    on_press: M,
    on_move: impl Fn(iced::Point) -> M + 'a,
    on_resize: M,
) -> Element<'a, M> {
    let k = clamp_scale(scale);
    let header = mouse_area(
        container(
            row![
                text("TOKENS")
                    .size(10.0 * k)
                    .font(fonts::MONO_BOLD)
                    .color(t.text_dim),
                Space::new().width(Length::Fill),
                text("\u{2261}").size(12.0 * k).color(t.text_dim),
            ]
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([0.0, 10.0 * k]))
        .width(Length::Fill)
        .height(HEADER_H * k)
        .align_y(Alignment::Center)
        .style(move |_| container::background(t.surface_hi)),
    )
    .on_press(on_press)
    .on_move(on_move)
    .interaction(mouse::Interaction::Grab);
    let grip = mouse_area(
        container(text("\u{25e2}").size(11.0 * k).color(t.text_dim))
            .width(Length::Fill)
            .height(FOOTER_H * k)
            .padding(Padding::from([0.0, 4.0 * k]))
            .align_x(Alignment::End)
            .align_y(Alignment::Center),
    )
    .on_press(on_resize)
    .interaction(mouse::Interaction::ResizingDiagonallyDown);
    container(column![
        header,
        block(Tool::Claude, snap, t, k),
        block(Tool::Codex, snap, t, k),
        grip
    ])
    .width(card_width(k))
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
    use super::{card_height, card_width, clamp_scale, fmt_tokens, MAX_SCALE, MIN_SCALE};

    #[test]
    fn scale_is_clamped_and_sizes_follow_it() {
        assert_eq!(clamp_scale(0.1), MIN_SCALE);
        assert_eq!(clamp_scale(9.0), MAX_SCALE);
        assert_eq!(clamp_scale(f32::NAN), 1.0);
        assert!((card_width(2.0) - 2.0 * card_width(1.0)).abs() < 0.01);
        assert!((card_height(2.0) - 2.0 * card_height(1.0)).abs() < 0.01);
    }

    #[test]
    fn tokens_are_compact() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1_234), "1.2k");
        assert_eq!(fmt_tokens(2_500_000), "2.5M");
        assert_eq!(fmt_tokens(3_000_000_000), "3.0B");
    }
}
