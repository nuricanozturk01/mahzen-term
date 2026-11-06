//! Drawing the store: a search line, a list with status badges, and the install confirmation.

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use mahzenterm_ui_kit::Tokens;

use crate::store::{Entry, Mode, Store, VISIBLE_ROWS};

fn badge<'a, M: 'a>(label: &'static str, color: Color, t: Tokens) -> Element<'a, M> {
    container(text(label).size(10).color(t.bg))
        .padding(Padding::from([1.0, 6.0]))
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border::default().rounded(4.0),
            ..container::Style::default()
        })
        .into()
}

fn status<'a, M: 'a>(e: &Entry, t: Tokens) -> Element<'a, M> {
    if e.installed {
        badge("OPEN", t.ok, t)
    } else if e.plan.is_some() {
        badge("INSTALL", t.accent, t)
    } else {
        badge("N/A", t.text_dim, t)
    }
}

fn confirm<'a, M: Clone + 'a>(store: &Store, t: Tokens) -> Element<'a, M> {
    let Some(e) = store.confirming() else {
        return Space::new().into();
    };
    let cmd = e.plan.as_ref().map(|p| p.display()).unwrap_or_default();
    let how = e.plan.as_ref().map_or("", |p| p.manager.label());
    let mut col = column![
        text(format!("Install {}?", e.app.name))
            .size(15)
            .color(t.text),
        text(format!("via {how}")).size(11).color(t.text_dim),
        container(text(cmd).size(13).color(t.accent))
            .padding(Padding::from([8.0, 10.0]))
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(t.bg)),
                border: Border {
                    color: t.border,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..container::Style::default()
            }),
    ]
    .spacing(8);
    if e.plan.as_ref().is_some_and(|p| p.uses_sudo()) {
        col = col.push(
            text("This asks for your password in the new pane.")
                .size(11)
                .color(t.warn),
        );
    }
    col = col.push(
        text("The command runs in a new pane so you can watch it.   \u{23ce} run   esc back")
            .size(11)
            .color(t.text_dim),
    );
    col.into()
}

/// The overlay card. `on_row` reports a click on a row, `on_close` a click outside the card.
pub fn view<'a, M: Clone + 'a>(
    store: &'a Store,
    t: Tokens,
    on_row: impl Fn(usize) -> M + 'a,
    on_close: M,
) -> Element<'a, M> {
    let query = if store.query.is_empty() {
        row![
            text("\u{203a}").size(16).color(t.accent),
            text("Search terminal apps\u{2026}")
                .size(15)
                .color(t.text_dim)
        ]
    } else {
        row![
            text("\u{203a}").size(16).color(t.accent),
            text(format!("{}\u{258f}", store.query))
                .size(15)
                .color(t.text)
        ]
    }
    .spacing(10)
    .align_y(Alignment::Center);

    let body: Element<'a, M> = if matches!(store.mode, Mode::Confirm(_)) {
        container(confirm(store, t))
            .padding(Padding::from([8.0, 6.0]))
            .into()
    } else {
        let mut list = column![].spacing(1);
        for (i, ei) in store
            .results
            .iter()
            .enumerate()
            .skip(store.first_visible())
            .take(VISIBLE_ROWS)
        {
            let e = &store.entries[*ei];
            let selected = i == store.selected;
            let line = row![
                column![
                    text(e.app.name.clone()).size(14).color(if selected {
                        t.text
                    } else {
                        t.text_dim
                    }),
                    text(e.app.summary.clone()).size(11).color(t.text_dim),
                ]
                .spacing(1),
                Space::new().width(Length::Fill),
                text(e.app.categories.join(", ")).size(10).color(t.text_dim),
                status(e, t),
            ]
            .spacing(10)
            .align_y(Alignment::Center);
            let rowbox = container(line)
                .width(Length::Fill)
                .padding(Padding::from([6.0, 12.0]))
                .style(move |_| container::Style {
                    background: selected.then_some(Background::Color(t.surface_hi)),
                    border: Border::default().rounded(6.0),
                    ..container::Style::default()
                });
            list = list.push(mouse_area(rowbox).on_press(on_row(i)));
        }
        if store.results.is_empty() {
            list = list.push(
                container(text("No app matches").size(13).color(t.text_dim))
                    .padding(Padding::from([10.0, 14.0])),
            );
        }
        if let Some(n) = &store.notice {
            list = list.push(container(text(n.clone()).size(12).color(t.warn)).padding(8));
        }
        list.into()
    };

    let footer = row![
        text("\u{2191}\u{2193} navigate   \u{23ce} install / open   esc close")
            .size(11)
            .color(t.text_dim),
        Space::new().width(Length::Fill),
        text(if store.problems.is_empty() {
            format!("{} apps", store.results.len())
        } else {
            format!(
                "{} apps, {} skipped",
                store.results.len(),
                store.problems.len()
            )
        })
        .size(11)
        .color(t.text_dim),
    ];
    let card = container(
        column![
            container(row![
                text("TUI STORE").size(11).color(t.accent),
                Space::new().width(Length::Fill),
                text("commands are shown before they run")
                    .size(10)
                    .color(t.text_dim),
            ])
            .padding(Padding::from([2.0, 8.0])),
            container(query).padding(Padding::from([4.0, 8.0])),
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(move |_| container::background(t.border)),
            body,
            container(footer).padding(Padding::from([4.0, 8.0])),
        ]
        .spacing(8),
    )
    .width(Length::Fixed(680.0))
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
            .align_x(Alignment::Center)
            .padding(Padding::from([60.0, 0.0]))
    ]
    .into()
}
