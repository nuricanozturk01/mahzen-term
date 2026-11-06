//! Built-in rendering of Box Drawing (U+2500..257F) and Block Elements (U+2580..259F).
//!
//! Fonts rarely align these glyphs to the cell grid, leaving gaps between neighbours. Drawing them as
//! rectangles makes lines join seamlessly at any font size.

/// A filled rectangle in cell-local pixels with an opacity (for shade characters).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub alpha: f32,
}

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect {
        x,
        y,
        w,
        h,
        alpha: 1.0,
    }
}

/// (left, right, up, down) arm weights: 0 none, 1 light, 2 heavy, 3 double.
fn arms(ch: char) -> Option<(u8, u8, u8, u8)> {
    const T: [(u8, u8, u8, u8); 0x80] = [
        (1, 1, 0, 0),
        (2, 2, 0, 0),
        (0, 0, 1, 1),
        (0, 0, 2, 2),
        (1, 1, 0, 0),
        (2, 2, 0, 0),
        (0, 0, 1, 1),
        (0, 0, 2, 2),
        (1, 1, 0, 0),
        (2, 2, 0, 0),
        (0, 0, 1, 1),
        (0, 0, 2, 2),
        (0, 1, 0, 1),
        (0, 2, 0, 1),
        (0, 1, 0, 2),
        (0, 2, 0, 2),
        (1, 0, 0, 1),
        (2, 0, 0, 1),
        (1, 0, 0, 2),
        (2, 0, 0, 2),
        (0, 1, 1, 0),
        (0, 2, 1, 0),
        (0, 1, 2, 0),
        (0, 2, 2, 0),
        (1, 0, 1, 0),
        (2, 0, 1, 0),
        (1, 0, 2, 0),
        (2, 0, 2, 0),
        (0, 1, 1, 1),
        (0, 2, 1, 1),
        (0, 1, 2, 1),
        (0, 1, 1, 2),
        (0, 1, 2, 2),
        (0, 2, 2, 1),
        (0, 2, 1, 2),
        (0, 2, 2, 2),
        (1, 0, 1, 1),
        (2, 0, 1, 1),
        (1, 0, 2, 1),
        (1, 0, 1, 2),
        (1, 0, 2, 2),
        (2, 0, 2, 1),
        (2, 0, 1, 2),
        (2, 0, 2, 2),
        (1, 1, 0, 1),
        (2, 1, 0, 1),
        (1, 2, 0, 1),
        (2, 2, 0, 1),
        (1, 1, 0, 2),
        (2, 1, 0, 2),
        (1, 2, 0, 2),
        (2, 2, 0, 2),
        (1, 1, 1, 0),
        (2, 1, 1, 0),
        (1, 2, 1, 0),
        (2, 2, 1, 0),
        (1, 1, 2, 0),
        (2, 1, 2, 0),
        (1, 2, 2, 0),
        (2, 2, 2, 0),
        (1, 1, 1, 1),
        (2, 1, 1, 1),
        (1, 2, 1, 1),
        (2, 2, 1, 1),
        (1, 1, 2, 1),
        (1, 1, 1, 2),
        (1, 1, 2, 2),
        (2, 1, 2, 1),
        (1, 2, 2, 1),
        (2, 1, 1, 2),
        (1, 2, 1, 2),
        (2, 2, 2, 1),
        (2, 2, 1, 2),
        (2, 1, 2, 2),
        (1, 2, 2, 2),
        (2, 2, 2, 2),
        (1, 1, 0, 0),
        (2, 2, 0, 0),
        (0, 0, 1, 1),
        (0, 0, 2, 2),
        (3, 3, 0, 0),
        (0, 0, 3, 3),
        (0, 3, 0, 1),
        (0, 1, 0, 3),
        (0, 3, 0, 3),
        (3, 0, 0, 1),
        (1, 0, 0, 3),
        (3, 0, 0, 3),
        (0, 3, 1, 0),
        (0, 1, 3, 0),
        (0, 3, 3, 0),
        (3, 0, 1, 0),
        (1, 0, 3, 0),
        (3, 0, 3, 0),
        (0, 3, 1, 1),
        (0, 1, 3, 3),
        (0, 3, 3, 3),
        (3, 0, 1, 1),
        (1, 0, 3, 3),
        (3, 0, 3, 3),
        (3, 3, 0, 1),
        (1, 1, 0, 3),
        (3, 3, 0, 3),
        (3, 3, 1, 0),
        (1, 1, 3, 0),
        (3, 3, 3, 0),
        (3, 3, 1, 1),
        (1, 1, 3, 3),
        (3, 3, 3, 3),
        (0, 1, 0, 1),
        (1, 0, 0, 1),
        (1, 0, 1, 0),
        (0, 1, 1, 0),
        (0, 0, 0, 0),
        (0, 0, 0, 0),
        (0, 0, 0, 0),
        (1, 0, 0, 0),
        (0, 0, 1, 0),
        (0, 1, 0, 0),
        (0, 0, 0, 1),
        (2, 0, 0, 0),
        (0, 0, 2, 0),
        (0, 2, 0, 0),
        (0, 0, 0, 2),
        (1, 2, 0, 0),
        (0, 0, 1, 2),
        (2, 1, 0, 0),
        (0, 0, 2, 1),
    ];
    let code = ch as u32;
    if !(0x2500..=0x257F).contains(&code) {
        return None;
    }
    let a = T[(code - 0x2500) as usize];
    // U+2571..2573 are diagonals: left to the font.
    (a != (0, 0, 0, 0)).then_some(a)
}

/// Pixel extent of an arm perpendicular to its direction.
fn extent(weight: u8, t: f32) -> f32 {
    match weight {
        0 => 0.0,
        1 => t,
        2 => t * 2.0,
        _ => t * 3.0,
    }
}

/// Strips (offset, size) across the line axis for a weight, centred on `center`.
fn strips(weight: u8, t: f32, center: f32) -> Vec<(f32, f32)> {
    let e = extent(weight, t);
    let start = (center - (e / 2.0).floor()).floor();
    match weight {
        0 => vec![],
        3 => vec![(start, t), (start + 2.0 * t, t)],
        _ => vec![(start, e)],
    }
}

/// Rectangles for a built-in glyph in a `cw` x `chh` pixel cell, or `None` to use the font.
pub fn builtin_glyph(ch: char, cw: f32, chh: f32) -> Option<Vec<Rect>> {
    if let Some((l, rt, u, d)) = arms(ch) {
        let t = (chh / 14.0).floor().max(1.0);
        let cx = (cw / 2.0).floor();
        let cy = (chh / 2.0).floor();
        let vext = extent(u.max(d), t);
        let hext = extent(l.max(rt), t);
        let mut out = Vec::new();
        // Horizontal arms.
        for (weight, x0, x1) in [
            (l, 0.0, cx + (vext / 2.0).ceil()),
            (rt, cx - (vext / 2.0).floor(), cw),
        ] {
            for (y, h) in strips(weight, t, cy) {
                out.push(r(x0, y, x1 - x0, h));
            }
        }
        // Vertical arms.
        for (weight, y0, y1) in [
            (u, 0.0, cy + (hext / 2.0).ceil()),
            (d, cy - (hext / 2.0).floor(), chh),
        ] {
            for (x, w) in strips(weight, t, cx) {
                out.push(r(x, y0, w, y1 - y0));
            }
        }
        return Some(out);
    }

    let code = ch as u32;
    let hw = (cw / 2.0).round();
    let hh = (chh / 2.0).round();
    let eighth_w = |n: f32| (cw * n / 8.0).round();
    let eighth_h = |n: f32| (chh * n / 8.0).round();
    let quad = |ul: bool, ur: bool, ll: bool, lr: bool| {
        let mut v = Vec::new();
        if ul {
            v.push(r(0.0, 0.0, hw, hh));
        }
        if ur {
            v.push(r(hw, 0.0, cw - hw, hh));
        }
        if ll {
            v.push(r(0.0, hh, hw, chh - hh));
        }
        if lr {
            v.push(r(hw, hh, cw - hw, chh - hh));
        }
        v
    };
    Some(match code {
        0x2580 => vec![r(0.0, 0.0, cw, hh)],
        0x2581..=0x2588 => {
            let h = eighth_h((code - 0x2580) as f32);
            vec![r(0.0, chh - h, cw, h)]
        }
        0x2589..=0x258F => vec![r(0.0, 0.0, eighth_w(8.0 - (code - 0x2588) as f32), chh)],
        0x2590 => vec![r(hw, 0.0, cw - hw, chh)],
        0x2591..=0x2593 => {
            let alpha = (code - 0x2590) as f32 * 0.25;
            vec![Rect {
                x: 0.0,
                y: 0.0,
                w: cw,
                h: chh,
                alpha,
            }]
        }
        0x2594 => vec![r(0.0, 0.0, cw, eighth_h(1.0))],
        0x2595 => vec![r(cw - eighth_w(1.0), 0.0, eighth_w(1.0), chh)],
        0x2596 => quad(false, false, true, false),
        0x2597 => quad(false, false, false, true),
        0x2598 => quad(true, false, false, false),
        0x2599 => quad(true, false, true, true),
        0x259A => quad(true, false, false, true),
        0x259B => quad(true, true, true, false),
        0x259C => quad(true, true, false, true),
        0x259D => quad(false, true, false, false),
        0x259E => quad(false, true, true, false),
        0x259F => quad(false, true, true, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 9.0;
    const H: f32 = 20.0;

    fn covered(rects: &[Rect], x: f32, y: f32) -> bool {
        rects
            .iter()
            .any(|q| x >= q.x && x < q.x + q.w && y >= q.y && y < q.y + q.h)
    }

    fn glyph(c: char) -> Vec<Rect> {
        builtin_glyph(c, W, H).unwrap_or_else(|| panic!("no builtin for {c}"))
    }

    #[test]
    fn horizontal_line_spans_whole_cell_width_without_gap() {
        let g = glyph('─');
        for x in 0..9 {
            assert!(covered(&g, x as f32 + 0.5, H / 2.0), "gap at x={x}");
        }
        assert!(!covered(&g, 4.0, 2.0));
    }

    #[test]
    fn vertical_line_spans_whole_cell_height() {
        let g = glyph('│');
        for y in 0..20 {
            assert!(covered(&g, 4.5, y as f32 + 0.5), "gap at y={y}");
        }
    }

    #[test]
    fn corner_joins_its_two_arms() {
        let g = glyph('┌');
        assert!(covered(&g, 8.5, 10.0), "right arm end");
        assert!(covered(&g, 4.5, 19.5), "down arm end");
        assert!(covered(&g, 4.5, 10.0), "junction");
        assert!(!covered(&g, 0.5, 10.0), "no left arm");
        assert!(!covered(&g, 4.5, 0.5), "no up arm");
    }

    #[test]
    fn cross_covers_all_four_edges() {
        let g = glyph('┼');
        assert!(covered(&g, 0.5, 10.0) && covered(&g, 8.5, 10.0));
        assert!(covered(&g, 4.5, 0.5) && covered(&g, 4.5, 19.5));
    }

    #[test]
    fn heavy_is_thicker_than_light() {
        let thickness = |c: char| {
            let g = glyph(c);
            (0..20)
                .filter(|&y| covered(&g, 4.5, y as f32 + 0.5))
                .count()
        };
        assert!(thickness('━') > thickness('─'));
    }

    #[test]
    fn double_line_has_two_strips_with_a_gap() {
        let g = glyph('═');
        let on: Vec<bool> = (0..20).map(|y| covered(&g, 4.5, y as f32 + 0.5)).collect();
        let runs = on.windows(2).filter(|w| !w[0] && w[1]).count() + usize::from(on[0]);
        assert_eq!(runs, 2, "{on:?}");
    }

    #[test]
    fn block_elements_cover_expected_area() {
        assert!(covered(&glyph('█'), 0.5, 0.5) && covered(&glyph('█'), 8.5, 19.5));
        let upper = glyph('▀');
        assert!(covered(&upper, 4.0, 2.0) && !covered(&upper, 4.0, 15.0));
        let lower = glyph('▄');
        assert!(!covered(&lower, 4.0, 2.0) && covered(&lower, 4.0, 15.0));
        let left = glyph('▌');
        assert!(covered(&left, 1.0, 10.0) && !covered(&left, 8.0, 10.0));
    }

    #[test]
    fn shades_use_opacity_and_quadrants_are_distinct() {
        assert_eq!(glyph('░')[0].alpha, 0.25);
        assert_eq!(glyph('▒')[0].alpha, 0.5);
        assert_eq!(glyph('▓')[0].alpha, 0.75);
        let ul = glyph('▘');
        assert!(covered(&ul, 1.0, 1.0) && !covered(&ul, 8.0, 18.0));
        let lr = glyph('▗');
        assert!(covered(&lr, 8.0, 18.0) && !covered(&lr, 1.0, 1.0));
    }

    #[test]
    fn ordinary_text_and_diagonals_use_the_font() {
        assert!(builtin_glyph('a', W, H).is_none());
        assert!(builtin_glyph('╱', W, H).is_none());
        assert!(builtin_glyph('日', W, H).is_none());
    }
}
