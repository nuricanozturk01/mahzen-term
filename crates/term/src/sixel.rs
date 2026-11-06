//! Sixel decoder (`DCS q ... ST`): the bitmap format of the DEC VT340, still used by `img2sixel`, `chafa` and `lsix`.
//!
//! The data comes from whatever program writes to the terminal, so every dimension is bounded while decoding (a
//! hostile `!99999999~` cannot allocate gigabytes) and malformed input yields an error, never a panic.

use std::sync::Arc;

use crate::gfx::{Image, MAX_DIM};

/// Most pixels in one decoded image.
const MAX_PIXELS: usize = 40 * 1024 * 1024;
/// Palette size (registers 0..=255).
const REGISTERS: usize = 256;

/// The 16 default VT340 registers (RGB, 0..=255).
const DEFAULT_PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0],
    [51, 51, 204],
    [204, 33, 33],
    [51, 204, 51],
    [204, 51, 204],
    [51, 204, 204],
    [204, 204, 51],
    [135, 135, 135],
    [66, 66, 66],
    [84, 84, 153],
    [153, 66, 66],
    [84, 153, 84],
    [153, 84, 153],
    [84, 153, 153],
    [153, 153, 84],
    [204, 204, 204],
];

fn percent(v: u32) -> u8 {
    (v.min(100) * 255 / 100) as u8
}

/// DEC hue 0 is blue (not red as in HSL): shift to standard HSL, then convert. All inputs are clamped.
fn hls_to_rgb(h: u32, l: u32, s: u32) -> [u8; 3] {
    let h = f64::from((h % 360 + 240) % 360) / 360.0;
    let l = f64::from(l.min(100)) / 100.0;
    let s = f64::from(s.min(100)) / 100.0;
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return [v, v, v];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    [channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0)]
}

/// A growing RGBA canvas.
struct Canvas {
    width: usize,
    height: usize,
    /// Row-major RGBA, transparent where nothing was drawn.
    px: Vec<u8>,
}

impl Canvas {
    fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            px: Vec::new(),
        }
    }

    /// Make room for the pixel at (`x`, `y`); false when the image would be too big.
    fn reserve(&mut self, x: usize, y: usize) -> bool {
        let (need_w, need_h) = (x + 1, y + 1);
        if need_w > MAX_DIM as usize || need_h > MAX_DIM as usize {
            return false;
        }
        if need_w <= self.width && need_h <= self.height {
            return true;
        }
        let (new_w, new_h) = (need_w.max(self.width), need_h.max(self.height));
        if new_w.saturating_mul(new_h) > MAX_PIXELS {
            return false;
        }
        let mut px = vec![0u8; new_w * new_h * 4];
        for row in 0..self.height {
            px[row * new_w * 4..row * new_w * 4 + self.width * 4]
                .copy_from_slice(&self.px[row * self.width * 4..(row + 1) * self.width * 4]);
        }
        self.px = px;
        self.width = new_w;
        self.height = new_h;
        true
    }

    fn set(&mut self, x: usize, y: usize, rgb: [u8; 3]) -> bool {
        if !self.reserve(x, y) {
            return false;
        }
        let i = (y * self.width + x) * 4;
        self.px[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        true
    }
}

/// Parse numbers separated by `;` at the start of `data[at..]`; returns them and the index after them.
fn numbers(data: &[u8], mut at: usize) -> (Vec<u32>, usize) {
    let mut out = Vec::new();
    let mut cur: Option<u32> = None;
    while at < data.len() {
        match data[at] {
            b @ b'0'..=b'9' => {
                cur = Some(
                    cur.unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(u32::from(b - b'0'))
                        .min(1_000_000),
                );
            }
            b';' => out.push(cur.take().unwrap_or(0)),
            _ => break,
        }
        at += 1;
    }
    if let Some(c) = cur {
        out.push(c);
    }
    (out, at)
}

/// Draw `bits` (six pixels, bit 0 on top) `repeat` times from column `*x` of band `band`.
fn draw(
    canvas: &mut Canvas,
    palette: &[[u8; 3]; REGISTERS],
    x: &mut usize,
    band: usize,
    color: usize,
    bits: u8,
    repeat: usize,
) -> Result<(), String> {
    // A band is always six rows tall, and blank columns still count towards the width.
    if repeat > 0 && !canvas.reserve(*x + repeat - 1, band * 6 + 5) {
        return Err("image is too large".into());
    }
    for _ in 0..repeat {
        for bit in 0..6 {
            if bits & (1 << bit) != 0 && !canvas.set(*x, band * 6 + bit, palette[color]) {
                return Err("image is too large".into());
            }
        }
        *x += 1;
    }
    Ok(())
}

/// Decode the DCS payload: the text after `ESC P`, i.e. `params q data`.
pub fn decode(payload: &[u8]) -> Result<Arc<Image>, String> {
    // Skip the DCS parameters up to the final `q`.
    let q = payload
        .iter()
        .position(|b| !(b.is_ascii_digit() || *b == b';'))
        .filter(|i| payload[*i] == b'q')
        .ok_or("not a sixel sequence")?;
    let data = &payload[q + 1..];
    let mut palette = [[0u8; 3]; REGISTERS];
    palette[..16].copy_from_slice(&DEFAULT_PALETTE);
    let mut canvas = Canvas::new();
    let (mut x, mut band, mut color) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'"' => {
                // Raster attributes `Pan;Pad;Ph;Pv`: reserve the declared size so the image keeps it.
                let (n, next) = numbers(data, i + 1);
                i = next;
                if let (Some(&w), Some(&h)) = (n.get(2), n.get(3)) {
                    if w > 0 && h > 0 && !canvas.reserve(w as usize - 1, h as usize - 1) {
                        return Err("image is too large".into());
                    }
                }
            }
            b'#' => {
                let (n, next) = numbers(data, i + 1);
                i = next;
                let reg = n.first().copied().unwrap_or(0) as usize % REGISTERS;
                if n.len() >= 5 {
                    palette[reg] = match n[1] {
                        2 => [percent(n[2]), percent(n[3]), percent(n[4])],
                        1 => hls_to_rgb(n[2], n[3], n[4]),
                        _ => palette[reg],
                    };
                }
                color = reg;
            }
            b'!' => {
                let (n, next) = numbers(data, i + 1);
                i = next;
                let count = n.first().copied().unwrap_or(1).max(1) as usize;
                let ch = *data.get(i).ok_or("sixel data ends inside a repeat")?;
                if !(b'?'..=b'~').contains(&ch) {
                    return Err("bad repeat in sixel data".into());
                }
                if x.saturating_add(count) > MAX_DIM as usize {
                    return Err("image is too large".into());
                }
                draw(&mut canvas, &palette, &mut x, band, color, ch - b'?', count)?;
                i += 1;
            }
            b'$' => {
                x = 0;
                i += 1;
            }
            b'-' => {
                x = 0;
                band += 1;
                if band * 6 >= MAX_DIM as usize {
                    return Err("image is too large".into());
                }
                i += 1;
            }
            ch @ b'?'..=b'~' => {
                draw(&mut canvas, &palette, &mut x, band, color, ch - b'?', 1)?;
                i += 1;
            }
            _ => i += 1, // whitespace and unknown bytes are ignored
        }
    }
    if canvas.width == 0 || canvas.height == 0 {
        return Err("empty sixel image".into());
    }
    Image::new(canvas.width as u32, canvas.height as u32, canvas.px)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &Image, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        [
            img.rgba[i],
            img.rgba[i + 1],
            img.rgba[i + 2],
            img.rgba[i + 3],
        ]
    }

    #[test]
    fn a_solid_band_with_a_defined_colour() {
        // 4 wide, 6 tall, pure green.
        let img = decode(b"0;1;0q\"1;1;4;6#1;2;0;100;0#1~~~~").unwrap();
        assert_eq!((img.width, img.height), (4, 6));
        for y in 0..6 {
            for x in 0..4 {
                assert_eq!(px(&img, x, y), [0, 255, 0, 255]);
            }
        }
    }

    #[test]
    fn bits_select_rows_and_unset_pixels_stay_transparent() {
        // `@` = only the top row (bit 0), `A` = second row. Registers 1 red, 2 blue.
        let img = decode(b"q#1;2;100;0;0#2;2;0;0;100#1@#2A").unwrap();
        assert_eq!((img.width, img.height), (2, 6));
        assert_eq!(px(&img, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&img, 0, 1)[3], 0, "not drawn by the red pass");
        assert_eq!(px(&img, 1, 1), [0, 0, 255, 255]);
        assert_eq!(px(&img, 1, 0)[3], 0);
    }

    #[test]
    fn repeats_carriage_returns_and_new_bands() {
        // Band 0: red x4 via a repeat; `$` returns and draws blue over the first two columns; `-` starts band 1.
        let img = decode(b"q#1;2;100;0;0#2;2;0;0;100#1!4~$#2!2~-#1!4~").unwrap();
        assert_eq!((img.width, img.height), (4, 12));
        assert_eq!(px(&img, 0, 3), [0, 0, 255, 255], "blue overwrote red");
        assert_eq!(px(&img, 3, 3), [255, 0, 0, 255]);
        assert_eq!(px(&img, 0, 9), [255, 0, 0, 255], "second band");
    }

    #[test]
    fn the_default_palette_and_hls_work() {
        let img = decode(b"q#2~").unwrap();
        assert_eq!(&px(&img, 0, 0)[..3], &[204, 33, 33], "default register 2");
        // HLS: DEC hue 120 is red; full saturation at half lightness.
        let img = decode(b"q#5;1;120;50;100~").unwrap();
        let p = px(&img, 0, 0);
        assert!(p[0] > 240 && p[1] < 15 && p[2] < 15, "{p:?}");
        assert_eq!(hls_to_rgb(0, 50, 0), [128, 128, 128]);
    }

    #[test]
    fn raster_attributes_reserve_the_declared_size() {
        let img = decode(b"q\"1;1;10;12#1;2;100;100;100#1~").unwrap();
        assert_eq!((img.width, img.height), (10, 12));
    }

    #[test]
    fn hostile_input_is_refused_without_big_allocations() {
        for bad in [
            &b"q!99999999~"[..],
            b"q\"1;1;100000;100000",
            b"q!",
            b"q!5\x01",
            b"",
            b"q",
            b"0;0;0",
            b"q#1;2;0;0;0",
        ] {
            assert!(decode(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        // Many bands: bounded by the height limit.
        let mut many = b"q#1;2;100;0;0".to_vec();
        for _ in 0..2000 {
            many.extend_from_slice(b"~-");
        }
        assert!(decode(&many).is_err());
        // Out-of-range numbers and registers are clamped, junk bytes ignored.
        assert!(decode(b"q#999;2;500;500;500\x00\x01 ~").is_ok());
    }
}
