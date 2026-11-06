//! The AI sessions card: live Claude Code sessions, jump to the pane each one runs in.

use iced::widget::pin;
use iced::{Element, Point};
use mahzenterm_ai_sessions::{card_height, card_width, view as card_view, Session, Status, CARD_W};

use super::{CardId, Message, PaneKey, Shell};

impl Shell {
    fn sessions_geom(&self, scale: f32) -> ((f32, f32), (f32, f32)) {
        let n = self.sessions.len();
        let win = (self.win_size.width, self.win_size.height);
        (
            self.sessions_settings.origin(win, n, scale),
            (card_width(scale), card_height(n, scale)),
        )
    }

    pub(super) fn sessions_origin(&self) -> (f32, f32) {
        let win = (self.win_size.width, self.win_size.height);
        self.sessions_card.origin(win, &|s| self.sessions_geom(s))
    }

    pub(super) fn set_sessions_visible(&mut self, visible: bool) {
        self.sessions_card.set_visible(visible);
        if visible {
            self.sessions_worker.show(self.sessions_settings.period());
        } else {
            self.sessions_worker.hide();
        }
        self.state_dirty = true;
    }

    pub(super) fn toggle_sessions(&mut self) {
        self.set_sessions_visible(!self.sessions_card.visible);
    }

    pub(super) fn reset_sessions_position(&mut self) {
        self.sessions_card.pos = None;
        self.state_dirty = true;
    }

    pub(super) fn step_sessions_scale(&mut self, steps: f32) {
        let win = (self.win_size.width, self.win_size.height);
        let mut card = self.sessions_card.clone();
        card.step_scale(steps, win, &|s| self.sessions_geom(s));
        self.sessions_card = card;
        self.state_dirty = true;
    }

    pub(super) fn sessions_pointer(&mut self, pointer: Point) -> bool {
        let win = (self.win_size.width, self.win_size.height);
        let mut card = self.sessions_card.clone();
        let used = card.on_pointer(pointer, win, &|s| self.sessions_geom(s), CARD_W);
        if used {
            self.sessions_card = card;
        }
        used
    }

    pub(super) fn set_sessions(&mut self, sessions: Vec<Session>) {
        self.sessions = sessions;
    }

    /// Every pane with a running shell: key and shell pid.
    fn pane_pids(&self) -> Vec<(PaneKey, u32)> {
        self.runtimes
            .iter()
            .filter_map(|(k, rt)| Some((*k, rt.session()?.pid()?)))
            .collect()
    }

    /// Show the pane `key`: its tab becomes active and the pane focused (zoom follows).
    pub(super) fn focus_pane_key(&mut self, key: PaneKey) {
        if self.window.activate(key.0).is_err() {
            return;
        }
        let tab = self.window.active_tab_mut();
        if tab.pane(key.1).is_none() {
            return;
        }
        tab.focus = key.1;
        if tab.zoomed.is_some() {
            tab.zoomed = Some(key.1);
        }
        self.state_dirty = true;
        self.sync();
        self.invalidate_active();
    }

    /// Click on a session row: go to its pane.
    pub(super) fn focus_session(&mut self, index: usize) {
        let Some(s) = self.sessions.get(index) else {
            return;
        };
        match s.owner_pane(&self.pane_pids()) {
            Some(key) => self.focus_pane_key(key),
            None => {
                self.problem = Some(format!(
                    "{} does not run in a MahzenTerm pane (started elsewhere)",
                    if s.name.is_empty() {
                        "that session"
                    } else {
                        &s.name
                    }
                ));
            }
        }
    }

    /// `ai_sessions.focus_waiting`: jump to the first session that needs the user.
    pub(super) fn focus_waiting_session(&mut self) {
        let panes = self.pane_pids();
        let target = self
            .sessions
            .iter()
            .filter(|s| s.status == Status::Waiting)
            .find_map(|s| s.owner_pane(&panes));
        match target {
            Some(key) => self.focus_pane_key(key),
            None => {
                self.problem = Some(
                    "no AI session in these panes needs you (open the sessions card to refresh)"
                        .into(),
                );
            }
        }
    }

    pub(super) fn sessions_view(&self) -> Option<Element<'_, Message>> {
        if !self.sessions_card.visible {
            return None;
        }
        let (x, y) = self.sessions_origin();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
        let card = card_view(
            &self.sessions,
            now_ms,
            self.tokens,
            self.sessions_card.scale,
            Message::CardPress(CardId::Sessions),
            |p| Message::CardHover(CardId::Sessions, p),
            Message::SessionRow,
            Message::CardResizePress(CardId::Sessions),
        );
        Some(pin(card).x(x).y(y).into())
    }
}
