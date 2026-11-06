//! The AI token card: a floating panel the user drags and resizes anywhere over the window.

use iced::widget::pin;
use iced::{Element, Point};
use mahzenterm_ai_usage::{card_height, card_width, view as card_view, Snapshot, CARD_W};

use super::{CardId, Message, Shell};

impl Shell {
    fn win(&self) -> (f32, f32) {
        (self.win_size.width, self.win_size.height)
    }

    /// Default origin and size of the token card at `scale`.
    fn usage_geom(&self, scale: f32) -> ((f32, f32), (f32, f32)) {
        (
            self.usage_settings.origin(self.win(), scale),
            (card_width(scale), card_height(scale)),
        )
    }

    /// Top-left of the card in window pixels: the dragged spot, else the configured anchor.
    pub(super) fn usage_origin(&self) -> (f32, f32) {
        self.usage.origin(self.win(), &|s| self.usage_geom(s))
    }

    pub(super) fn set_usage_visible(&mut self, visible: bool) {
        self.usage.set_visible(visible);
        if visible {
            self.usage_worker.show(self.usage_settings.period());
        } else {
            self.usage_worker.hide();
        }
        self.state_dirty = true;
    }

    pub(super) fn toggle_usage(&mut self) {
        self.set_usage_visible(!self.usage.visible);
    }

    pub(super) fn reset_usage_position(&mut self) {
        self.usage.pos = None;
        self.state_dirty = true;
    }

    pub(super) fn set_usage_scale(&mut self, scale: f32) {
        let win = self.win();
        let mut card = self.usage.clone();
        card.set_scale(scale, win, &|s| self.usage_geom(s));
        self.usage = card;
        self.state_dirty = true;
    }

    pub(super) fn step_usage_scale(&mut self, steps: f32) {
        self.set_usage_scale(self.usage.scale + 0.1 * steps);
    }

    /// Pointer motion for a drag or resize of the token card; true when it was used.
    pub(super) fn usage_pointer(&mut self, pointer: Point) -> bool {
        let win = self.win();
        let mut card = self.usage.clone();
        let used = card.on_pointer(pointer, win, &|s| self.usage_geom(s), CARD_W);
        if used {
            self.usage = card;
        }
        used
    }

    pub(super) fn set_usage_snapshot(&mut self, snap: Snapshot) {
        self.usage_snap = snap;
    }

    /// The card positioned over the window, or `None` while hidden.
    pub(super) fn usage_view(&self) -> Option<Element<'_, Message>> {
        if !self.usage.visible {
            return None;
        }
        let (x, y) = self.usage_origin();
        let card = card_view(
            &self.usage_snap,
            self.tokens,
            self.usage.scale,
            Message::CardPress(CardId::Usage),
            |p| Message::CardHover(CardId::Usage, p),
            Message::CardResizePress(CardId::Usage),
        );
        Some(pin(card).x(x).y(y).into())
    }
}
