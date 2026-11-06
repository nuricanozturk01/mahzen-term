//! Color palette and resolution of VT colors to RGB.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb as VteRgb};

/// 8-bit RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<VteRgb> for Rgb {
    fn from(c: VteRgb) -> Self {
        Rgb(c.r, c.g, c.b)
    }
}

impl Rgb {
    /// Scale brightness by `num/den` (used for DIM text).
    pub fn scaled(self, num: u32, den: u32) -> Rgb {
        let f = |v: u8| ((u32::from(v) * num) / den).min(255) as u8;
        Rgb(f(self.0), f(self.1), f(self.2))
    }
}

/// Terminal color theme: default fg/bg/cursor and the 16 ANSI colors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub ansi: [Rgb; 16],
}

impl Default for Palette {
    fn default() -> Self {
        let h = |v: u32| Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        Palette {
            fg: h(0xcfd8e3),
            bg: h(0x090e15),
            cursor: h(0x2ee6d6),
            ansi: [
                h(0x1a212c),
                h(0xff6b81),
                h(0x5be3a4),
                h(0xf6c76b),
                h(0x5aa9ff),
                h(0xc58bff),
                h(0x2ee6d6),
                h(0xb8c4d2),
                h(0x3a4656),
                h(0xff8fa0),
                h(0x86f0bf),
                h(0xffd98e),
                h(0x86c1ff),
                h(0xd9afff),
                h(0x7af5ea),
                h(0xeef3f9),
            ],
        }
    }
}

impl Palette {
    /// xterm 256-color index (0..=255) to RGB.
    pub fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..=15 => self.ansi[usize::from(i)],
            16..=231 => {
                let n = i - 16;
                let lvl = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                Rgb(lvl(n / 36), lvl((n / 6) % 6), lvl(n % 6))
            }
            232..=255 => {
                let g = 8 + 10 * (i - 232);
                Rgb(g, g, g)
            }
        }
    }

    /// Resolve a VT color; `overrides` are colors set at runtime via OSC 4/10/11.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(c) => c.into(),
            Color::Indexed(i) => overrides[usize::from(i)]
                .map(Rgb::from)
                .unwrap_or_else(|| self.indexed(i)),
            Color::Named(n) => {
                let idx = n as usize;
                if let Some(c) = overrides[idx] {
                    return c.into();
                }
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::DimForeground => self.fg.scaled(2, 3),
                    NamedColor::Background => self.bg,
                    NamedColor::Cursor => self.cursor,
                    _ if idx < 16 => self.ansi[idx],
                    // Dim variants of the first eight colors.
                    _ => self.ansi[(idx - NamedColor::DimBlack as usize).min(7)].scaled(2, 3),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_grayscale_match_xterm() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), Rgb(0, 0, 0));
        assert_eq!(p.indexed(196), Rgb(255, 0, 0));
        assert_eq!(p.indexed(231), Rgb(255, 255, 255));
        assert_eq!(p.indexed(232), Rgb(8, 8, 8));
        assert_eq!(p.indexed(255), Rgb(238, 238, 238));
    }

    #[test]
    fn named_colors_resolve_to_palette() {
        let p = Palette::default();
        let o = Colors::default();
        assert_eq!(p.resolve(Color::Named(NamedColor::Foreground), &o), p.fg);
        assert_eq!(p.resolve(Color::Named(NamedColor::Red), &o), p.ansi[1]);
        assert_eq!(
            p.resolve(Color::Spec(VteRgb { r: 1, g: 2, b: 3 }), &o),
            Rgb(1, 2, 3)
        );
    }
}
