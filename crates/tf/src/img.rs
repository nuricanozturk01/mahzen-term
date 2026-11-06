//! `tf img`: show image files in the terminal through the Kitty graphics protocol.
//!
//! Any format the `image` crate reads is decoded here and re-encoded as PNG, so the terminal only ever needs to
//! understand PNG. The terminal fits an image wider than the pane to the pane's width by itself.

use std::io::Cursor;
use std::path::Path;

use base64::Engine;

/// Largest input file accepted, in bytes.
const MAX_FILE: u64 = 64 * 1024 * 1024;
/// Largest side accepted, in pixels (the terminal limits itself to 8192).
const MAX_SIDE: u32 = 16_384;
/// Bytes of base64 per escape sequence, as the protocol recommends.
const CHUNK: usize = 4096;

/// What was decoded from a file.
#[derive(Debug)]
pub struct Loaded {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// Decode `bytes` (any supported format) and re-encode as PNG. Images above the terminal's limit are scaled down.
pub fn load(bytes: &[u8], name: &str) -> Result<Loaded, String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(format!("{name}: larger than 64 MiB"));
    }
    if bytes.starts_with(b"<?xml") || bytes.windows(4).take(200).any(|w| w == b"<svg") {
        return Err(format!("{name}: SVG is not supported yet"));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("{name}: {e}"))?;
    // Some formats (TGA) have no signature to recognise: fall back to the file extension.
    if reader.format().is_none() {
        if let Ok(format) = image::ImageFormat::from_path(name) {
            reader.set_format(format);
        }
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    reader.limits(limits);
    let mut img = reader
        .decode()
        .map_err(|e| format!("{name}: not an image this build can read ({e})"))?;
    // The terminal accepts at most 8192 px per side: shrink instead of failing.
    if img.width() > 8192 || img.height() > 8192 {
        img = img.resize(8192, 8192, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    let mut png = Vec::new();
    rgba.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("{name}: {e}"))?;
    Ok(Loaded { width, height, png })
}

/// The escape sequences that draw `png` (chunked), at `cols` cells wide when given.
pub fn kitty_sequences(png: &[u8], cols: Option<u32>) -> Vec<u8> {
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let mut out = Vec::with_capacity(data.len() + data.len() / CHUNK * 16 + 64);
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(CHUNK).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let ctl = if i == 0 {
            match cols {
                Some(c) => format!("a=T,f=100,q=2,c={c},m={more}"),
                None => format!("a=T,f=100,q=2,m={more}"),
            }
        } else {
            format!("q=2,m={more}")
        };
        out.extend_from_slice(b"\x1b_G");
        out.extend_from_slice(ctl.as_bytes());
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Cell size assumed for fitting (no query: asking the terminal can stall when it does not answer).
const CELL_PX: (u32, u32) = (8, 17);

/// How many columns an image of `w` x `h` pixels may take so that it fits the pane in both directions;
/// `None` when the pane size is unknown. `reserve` rows stay free for a caption and the prompt.
pub fn fit_cols(w: u32, h: u32, reserve: u32) -> Option<u32> {
    if w == 0 || h == 0 {
        return None;
    }
    let (cols, rows) =
        terminal_size::terminal_size().map(|(c, r)| (u32::from(c.0), u32::from(r.0)))?;
    Some(fit_cols_for(
        w,
        h,
        (cols, rows.saturating_sub(reserve).max(1)),
        CELL_PX,
    ))
}

/// The arithmetic of [`fit_cols`]: the largest column count that keeps the picture inside `pane` (cells) at the
/// given cell size in pixels, never wider than the picture's own size.
pub fn fit_cols_for(w: u32, h: u32, pane: (u32, u32), cell: (u32, u32)) -> u32 {
    let (cw, ch) = (cell.0.max(1), cell.1.max(1));
    let natural = w.div_ceil(cw);
    let max_h_px = pane.1 * ch;
    let by_height = ((u64::from(max_h_px) * u64::from(w)) / u64::from(h) / u64::from(cw)) as u32;
    natural.min(pane.0).min(by_height.max(1)).max(1)
}

/// One raw RGB video frame as Kitty graphics: transmitted with image id `id`, which replaces the previous frame's
/// placement, drawn `cols` cells wide and without moving the cursor (so the next frame lands in the same place).
pub fn kitty_rgb_frame(width: u32, height: u32, rgb: &[u8], cols: u32, id: u32) -> Vec<u8> {
    let data = base64::engine::general_purpose::STANDARD.encode(rgb);
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(CHUNK).collect();
    let mut out = Vec::with_capacity(data.len() + data.len() / CHUNK * 16 + 96);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let ctl = if i == 0 {
            format!("a=T,f=24,s={width},v={height},i={id},C=1,q=2,c={cols},m={more}")
        } else {
            format!("q=2,m={more}")
        };
        out.extend_from_slice(b"\x1b_G");
        out.extend_from_slice(ctl.as_bytes());
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Short label printed under an image: `name  160x96`.
pub fn caption(path: &Path, w: u32, h: u32) -> String {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    format!("{clean}  {w}x{h}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mahzenterm_term::gfx::{Action, Kitty, Raw, Scanner};

    fn sample(format: image::ImageFormat) -> Vec<u8> {
        let mut px = image::RgbaImage::new(5, 3);
        for (i, p) in px.pixels_mut().enumerate() {
            *p = image::Rgba([(i * 10) as u8, 100, 200, 255]);
        }
        let mut out = Vec::new();
        match format {
            // JPEG has no alpha channel.
            image::ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(px)
                .to_rgb8()
                .write_to(&mut Cursor::new(&mut out), format)
                .unwrap(),
            _ => px.write_to(&mut Cursor::new(&mut out), format).unwrap(),
        }
        out
    }

    #[test]
    fn every_supported_format_becomes_a_png_of_the_same_size() {
        use image::ImageFormat::*;
        for f in [Png, Jpeg, Gif, Bmp, Tiff, Ico, Tga, Qoi, WebP] {
            let bytes = sample(f);
            let name = format!("pic.{}", f.extensions_str()[0]);
            let l = load(&bytes, &name).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_eq!((l.width, l.height), (5, 3), "{f:?}");
            assert!(l.png.starts_with(b"\x89PNG"), "{f:?}");
        }
    }

    #[test]
    fn bad_input_is_explained() {
        assert!(load(b"not an image", "a.txt")
            .unwrap_err()
            .contains("not an image"));
        assert!(load(b"<svg xmlns='http://www.w3.org/2000/svg'/>", "a.svg")
            .unwrap_err()
            .contains("SVG"));
        assert!(load(b"<?xml version='1.0'?><svg/>", "a.svg")
            .unwrap_err()
            .contains("SVG"));
        assert!(load(b"", "empty").is_err());
    }

    #[test]
    fn the_terminal_side_reads_back_what_tf_wrote() {
        // A big image forces several chunks; feeding them through the real scanner and handler must give the pixels.
        let mut big = image::RgbaImage::new(300, 200);
        for (i, p) in big.pixels_mut().enumerate() {
            *p = image::Rgba([(i % 251) as u8, (i % 13) as u8, 7, 255]);
        }
        let mut png = Vec::new();
        big.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let bytes = kitty_sequences(&png, Some(20));
        assert!(
            bytes.windows(3).filter(|w| w == b"\x1b_G").count() > 1,
            "chunked"
        );
        let mut scanner = Scanner::default();
        let mut kitty = Kitty::default();
        let mut placed = None;
        for (_, raw) in scanner.scan(&bytes) {
            let Raw::Kitty(text) = raw else {
                panic!("not kitty")
            };
            let out = kitty.handle(&text);
            assert!(out.reply.is_none(), "q=2 asks for no replies");
            if let Action::Place { image, cols, .. } = out.action {
                placed = Some((image, cols));
            }
        }
        let (image, cols) = placed.expect("the image was placed");
        assert_eq!((image.width, image.height), (300, 200));
        assert_eq!(cols, Some(20));
        assert_eq!(&image.rgba[..4], &[0, 0, 7, 255]);
    }

    #[test]
    fn video_frames_round_trip_through_the_terminal_side() {
        let (w, h) = (16u32, 9u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        let bytes = kitty_rgb_frame(w, h, &rgb, 40, 7);
        let mut scanner = Scanner::default();
        let mut kitty = Kitty::default();
        let mut placed = None;
        for (_, raw) in scanner.scan(&bytes) {
            let Raw::Kitty(text) = raw else {
                panic!("not kitty")
            };
            if let Action::Place {
                image,
                id,
                cols,
                stay,
                ..
            } = kitty.handle(&text).action
            {
                placed = Some((image, id, cols, stay));
            }
        }
        let (image, id, cols, stay) = placed.expect("placed");
        assert_eq!(
            (image.width, image.height, id, cols, stay),
            (16, 9, 7, Some(40), true)
        );
        assert_eq!(
            &image.rgba[..8],
            &[0, 1, 2, 255, 3, 4, 5, 255],
            "RGB became RGBA"
        );
    }

    #[test]
    fn pictures_are_fitted_to_the_pane_in_both_directions() {
        assert_eq!(fit_cols_for(1000, 3000, (100, 30), (8, 16)), 20);
        assert_eq!(fit_cols_for(4000, 1000, (100, 30), (8, 16)), 100);
        assert_eq!(fit_cols_for(80, 80, (100, 30), (8, 16)), 10);
        assert!(fit_cols_for(1, 100_000, (100, 30), (8, 16)) >= 1);
    }

    #[test]
    fn captions_drop_control_characters() {
        assert_eq!(caption(Path::new("/a/b/pic.png"), 4, 2), "pic.png  4x2");
        assert_eq!(
            caption(Path::new("/a/ev\u{1b}il.png"), 1, 1),
            "evil.png  1x1"
        );
    }
}
