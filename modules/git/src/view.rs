//! The Git panel overlay in theme tokens.

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{Background, Border, Element, Length, Padding};
use mahzenterm_ui_kit::{fonts, Tokens};

use crate::panel::{Confirm, Mode, Panel};

/// Rows of the file list shown at once.
const VISIBLE: usize = 12;

/// The overlay: backdrop (click closes, `on_close`) and the card near the top. `branch` is the current branch name.
pub fn view<'a, M: Clone + 'a>(
    panel: &'a Panel,
    branch: &str,
    t: Tokens,
    on_close: M,
) -> Element<'a, M> {
    let header = row![
        text("GIT")
            .size(10)
            .font(fonts::MONO_BOLD)
            .color(t.text_dim),
        text(branch.to_owned()).size(12).color(t.accent),
        Space::new().width(Length::Fill),
        text(format!(
            "{} changed, {} staged",
            panel.entries.len(),
            panel.staged_count()
        ))
        .size(11)
        .color(t.text_dim),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);

    let body: Element<'a, M> = match &panel.mode {
        Mode::Branches { list, sel } => {
            let mut col = column![].spacing(1);
            for (i, b) in list.iter().enumerate() {
                let selected = i == *sel;
                let label = format!("{} {}", if b.current { "\u{25cf}" } else { " " }, b.name);
                col = col.push(line(
                    label,
                    String::new(),
                    selected,
                    if b.current { t.accent } else { t.text },
                    t,
                ));
            }
            col.into()
        }
        _ => {
            let mut col = column![].spacing(1);
            let first = panel.selected.saturating_sub(VISIBLE - 1);
            for (i, e) in panel.entries.iter().enumerate().skip(first).take(VISIBLE) {
                let code = format!("{}{}", e.staged, e.worktree).replace(' ', "\u{b7}");
                let colour = if e.is_conflict() {
                    t.danger
                } else if e.is_staged() {
                    t.ok
                } else if e.is_untracked() {
                    t.text_dim
                } else {
                    t.warn
                };
                col = col.push(line(e.path.clone(), code, i == panel.selected, colour, t));
            }
            if panel.entries.is_empty() {
                let msg = if panel.busy {
                    "reading..."
                } else {
                    "working tree clean"
                };
                col = col.push(
                    container(text(msg).size(13).color(t.text_dim))
                        .padding(Padding::from([10.0, 14.0])),
                );
            }
            col.into()
        }
    };

    let prompt: Element<'a, M> = match &panel.mode {
        Mode::Commit(msg) => container(
            row![
                text("commit \u{203a}").size(13).color(t.accent),
                text(format!("{msg}\u{258f}")).size(13).color(t.text)
            ]
            .spacing(8),
        )
        .padding(Padding::from([4.0, 8.0]))
        .into(),
        Mode::Confirm(c) => {
            let q = match c {
                Confirm::Discard(e) => {
                    format!("discard all changes to {}? press y to confirm", e.path)
                }
                Confirm::Push => "push to the remote? press y to confirm".to_owned(),
            };
            container(text(q).size(13).color(t.warn))
                .padding(Padding::from([4.0, 8.0]))
                .into()
        }
        _ => match &panel.notice {
            Some((m, err)) => {
                container(
                    text(m.clone())
                        .size(12)
                        .color(if *err { t.danger } else { t.ok }),
                )
                .padding(Padding::from([4.0, 8.0]))
                .into()
            }
            None => Space::new().height(Length::Fixed(0.0)).into(),
        },
    };

    let hint = match panel.mode {
        Mode::List => "space stage  a all  u unstage all  c commit  b branches  d diff  g log  x discard  p push  l pull  r refresh  esc close",
        Mode::Commit(_) => "enter commit   esc back",
        Mode::Branches { .. } => "\u{2191}\u{2193} choose   enter switch   esc back",
        Mode::Confirm(_) => "y yes   any other key cancels",
    };
    let card = container(
        column![
            container(header).padding(Padding::from([2.0, 8.0])),
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(move |_| container::background(t.border)),
            body,
            prompt,
            container(text(hint).size(10).color(t.text_dim)).padding(Padding::from([4.0, 8.0])),
        ]
        .spacing(8),
    )
    .width(Length::Fixed(760.0))
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
            .height(Length::Fill),
    )
    .on_press(on_close);
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

fn line<'a, M: 'a>(
    label: String,
    code: String,
    selected: bool,
    colour: iced::Color,
    t: Tokens,
) -> Element<'a, M> {
    container(
        row![
            text(code)
                .size(12)
                .font(fonts::MONO)
                .color(colour)
                .width(Length::Fixed(28.0)),
            text(label)
                .size(13)
                .color(if selected { t.text } else { colour }),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([5.0, 12.0]))
    .style(move |_| container::Style {
        background: selected.then_some(Background::Color(t.surface_hi)),
        border: Border::default().rounded(6.0),
        ..container::Style::default()
    })
    .into()
}
