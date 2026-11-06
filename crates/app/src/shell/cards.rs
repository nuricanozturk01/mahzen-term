//! Floating cards (AI token usage, AI sessions): shared drag, resize and placement state.
//!
//! A card is positioned in window pixels. Until the user drags it, it sits at its configured anchor; its size is a
//! scale factor. Both are clamped so the card never leaves the window.

use iced::Point;

/// Smallest and largest card scale.
pub(super) const MIN_SCALE: f32 = 0.6;
pub(super) const MAX_SCALE: f32 = 2.5;

/// Where a card would sit by default and how big it is, for a given scale: `(default origin, size)`.
pub(super) type Geom<'a> = &'a dyn Fn(f32) -> ((f32, f32), (f32, f32));

/// The largest top-left-anchored rectangle of `area` (width, height) that no card overlaps, found by cutting at card
/// edges. Cards hugging the right and bottom edges therefore carve out side and bottom bands. Falls back to the whole
/// area when the free part would be unusably small (under a quarter of each side).
/// `cards` are `(x, y, w, h)` in the same coordinates as `area` (origin top-left).
pub(super) fn free_area(area: (f32, f32), cards: &[(f32, f32, f32, f32)]) -> (f32, f32) {
    let mut ws = vec![area.0];
    let mut hs = vec![area.1];
    for &(x, y, ..) in cards {
        ws.push(x.clamp(0.0, area.0));
        hs.push(y.clamp(0.0, area.1));
    }
    let hits = |w: f32, h: f32| {
        cards
            .iter()
            .any(|&(x, y, cw, ch)| x < w && x + cw > 0.0 && y < h && y + ch > 0.0)
    };
    let mut best: Option<(f32, f32)> = None;
    for &w in &ws {
        for &h in &hs {
            if w < area.0 * 0.25 || h < area.1 * 0.25 || hits(w, h) {
                continue;
            }
            if best.is_none_or(|(bw, bh)| w * h > bw * bh) {
                best = Some((w, h));
            }
        }
    }
    best.unwrap_or(area)
}

/// Placement and interaction state of one card.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CardState {
    pub visible: bool,
    /// Where the user dragged it; `None` follows the configured anchor.
    pub pos: Option<(f32, f32)>,
    pub scale: f32,
    /// Pointer position inside the card header, to grab the card at that spot.
    pub grab: Point,
    pub drag: bool,
    pub resize: bool,
}

pub(super) fn clamp_scale(s: f32) -> f32 {
    if s.is_nan() {
        1.0
    } else {
        s.clamp(MIN_SCALE, MAX_SCALE)
    }
}

fn clamp_pos(pos: (f32, f32), win: (f32, f32), size: (f32, f32)) -> (f32, f32) {
    (
        pos.0.clamp(0.0, (win.0 - size.0).max(0.0)),
        pos.1.clamp(0.0, (win.1 - size.1).max(0.0)),
    )
}

impl CardState {
    pub fn new(visible: bool, pos: Option<(f32, f32)>, scale: f32) -> Self {
        Self {
            visible,
            pos,
            scale: clamp_scale(scale),
            grab: Point::ORIGIN,
            drag: false,
            resize: false,
        }
    }

    /// Top-left in window pixels, kept inside the window.
    pub fn origin(&self, win: (f32, f32), geom: Geom<'_>) -> (f32, f32) {
        let (default, size) = geom(self.scale);
        clamp_pos(self.pos.unwrap_or(default), win, size)
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        self.drag = false;
        self.resize = false;
    }

    /// Follow the pointer (window coordinates) while dragging.
    pub fn drag_to(&mut self, pointer: Point, win: (f32, f32), geom: Geom<'_>) {
        let (_, size) = geom(self.scale);
        self.pos = Some(clamp_pos(
            (pointer.x - self.grab.x, pointer.y - self.grab.y),
            win,
            size,
        ));
    }

    /// Change the size, keeping the top-left corner where it is (and the card inside the window).
    pub fn set_scale(&mut self, scale: f32, win: (f32, f32), geom: Geom<'_>) {
        let at = self.origin(win, geom);
        self.scale = clamp_scale(scale);
        let (_, size) = geom(self.scale);
        self.pos = Some(clamp_pos(at, win, size));
    }

    /// Dragging the corner grip: the pointer sets the right edge (`base_w` is the width at scale 1).
    pub fn resize_to(&mut self, pointer: Point, win: (f32, f32), geom: Geom<'_>, base_w: f32) {
        let (x, _) = self.origin(win, geom);
        self.set_scale((pointer.x - x) / base_w, win, geom);
    }

    /// Keyboard sizing: one step is 10 % of the base size.
    pub fn step_scale(&mut self, steps: f32, win: (f32, f32), geom: Geom<'_>) {
        self.set_scale(self.scale + 0.1 * steps, win, geom);
    }

    /// Mouse moved: route to a resize or drag in progress. Returns true when the card used it.
    pub fn on_pointer(
        &mut self,
        pointer: Point,
        win: (f32, f32),
        geom: Geom<'_>,
        base_w: f32,
    ) -> bool {
        if self.resize {
            self.resize_to(pointer, win, geom, base_w);
            true
        } else if self.drag {
            self.drag_to(pointer, win, geom);
            true
        } else {
            false
        }
    }

    /// Button released: ends a drag or resize. Returns true when one was in progress.
    pub fn on_release(&mut self) -> bool {
        std::mem::take(&mut self.resize) | std::mem::take(&mut self.drag)
    }

    pub fn busy(&self) -> bool {
        self.drag || self.resize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: (f32, f32) = (1000.0, 800.0);

    #[test]
    fn free_area_leaves_the_rest_of_the_window_to_the_terminal() {
        assert_eq!(free_area((1000.0, 800.0), &[]), (1000.0, 800.0));
        // A column on the right.
        let right = (800.0, 0.0, 200.0, 300.0);
        assert_eq!(free_area((1000.0, 800.0), &[right]), (800.0, 800.0));
        // Right column plus a strip at the bottom: terminal keeps the top-left block.
        let bottom = (0.0, 700.0, 1000.0, 100.0);
        assert_eq!(free_area((1000.0, 800.0), &[right, bottom]), (800.0, 700.0));
        // A card in the middle that would leave too little space: ignored (floats over the terminal).
        let mid = (10.0, 10.0, 900.0, 700.0);
        assert_eq!(free_area((1000.0, 800.0), &[mid]), (1000.0, 800.0));
    }

    fn geom(scale: f32) -> ((f32, f32), (f32, f32)) {
        ((20.0, 30.0), (200.0 * scale, 100.0 * scale))
    }

    #[test]
    fn starts_at_the_default_and_follows_a_drag_inside_the_window() {
        let mut c = CardState::new(true, None, 1.0);
        assert_eq!(c.origin(WIN, &geom), (20.0, 30.0));
        c.grab = Point::new(10.0, 5.0);
        c.drag_to(Point::new(310.0, 205.0), WIN, &geom);
        assert_eq!(c.origin(WIN, &geom), (300.0, 200.0));
        c.drag_to(Point::new(-500.0, 9000.0), WIN, &geom);
        assert_eq!(c.origin(WIN, &geom), (0.0, 700.0));
    }

    #[test]
    fn scale_changes_keep_the_corner_and_stay_in_range() {
        let mut c = CardState::new(true, Some((100.0, 100.0)), 1.0);
        c.set_scale(2.0, WIN, &geom);
        assert_eq!(c.origin(WIN, &geom), (100.0, 100.0));
        c.set_scale(99.0, WIN, &geom);
        assert_eq!(c.scale, MAX_SCALE);
        c.set_scale(0.0, WIN, &geom);
        assert_eq!(c.scale, MIN_SCALE);
        c.set_scale(f32::NAN, WIN, &geom);
        assert_eq!(c.scale, 1.0);
        // Growing near the edge pulls the card back inside.
        let mut c = CardState::new(true, Some((900.0, 700.0)), 1.0);
        c.set_scale(2.0, WIN, &geom);
        let (x, y) = c.origin(WIN, &geom);
        assert!(x + 400.0 <= 1000.0 && y + 200.0 <= 800.0);
    }

    #[test]
    fn pointer_routing_and_release() {
        let mut c = CardState::new(true, Some((100.0, 100.0)), 1.0);
        assert!(!c.on_pointer(Point::new(5.0, 5.0), WIN, &geom, 200.0));
        c.resize = true;
        assert!(c.on_pointer(Point::new(500.0, 150.0), WIN, &geom, 200.0));
        assert!(
            (c.scale - 2.0).abs() < 1e-4,
            "right edge at the pointer: (500-100)/200"
        );
        assert!(c.busy());
        assert!(c.on_release());
        assert!(!c.busy() && !c.on_release());
        c.step_scale(-3.0, WIN, &geom);
        assert!((c.scale - 1.7).abs() < 1e-4);
    }
}
