//! Surfaces: module panels that can be shown in three places, chosen by the user at any time:
//! a floating **card** over the terminal, a **tab** of their own, or a separate **window** (for another monitor).
//!
//! A surface is a name (`module` + `panel`, the same pair a `PaneKind::Panel` carries) and a view. The two built-in ones
//! are the token usage panel and the AI sessions panel; the same contract is what plugin panels will use. A surface in a
//! tab or a window starts its data source just like the card does, and every placement shows the same live data.

use iced::widget::{container, text, Space};
use iced::{window, Element, Length, Padding, Rectangle, Size, Task};
use mahzenterm_core::workspace::PaneKind;

use super::widgets::Widget;
use super::{Message, PaneKey, Shell};

/// Scale a surface is drawn at in a tab or a window (a card on the terminal is usually smaller).
const BIG: f32 = 1.4;
/// Breathing room around a surface in a tab or a window.
const MARGIN: f32 = 20.0;

/// A panel that has a card, a tab and a window form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    /// Claude Code and Codex token use.
    Tokens,
    /// Running AI sessions.
    Sessions,
    /// An info-card widget (Kubernetes, Docker, Git).
    Widget(Widget),
}

impl Surface {
    pub const ALL: [Surface; 2 + Widget::ALL.len()] = {
        let mut all = [Surface::Tokens; 2 + Widget::ALL.len()];
        all[1] = Surface::Sessions;
        let mut i = 0;
        while i < Widget::ALL.len() {
            all[2 + i] = Surface::Widget(Widget::ALL[i]);
            i += 1;
        }
        all
    };

    fn ids(self) -> (&'static str, &'static str) {
        match self {
            Self::Tokens => ("ai_usage", "tokens"),
            Self::Sessions => ("ai_sessions", "sessions"),
            Self::Widget(w) => (w.key(), "card"),
        }
    }

    /// The surface a panel pane stands for, if the module is known.
    pub fn from_kind(kind: &PaneKind) -> Option<Self> {
        let PaneKind::Panel { module, panel } = kind else {
            return None;
        };
        Self::ALL
            .into_iter()
            .find(|s| s.ids() == (module.as_str(), panel.as_str()))
    }

    /// The pane kind that shows this surface in a tab.
    pub fn kind(self) -> PaneKind {
        let (module, panel) = self.ids();
        PaneKind::Panel {
            module: module.into(),
            panel: panel.into(),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Tokens => "Token usage",
            Self::Sessions => "AI sessions",
            Self::Widget(w) => w.title(),
        }
    }

    /// Window size that fits the surface at the big scale.
    fn window_size(self) -> Size {
        match self {
            Self::Tokens => Size::new(
                mahzenterm_ai_usage::card_width(BIG) + 2.0 * MARGIN,
                mahzenterm_ai_usage::card_height(BIG) + 2.0 * MARGIN,
            ),
            Self::Sessions => Size::new(
                mahzenterm_ai_sessions::card_width(BIG) + 2.0 * MARGIN,
                mahzenterm_ai_sessions::card_height(mahzenterm_ai_sessions::MAX_ROWS, BIG)
                    + 2.0 * MARGIN,
            ),
            Self::Widget(_) => Shell::widget_window_size(BIG, MARGIN),
        }
    }
}

impl Shell {
    /// The name of a surface (a plugin panel uses the plugin's own title).
    pub(super) fn surface_title(&self, s: Surface) -> String {
        match s {
            Surface::Widget(w) => self.widget_title(w),
            other => other.title().to_owned(),
        }
    }

    /// Whether a tab or a window currently shows the surface (so its data source has to run).
    pub(super) fn surface_in_use(&self, s: Surface) -> bool {
        self.windows.values().any(|w| *w == s)
            || self.window.tabs.iter().any(|t| {
                t.pane_ids().iter().any(|p| {
                    t.pane(*p)
                        .is_some_and(|pn| Surface::from_kind(&pn.kind) == Some(s))
                })
            })
    }

    /// Show the surface in a new tab.
    pub(super) fn open_surface_tab(&mut self, s: Surface) {
        self.window.new_tab(s.kind());
        self.state_dirty = true;
        self.sync();
        self.invalidate_active();
    }

    /// Show the surface in a window of its own; an already open one is brought to the front instead.
    pub(super) fn open_surface_window(&mut self, s: Surface) -> Task<Message> {
        if let Some(id) = self
            .windows
            .iter()
            .find(|(_, w)| **w == s)
            .map(|(id, _)| *id)
        {
            return window::gain_focus(id);
        }
        let (id, open) = window::open(window::Settings {
            size: s.window_size(),
            min_size: Some(Size::new(240.0, 160.0)),
            ..window::Settings::default()
        });
        self.windows.insert(id, s);
        open.map(|_| Message::Noop)
    }

    /// The surface's content at a given scale, with the placement buttons only where they make sense.
    fn surface_content(&self, s: Surface, scale: f32, buttons: bool) -> Element<'_, Message> {
        let place = |s: Surface| {
            buttons.then_some((Message::OpenSurfaceTab(s), Message::OpenSurfaceWindow(s)))
        };
        match s {
            Surface::Tokens => mahzenterm_ai_usage::view(
                &self.usage_snap,
                self.tokens,
                scale,
                Message::Noop,
                |_| Message::Noop,
                Message::Noop,
                place(s),
            ),
            Surface::Widget(w) => self.widget_content(w, scale),
            Surface::Sessions => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
                mahzenterm_ai_sessions::view(
                    &self.sessions,
                    now_ms,
                    self.tokens,
                    scale,
                    Message::Noop,
                    |_| Message::Noop,
                    Message::SessionRow,
                    Message::Noop,
                    place(s),
                )
            }
        }
    }

    /// A panel pane (a tab or a split showing a surface).
    pub(super) fn panel_pane<'a>(
        &'a self,
        key: PaneKey,
        outer: Rectangle,
        kind: &PaneKind,
    ) -> Element<'a, Message> {
        let t = self.tokens;
        let body: Element<'a, Message> = match Surface::from_kind(kind) {
            Some(s) => self.surface_content(s, BIG, false),
            None => text("This panel is not available: its module is not part of this build.")
                .size(13)
                .color(t.text_dim)
                .into(),
        };
        let framed = container(body)
            .width(Length::Fixed(outer.width))
            .height(Length::Fixed(outer.height))
            .padding(Padding::from(MARGIN))
            .style(move |_| container::background(t.bg));
        iced::widget::mouse_area(framed)
            .on_press(Message::FocusPane(key))
            .into()
    }

    /// The content of a secondary window.
    pub(super) fn surface_window_view(&self, id: window::Id) -> Element<'_, Message> {
        if self.detached.contains_key(&id) {
            return self.detached_view(id);
        }
        let t = self.tokens;
        let Some(s) = self.windows.get(&id).copied() else {
            return Space::new().into();
        };
        container(self.surface_content(s, BIG, false))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding::from(MARGIN))
            .style(move |_| container::background(t.bg))
            .into()
    }

    /// The window title of a secondary window.
    pub(super) fn surface_window_title(&self, id: window::Id) -> Option<String> {
        self.windows
            .get(&id)
            .map(|s| format!("{} - {}", self.surface_title(*s), mahzenterm_core::NAME))
    }
}
