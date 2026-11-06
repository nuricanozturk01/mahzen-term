//! UI colour tokens. Widgets take colours only from here, never from literals (CLAUDE.md UX rules).
//!
//! The theme engine (M2.2) will load these from TOML; until then `Tokens::dark()` is the built-in set.

use iced::Color;

/// Semantic colours for the application chrome.
///
/// The chrome is derived from the terminal's own background and foreground so panes, tabs and dividers read as
/// one surface instead of separate windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    /// Content surface: identical to the terminal background (also the active tab).
    pub bg: Color,
    /// Tab bar: slightly darker than the content so the active tab merges into it.
    pub surface: Color,
    /// Hovered controls and inactive-pane tint.
    pub surface_hi: Color,
    /// Dividers between panes.
    pub border: Color,
    /// Focus marker and active separator.
    pub accent: Color,
    /// Primary text.
    pub text: Color,
    /// Secondary text (hints, inactive tabs).
    pub text_dim: Color,
    /// Destructive hover (close buttons).
    pub danger: Color,
    /// Healthy state dots (live session).
    pub ok: Color,
    /// Attention (not an error).
    pub warn: Color,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// Linear blend: `t = 0.0` gives `a`, `t = 1.0` gives `b`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

impl Tokens {
    /// Derive the chrome from the terminal's background and foreground.
    /// Secondary text is brightened as far as needed to reach WCAG AA (4.5:1) on every chrome surface.
    pub fn from_terminal(
        bg: Color,
        fg: Color,
        accent: Color,
        danger: Color,
        ok: Color,
        warn: Color,
    ) -> Self {
        // Dark themes get a clearly darker bar; light themes only a slight tint so text stays readable on it.
        let surface = mix(
            bg,
            Color::BLACK,
            if luminance(bg) > 0.5 { 0.06 } else { 0.28 },
        );
        let surface_hi = mix(bg, fg, 0.09);
        let mut text_dim = mix(bg, fg, 0.72);
        let mut t: f32 = 0.72;
        while t < 1.0
            && [bg, surface, surface_hi]
                .iter()
                .any(|s| contrast(text_dim, *s) < 4.5)
        {
            t += 0.04;
            text_dim = mix(bg, fg, t.min(1.0));
        }
        Self {
            bg,
            surface,
            surface_hi,
            border: mix(bg, fg, 0.16),
            accent,
            text: fg,
            text_dim,
            danger,
            ok,
            warn,
        }
    }

    /// Built-in dark set, matching the default terminal palette.
    pub const fn dark() -> Self {
        // Precomputed `from_terminal` of the default theme; a test keeps them in sync.
        Self {
            bg: rgb(0x09, 0x0e, 0x15),
            surface: rgb(0x06, 0x0a, 0x0f),
            surface_hi: rgb(0x1b, 0x20, 0x28),
            border: rgb(0x29, 0x2e, 0x36),
            accent: rgb(0x2e, 0xe6, 0xd6),
            text: rgb(0xcf, 0xd8, 0xe3),
            text_dim: rgb(0x98, 0x9f, 0xa9),
            danger: rgb(0xff, 0x6b, 0x81),
            ok: rgb(0x5b, 0xe3, 0xa4),
            warn: rgb(0xf6, 0xc7, 0x6b),
        }
    }
}

impl Default for Tokens {
    fn default() -> Self {
        Self::dark()
    }
}

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: Color) -> f32 {
    let lin = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio between two colours (1.0 to 21.0).
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_known_values() {
        let c = contrast(Color::BLACK, Color::WHITE);
        assert!((c - 21.0).abs() < 0.01);
        assert!((contrast(Color::WHITE, Color::WHITE) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn dark_matches_its_derivation() {
        let d = Tokens::dark();
        let f = Tokens::from_terminal(d.bg, d.text, d.accent, d.danger, d.ok, d.warn);
        let close = |a: Color, b: Color| {
            (a.r - b.r).abs() < 0.004 && (a.g - b.g).abs() < 0.004 && (a.b - b.b).abs() < 0.004
        };
        assert!(
            close(d.surface, f.surface),
            "{:?} vs {:?}",
            d.surface,
            f.surface
        );
        assert!(close(d.surface_hi, f.surface_hi), "surface_hi");
        assert!(close(d.border, f.border), "border");
        assert!(close(d.text_dim, f.text_dim), "text_dim");
    }

    #[test]
    fn dark_text_meets_wcag_aa_on_every_surface() {
        let t = Tokens::dark();
        for bg in [t.bg, t.surface, t.surface_hi] {
            assert!(contrast(t.text, bg) >= 4.5, "text on {bg:?}");
            assert!(contrast(t.text_dim, bg) >= 4.5, "dim text on {bg:?}");
        }
        assert!(contrast(t.accent, t.surface) >= 4.5, "accent on surface");
        assert!(contrast(t.danger, t.surface_hi) >= 4.5, "danger on raised");
    }
}
