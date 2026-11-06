//! Drawing the Kubernetes panel.

use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use mahzenterm_ui_kit::{fonts, Tokens};

use crate::panel::{Confirm, Item, Load, Mode, Panel, Tone, VISIBLE_ROWS};

fn tone_color(t: Tone, tk: Tokens) -> Color {
    match t {
        Tone::Good => tk.ok,
        Tone::Pending | Tone::Warn => tk.warn,
        Tone::Bad => tk.danger,
        Tone::Neutral => tk.text_dim,
    }
}

fn badge<'a, M: 'a>(label: String, color: Color, tk: Tokens) -> Element<'a, M> {
    if label.is_empty() {
        return Space::new().into();
    }
    container(text(label).size(10).color(tk.bg))
        .padding(Padding::from([1.0, 6.0]))
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border::default().rounded(4.0),
            ..container::Style::default()
        })
        .into()
}

fn item_row<'a, M: Clone + 'a>(it: &Item, selected: bool, tk: Tokens) -> Element<'a, M> {
    let name_color = if it.prod {
        tk.danger
    } else if selected {
        tk.text
    } else {
        tk.text_dim
    };
    let mut line = row![text(it.name.clone()).size(13).color(name_color)]
        .spacing(10)
        .align_y(Alignment::Center);
    if it.prod {
        line = line.push(badge("PROD".into(), tk.danger, tk));
    }
    line = line
        .push(Space::new().width(Length::Fill))
        .push(text(it.detail.clone()).size(10).color(tk.text_dim))
        .push(badge(it.badge.clone(), tone_color(it.tone, tk), tk));
    container(line)
        .width(Length::Fill)
        .padding(Padding::from([6.0, 12.0]))
        .style(move |_| container::Style {
            background: selected.then_some(Background::Color(tk.surface_hi)),
            border: Border::default().rounded(6.0),
            ..container::Style::default()
        })
        .into()
}

fn title(mode: Mode) -> &'static str {
    match mode {
        Mode::Pods => "PODS",
        Mode::Contexts => "CONTEXTS",
        Mode::Namespaces => "NAMESPACES",
        Mode::Deployments => "DEPLOYMENTS",
    }
}

/// The overlay card. `on_row` reports a row click, `on_close` a click outside.
pub fn view<'a, M: Clone + 'a>(
    p: &'a Panel,
    tk: Tokens,
    on_row: impl Fn(usize) -> M + 'a,
    on_close: M,
) -> Element<'a, M> {
    let scope = p.scope.as_ref().map_or_else(
        || "no context".to_owned(),
        |s| format!("{} / {}", s.context, s.namespace),
    );
    let head = row![
        text("KUBERNETES").size(11).color(tk.accent),
        text(title(p.mode))
            .size(11)
            .font(fonts::MONO_BOLD)
            .color(tk.text_dim),
        Space::new().width(Length::Fill),
        text(scope)
            .size(11)
            .color(if p.prod { tk.danger } else { tk.text_dim }),
        badge(
            if p.prod { "PROD".into() } else { String::new() },
            tk.danger,
            tk
        ),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let query = if p.query.is_empty() {
        row![
            text("\u{203a}").size(16).color(tk.accent),
            text("Filter\u{2026}").size(15).color(tk.text_dim)
        ]
    } else {
        row![
            text("\u{203a}").size(16).color(tk.accent),
            text(format!("{}\u{258f}", p.query)).size(15).color(tk.text)
        ]
    }
    .spacing(10)
    .align_y(Alignment::Center);

    let body: Element<'a, M> = if let Some(c) = &p.confirm {
        let what = match c {
            Confirm::UseContext(n) => format!("Switch to production context {n}?"),
            Confirm::Exec(pod) => format!("Open a shell in {pod} on a production cluster?"),
            Confirm::DeletePod(pod) => format!("Delete pod {pod}?"),
            Confirm::Scale { name, from, to } => {
                format!("Scale {name} from {from} to {to} replicas?")
            }
        };
        let destructive = matches!(c, Confirm::DeletePod(_) | Confirm::Scale { .. });
        container(
            column![
                text(what).size(15).color(tk.danger),
                text(match (destructive, p.prod) {
                    (true, true) => "This changes a PRODUCTION cluster.   \u{23ce} yes   esc no",
                    (true, false) => "This changes the cluster.   \u{23ce} yes   esc no",
                    _ => "This is a production context.   \u{23ce} yes   esc no",
                })
                .size(11)
                .color(tk.text_dim),
            ]
            .spacing(8),
        )
        .padding(Padding::from([10.0, 6.0]))
        .into()
    } else {
        match &p.load {
            Load::Loading => container(text("loading\u{2026}").size(13).color(tk.text_dim))
                .padding(Padding::from([10.0, 14.0]))
                .into(),
            Load::Failed(why) => container(
                column![
                    text("Could not load").size(13).color(tk.danger),
                    text(why.clone()).size(12).color(tk.text_dim),
                    text("ctrl+r tries again").size(11).color(tk.text_dim),
                ]
                .spacing(4),
            )
            .padding(Padding::from([10.0, 14.0]))
            .into(),
            Load::Ready => {
                let mut list = column![].spacing(1);
                for (i, ii) in p
                    .results
                    .iter()
                    .enumerate()
                    .skip(p.first_visible())
                    .take(VISIBLE_ROWS)
                {
                    list = list.push(
                        mouse_area(item_row(&p.items[*ii], i == p.selected, tk))
                            .on_press(on_row(i)),
                    );
                }
                if p.results.is_empty() {
                    list = list.push(
                        container(text("nothing here").size(13).color(tk.text_dim))
                            .padding(Padding::from([10.0, 14.0])),
                    );
                }
                list.into()
            }
        }
    };
    let hint = match p.mode {
        Mode::Pods => "\u{23ce} logs  ^x shell  ^d describe  del delete  ^e deployments  ^r refresh  ^n ns  ^k contexts  esc",
        Mode::Deployments => "^\u{2191} more replicas   ^\u{2193} fewer   ^p pods   ^n namespaces   ^r refresh   esc close",
        Mode::Contexts => "\u{23ce} switch   ^p pods   ^n namespaces   esc close",
        Mode::Namespaces => "\u{23ce} select   ^p pods   ^k contexts   esc close",
    };
    let card = container(
        column![
            container(head).padding(Padding::from([2.0, 8.0])),
            container(query).padding(Padding::from([4.0, 8.0])),
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(move |_| container::background(tk.border)),
            body,
            container(text(hint).size(11).color(tk.text_dim)).padding(Padding::from([4.0, 8.0])),
        ]
        .spacing(8),
    )
    .width(Length::Fixed(720.0))
    .padding(10)
    .style(move |_| container::Style {
        background: Some(Background::Color(tk.surface)),
        border: Border {
            color: if p.prod { tk.danger } else { tk.border },
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
