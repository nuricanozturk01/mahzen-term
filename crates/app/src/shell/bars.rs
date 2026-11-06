//! The tab bar and status bar: both are rows of widgets chosen by the user's config.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use iced::widget::{button, column, container, mouse_area, pin, row, stack, text, Space};
use iced::{border, mouse, window, Alignment, Background, Border, Color, Element, Length, Padding};
use mahzenterm_core::bars::{StatusbarConfig, TabbarConfig, WidgetDef, WidgetKind, WidgetStyle};
use mahzenterm_core::config::Config;
use mahzenterm_ui_kit::fonts;
use mahzenterm_ui_kit::Tokens;

use super::{truncate, Message, PaneRuntime, Shell, STATUS_H, TAB_BAR_H};
use crate::chrome;

/// Height of a tab, sitting on the bottom edge of the bar.
const TAB_H: f32 = 33.0;
/// Text size in bars.
const BAR_TEXT: f32 = 12.0;
/// Thickness of the invisible window-resize border.
const EDGE: f32 = 4.0;
const CORNER: f32 = 12.0;
const NOWRAP: iced::widget::text::Wrapping = iced::widget::text::Wrapping::None;

/// Bar layout and widget definitions taken from the config.
pub(super) struct Bars {
    pub tabbar: TabbarConfig,
    pub statusbar: StatusbarConfig,
    pub widgets: BTreeMap<String, WidgetDef>,
    /// The tab bar doubles as the window's title bar.
    pub custom_titlebar: bool,
}

impl Bars {
    pub fn from_config(c: &Config) -> Self {
        Self {
            tabbar: c.tabbar.clone(),
            statusbar: c.statusbar.clone(),
            widgets: c.widgets.clone(),
            custom_titlebar: c.window.custom_titlebar,
        }
    }

    /// The definition behind `id`: a user definition, else the built-in of that name.
    fn def(&self, id: &str) -> Option<WidgetDef> {
        self.widgets
            .get(id)
            .cloned()
            .or_else(|| WidgetKind::builtin(id).map(WidgetDef::of))
    }
}

fn human_bytes(b: u64) -> String {
    const GIB: f64 = 1_073_741_824.0;
    const MIB: f64 = 1_048_576.0;
    let v = b as f64;
    if v >= GIB {
        format!("{:.1} GB", v / GIB)
    } else {
        format!("{:.0} MB", v / MIB)
    }
}

fn human_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Format a time with a user-supplied `strftime` pattern; a bad pattern shows a hint instead of failing.
fn clock_text(pattern: &str) -> String {
    let mut out = String::new();
    if write!(out, "{}", chrono::Local::now().format(pattern)).is_err() {
        return "bad clock format".into();
    }
    out
}

/// A small horizontal gauge, `pct` in 0..=100.
fn gauge<'a>(pct: f32, fill: Color, t: Tokens) -> Element<'a, Message> {
    const W: f32 = 40.0;
    let pct = pct.clamp(0.0, 100.0);
    let filled = if pct < 0.5 {
        0.0
    } else {
        (W * pct / 100.0).max(2.0)
    };
    container(
        container(Space::new())
            .width(Length::Fixed(filled))
            .height(Length::Fixed(4.0))
            .style(move |_| container::Style {
                background: Some(Background::Color(fill)),
                border: Border::default().rounded(2.0),
                ..container::Style::default()
            }),
    )
    .width(Length::Fixed(W))
    .height(Length::Fixed(4.0))
    .style(move |_| container::Style {
        background: Some(Background::Color(t.surface_hi)),
        border: Border::default().rounded(2.0),
        ..container::Style::default()
    })
    .into()
}

/// Text that scrolls sideways through a window of `cols` characters when it is longer than that.
fn marquee<'a>(
    txt: &str,
    cols: usize,
    color: Color,
    elapsed: f32,
    speed: f32,
) -> Element<'a, Message> {
    let adv = fonts::ADVANCE_EM * BAR_TEXT;
    let width = cols as f32 * adv;
    let n = txt.chars().count();
    if n <= cols {
        return container(
            text(txt.to_owned())
                .size(BAR_TEXT)
                .wrapping(NOWRAP)
                .color(color),
        )
        .width(Length::Fixed(width))
        .into();
    }
    const GAP: usize = 5;
    let cycle = (n + GAP) as f32 * adv;
    let offset = (elapsed * speed * adv) % cycle;
    let gap = " ".repeat(GAP);
    let doubled = format!("{txt}{gap}{txt}{gap}");
    container(pin(text(doubled).size(BAR_TEXT).wrapping(NOWRAP).color(color)).x(-offset))
        .width(Length::Fixed(width))
        .height(Length::Fixed(BAR_TEXT * 1.3))
        .clip(true)
        .into()
}

impl Shell {
    fn token(&self, name: &str) -> Color {
        let t = self.tokens;
        match name {
            "accent" => t.accent,
            "text_dim" => t.text_dim,
            "ok" => t.ok,
            "warn" => t.warn,
            "danger" => t.danger,
            _ => t.text,
        }
    }

    /// Prefix a value with the widget's icon and label.
    fn compose<'a>(
        &self,
        def: &WidgetDef,
        default_label: &str,
        value: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let t = self.tokens;
        let mut r = row![].spacing(7).align_y(Alignment::Center);
        if let Some(icon) = &def.icon {
            r = r.push(text(icon.clone()).size(BAR_TEXT + 1.0).color(t.accent));
        }
        let label = def
            .label
            .clone()
            .unwrap_or_else(|| default_label.to_owned());
        if !label.is_empty() {
            r = r.push(
                text(label)
                    .size(10)
                    .font(fonts::MONO_BOLD)
                    .color(t.text_dim),
            );
        }
        r.push(value).into()
    }

    /// Plain text of a widget, optionally fixed-width or scrolling.
    fn text_area<'a>(&self, txt: &str, def: &WidgetDef, color: Color) -> Element<'a, Message> {
        if def.marquee {
            let cols = usize::from(def.width.unwrap_or(24));
            let elapsed = self.started.elapsed().as_secs_f32();
            return marquee(txt, cols, color, elapsed, def.speed.unwrap_or(8.0));
        }
        let shown = def
            .width
            .map_or_else(|| txt.to_owned(), |w| truncate(txt, usize::from(w)));
        let t = text(shown).size(BAR_TEXT).wrapping(NOWRAP).color(color);
        match def.width {
            Some(w) => container(t)
                .width(Length::Fixed(f32::from(w) * fonts::ADVANCE_EM * BAR_TEXT))
                .into(),
            None => t.into(),
        }
    }

    fn meter<'a>(
        &self,
        def: &WidgetDef,
        label: &str,
        pct: f32,
        shown: String,
    ) -> Element<'a, Message> {
        let t = self.tokens;
        let color = def.color.as_deref().map(|c| self.token(c));
        let tone = color.unwrap_or(if pct >= 90.0 {
            t.danger
        } else if pct >= 70.0 {
            t.warn
        } else {
            t.accent
        });
        let value = text(shown)
            .size(BAR_TEXT)
            .wrapping(NOWRAP)
            .color(color.unwrap_or(t.text));
        let inner: Element<'a, Message> =
            if matches!(def.style, WidgetStyle::Text | WidgetStyle::Chip) {
                value.into()
            } else {
                row![gauge(pct, tone, t), value]
                    .spacing(7)
                    .align_y(Alignment::Center)
                    .into()
            };
        self.compose(def, label, inner)
    }

    fn brand<'a>(&self) -> Element<'a, Message> {
        let t = self.tokens;
        let logo = container(text("M").size(13).font(fonts::MONO_BOLD).color(t.bg))
            .width(Length::Fixed(22.0))
            .height(Length::Fixed(22.0))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_| container::Style {
                background: Some(Background::Color(t.accent)),
                border: Border::default().rounded(6.0),
                ..container::Style::default()
            });
        row![
            logo,
            text(chrome::spaced("MAHZENTERM"))
                .size(10)
                .font(fonts::MONO_BOLD)
                .color(t.text_dim)
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
    }

    fn controls<'a>(&self) -> Option<Element<'a, Message>> {
        if !self.bars.custom_titlebar {
            return None;
        }
        let t = self.tokens;
        let shape = move |w: f32, h: f32, border_w: f32| -> Element<'a, Message> {
            container(Space::new())
                .width(Length::Fixed(w))
                .height(Length::Fixed(h))
                .style(move |_| container::Style {
                    background: (border_w == 0.0).then_some(Background::Color(t.text_dim)),
                    border: Border {
                        color: t.text_dim,
                        width: border_w,
                        radius: 1.5.into(),
                    },
                    ..container::Style::default()
                })
                .into()
        };
        let ctl =
            move |icon: Element<'a, Message>, msg: Message, danger: bool| -> Element<'a, Message> {
                button(
                    container(icon)
                        .width(Length::Fixed(40.0))
                        .height(Length::Fixed(28.0))
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center),
                )
                .padding(0)
                .on_press(msg)
                .style(move |_, status| {
                    let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
                    button::Style {
                        background: hot.then_some(Background::Color(if danger {
                            t.danger
                        } else {
                            t.surface_hi
                        })),
                        text_color: if hot && danger { t.bg } else { t.text_dim },
                        border: Border::default().rounded(6.0),
                        ..button::Style::default()
                    }
                })
                .into()
            };
        Some(
            row![
                ctl(shape(10.0, 1.0, 0.0), Message::Minimize, false),
                ctl(shape(10.0, 10.0, 1.0), Message::ToggleMaximize, false),
                ctl(text("×").size(17).into(), Message::CloseWindow, true),
            ]
            .spacing(2)
            .align_y(Alignment::Center)
            .into(),
        )
    }

    /// One widget by id; `None` when it has nothing to show right now (or the id is unknown).
    fn widget<'a>(&'a self, id: &str) -> Option<Element<'a, Message>> {
        let def = self.bars.def(id)?;
        let t = self.tokens;
        let tab = self.window.active_tab();
        let tint = def.color.as_deref().map(|c| self.token(c));
        let body: Element<'a, Message> = match def.kind {
            WidgetKind::Brand => self.brand(),
            WidgetKind::Status => {
                let rt = self.focused_runtime()?;
                let alive = rt.session().is_some_and(|s| !s.exited());
                row![
                    chrome::dot(if alive { t.ok } else { t.danger }),
                    text(truncate(rt.display_title(), 40))
                        .size(BAR_TEXT)
                        .wrapping(NOWRAP)
                        .color(tint.unwrap_or(t.text))
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
            }
            WidgetKind::Pane => {
                let panes = tab.pane_ids();
                if panes.len() < 2 {
                    return None;
                }
                let at = panes
                    .iter()
                    .position(|p| *p == tab.focus)
                    .map_or(1, |i| i + 1);
                chrome::tag(format!("PANE {at}/{}", panes.len()), t, false)
            }
            WidgetKind::Tab => {
                let n = self.window.tabs.len();
                if n < 2 {
                    return None;
                }
                let at = self
                    .window
                    .tabs
                    .iter()
                    .position(|x| x.id == tab.id)
                    .map_or(1, |i| i + 1);
                chrome::tag(format!("TAB {at}/{n}"), t, false)
            }
            WidgetKind::Zoom => {
                tab.zoomed?;
                chrome::tag("ZOOM", t, true)
            }
            WidgetKind::Exit => {
                let code = self
                    .focused_runtime()
                    .and_then(PaneRuntime::session)
                    .and_then(mahzenterm_term::Session::last_exit)
                    .filter(|c| *c != 0)?;
                container(
                    text(format!("EXIT {code}"))
                        .size(10)
                        .font(fonts::MONO_BOLD)
                        .color(tint.unwrap_or(t.bg)),
                )
                .padding(Padding::from([1.0, 6.0]))
                .style(move |_| container::Style {
                    background: Some(Background::Color(t.danger)),
                    border: Border::default().rounded(3.0),
                    ..container::Style::default()
                })
                .into()
            }
            WidgetKind::Problem => problem_chip(truncate(self.problem.as_ref()?, 80), t),
            WidgetKind::Cpu => {
                let pct = self.stats.cpu;
                self.meter(&def, "CPU", pct, format!("{pct:>3.0}%"))
            }
            WidgetKind::Memory => {
                let pct = self.stats.mem_percent();
                let shown = if matches!(def.style, WidgetStyle::Text | WidgetStyle::Chip) {
                    format!(
                        "{} / {}",
                        human_bytes(self.stats.mem_used),
                        human_bytes(self.stats.mem_total)
                    )
                } else {
                    human_bytes(self.stats.mem_used)
                };
                self.meter(&def, "MEM", pct, shown)
            }
            WidgetKind::Load => {
                let v = text(format!("{:.2}", self.stats.load))
                    .size(BAR_TEXT)
                    .wrapping(NOWRAP)
                    .color(tint.unwrap_or(t.text));
                self.compose(&def, "LOAD", v.into())
            }
            WidgetKind::Uptime => {
                let v = text(human_uptime(self.stats.uptime_secs))
                    .size(BAR_TEXT)
                    .wrapping(NOWRAP)
                    .color(tint.unwrap_or(t.text));
                self.compose(&def, "UP", v.into())
            }
            WidgetKind::Clock => {
                let pattern = def.format.as_deref().unwrap_or("%H:%M");
                let v = text(clock_text(pattern))
                    .size(BAR_TEXT)
                    .wrapping(NOWRAP)
                    .color(tint.unwrap_or(t.text));
                self.compose(&def, "", v.into())
            }
            WidgetKind::Theme => row![
                chrome::dot(t.accent),
                text(self.theme.name.clone())
                    .size(BAR_TEXT)
                    .wrapping(NOWRAP)
                    .color(tint.unwrap_or(t.text))
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
            WidgetKind::Slogan => text(mahzenterm_core::SLOGAN)
                .size(BAR_TEXT)
                .wrapping(NOWRAP)
                .color(tint.unwrap_or(t.text_dim))
                .into(),
            WidgetKind::Spacer => Space::new().width(Length::Fixed(16.0)).into(),
            WidgetKind::Separator => container(Space::new())
                .width(Length::Fixed(1.0))
                .height(Length::Fixed(14.0))
                .style(move |_| container::background(t.border))
                .into(),
            WidgetKind::Controls => self.controls()?,
            WidgetKind::Text => {
                let v = self.text_area(
                    def.text.as_deref().unwrap_or(""),
                    &def,
                    tint.unwrap_or(t.text),
                );
                self.compose(&def, "", v)
            }
            WidgetKind::Command => {
                let out = self.outputs.get(id).map_or("…", String::as_str);
                let v = self.text_area(out, &def, tint.unwrap_or(t.text));
                self.compose(&def, "", v)
            }
        };
        let body: Element<'a, Message> = if def.style == WidgetStyle::Chip {
            container(body)
                .padding(Padding::from([2.0, 8.0]))
                .style(move |_| container::Style {
                    border: Border {
                        color: t.border,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..container::Style::default()
                })
                .into()
        } else {
            body
        };
        Some(match def.on_click {
            Some(cmd) => mouse_area(body)
                .on_press(Message::RunCommand(cmd))
                .interaction(mouse::Interaction::Pointer)
                .into(),
            None => body,
        })
    }

    fn group<'a>(&'a self, ids: &[String], gap: f32) -> Element<'a, Message> {
        let mut r = row![].spacing(gap).align_y(Alignment::Center);
        for id in ids {
            if let Some(w) = self.widget(id) {
                r = r.push(w);
            }
        }
        r.into()
    }

    /// Bottom bar: three widget groups, the middle one centred.
    pub(super) fn status_bar(&self) -> Element<'_, Message> {
        let t = self.tokens;
        let sb = &self.bars.statusbar;
        // Equal flexible gaps on both sides of the centre group: it never overlaps the sides, and sits in the
        // middle whenever they are of similar width.
        let bar = row![
            self.group(&sb.left, 14.0),
            Space::new().width(Length::Fill),
            self.group(&sb.center, 14.0),
            Space::new().width(Length::Fill),
            self.group(&sb.right, 14.0),
        ]
        .align_y(Alignment::Center);
        column![
            chrome::hairline(Length::Fill, t.border),
            container(bar)
                .width(Length::Fill)
                .height(Length::Fixed(STATUS_H - 1.0))
                .padding(Padding::from([0.0, 14.0]))
                .align_y(Alignment::Center)
                .style(move |_| container::background(t.surface)),
        ]
        .into()
    }

    /// Top bar: widgets, tabs, a drag area, widgets. Doubles as the title bar.
    pub(super) fn tab_bar(&self) -> Element<'_, Message> {
        let t = self.tokens;
        let cfg = &self.bars.tabbar;
        let label_chars = ((cfg.tab_width - 78.0) / (fonts::ADVANCE_EM * 13.0)).max(4.0) as usize;
        let mut tabs = row![].spacing(3).align_y(Alignment::End);
        for (i, tab) in self.window.tabs.iter().enumerate() {
            let active = tab.id == self.window.active;
            let alive = self
                .runtimes
                .get(&(tab.id, tab.focus))
                .and_then(PaneRuntime::session)
                .is_some_and(|s| !s.exited());
            let dot = if !alive {
                t.danger
            } else if active {
                t.accent
            } else {
                t.text_dim
            };
            let label = button(
                container(
                    row![
                        chrome::dot(dot),
                        text(truncate(&self.tab_label(tab, i), label_chars))
                            .size(13)
                            .wrapping(NOWRAP)
                            .color(if active { t.text } else { t.text_dim })
                    ]
                    .spacing(9)
                    .align_y(Alignment::Center),
                )
                .height(Length::Fill)
                .align_y(Alignment::Center),
            )
            .on_press(Message::SelectTab(tab.id))
            .padding(Padding::from([0.0, 12.0]))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_, status| {
                if active {
                    button::Style {
                        text_color: t.text,
                        ..button::Style::default()
                    }
                } else {
                    super::flat_button(t, status, false, false)
                }
            });
            let mut line = row![label].align_y(Alignment::Center).height(Length::Fill);
            if cfg.close_buttons {
                line = line.push(super::icon_button("×", t, true, Message::CloseTab(tab.id)));
            }
            // The active tab takes the terminal's background, so it flows into the content, and carries an
            // accent line on top.
            let accent = container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(2.0))
                .style(move |_| {
                    container::background(if active { t.accent } else { Color::TRANSPARENT })
                });
            tabs = tabs.push(
                container(column![accent, line])
                    .width(Length::Fixed(cfg.tab_width))
                    .height(Length::Fixed(TAB_H))
                    .padding(Padding {
                        right: 4.0,
                        ..Padding::ZERO
                    })
                    .style(move |_| container::Style {
                        background: active.then_some(Background::Color(t.bg)),
                        border: Border::default().rounded(border::top(8.0)),
                        ..container::Style::default()
                    }),
            );
        }
        if cfg.new_tab_button {
            tabs = tabs.push(
                container(super::icon_button("+", t, false, Message::NewTab))
                    .height(Length::Fixed(TAB_H))
                    .align_y(Alignment::Center),
            );
        }
        let bar = row![
            container(self.group(&cfg.left, 12.0))
                .height(Length::Fill)
                .align_y(Alignment::Center)
                .padding(Padding {
                    right: 14.0,
                    ..Padding::ZERO
                }),
            container(tabs).height(Length::Fill).align_y(Alignment::End),
            Space::new().width(Length::Fill),
            container(self.group(&cfg.right, 12.0))
                .height(Length::Fill)
                .align_y(Alignment::Center),
        ]
        .align_y(Alignment::Center);
        let bar = container(bar)
            .width(Length::Fill)
            .height(Length::Fixed(TAB_BAR_H - 1.0))
            .padding(Padding {
                left: 14.0,
                right: 8.0,
                ..Padding::ZERO
            })
            .style(move |_| container::background(t.surface));
        let bar: Element<'_, Message> = if self.bars.custom_titlebar {
            mouse_area(bar)
                .on_press(Message::DragWindow)
                .on_double_click(Message::ToggleMaximize)
                .into()
        } else {
            bar.into()
        };
        column![bar, chrome::hairline(Length::Fill, t.border)].into()
    }

    /// Invisible grips along the window edge so a title-bar-less window can still be resized.
    pub(super) fn resize_grips(&self) -> Option<Element<'_, Message>> {
        if !self.bars.custom_titlebar || self.fullscreen {
            return None;
        }
        let (w, h) = (self.win_size.width, self.win_size.height);
        let (e, c) = (EDGE, CORNER);
        let grip =
            |x: f32, y: f32, gw: f32, gh: f32, dir: window::Direction, cur: mouse::Interaction| {
                pin(mouse_area(
                    Space::new()
                        .width(Length::Fixed(gw.max(1.0)))
                        .height(Length::Fixed(gh.max(1.0))),
                )
                .on_press(Message::ResizeEdge(dir))
                .interaction(cur))
                .x(x)
                .y(y)
                .into()
            };
        use mouse::Interaction as I;
        use window::Direction as D;
        let grips: Vec<Element<'_, Message>> = vec![
            grip(c, 0.0, w - 2.0 * c, e, D::North, I::ResizingVertically),
            grip(c, h - e, w - 2.0 * c, e, D::South, I::ResizingVertically),
            grip(0.0, c, e, h - 2.0 * c, D::West, I::ResizingHorizontally),
            grip(w - e, c, e, h - 2.0 * c, D::East, I::ResizingHorizontally),
            grip(0.0, 0.0, c, c, D::NorthWest, I::ResizingDiagonallyDown),
            grip(w - c, 0.0, c, c, D::NorthEast, I::ResizingDiagonallyUp),
            grip(0.0, h - c, c, c, D::SouthWest, I::ResizingDiagonallyUp),
            grip(w - c, h - c, c, c, D::SouthEast, I::ResizingDiagonallyDown),
        ];
        Some(stack(grips).width(Length::Fill).height(Length::Fill).into())
    }
}

/// Status-line warning: what is wrong and where, in the danger colour.
pub(super) fn problem_chip<'a>(label: String, t: Tokens) -> Element<'a, Message> {
    container(text(label).size(10).color(t.danger))
        .padding(Padding::from([1.0, 6.0]))
        .style(move |_| container::Style {
            border: Border {
                color: t.danger,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_bytes(512 * 1_048_576), "512 MB");
        assert_eq!(human_bytes(3 * 1_073_741_824 / 2), "1.5 GB");
        assert_eq!(human_uptime(90), "1m");
        assert_eq!(human_uptime(3 * 3600 + 120), "3h 2m");
        assert_eq!(human_uptime(2 * 86_400 + 5 * 3600), "2d 5h");
    }

    #[test]
    fn clock_survives_a_bad_pattern() {
        assert_eq!(clock_text("%H").len(), 2);
        assert_eq!(clock_text("%Q%"), "bad clock format");
    }
}
