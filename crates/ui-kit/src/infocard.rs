//! A generic floating info card: a draggable header, rows of `label  value`, a resize grip. Widgets that only show a
//! few live facts (Docker, Kubernetes, Git, ...) describe their data as an [`InfoCard`] and get the same chrome,
//! drag, resize and tab/window buttons as the built-in cards, in theme tokens only.

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{mouse, Alignment, Color, Element, Length, Padding};

use crate::{fonts, Tokens};

/// Card width in pixels at scale 1.
pub const CARD_W: f32 = 252.0;
/// Height of the drag handle at scale 1.
pub const HEADER_H: f32 = 26.0;
/// Height of one row at scale 1.
pub const ROW_H: f32 = 19.0;
const FOOTER_H: f32 = 14.0;
const PAD_Y: f32 = 6.0;
/// Most rows a card shows (the rest is summarised in the last row by the data source).
pub const MAX_ROWS: usize = 12;
/// Smallest and largest card scale.
pub const MIN_SCALE: f32 = 0.6;
pub const MAX_SCALE: f32 = 2.5;

/// How a value is coloured (always from theme tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Normal,
    Dim,
    Good,
    Warn,
    Bad,
    Accent,
}

/// One line of a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub value: String,
    pub tone: Tone,
}

impl Row {
    pub fn new(label: impl Into<String>, value: impl Into<String>, tone: Tone) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone,
        }
    }
}

/// Everything a card shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InfoCard {
    pub title: String,
    /// Right side of the header (a count, a context name).
    pub badge: String,
    pub rows: Vec<Row>,
    /// Shown instead of rows when there are none (also the place for "docker not found" and similar).
    pub empty: String,
}

impl InfoCard {
    /// Rows actually drawn (at least one line, at most [`MAX_ROWS`]).
    pub fn shown(&self) -> usize {
        self.rows.len().clamp(1, MAX_ROWS)
    }
}

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
    CARD_W * clamp_scale(scale)
}

/// Card height for `rows` rows at `scale`.
pub fn card_height(rows: usize, scale: f32) -> f32 {
    let rows = rows.clamp(1, MAX_ROWS) as f32;
    (HEADER_H + 2.0 * PAD_Y + ROW_H * rows + FOOTER_H) * clamp_scale(scale)
}

fn tone_color(t: Tokens, tone: Tone) -> Color {
    match tone {
        Tone::Normal => t.text,
        Tone::Dim => t.text_dim,
        Tone::Good => t.ok,
        Tone::Warn => t.warn,
        Tone::Bad => t.danger,
        Tone::Accent => t.accent,
    }
}

fn place_buttons<'a, M: Clone + 'a>(place: Option<(M, M)>, t: Tokens, k: f32) -> Element<'a, M> {
    let Some((tab, win)) = place else {
        return text("\u{2261}").size(12.0 * k).color(t.text_dim).into();
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

/// Cut `s` to `max` characters with an ellipsis.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}

/// The card. The header reports the pointer (`on_move`, header-relative) and presses (`on_press`) for dragging;
/// the grip reports `on_resize`.
pub fn view<'a, M: Clone + 'a>(
    card: &InfoCard,
    t: Tokens,
    scale: f32,
    on_press: M,
    on_move: impl Fn(iced::Point) -> M + 'a,
    on_resize: M,
    place: Option<(M, M)>,
) -> Element<'a, M> {
    let k = clamp_scale(scale);
    let header = mouse_area(
        container(
            row![
                text(card.title.to_uppercase())
                    .size(10.0 * k)
                    .font(fonts::MONO_BOLD)
                    .color(t.text_dim),
                text(clip(&card.badge, 18)).size(10.0 * k).color(t.accent),
                Space::new().width(Length::Fill),
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

    // About how many characters fit on one line next to the label.
    let chars = ((CARD_W - 20.0) / (fonts::ADVANCE_EM * 11.0)) as usize;
    let mut body = column![].spacing(0.0);
    if card.rows.is_empty() {
        body = body.push(
            container(
                text(clip(&card.empty, chars))
                    .size(11.0 * k)
                    .color(t.text_dim),
            )
            .height(ROW_H * k)
            .align_y(Alignment::Center),
        );
    }
    for r in card.rows.iter().take(MAX_ROWS) {
        let value_chars = chars.saturating_sub(r.label.chars().count() + 2).max(4);
        body = body.push(
            container(
                row![
                    text(clip(&r.label, chars)).size(11.0 * k).color(t.text_dim),
                    Space::new().width(Length::Fill),
                    text(clip(&r.value, value_chars))
                        .size(11.0 * k)
                        .color(tone_color(t, r.tone)),
                ]
                .spacing(6.0 * k)
                .align_y(Alignment::Center),
            )
            .height(ROW_H * k)
            .align_y(Alignment::Center),
        );
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

    container(column![
        header,
        container(body).padding(Padding::from([PAD_Y * k, 10.0 * k])),
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
    use super::*;

    #[test]
    fn height_grows_with_rows_and_is_bounded() {
        assert!(card_height(5, 1.0) > card_height(1, 1.0));
        assert_eq!(card_height(0, 1.0), card_height(1, 1.0));
        assert_eq!(card_height(500, 1.0), card_height(MAX_ROWS, 1.0));
        assert_eq!(card_width(f32::NAN), CARD_W);
        assert!((card_width(10.0) - CARD_W * MAX_SCALE).abs() < f32::EPSILON);
    }

    #[test]
    fn long_text_is_clipped_with_an_ellipsis() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("a-very-long-name", 6), "a-ver\u{2026}");
        assert_eq!(clip("", 0), "");
    }

    #[test]
    fn shown_counts_at_least_one_line() {
        assert_eq!(InfoCard::default().shown(), 1);
    }
}
