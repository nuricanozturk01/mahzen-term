//! The sessions card: a drag handle, one row per session, and a resize grip.

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{mouse, Alignment, Background, Border, Element, Length, Padding, Point};
use mahzenterm_ui_kit::{fonts, Tokens};

use crate::{summarize, Session, Status};

/// Card width at scale 1.
pub const CARD_W: f32 = 300.0;
const HEADER_H: f32 = 26.0;
const ROW_H: f32 = 38.0;
const FOOTER_H: f32 = 14.0;
/// Most session rows shown; the rest are summarised in the header.
pub const MAX_ROWS: usize = 6;
pub const MIN_SCALE: f32 = 0.6;
pub const MAX_SCALE: f32 = 2.5;

pub fn clamp_scale(s: f32) -> f32 {
    if s.is_nan() {
        1.0
    } else {
        s.clamp(MIN_SCALE, MAX_SCALE)
    }
}

pub fn card_width(scale: f32) -> f32 {
    CARD_W * scale
}

/// Height for `sessions` sessions (an empty list still shows one hint row).
pub fn card_height(sessions: usize, scale: f32) -> f32 {
    (HEADER_H + ROW_H * sessions.clamp(1, MAX_ROWS) as f32 + FOOTER_H) * scale
}

/// `1700000000000` -> `3m` style age.
fn age(now_ms: u64, then_ms: u64) -> String {
    if then_ms == 0 || then_ms > now_ms {
        return String::new();
    }
    let s = (now_ms - then_ms) / 1000;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// Last path components that fit, with a leading `…`.
fn short_cwd(cwd: &str, max: usize) -> String {
    let chars: Vec<char> = cwd.chars().collect();
    if chars.len() <= max {
        return cwd.to_owned();
    }
    let tail: String = chars[chars.len() - (max - 1)..].iter().collect();
    format!("\u{2026}{tail}")
}

fn status_color(s: Status, t: Tokens) -> iced::Color {
    match s {
        Status::Busy => t.accent,
        Status::Waiting => t.warn,
        Status::Idle => t.ok,
        Status::Shell | Status::Unknown => t.text_dim,
    }
}

fn session_row<'a, M: Clone + 'a>(
    s: &Session,
    index: usize,
    now_ms: u64,
    t: Tokens,
    k: f32,
    on_row: &(impl Fn(usize) -> M + 'a),
) -> Element<'a, M> {
    let color = status_color(s.status, t);
    let name = if s.name.is_empty() {
        format!("pid {}", s.pid)
    } else {
        s.name.clone()
    };
    let label = match (&s.waiting_for, s.status) {
        (Some(w), Status::Waiting) => format!("needs you: {w}"),
        _ => s.status.label().to_owned(),
    };
    let max_cwd = (((CARD_W - 28.0) * k) / (6.2 * k)).floor().max(8.0) as usize;
    let body = container(
        column![
            row![
                text("\u{25cf}").size(9.0 * k).color(color),
                text(name)
                    .size(12.0 * k)
                    .font(fonts::MONO_BOLD)
                    .color(t.text),
                Space::new().width(Length::Fill),
                text(label).size(11.0 * k).color(color),
                text(age(now_ms, s.updated_ms))
                    .size(10.0 * k)
                    .color(t.text_dim),
            ]
            .spacing(6.0 * k)
            .align_y(Alignment::Center),
            text(short_cwd(&s.cwd, max_cwd))
                .size(10.0 * k)
                .color(t.text_dim),
        ]
        .spacing(2.0 * k),
    )
    .height(ROW_H * k)
    .width(Length::Fill)
    .padding(Padding::from([4.0 * k, 10.0 * k]))
    .clip(true);
    mouse_area(body)
        .on_press(on_row(index))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

/// Small `tab` and `win` buttons that move the card into a tab or a window of its own.
fn place_buttons<'a, M: Clone + 'a>(place: Option<(M, M)>, t: Tokens, k: f32) -> Element<'a, M> {
    let Some((tab, win)) = place else {
        return Space::new().into();
    };
    let btn = |label: &'static str, msg: M| {
        mouse_area(
            text(label)
                .size(9.0 * k)
                .font(fonts::MONO_BOLD)
                .color(t.accent),
        )
        .on_press(msg)
        .interaction(mouse::Interaction::Pointer)
    };
    row![btn("TAB", tab), btn("WIN", win)]
        .spacing(8.0 * k)
        .into()
}

/// The card. Header: drag (`on_press`/`on_move`); rows: `on_row(index)`; grip: `on_resize`.
#[allow(clippy::too_many_arguments)]
pub fn view<'a, M: Clone + 'a>(
    sessions: &'a [Session],
    now_ms: u64,
    t: Tokens,
    scale: f32,
    on_press: M,
    on_move: impl Fn(Point) -> M + 'a,
    on_row: impl Fn(usize) -> M + 'a,
    on_resize: M,
    place: Option<(M, M)>,
) -> Element<'a, M> {
    let k = clamp_scale(scale);
    let sum = summarize(sessions);
    // Only the parts that are not zero, so the header stays short enough for the placement buttons.
    let summary = if sessions.is_empty() {
        String::new()
    } else {
        let mut parts = Vec::new();
        if sum.busy > 0 {
            parts.push(format!("{} busy", sum.busy));
        }
        if sum.waiting > 0 {
            parts.push(format!("{} need you", sum.waiting));
        }
        if sum.idle > 0 {
            parts.push(format!("{} idle", sum.idle));
        }
        parts.join(" \u{b7} ")
    };
    let header = mouse_area(
        container(
            row![
                text("AI SESSIONS")
                    .size(10.0 * k)
                    .font(fonts::MONO_BOLD)
                    .color(t.text_dim),
                Space::new().width(Length::Fill),
                text(summary).size(10.0 * k).color(if sum.waiting > 0 {
                    t.warn
                } else {
                    t.text_dim
                }),
                place_buttons(place, t, k),
            ]
            .spacing(8.0 * k)
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
    let mut col = column![header];
    if sessions.is_empty() {
        col = col.push(
            container(
                text("No Claude Code sessions running")
                    .size(11.0 * k)
                    .color(t.text_dim),
            )
            .height(ROW_H * k)
            .padding(Padding::from([10.0 * k, 10.0 * k])),
        );
    }
    for (i, s) in sessions.iter().take(MAX_ROWS).enumerate() {
        col = col.push(session_row(s, i, now_ms, t, k, &on_row));
    }
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
    container(col.push(grip))
        .width(card_width(k))
        .style(move |_| container::Style {
            background: Some(Background::Color(t.surface)),
            border: Border {
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
    use super::*;

    #[test]
    fn ages_are_compact() {
        assert_eq!(age(10_000, 4_000), "6s");
        assert_eq!(age(1_000_000, 1_000_000 - 180_000), "3m");
        assert_eq!(age(10_000_000, 10_000_000 - 7_300_000), "2h");
        assert_eq!(age(5, 10), "");
        assert_eq!(age(5, 0), "");
    }

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(short_cwd("/a/b", 20), "/a/b");
        let s = short_cwd("/home/user/projects/very/deep/app", 12);
        assert_eq!(s.chars().count(), 12);
        assert!(s.starts_with('\u{2026}') && s.ends_with("deep/app"));
    }

    #[test]
    fn height_follows_the_row_count_within_limits() {
        assert_eq!(card_height(0, 1.0), card_height(1, 1.0));
        assert!(card_height(3, 1.0) > card_height(1, 1.0));
        assert_eq!(card_height(50, 1.0), card_height(MAX_ROWS, 1.0));
        assert!((card_height(2, 2.0) - 2.0 * card_height(2, 1.0)).abs() < 0.01);
    }
}
