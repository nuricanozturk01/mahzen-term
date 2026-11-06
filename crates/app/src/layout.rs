//! Pixel layout of one tab: maps the core's normalized pane tree to rectangles and draggable dividers.
//!
//! Panes tile the content area edge to edge (one continuous surface); neighbours are separated by a 1 px divider
//! drawn on the shared border, with a wider invisible hit area for dragging. Pure functions, unit-tested.

use iced::{Point, Rectangle, Size};
use mahzenterm_core::workspace::{Axis, PaneId, Rect, SplitInfo, Tab};

/// Visible divider thickness.
pub const DIVIDER: f32 = 1.0;
/// Draggable strip centred on a divider.
pub const SASH_HIT: f32 = 9.0;
/// Space between a pane's edge and its terminal grid.
pub const PAD_X: f32 = 12.0;
pub const PAD_Y: f32 = 8.0;
/// Height of the thin title strip shown on every pane while a tab has several.
pub const STRIP_H: f32 = 28.0;

/// Pixel rectangles of one pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanePx {
    pub id: PaneId,
    /// The pane's share of the content area; neighbours touch.
    pub outer: Rectangle,
    /// Area available to the terminal grid.
    pub body: Rectangle,
}

/// A draggable divider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SashPx {
    pub a_first: PaneId,
    pub b_first: PaneId,
    pub axis: Axis,
    /// The visible 1 px line.
    pub line: Rectangle,
    /// Invisible drag area around the line.
    pub hit: Rectangle,
    /// Pixel area of the whole split; dragging maps the pointer into this.
    pub split: Rectangle,
}

fn to_px(r: Rect, area: Size) -> Rectangle {
    Rectangle {
        x: (r.x * area.width).round(),
        y: (r.y * area.height).round(),
        width: (r.w * area.width).round(),
        height: (r.h * area.height).round(),
    }
}

/// Whether panes carry a title strip: only when there is more than one pane to tell apart.
pub fn show_strips(tab: &Tab) -> bool {
    tab.pane_ids().len() > 1
}

/// Pane rectangles for the tab inside `area`.
pub fn panes(tab: &Tab, area: Size) -> Vec<PanePx> {
    let top = if show_strips(tab) { STRIP_H } else { PAD_Y };
    tab.layout()
        .into_iter()
        .map(|(id, r)| {
            let outer = to_px(r, area);
            let body = Rectangle {
                x: outer.x + PAD_X,
                y: outer.y + top,
                width: (outer.width - 2.0 * PAD_X).max(1.0),
                height: (outer.height - top - PAD_Y).max(1.0),
            };
            PanePx { id, outer, body }
        })
        .collect()
}

/// Dividers for the tab inside `area`.
pub fn sashes(tab: &Tab, area: Size) -> Vec<SashPx> {
    tab.splits()
        .into_iter()
        .map(|s: SplitInfo| {
            let split = to_px(s.rect, area);
            let (line, hit) = match s.axis {
                Axis::Horizontal => {
                    let x = (split.x + split.width * s.ratio).round();
                    (
                        Rectangle {
                            x: x - DIVIDER / 2.0,
                            y: split.y,
                            width: DIVIDER,
                            height: split.height,
                        },
                        Rectangle {
                            x: x - SASH_HIT / 2.0,
                            y: split.y,
                            width: SASH_HIT,
                            height: split.height,
                        },
                    )
                }
                Axis::Vertical => {
                    let y = (split.y + split.height * s.ratio).round();
                    (
                        Rectangle {
                            x: split.x,
                            y: y - DIVIDER / 2.0,
                            width: split.width,
                            height: DIVIDER,
                        },
                        Rectangle {
                            x: split.x,
                            y: y - SASH_HIT / 2.0,
                            width: split.width,
                            height: SASH_HIT,
                        },
                    )
                }
            };
            SashPx {
                a_first: s.a_first,
                b_first: s.b_first,
                axis: s.axis,
                line,
                hit,
                split,
            }
        })
        .collect()
}

/// Ratio for the pointer position `p` (in the same coordinates as `split`) when dragging a divider.
pub fn ratio_at(axis: Axis, split: Rectangle, p: Point) -> f32 {
    match axis {
        Axis::Horizontal => (p.x - split.x) / split.width.max(1.0),
        Axis::Vertical => (p.y - split.y) / split.height.max(1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mahzenterm_core::workspace::{PaneKind, TabId};

    fn area() -> Size {
        Size::new(1000.0, 600.0)
    }

    fn overlap(a: Rectangle, b: Rectangle) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    #[test]
    fn single_pane_fills_the_whole_area() {
        let tab = Tab::new(TabId(0), PaneKind::terminal());
        let p = panes(&tab, area());
        assert_eq!(p.len(), 1);
        assert_eq!(
            p[0].outer,
            Rectangle {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 600.0
            }
        );
        assert_eq!(p[0].body.x, PAD_X);
    }

    #[test]
    fn strips_appear_only_with_several_panes_and_reserve_their_height() {
        let mut tab = Tab::new(TabId(0), PaneKind::terminal());
        assert!(!show_strips(&tab));
        let single = panes(&tab, area())[0];
        assert_eq!(single.body.y, single.outer.y + PAD_Y);
        tab.split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        assert!(show_strips(&tab));
        for p in panes(&tab, area()) {
            assert_eq!(p.body.y, p.outer.y + STRIP_H);
            assert!(p.body.y + p.body.height <= p.outer.y + p.outer.height);
        }
        // Zoomed panes keep their strip (there are still several panes in the tab).
        tab.toggle_zoom(PaneId(0)).unwrap();
        assert_eq!(panes(&tab, area())[0].body.y, STRIP_H);
    }

    #[test]
    fn split_panes_touch_without_gap_or_overlap() {
        let mut tab = Tab::new(TabId(0), PaneKind::terminal());
        tab.split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        let p = panes(&tab, area());
        assert!(!overlap(p[0].outer, p[1].outer));
        assert_eq!(
            p[0].outer.x + p[0].outer.width,
            p[1].outer.x,
            "no gap between panes"
        );
        assert_eq!(p[0].outer.width + p[1].outer.width, 1000.0);
    }

    #[test]
    fn divider_line_sits_on_the_shared_border_inside_its_hit_area() {
        let mut tab = Tab::new(TabId(0), PaneKind::terminal());
        tab.split(PaneId(0), Axis::Vertical, PaneKind::terminal())
            .unwrap();
        let p = panes(&tab, area());
        let s = sashes(&tab, area());
        assert_eq!(s.len(), 1);
        let border_y = p[0].outer.y + p[0].outer.height;
        assert!((s[0].line.y + DIVIDER / 2.0 - border_y).abs() < 1e-3);
        assert!(s[0].hit.y < s[0].line.y && s[0].hit.y + s[0].hit.height > s[0].line.y + DIVIDER);
        assert_eq!(s[0].line.width, 1000.0);
    }

    #[test]
    fn terminal_grids_stay_clear_of_the_drag_area() {
        let mut tab = Tab::new(TabId(0), PaneKind::terminal());
        tab.split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        let p = panes(&tab, area());
        let s = sashes(&tab, area());
        assert!(!overlap(s[0].hit, p[0].body));
        assert!(!overlap(s[0].hit, p[1].body));
    }

    #[test]
    fn ratio_follows_pointer_inside_split() {
        let split = Rectangle {
            x: 100.0,
            y: 0.0,
            width: 400.0,
            height: 300.0,
        };
        let r = ratio_at(Axis::Horizontal, split, Point::new(200.0, 50.0));
        assert!((r - 0.25).abs() < 1e-6);
        let r = ratio_at(Axis::Vertical, split, Point::new(0.0, 150.0));
        assert!((r - 0.5).abs() < 1e-6);
    }

    #[test]
    fn zoomed_tab_has_one_pane_and_no_dividers() {
        let mut tab = Tab::new(TabId(0), PaneKind::terminal());
        let b = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        tab.toggle_zoom(b).unwrap();
        assert_eq!(panes(&tab, area()).len(), 1);
        assert!(sashes(&tab, area()).is_empty());
    }
}
