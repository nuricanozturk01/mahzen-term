//! Drawing the explorer with theme tokens: a header, a window of rows, no translucent layers.

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{mouse, Alignment, Background, Border, Element, Length, Padding};
use mahzenterm_ui_kit::{fonts, Tokens};

use crate::{Explorer, Row};

/// What the user did in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Clicked a row: select it (folders also open or close).
    Click(usize),
    /// Double-clicked a row: open a file.
    Open(usize),
    /// Wheel over the list; positive scrolls down.
    Scroll(isize),
    /// Go to the parent folder.
    Up,
    ToggleHidden,
    /// Read the folders again (also marks from git).
    Refresh,
    Close,
}

/// Height of one row in pixels (the app sizes the window of rows from it).
pub const ROW_H: f32 = 24.0;
/// Pixels above the list (header and divider).
pub const HEADER_H: f32 = 40.0;

/// Width of the dock in pixels, divider included.
pub const PANEL_W: f32 = 264.0;
/// Approximate advance of one glyph at the row font size (JetBrains Mono at 13 px).
const GLYPH_W: f32 = 7.9;
/// Pixels a row spends on the selection bar, marker, gaps and padding.
const ROW_CHROME: f32 = 2.0 + 14.0 + 12.0 + 16.0 + 6.0;

/// Shorten `name` to `max` characters, cutting from the middle so the extension stays visible.
pub fn fit_name(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        return name.to_owned();
    }
    if max <= 1 {
        return "\u{2026}".to_owned();
    }
    let keep = max - 1;
    let tail = (keep / 3).min(12);
    let head = keep - tail;
    let mut out: String = chars[..head].iter().collect();
    out.push('\u{2026}');
    out.extend(&chars[chars.len() - tail..]);
    out
}

fn dot_color(name: &str, t: Tokens) -> iced::Color {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some(
            "rs" | "py" | "js" | "ts" | "go" | "c" | "h" | "cpp" | "java" | "kt" | "sh" | "lua"
            | "rb",
        ) => t.accent,
        Some("md" | "txt" | "rst") => t.text,
        Some("toml" | "yaml" | "yml" | "json" | "ini" | "conf" | "lock") => t.warn,
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "mp4" | "mp3") => t.ok,
        _ => t.text_dim,
    }
}

fn mark_color(m: crate::git::Mark, t: Tokens) -> iced::Color {
    use crate::git::Mark;
    match m {
        Mark::Untracked | Mark::Added | Mark::Renamed => t.ok,
        Mark::Modified => t.warn,
        Mark::Deleted | Mark::Conflict => t.danger,
    }
}

fn row_view<'a, M: Clone + 'a>(
    r: &Row,
    mark: Option<crate::git::Mark>,
    index: usize,
    selected: bool,
    focused: bool,
    t: Tokens,
    on: &(impl Fn(Event) -> M + 'a),
) -> Element<'a, M> {
    let hidden = r.name.starts_with('.');
    let color = if hidden { t.text_dim } else { t.text };
    let marker: Element<'a, M> = if r.is_dir {
        text(if r.expanded { "\u{25be}" } else { "\u{25b8}" })
            .size(12)
            .color(t.accent)
            .into()
    } else {
        text("\u{25cf}").size(8).color(dot_color(&r.name, t)).into()
    };
    let room = PANEL_W - ROW_CHROME - r.depth as f32 * 14.0;
    let max_chars = (room / GLYPH_W).floor().max(4.0) as usize;
    // Leave room for the badge letter.
    let max_chars = if mark.is_some() {
        max_chars.saturating_sub(2)
    } else {
        max_chars
    };
    let label = text(fit_name(&r.name, max_chars))
        .size(13)
        .wrapping(Wrapping::None)
        .color(match mark {
            Some(m) if !r.is_dir => mark_color(m, t),
            _ if r.is_dir => t.accent,
            _ => color,
        });
    let badge: Element<'a, M> = match mark {
        Some(m) => container(
            text(m.letter())
                .size(11)
                .font(fonts::MONO_BOLD)
                .color(mark_color(m, t)),
        )
        .width(Length::Fill)
        .align_x(Alignment::End)
        .into(),
        None => Space::new().into(),
    };
    let line = row![
        Space::new().width(Length::Fixed(r.depth as f32 * 14.0)),
        container(marker)
            .width(Length::Fixed(14.0))
            .align_x(Alignment::Center),
        label,
        badge
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let bar = container(Space::new())
        .width(Length::Fixed(2.0))
        .height(Length::Fill)
        .style(move |_| {
            container::background(if selected && focused {
                t.accent
            } else {
                iced::Color::TRANSPARENT
            })
        });
    let body = container(
        row![bar, container(line).padding(Padding::from([0.0, 8.0]))].align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(ROW_H))
    .clip(true)
    .align_y(Alignment::Center)
    .style(move |_| container::Style {
        background: selected.then_some(Background::Color(t.surface_hi)),
        ..container::Style::default()
    });
    mouse_area(body)
        .on_press(on(Event::Click(index)))
        .on_double_click(on(Event::Open(index)))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn small_button<'a, M: Clone + 'a>(
    label: &'static str,
    on: M,
    lit: bool,
    t: Tokens,
) -> Element<'a, M> {
    button(text(label).size(12).font(fonts::MONO_BOLD).center())
        .padding(Padding::from([2.0, 7.0]))
        .on_press(on)
        .style(move |_, status| {
            let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: (hot || lit).then_some(Background::Color(if lit {
                    t.accent
                } else {
                    t.surface_hi
                })),
                text_color: if lit { t.bg } else { t.text },
                border: Border::default().rounded(5.0),
                ..button::Style::default()
            }
        })
        .into()
}

/// The panel: header with the folder name and buttons, then `capacity` rows from the scroll offset.
pub fn view<'a, M: Clone + 'a>(
    ex: &'a Explorer,
    capacity: usize,
    focused: bool,
    t: Tokens,
    on: impl Fn(Event) -> M + 'a,
) -> Element<'a, M> {
    let folder = ex.root().file_name().map_or_else(
        || ex.root().display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let header = container(
        row![
            text(match ex.search_query() {
                Some(q) => format!("/{q}\u{258f}"),
                None => folder,
            })
            .size(12)
            .font(fonts::MONO_BOLD)
            .wrapping(Wrapping::None)
            .color(t.text),
            Space::new().width(Length::Fill),
            small_button("\u{2191}", on(Event::Up), false, t),
            small_button("\u{21bb}", on(Event::Refresh), false, t),
            small_button(".*", on(Event::ToggleHidden), ex.show_hidden(), t),
            small_button("\u{00d7}", on(Event::Close), false, t),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(HEADER_H - 1.0))
    .padding(Padding::from([0.0, 10.0]))
    .align_y(Alignment::Center);
    let divider = container(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(move |_| container::background(t.border));
    let mut list = column![];
    let prompt = ex.prompt_line();
    // The prompt takes one row of the list area.
    let capacity = if prompt.is_some() {
        capacity.saturating_sub(1).max(1)
    } else {
        capacity
    };
    if let Some(line) = prompt {
        let max = ((PANEL_W - 24.0) / GLYPH_W).floor() as usize;
        list = list.push(
            container(
                text(fit_name(&line, max))
                    .size(12)
                    .wrapping(Wrapping::None)
                    .color(t.accent),
            )
            .width(Length::Fill)
            .height(Length::Fixed(ROW_H))
            .padding(Padding::from([0.0, 12.0]))
            .align_y(Alignment::Center)
            .clip(true)
            .style(move |_| container::background(t.surface_hi)),
        );
    }
    let end = (ex.scroll() + capacity).min(ex.rows().len());
    for (i, r) in ex.rows().iter().enumerate().take(end).skip(ex.scroll()) {
        list = list.push(row_view(
            r,
            ex.mark(&r.path),
            i,
            i == ex.selected(),
            focused,
            t,
            &on,
        ));
    }
    if let Some(err) = ex.error() {
        list = list.push(
            container(text(err.to_owned()).size(11).color(t.danger))
                .padding(Padding::from([6.0, 10.0])),
        );
    } else if ex.rows().is_empty() {
        let msg = if ex.index_pending() {
            "indexing\u{2026}"
        } else if ex.searching() {
            "no matching file"
        } else {
            "empty folder"
        };
        list = list.push(
            container(text(msg).size(12).color(t.text_dim)).padding(Padding::from([6.0, 10.0])),
        );
    }
    let on_wheel = move |d: mouse::ScrollDelta| {
        let y = match d {
            mouse::ScrollDelta::Lines { y, .. } => y * 3.0,
            mouse::ScrollDelta::Pixels { y, .. } => y / ROW_H,
        };
        on(Event::Scroll(-(y.round() as isize)))
    };
    container(column![
        header,
        divider,
        mouse_area(list.width(Length::Fill)).on_scroll(on_wheel)
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container::background(t.surface))
    .into()
}

#[cfg(test)]
mod tests {
    use super::fit_name;

    #[test]
    fn long_names_keep_their_extension() {
        assert_eq!(fit_name("short.rs", 20), "short.rs");
        let f = fit_name("nuricanozturk01~gitnode~S5AX23.dockerbuild.zip", 26);
        assert_eq!(f.chars().count(), 26);
        assert!(f.contains('\u{2026}') && f.ends_with(".zip"), "{f}");
        assert_eq!(fit_name("abcdef", 1), "\u{2026}");
    }
}
