# Images in the terminal

MahzenTerm draws images that programs print, with two protocols:

- **Kitty graphics protocol** (`kitten icat`, `yazi`, `chafa -f kitty`, `timg -p kitty`, ...): transmit and display
  (`a=T`), transmit then place (`a=t` + `a=p`), delete (`a=d`, `d=a|A|i|I`), support query (`a=q`, answered on the PTY),
  PNG (`f=100`), raw RGB (`f=24`) and RGBA (`f=32`), chunked transfers (`m=1`), `c`/`r` cell size, `C=1` (keep cursor).
- **Sixel** (`img2sixel`, `chafa -f sixels`, `lsix`, `timg -p sixel`): `DCS q ... ST` with colour registers (RGB and HLS),
  repeats (`!`), carriage return (`$`), new band (`-`), raster attributes. Pixels a program does not draw stay transparent.
- **iTerm2 inline images** (`imgcat`, `chafa -f iterm`, `wezterm imgcat`): `OSC 1337 ; File=inline=1;width=..;height=..:base64`
  with PNG, JPEG, GIF (first frame), WebP and BMP. `width`/`height` as cells, `N%` or `auto`.

An image is anchored to the text: it scrolls with the output, and is sized in cells from its pixels and the font's cell
size (an image wider than the pane is scaled down to the pane width). The cursor ends on the line below it. Images
placed on the alternate screen (full-screen programs) disappear when that screen is left. A window resize drops the
images (reflow changes the lines they are anchored to); programs such as `yazi` draw them again.

Not supported yet: animation (GIF/WebP show the first frame), Kitty image cropping/offset/z-index, compressed
Kitty data (`o=z`), unicode placeholders, `ED` clearing images explicitly (they scroll away).

## `tf img`: view image files

```
tf img [--width COLS] [--wait] <file>...
```

Decodes PNG, JPEG, GIF (first frame), WebP, BMP, TIFF, ICO, TGA, PNM and QOI, re-encodes as PNG and draws it with the
Kitty protocol (so it also works in kitty, WezTerm and Ghostty), then prints the name and pixel size. An image wider than
the pane is fitted to the pane by the terminal; `--width` sets a size in columns. Several files are shown one after the other;
unreadable ones are reported and the rest are still shown. `--wait` keeps a pane open until Enter. SVG is not supported yet
(it needs a vector renderer).

In the explorer, Enter on an image file opens it in a split with `tf img --wait`.

## Safety

Everything a program prints is untrusted. Only **direct (in-band) transmission** is accepted: Kitty's `t=f` (file path),
`t=t` (temporary file) and `t=s` (shared memory) are refused, so a program cannot make the terminal read a local file.
Limits: 8192 px per side, 40 megapixels per image, 40 MB of escape text, 64 stored images and 256 MB of stored pixels per
pane, 256 images on screen. Bad data gets a Kitty error reply (`EINVAL:...`) or is ignored, never a crash.

Cost: decoding uses the `image` crate with PNG, JPEG, GIF, WebP and BMP codecs (the release binary grew from about 7 to
11 MB). The same crate will serve the image viewer (PLAN.md 5.8).

Try it: `printf '\033_Ga=T,f=24,s=2,v=2;/wAAAP8AAAD/////\033\\'` draws a 2x2 pixel image.
