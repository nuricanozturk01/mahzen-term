//! The Docker panel overlay in theme tokens.

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{Background, Border, Element, Length, Padding};
use mahzenterm_ui_kit::{fonts, Tokens};

use crate::panel::{Confirm, Panel};

const VISIBLE: usize = 12;

/// The overlay: a backdrop (click closes, `on_close`) and the card near the top.
pub fn view<'a, M: Clone + 'a>(panel: &'a Panel, t: Tokens, on_close: M) -> Element<'a, M> {
    let running = panel.containers.iter().filter(|c| c.running()).count();
    let header = row![
        text("DOCKER")
            .size(10)
            .font(fonts::MONO_BOLD)
            .color(t.text_dim),
        Space::new().width(Length::Fill),
        text(format!("{running}/{} running", panel.containers.len()))
            .size(11)
            .color(t.text_dim),
    ]
    .align_y(iced::Alignment::Center);
    let mut list = column![].spacing(1);
    let first = panel.selected.saturating_sub(VISIBLE - 1);
    for (i, c) in panel
        .containers
        .iter()
        .enumerate()
        .skip(first)
        .take(VISIBLE)
    {
        let selected = i == panel.selected;
        let colour = if c.status.contains("unhealthy") {
            t.danger
        } else if c.running() {
            t.ok
        } else {
            t.text_dim
        };
        list = list.push(
            container(
                row![
                    text("\u{25cf}").size(10).color(colour),
                    text(c.name.clone())
                        .size(13)
                        .color(if selected { t.text } else { colour })
                        .width(Length::FillPortion(3)),
                    text(c.image.clone())
                        .size(12)
                        .color(t.text_dim)
                        .width(Length::FillPortion(3)),
                    text(c.status.clone())
                        .size(12)
                        .color(t.text_dim)
                        .width(Length::FillPortion(3)),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Padding::from([5.0, 12.0]))
            .style(move |_| container::Style {
                background: selected.then_some(Background::Color(t.surface_hi)),
                border: Border::default().rounded(6.0),
                ..container::Style::default()
            }),
        );
    }
    if panel.containers.is_empty() {
        let msg = if panel.busy {
            "reading..."
        } else {
            "no containers"
        };
        list = list.push(
            container(text(msg).size(13).color(t.text_dim)).padding(Padding::from([10.0, 14.0])),
        );
    }
    let prompt: Element<'a, M> = match (&panel.confirm, &panel.notice) {
        (Some(Confirm::Remove(c)), _) => container(
            text(format!(
                "remove {} (stops it first)? press y to confirm",
                c.name
            ))
            .size(13)
            .color(t.warn),
        )
        .padding(Padding::from([4.0, 8.0]))
        .into(),
        (None, Some((m, err))) => {
            container(
                text(m.clone())
                    .size(12)
                    .color(if *err { t.danger } else { t.ok }),
            )
            .padding(Padding::from([4.0, 8.0]))
            .into()
        }
        _ => Space::new().height(Length::Fixed(0.0)).into(),
    };
    let card = container(
        column![
            container(header).padding(Padding::from([2.0, 8.0])),
            container(Space::new()).width(Length::Fill).height(Length::Fixed(1.0)).style(move |_| container::background(t.border)),
            list,
            prompt,
            container(text("s start  x stop  t restart  l/enter logs  e shell  d remove  r refresh  esc close").size(10).color(t.text_dim))
                .padding(Padding::from([4.0, 8.0])),
        ]
        .spacing(8),
    )
    .width(Length::Fixed(760.0))
    .padding(10)
    .style(move |_| container::Style {
        background: Some(Background::Color(t.surface)),
        border: Border { color: t.border, width: 1.0, radius: 12.0.into() },
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
