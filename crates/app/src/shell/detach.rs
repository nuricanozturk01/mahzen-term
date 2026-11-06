//! Moving a terminal pane into a window of its own (for a second or third monitor), and bringing it back.
//!
//! The shell session keeps running: only the view moves. While detached, the pane is not part of any tab; closing
//! its window re-attaches it as a new tab, so a shell is never lost by accident.

use iced::keyboard::Event as KeyEvent;
use iced::widget::{container, Space};
use iced::{clipboard, window, Color, Element, Length, Padding, Size, Task};
use mahzenterm_core::workspace::PaneKind;
use mahzenterm_ui_kit::TerminalView;

use super::{layout, rgb8, Message, PaneKey, Shell, Term};

impl Shell {
    /// `pane.detach`: the focused terminal pane moves to a new window.
    pub(super) fn detach_focused(&mut self) -> Task<Message> {
        let key = self.focused_key();
        if !matches!(
            self.runtimes.get(&key).map(|r| &r.term),
            Some(Term::Running { .. })
        ) {
            self.problem = Some("only a running terminal pane can move to its own window".into());
            return Task::none();
        }
        let Some(tab) = self.window.tabs.iter_mut().find(|t| t.id == key.0) else {
            return Task::none();
        };
        if tab.pane_ids().len() > 1 {
            let _ = tab.close(key.1);
        } else if self.window.tabs.len() > 1 {
            let _ = self.window.close_tab(key.0);
        } else {
            self.problem = Some(
                "this is the only pane: open another tab or split first, then move this one".into(),
            );
            return Task::none();
        }
        let (id, open) = window::open(window::Settings {
            size: Size::new(900.0, 560.0),
            min_size: Some(Size::new(240.0, 120.0)),
            ..window::Settings::default()
        });
        self.detached.insert(id, key);
        self.state_dirty = true;
        self.explorer_focus = false;
        self.sync();
        self.invalidate_active();
        open.map(|_| Message::Noop)
    }

    /// The window of a detached pane closed (or the program in it ended): the shell comes back as a new tab.
    pub(super) fn reattach(&mut self, id: window::Id) {
        let Some(old) = self.detached.remove(&id) else {
            return;
        };
        let Some(mut rt) = self.runtimes.remove(&old) else {
            return;
        };
        let tab = self.window.new_tab(PaneKind::Terminal {
            cwd: None,
            shell: None,
            args: Vec::new(),
        });
        let new = (tab, self.window.active_tab().focus);
        // The session still reports activity under its old key; the alias points that at the new one.
        rt.key = new;
        self.wake_alias.insert(old, new);
        self.runtimes.insert(new, rt);
        self.state_dirty = true;
        self.sync();
        self.invalidate_active();
    }

    /// The program in a detached pane ended: close its window (the shell is gone, nothing to bring back).
    pub(super) fn detached_exited(&mut self, key: PaneKey) -> Option<Task<Message>> {
        let id = self
            .detached
            .iter()
            .find(|(_, k)| **k == key)
            .map(|(id, _)| *id)?;
        self.detached.remove(&id);
        self.runtimes.remove(&key);
        Some(window::close(id))
    }

    /// Size the detached pane's terminal to its window.
    pub(super) fn resize_detached(&mut self, id: window::Id, size: Size) {
        let Some(key) = self.detached.get(&id).copied() else {
            return;
        };
        let inner = Size::new(
            size.width - 2.0 * layout::PAD_X,
            size.height - 2.0 * layout::PAD_Y,
        );
        let grid = self.metrics.grid_for(inner);
        let metrics = self.metrics;
        if let Some(rt) = self.runtimes.get_mut(&key) {
            rt.resize(grid, metrics);
            rt.cache.clear();
        }
    }

    /// A key typed in a detached window goes to its shell (Ctrl+Shift+V pastes).
    pub(super) fn detached_key(&mut self, key: PaneKey, ev: &KeyEvent) -> Task<Message> {
        let KeyEvent::KeyPressed {
            key: k,
            modifiers,
            text,
            ..
        } = ev
        else {
            return Task::none();
        };
        if modifiers.control()
            && modifiers.shift()
            && matches!(k.as_ref(), iced::keyboard::Key::Character(c) if c.eq_ignore_ascii_case("v"))
        {
            return clipboard::read().map(move |t| Message::PasteTo(key, t));
        }
        if let Some((tk, tm)) = super::to_term_key(k, text.as_deref(), *modifiers) {
            if let Some(session) = self
                .runtimes
                .get(&key)
                .and_then(super::PaneRuntime::session)
            {
                session.select_clear();
                session.send_key(tk, tm);
            }
        }
        Task::none()
    }

    pub(super) fn paste_to(&mut self, key: PaneKey, text: Option<String>) {
        if let (Some(t), Some(s)) = (
            text,
            self.runtimes
                .get(&key)
                .and_then(super::PaneRuntime::session),
        ) {
            s.paste(&t);
        }
    }

    /// The content of a detached pane's window.
    pub(super) fn detached_view(&self, id: window::Id) -> Element<'_, Message> {
        let t = self.tokens;
        let Some(key) = self.detached.get(&id).copied() else {
            return Space::new().into();
        };
        let Some(rt) = self.runtimes.get(&key) else {
            return Space::new().into();
        };
        let Term::Running { snapshot, .. } = &rt.term else {
            return Space::new().into();
        };
        let bg = Color::from_rgb8(snapshot.bg.0, snapshot.bg.1, snapshot.bg.2);
        let view = TerminalView::new(snapshot, &rt.cache, self.metrics, move |a| {
            Message::View(key, a)
        })
        .focused(true)
        .selection(self.theme.selection)
        .highlights(
            rgb8(mahzenterm_ui_kit::mix(t.bg, t.warn, 0.35)),
            rgb8(t.warn),
            rgb8(t.accent),
        )
        .into_element();
        container(view)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding {
                top: layout::PAD_Y,
                right: layout::PAD_X,
                bottom: layout::PAD_Y,
                left: layout::PAD_X,
            })
            .style(move |_| container::background(bg))
            .into()
    }

    /// Window title of a detached pane.
    pub(super) fn detached_title(&self, id: window::Id) -> Option<String> {
        let key = self.detached.get(&id)?;
        let rt = self.runtimes.get(key)?;
        Some(format!(
            "{} - {}",
            rt.display_title(),
            mahzenterm_core::NAME
        ))
    }
}
