//! The AI token card: a floating panel the user drags anywhere over the window.

use iced::widget::pin;
use iced::{Element, Point};
use mahzenterm_ai_usage::{clamp_origin, view as card_view, Snapshot};

use super::{Message, Shell};

impl Shell {
    /// Top-left of the card in window pixels: the dragged spot, else the configured anchor.
    pub(super) fn usage_origin(&self) -> (f32, f32) {
        let win = (self.win_size.width, self.win_size.height);
        let pos = self
            .usage_pos
            .unwrap_or_else(|| self.usage_settings.origin(win));
        clamp_origin(pos, win)
    }

    pub(super) fn set_usage_visible(&mut self, visible: bool) {
        self.usage_visible = visible;
        self.usage_drag = false;
        if visible {
            self.usage_worker.show(self.usage_settings.period());
        } else {
            self.usage_worker.hide();
        }
        self.state_dirty = true;
    }

    pub(super) fn toggle_usage(&mut self) {
        self.set_usage_visible(!self.usage_visible);
    }

    pub(super) fn reset_usage_position(&mut self) {
        self.usage_pos = None;
        self.state_dirty = true;
    }

    /// Pointer over the header, relative to the card (remembered so a press can grab at that spot).
    pub(super) fn usage_hover(&mut self, p: Point) {
        self.usage_grab = p;
    }

    pub(super) fn usage_press(&mut self) {
        self.usage_drag = true;
    }

    /// Follow the pointer (window coordinates) while dragging.
    pub(super) fn usage_drag_to(&mut self, pointer: Point) {
        let win = (self.win_size.width, self.win_size.height);
        self.usage_pos = Some(clamp_origin(
            (pointer.x - self.usage_grab.x, pointer.y - self.usage_grab.y),
            win,
        ));
    }

    pub(super) fn set_usage_snapshot(&mut self, snap: Snapshot) {
        self.usage_snap = snap;
    }

    /// The card positioned over the window, or `None` while hidden.
    pub(super) fn usage_view(&self) -> Option<Element<'_, Message>> {
        if !self.usage_visible {
            return None;
        }
        let (x, y) = self.usage_origin();
        let card = card_view(
            &self.usage_snap,
            self.tokens,
            Message::UsagePress,
            Message::UsageHover,
        );
        Some(pin(card).x(x).y(y).into())
    }
}
