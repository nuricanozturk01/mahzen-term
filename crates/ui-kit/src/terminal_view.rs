//! Canvas-based terminal grid renderer.
//!
//! First-generation renderer: per-row text runs on an Iced canvas, cached until the next snapshot.
//! It is deliberately simple; M1 measures it against the performance budget (PLAN.md section 1)
//! before a glyph-atlas renderer replaces it.

use crate::glyphs;
use iced::mouse;
use iced::widget::canvas::{self, Cache, Frame, Geometry, Path, Stroke, Text};
use iced::{
    alignment, font, Color, Element, Font, Length, Pixels, Point, Rectangle, Renderer, Size, Theme,
};
use mahzenterm_term::session::{CursorShapeKind, LinkSpan, Mark};
use mahzenterm_term::{Rgb, Snapshot};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Image handles by image id: building a handle uploads (or decodes) the pixels, so it is done once per image.
fn image_handle(image: &mahzenterm_term::gfx::Image) -> iced::widget::image::Handle {
    static CACHE: OnceLock<Mutex<HashMap<u64, iced::widget::image::Handle>>> = OnceLock::new();
    /// Most handles kept; a terminal rarely shows more images than this at once.
    const MAX: usize = 64;
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut map) = cache.lock() else {
        return iced::widget::image::Handle::from_rgba(
            image.width,
            image.height,
            image.rgba.clone(),
        );
    };
    if map.len() >= MAX && !map.contains_key(&image.id) {
        map.clear();
    }
    map.entry(image.id)
        .or_insert_with(|| {
            iced::widget::image::Handle::from_rgba(image.width, image.height, image.rgba.clone())
        })
        .clone()
}

/// Cell geometry derived from the font size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub font_size: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Metrics {
    /// Monospace advance is ~0.6 em for the common fonts; line height 1.3 em.
    pub fn for_font_size(font_size: f32) -> Self {
        Self {
            font_size,
            cell_width: (font_size * crate::fonts::ADVANCE_EM).round(),
            cell_height: (font_size * 1.3).round(),
        }
    }

    /// Whole cells that fit into `size`.
    pub fn grid_for(&self, size: Size) -> (u16, u16) {
        let cols = ((size.width / self.cell_width).floor().clamp(1.0, 1000.0) as u16).max(1);
        let rows = ((size.height / self.cell_height).floor().clamp(1.0, 1000.0) as u16).max(1);
        (cols, rows)
    }
}

/// Colours of the highlights drawn over cell backgrounds.
#[derive(Debug, Clone, Copy)]
struct Marks {
    selection: Rgb,
    found: Rgb,
    current: Rgb,
}

fn cell_bg(c: &mahzenterm_term::Cell, m: Marks) -> Rgb {
    match c.mark {
        Mark::Current => m.current,
        Mark::Found => m.found,
        Mark::None if c.selected => m.selection,
        Mark::None => c.bg,
    }
}

fn color(c: Rgb) -> Color {
    Color::from_rgb8(c.0, c.1, c.2)
}

/// User interaction reported by the widget, in grid coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewAction {
    /// Positive scrolls back in history.
    Scroll(i32),
    /// Left button pressed; `clicks` is 1, 2 or 3 for single/double/triple click.
    Press {
        col: u16,
        row: u16,
        clicks: u8,
    },
    Drag {
        col: u16,
        row: u16,
    },
    Release,
    /// Ctrl/Cmd+click on a link: the target to open.
    OpenLink(String),
    /// Ctrl/Cmd+click on plain text that is not a URL: the word under the pointer (maybe a `file:line`).
    OpenToken {
        token: String,
        col: u16,
        row: u16,
    },
}

/// Interaction state kept by the canvas between events.
#[derive(Debug, Default)]
pub struct Interaction {
    dragging: bool,
    /// Ctrl (Cmd on macOS) is held: links are live.
    command: bool,
    /// Link under the pointer while `command` is held.
    hover: Option<LinkSpan>,
    last_click: Option<(std::time::Instant, u16, u16, u8)>,
}

const MULTI_CLICK_MS: u128 = 400;
const SELECTION_BG: Rgb = Rgb(0x58, 0x5b, 0x70);

/// The terminal widget. Create one per frame in `view`.
pub struct TerminalView<'a, Message> {
    snapshot: &'a Snapshot,
    cache: &'a Cache,
    metrics: Metrics,
    focused: bool,
    selection: Rgb,
    found: Rgb,
    current: Rgb,
    link: Rgb,
    on_action: Box<dyn Fn(ViewAction) -> Message + 'a>,
}

impl<'a, Message: 'a> TerminalView<'a, Message> {
    pub fn new(
        snapshot: &'a Snapshot,
        cache: &'a Cache,
        metrics: Metrics,
        on_action: impl Fn(ViewAction) -> Message + 'a,
    ) -> Self {
        Self {
            snapshot,
            cache,
            metrics,
            focused: true,
            selection: SELECTION_BG,
            found: Rgb(0x5c, 0x4a, 0x1a),
            current: Rgb(0xf6, 0xc7, 0x6b),
            link: Rgb(0x2e, 0xe6, 0xd6),
            on_action: Box::new(on_action),
        }
    }

    /// Background colour of selected cells (from the theme).
    #[must_use]
    pub fn selection(mut self, selection: Rgb) -> Self {
        self.selection = selection;
        self
    }

    /// Search highlight colours (`found` for every match, `current` for the one the user is on) and the colour
    /// of the underline shown under a hovered link.
    #[must_use]
    pub fn highlights(mut self, found: Rgb, current: Rgb, link: Rgb) -> Self {
        self.found = found;
        self.current = current;
        self.link = link;
        self
    }

    /// Whether this pane has keyboard focus. Unfocused panes draw a hollow cursor.
    /// The caller must clear the geometry cache when this changes.
    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub fn into_element(self) -> Element<'a, Message> {
        canvas::Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

/// A horizontal run of text cells sharing one style.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub col: u16,
    /// Cells covered (a wide character covers two).
    pub cells: u16,
    pub text: String,
    pub fg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Split one row into font-drawn text runs and cells drawn by the built-in glyph renderer.
/// Blank cells end runs only at the row's trailing edge; wide characters form their own run.
pub fn split_row(snap: &Snapshot, row: u16) -> (Vec<Run>, Vec<(u16, char, Rgb)>) {
    let mut runs: Vec<Run> = Vec::new();
    let mut builtin = Vec::new();
    let mut cur: Option<Run> = None;
    let flush = |cur: &mut Option<Run>, runs: &mut Vec<Run>| {
        if let Some(mut run) = cur.take() {
            // Trailing blanks carry no ink (underline/strike on blanks is rare and cheap to lose).
            let trimmed = run.text.trim_end_matches(' ').chars().count() as u16;
            if trimmed > 0 {
                run.text.truncate(run.text.trim_end_matches(' ').len());
                run.cells = trimmed;
                runs.push(run);
            }
        }
    };
    for col in 0..snap.cols {
        let c = snap.cell(col, row);
        if c.ch == '\0' {
            continue; // spacer half of a wide character
        }
        // Text on the current search match is drawn in the background colour to stay readable on the highlight.
        let fg = if c.mark == Mark::Current {
            snap.bg
        } else {
            c.fg
        };
        if c.ch != ' ' && glyphs::builtin_glyph(c.ch, 1.0, 1.0).is_some() {
            flush(&mut cur, &mut runs);
            builtin.push((col, c.ch, fg));
            continue;
        }
        let f = c.flags;
        let same = cur.as_ref().is_some_and(|r| {
            !f.wide
                && r.fg == fg
                && r.bold == f.bold
                && r.italic == f.italic
                && r.underline == f.underline
                && r.strike == f.strike
        });
        if f.wide || !same {
            flush(&mut cur, &mut runs);
            if c.ch == ' ' && !f.wide {
                continue; // do not start runs on blanks
            }
            cur = Some(Run {
                col,
                cells: 1,
                text: String::new(),
                fg,
                bold: f.bold,
                italic: f.italic,
                underline: f.underline,
                strike: f.strike,
            });
        }
        if let Some(r) = cur.as_mut() {
            r.text.push(c.ch);
            r.cells = r.text.chars().count() as u16;
        }
        if f.wide {
            if let Some(mut r) = cur.take() {
                r.cells = 2;
                runs.push(r);
            }
        }
    }
    flush(&mut cur, &mut runs);
    (runs, builtin)
}

impl<Message> TerminalView<'_, Message> {
    /// Recompute the hovered link; asks for a redraw only when it changed.
    fn refresh_hover(
        &self,
        state: &mut Interaction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let now = if state.command && cursor.is_over(bounds) {
            self.cell_at(bounds, cursor)
                .and_then(|(c, r)| self.snapshot.link_at(c, r))
        } else {
            None
        };
        if now == state.hover {
            return None;
        }
        state.hover = now;
        Some(canvas::Action::request_redraw())
    }

    /// Grid cell under the cursor, clamped to the grid.
    fn cell_at(&self, bounds: Rectangle, cursor: mouse::Cursor) -> Option<(u16, u16)> {
        let p = cursor.position()?;
        let x = (p.x - bounds.x).max(0.0);
        let y = (p.y - bounds.y).max(0.0);
        let col = ((x / self.metrics.cell_width) as u16).min(self.snapshot.cols.saturating_sub(1));
        let row = ((y / self.metrics.cell_height) as u16).min(self.snapshot.rows.saturating_sub(1));
        Some((col, row))
    }
}

impl<Message> canvas::Program<Message> for TerminalView<'_, Message> {
    type State = Interaction;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if let iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) = event {
            state.command = m.command();
            return self.refresh_hover(state, bounds, cursor);
        }
        let iced::Event::Mouse(ev) = event else {
            return None;
        };
        let publish =
            |a: ViewAction| Some(canvas::Action::publish((self.on_action)(a)).and_capture());
        match ev {
            mouse::Event::WheelScrolled { delta } if cursor.is_over(bounds) => {
                let lines = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => (y * 3.0).round() as i32,
                    mouse::ScrollDelta::Pixels { y, .. } => {
                        (y / self.metrics.cell_height).round() as i32
                    }
                };
                (lines != 0)
                    .then(|| publish(ViewAction::Scroll(lines)))
                    .flatten()
            }
            mouse::Event::ButtonPressed(mouse::Button::Left) if cursor.is_over(bounds) => {
                let (col, row) = self.cell_at(bounds, cursor)?;
                if state.command {
                    if let Some(link) = self.snapshot.link_at(col, row) {
                        return publish(ViewAction::OpenLink(link.url));
                    }
                    if let Some(token) = self.snapshot.token_at(col, row) {
                        return publish(ViewAction::OpenToken { token, col, row });
                    }
                }
                let now = std::time::Instant::now();
                let clicks = match state.last_click {
                    Some((t, c, r, n))
                        if c == col
                            && r == row
                            && now.duration_since(t).as_millis() < MULTI_CLICK_MS =>
                    {
                        n % 3 + 1
                    }
                    _ => 1,
                };
                state.last_click = Some((now, col, row, clicks));
                state.dragging = true;
                publish(ViewAction::Press { col, row, clicks })
            }
            mouse::Event::CursorMoved { .. } if state.dragging => {
                let (col, row) = self.cell_at(bounds, cursor)?;
                publish(ViewAction::Drag { col, row })
            }
            mouse::Event::CursorMoved { .. } => self.refresh_hover(state, bounds, cursor),
            mouse::Event::ButtonReleased(mouse::Button::Left) if state.dragging => {
                state.dragging = false;
                publish(ViewAction::Release)
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            self.paint(frame, bounds.size());
        });
        let Some(link) = state.hover.as_ref().filter(|_| state.command) else {
            return vec![geometry];
        };
        // The hovered link is underlined on a separate layer, so the cached terminal geometry stays valid.
        let m = self.metrics;
        let mut overlay = Frame::new(renderer, bounds.size());
        overlay.fill_rectangle(
            Point::new(
                f32::from(link.start) * m.cell_width,
                f32::from(link.row) * m.cell_height + m.cell_height - 2.0,
            ),
            Size::new(f32::from(link.end - link.start + 1) * m.cell_width, 2.0),
            color(self.link),
        );
        vec![geometry, overlay.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.command && state.hover.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<Message> TerminalView<'_, Message> {
    fn paint(&self, frame: &mut Frame, size: Size) {
        let snap = self.snapshot;
        let m = self.metrics;
        frame.fill_rectangle(Point::ORIGIN, size, color(snap.bg));
        let marks = Marks {
            selection: self.selection,
            found: self.found,
            current: self.current,
        };

        for row in 0..snap.rows {
            let y = f32::from(row) * m.cell_height;

            // Background runs.
            let mut col = 0u16;
            while col < snap.cols {
                let bg = cell_bg(snap.cell(col, row), marks);
                let start = col;
                while col < snap.cols && cell_bg(snap.cell(col, row), marks) == bg {
                    col += 1;
                }
                if bg != snap.bg {
                    frame.fill_rectangle(
                        Point::new(f32::from(start) * m.cell_width, y),
                        Size::new(f32::from(col - start) * m.cell_width, m.cell_height),
                        color(bg),
                    );
                }
            }

            let (runs, builtin) = split_row(snap, row);
            for run in runs {
                let x = f32::from(run.col) * m.cell_width;
                frame.fill_text(Text {
                    content: run.text.clone(),
                    position: Point::new(x, y),
                    color: color(run.fg),
                    size: Pixels(m.font_size),
                    line_height: iced::widget::text::LineHeight::Absolute(Pixels(m.cell_height)),
                    font: Font {
                        weight: if run.bold {
                            font::Weight::Bold
                        } else {
                            font::Weight::Normal
                        },
                        style: if run.italic {
                            font::Style::Italic
                        } else {
                            font::Style::Normal
                        },
                        ..crate::fonts::MONO
                    },
                    align_y: alignment::Vertical::Top,
                    shaping: iced::widget::text::Shaping::Advanced,
                    ..Text::default()
                });
                if run.underline || run.strike {
                    let w = f32::from(run.cells) * m.cell_width;
                    let ly = if run.strike {
                        y + (m.cell_height / 2.0).floor()
                    } else {
                        y + m.cell_height - 2.0
                    };
                    frame.fill_rectangle(Point::new(x, ly), Size::new(w, 1.0), color(run.fg));
                }
            }
            for (col, ch, fg) in builtin {
                let ox = f32::from(col) * m.cell_width;
                if let Some(rects) = glyphs::builtin_glyph(ch, m.cell_width, m.cell_height) {
                    for q in rects {
                        let c = color(fg);
                        frame.fill_rectangle(
                            Point::new(ox + q.x, y + q.y),
                            Size::new(q.w, q.h),
                            Color { a: q.alpha, ..c },
                        );
                    }
                }
            }
        }

        // Images (Kitty graphics, iTerm2) cover the cells they were placed on.
        for img in &snap.images {
            let bounds = Rectangle::new(
                Point::new(
                    f32::from(img.col) * m.cell_width,
                    img.row as f32 * m.cell_height,
                ),
                Size::new(
                    f32::from(img.cols) * m.cell_width,
                    f32::from(img.rows) * m.cell_height,
                ),
            );
            frame.draw_image(bounds, canvas::Image::new(image_handle(&img.image)));
        }

        if let Some(cur) = snap.cursor {
            let x = f32::from(cur.col) * m.cell_width;
            let y = f32::from(cur.row) * m.cell_height;
            let accent = color(snap.cursor_color);
            let shape = if self.focused {
                cur.shape
            } else {
                CursorShapeKind::Hollow
            };
            match shape {
                CursorShapeKind::Block => {
                    frame.fill_rectangle(
                        Point::new(x, y),
                        Size::new(m.cell_width, m.cell_height),
                        Color { a: 0.6, ..accent },
                    );
                }
                CursorShapeKind::Underline => {
                    frame.fill_rectangle(
                        Point::new(x, y + m.cell_height - 2.0),
                        Size::new(m.cell_width, 2.0),
                        accent,
                    );
                }
                CursorShapeKind::Beam => {
                    frame.fill_rectangle(Point::new(x, y), Size::new(2.0, m.cell_height), accent);
                }
                CursorShapeKind::Hollow => {
                    frame.stroke(
                        &Path::rectangle(
                            Point::new(x + 0.5, y + 0.5),
                            Size::new(m.cell_width - 1.0, m.cell_height - 1.0),
                        ),
                        Stroke::default().with_color(accent).with_width(1.0),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_is_whole_cells_and_never_zero() {
        let m = Metrics {
            font_size: 10.0,
            cell_width: 6.0,
            cell_height: 13.0,
        };
        assert_eq!(m.grid_for(Size::new(600.0, 130.0)), (100, 10));
        assert_eq!(m.grid_for(Size::new(605.0, 139.0)), (100, 10));
        assert_eq!(m.grid_for(Size::new(1.0, 1.0)), (1, 1));
    }

    use mahzenterm_term::session::CellFlags;
    use mahzenterm_term::Cell;

    const FG: Rgb = Rgb(200, 200, 200);
    const RED: Rgb = Rgb(255, 0, 0);

    fn cell(ch: char, fg: Rgb, flags: CellFlags) -> Cell {
        Cell {
            ch,
            fg,
            bg: Rgb(0, 0, 0),
            flags,
            selected: false,
            mark: Mark::None,
            link: 0,
        }
    }

    fn snap(cells: Vec<Cell>) -> Snapshot {
        Snapshot {
            cols: cells.len() as u16,
            rows: 1,
            cells,
            cursor: None,
            bg: Rgb(0, 0, 0),
            cursor_color: Rgb(255, 255, 255),
            display_offset: 0,
            links: Vec::new(),
            images: Vec::new(),
        }
    }

    fn plain(s: &str) -> Vec<Cell> {
        s.chars()
            .map(|c| cell(c, FG, CellFlags::default()))
            .collect()
    }

    #[test]
    fn run_text_keeps_inner_blanks_and_trims_trailing_ones() {
        let (runs, _) = split_row(&snap(plain("ab cd   ")), 0);
        assert_eq!(runs.len(), 1);
        assert_eq!(
            (runs[0].col, runs[0].cells, runs[0].text.as_str()),
            (0, 5, "ab cd")
        );
    }

    #[test]
    fn leading_blanks_are_skipped_and_run_starts_at_first_ink() {
        let (runs, _) = split_row(&snap(plain("  hi")), 0);
        assert_eq!((runs[0].col, runs[0].text.as_str()), (2, "hi"));
    }

    #[test]
    fn color_change_splits_runs() {
        let mut c = plain("abcd");
        c[2].fg = RED;
        c[3].fg = RED;
        let (runs, _) = split_row(&snap(c), 0);
        assert_eq!(
            runs.iter()
                .map(|r| (r.col, r.text.as_str()))
                .collect::<Vec<_>>(),
            [(0, "ab"), (2, "cd")]
        );
    }

    #[test]
    fn underline_and_strike_changes_split_runs_so_lines_stay_short() {
        let u = CellFlags {
            underline: true,
            ..CellFlags::default()
        };
        let mut c = plain("ab  cd");
        c[2] = cell(' ', FG, u);
        c[3] = cell(' ', FG, u);
        c[4] = cell('c', FG, u);
        c[5] = cell('d', FG, u);
        let (runs, _) = split_row(&snap(c), 0);
        let under: Vec<_> = runs.iter().filter(|r| r.underline).collect();
        assert_eq!(under.len(), 1);
        assert_eq!(
            (under[0].col, under[0].cells, under[0].text.as_str()),
            (4, 2, "cd")
        );
        assert!(!runs[0].underline && runs[0].text == "ab");
    }

    #[test]
    fn wide_character_is_its_own_two_cell_run() {
        let wide = CellFlags {
            wide: true,
            ..CellFlags::default()
        };
        let c = vec![
            cell('a', FG, CellFlags::default()),
            cell('日', FG, wide),
            cell('\0', FG, CellFlags::default()),
            cell('b', FG, CellFlags::default()),
        ];
        let (runs, _) = split_row(&snap(c), 0);
        assert_eq!(runs.len(), 3);
        assert_eq!(
            (runs[1].col, runs[1].cells, runs[1].text.as_str()),
            (1, 2, "日")
        );
        assert_eq!((runs[2].col, runs[2].text.as_str()), (3, "b"));
    }

    #[test]
    fn box_drawing_cells_leave_the_text_run_and_are_listed_separately() {
        let (runs, builtin) = split_row(&snap(plain("a\u{2500}\u{2502}b")), 0);
        assert_eq!(
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(
            builtin
                .iter()
                .map(|(c, ch, _)| (*c, *ch))
                .collect::<Vec<_>>(),
            [(1, '\u{2500}'), (2, '\u{2502}')]
        );
    }
}
