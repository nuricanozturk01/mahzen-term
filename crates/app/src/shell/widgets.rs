//! Info-card widgets (Kubernetes, Docker, Git): small live panels the user places from the command palette
//! ("Widgets: ..."), as a floating card, a tab or a window, like the token card.
//!
//! Each widget describes its data as an `InfoCard` (ui-kit draws it). Data is fetched in the background only while a
//! card, tab or window shows the widget, and re-fetched on a slow timer; hidden widgets cost nothing.

use std::time::Duration;

use iced::widget::pin;
use iced::{Element, Point, Task};
use mahzenterm_ui_kit::infocard::{self, InfoCard};

use super::{CardId, CardState, Message, PaneKey, Shell, Surface};

/// A widget built on the generic info card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Widget {
    Kube,
    Docker,
    Git,
    /// A WebAssembly plugin the user allowed (slot = position in `[modules.plugins] allow`).
    Plugin(u8),
}

impl Widget {
    pub const ALL: [Widget; 3 + mahzenterm_plugins::MAX_PLUGINS] = [
        Widget::Kube,
        Widget::Docker,
        Widget::Git,
        Widget::Plugin(0),
        Widget::Plugin(1),
        Widget::Plugin(2),
        Widget::Plugin(3),
    ];

    /// Key used in `state.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Kube => "kube",
            Self::Docker => "docker",
            Self::Git => "git",
            Self::Plugin(0) => "plugin0",
            Self::Plugin(1) => "plugin1",
            Self::Plugin(2) => "plugin2",
            Self::Plugin(_) => "plugin3",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Kube => "Kubernetes",
            Self::Docker => "Docker",
            Self::Git => "Git",
            Self::Plugin(_) => "Plugin",
        }
    }

    /// The command that shows or hides the widget's card.
    pub fn command(self) -> &'static str {
        match self {
            Self::Kube => mahzenterm_kubernetes::id::CARD,
            Self::Docker => mahzenterm_docker::id::TOGGLE,
            Self::Git => mahzenterm_git::id::TOGGLE,
            Self::Plugin(0) => "plugins.show.0",
            Self::Plugin(1) => "plugins.show.1",
            Self::Plugin(2) => "plugins.show.2",
            Self::Plugin(_) => "plugins.show.3",
        }
    }

    pub fn from_command(cmd: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.command() == cmd)
    }
}

/// Where a widget card sits until the user drags it: bottom right, one slot per widget.
const GAP: f32 = 12.0;

impl Shell {
    /// Pause between two queries of a widget.
    fn widget_interval(&self, w: Widget) -> Duration {
        match w {
            Widget::Kube => Duration::from_secs(10),
            Widget::Docker | Widget::Git => Duration::from_secs(4),
            Widget::Plugin(i) => self
                .plugins
                .get(usize::from(i))
                .map_or(Duration::from_secs(5), |p| {
                    Duration::from_secs(p.interval_secs())
                }),
        }
    }

    /// Display name of a widget (a plugin's own title).
    pub(super) fn widget_title(&self, w: Widget) -> String {
        match w {
            Widget::Plugin(i) => self
                .plugins
                .get(usize::from(i))
                .map_or_else(|| w.title().to_owned(), |p| p.title().to_owned()),
            _ => w.title().to_owned(),
        }
    }

    fn widget_card(&self, w: Widget) -> CardState {
        self.widget_cards
            .get(&w)
            .cloned()
            .unwrap_or_else(|| CardState::new(false, None, 1.0))
    }

    fn widget_rows(&self, w: Widget) -> usize {
        self.widget_data.get(&w).map_or(3, InfoCard::shown)
    }

    /// Default origin and size of a widget card at `scale`.
    pub(super) fn widget_geom(&self, w: Widget, scale: f32) -> ((f32, f32), (f32, f32)) {
        let size = (
            infocard::card_width(scale),
            infocard::card_height(self.widget_rows(w), scale),
        );
        let slot = Widget::ALL.iter().position(|x| *x == w).unwrap_or(0) as f32;
        let x = self.win_size.width - (slot + 1.0) * (size.0 + GAP);
        let y = self.win_size.height - self.status_h() - size.1 - GAP;
        ((x, y), size)
    }

    /// Top-left of a visible widget card in window pixels.
    pub(super) fn widget_origin(&self, w: Widget) -> (f32, f32) {
        let win = (self.win_size.width, self.win_size.height);
        self.widget_card(w).origin(win, &|s| self.widget_geom(w, s))
    }

    pub(super) fn widget_visible(&self, w: Widget) -> bool {
        self.widget_cards.get(&w).is_some_and(|c| c.visible)
    }

    /// A card, tab or window shows the widget, so its data must be kept fresh.
    pub(super) fn widget_wanted(&self, w: Widget) -> bool {
        self.widget_visible(w) || self.surface_in_use(Surface::Widget(w))
    }

    pub(super) fn set_widget_visible(&mut self, w: Widget, visible: bool) {
        self.widget_cards
            .entry(w)
            .or_insert_with(|| CardState::new(false, None, 1.0))
            .set_visible(visible);
        if visible {
            self.widget_refresh_now(w);
        }
        if visible && w == Widget::Kube {
            self.kube_enabled = true;
        }
        if self.bars.fit_terminal {
            self.sync();
            self.invalidate_active();
        }
        self.state_dirty = true;
    }

    pub(super) fn toggle_widget(&mut self, w: Widget) {
        self.set_widget_visible(w, !self.widget_visible(w));
    }

    /// Any widget card being dragged or resized.
    pub(super) fn widget_busy(&self) -> bool {
        self.widget_cards.values().any(CardState::busy)
    }

    /// Pointer motion for a drag or resize of a widget card; true when one used it.
    pub(super) fn widgets_pointer(&mut self, pointer: Point) -> bool {
        let win = (self.win_size.width, self.win_size.height);
        for w in Widget::ALL {
            let Some(mut card) = self.widget_cards.get(&w).filter(|c| c.busy()).cloned() else {
                continue;
            };
            let used = card.on_pointer(pointer, win, &|s| self.widget_geom(w, s), infocard::CARD_W);
            self.widget_cards.insert(w, card);
            if used {
                return true;
            }
        }
        false
    }

    /// Button released: ends a drag or resize of a widget card.
    pub(super) fn widgets_release(&mut self) -> bool {
        let mut any = false;
        for c in self.widget_cards.values_mut() {
            any |= c.on_release();
        }
        any
    }

    /// What the Git card follows: which pane has focus and the folder it shows (cheap to compare on every update).
    fn git_follow(&self) -> (PaneKey, Option<String>) {
        let key = self.focused_key();
        (
            key,
            self.runtimes.get(&key).and_then(|rt| rt.cwd_label.clone()),
        )
    }

    /// Ask for a fresh query of a widget as soon as possible (its inputs changed).
    pub(super) fn widget_refresh_now(&mut self, w: Widget) {
        self.widget_next.remove(&w);
    }

    /// Start a query for every widget that is shown, due, and not already being queried. A query is built at the
    /// moment it starts, so it always uses the current folder and context; between queries a timer wakes the loop.
    pub(super) fn widget_tasks(&mut self) -> Task<Message> {
        // The Git card follows the focused pane: a new pane or folder refreshes it at once.
        let follow = self.git_follow();
        if self.widget_git_follow.as_ref() != Some(&follow) {
            self.widget_git_follow = Some(follow);
            self.widget_refresh_now(Widget::Git);
        }
        let now = std::time::Instant::now();
        let mut tasks = Vec::new();
        for w in Widget::ALL {
            if !self.widget_wanted(w)
                || matches!(w, Widget::Plugin(i) if usize::from(i) >= self.plugins.len())
            {
                continue;
            }
            let due = self.widget_next.get(&w).is_none_or(|t| now >= *t);
            if !due || self.widget_loading.contains(&w) {
                continue;
            }
            self.widget_loading.insert(w);
            let job = self.widget_job(w);
            tasks.push(Task::perform(async move { job() }, move |card| {
                Message::WidgetData(w, Box::new(card))
            }));
        }
        Task::batch(tasks)
    }

    /// The blocking query for a widget, with everything it needs copied out of the shell.
    fn widget_job(&self, w: Widget) -> Box<dyn FnOnce() -> InfoCard + Send> {
        match w {
            Widget::Docker => {
                let program = self.docker_program.clone();
                Box::new(move || mahzenterm_docker::card(&mahzenterm_docker::fetch(&program)))
            }
            Widget::Git => {
                let dir = self
                    .focused_cwd()
                    .or_else(mahzenterm_platform::home_dir)
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                Box::new(move || mahzenterm_git::card(&mahzenterm_git::fetch(&dir)))
            }
            Widget::Plugin(i) => {
                let plugin = self.plugins.get(usize::from(i)).cloned();
                let width = 40;
                Box::new(move || match plugin {
                    Some(p) => p.render(width).unwrap_or_else(|e| InfoCard {
                        title: p.title().to_owned(),
                        badge: "error".into(),
                        empty: e.0,
                        ..InfoCard::default()
                    }),
                    None => InfoCard::default(),
                })
            }
            Widget::Kube => {
                let scope = self.kube_scope();
                let program = self.kube_settings.program().to_owned();
                let prod = self.kube_prod();
                Box::new(move || {
                    let Some(scope) = scope else {
                        return mahzenterm_kubernetes::card(None, "", false, &Ok(Vec::new()));
                    };
                    let pods = mahzenterm_kubernetes::get_pods(&scope)
                        .ok_or_else(|| "bad context or namespace name".to_owned())
                        .and_then(|args| mahzenterm_kubernetes::run(&program, &args))
                        .and_then(|out| {
                            mahzenterm_kubernetes::parse_pods(&out, super::kube_panel::now_secs())
                                .ok_or_else(|| {
                                    "kubectl answered with something unexpected".to_owned()
                                })
                        });
                    mahzenterm_kubernetes::card(Some(&scope.context), &scope.namespace, prod, &pods)
                })
            }
        }
    }

    /// A query finished: keep the card and set a timer for the next one.
    pub(super) fn on_widget_data(&mut self, w: Widget, card: InfoCard) -> Task<Message> {
        self.widget_loading.remove(&w);
        self.widget_data.insert(w, card);
        let pause = self.widget_interval(w);
        self.widget_next
            .insert(w, std::time::Instant::now() + pause);
        Task::perform(
            async move {
                std::thread::sleep(pause);
            },
            |()| Message::WidgetTick,
        )
    }

    /// The widget card positioned over the window, or `None` while hidden.
    pub(super) fn widget_view(&self, w: Widget) -> Option<Element<'_, Message>> {
        let card = self.widget_card(w);
        if !card.visible {
            return None;
        }
        let (x, y) = self.widget_origin(w);
        let data = self
            .widget_data
            .get(&w)
            .cloned()
            .unwrap_or_else(|| loading(&self.widget_title(w)));
        let view = infocard::view(
            &data,
            self.tokens,
            card.scale,
            Message::CardPress(CardId::Widget(w)),
            move |p| Message::CardHover(CardId::Widget(w), p),
            Message::CardResizePress(CardId::Widget(w)),
            Some((
                Message::OpenSurfaceTab(Surface::Widget(w)),
                Message::OpenSurfaceWindow(Surface::Widget(w)),
            )),
        );
        Some(pin(view).x(x).y(y).into())
    }

    /// The widget's content in a tab or a window.
    pub(super) fn widget_content(&self, w: Widget, scale: f32) -> Element<'_, Message> {
        let data = self
            .widget_data
            .get(&w)
            .cloned()
            .unwrap_or_else(|| loading(&self.widget_title(w)));
        infocard::view(
            &data,
            self.tokens,
            scale,
            Message::Noop,
            |_| Message::Noop,
            Message::Noop,
            None,
        )
    }

    /// Window size that fits the widget at `scale`, with a margin.
    pub(super) fn widget_window_size(scale: f32, margin: f32) -> iced::Size {
        iced::Size::new(
            infocard::card_width(scale) + 2.0 * margin,
            infocard::card_height(infocard::MAX_ROWS, scale) + 2.0 * margin,
        )
    }
}

fn loading(title: &str) -> InfoCard {
    InfoCard {
        title: title.into(),
        empty: "loading\u{2026}".into(),
        ..InfoCard::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip() {
        for w in Widget::ALL {
            assert_eq!(Widget::from_command(w.command()), Some(w));
        }
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;

    const HELLO: &str = r#"
    (module
      (memory (export "memory") 1)
      (data (i32.const 1024) "title=Hello\nbadge=v1\nrow=good|status|ok")
      (func (export "mz_alloc") (param i32) (result i32) (i32.const 0))
      (func (export "mz_render") (param i32 i32) (result i64)
        (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 40))))
    "#;

    #[test]
    fn an_allowed_plugin_becomes_a_widget_card_slot() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("hello");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"hello\"\ntitle = \"Hello Plugin\"\ninterval_secs = 2\n",
        )
        .unwrap();
        std::fs::write(dir.join("plugin.wasm"), wat::parse_str(HELLO).unwrap()).unwrap();
        let mut s = super::super::tests::shell();
        s.plugins = vec![std::sync::Arc::new(
            mahzenterm_plugins::Plugin::load(&dir).unwrap(),
        )];
        assert_eq!(s.widget_title(Widget::Plugin(0)), "Hello Plugin");
        assert_eq!(s.widget_interval(Widget::Plugin(0)), Duration::from_secs(2));
        // The job renders the plugin's answer.
        let card = s.widget_job(Widget::Plugin(0))();
        assert_eq!((card.title.as_str(), card.badge.as_str()), ("Hello", "v1"));
        // A slot without a plugin is never queried.
        s.set_widget_visible(Widget::Plugin(1), true);
        let _ = s.widget_tasks();
        assert!(!s.widget_loading.contains(&Widget::Plugin(1)));
        s.set_widget_visible(Widget::Plugin(0), true);
        let _ = s.widget_tasks();
        assert!(s.widget_loading.contains(&Widget::Plugin(0)));
        // Tabs and windows use the plugin's title too.
        assert_eq!(
            s.surface_title(Surface::Widget(Widget::Plugin(0))),
            "Hello Plugin"
        );
    }
}
