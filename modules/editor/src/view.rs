//! Drawing the editor pane and turning mouse input into editor messages.
//!
//! The text area is one Iced canvas (like the terminal view): monospace cells, only the visible lines are drawn, colours
//! come only from the theme's tokens. A status line under it shows the file, the cursor position, the find bar or the
//! go-to-line prompt.

use std::cell::Cell;
use std::time::Instant;

use iced::mouse;
use iced::widget::canvas::{self, Cache, Frame, Geometry, Path, Text};
use iced::widget::{column, container, row, text, Space};
use iced::{
    alignment, font, Color, Element, Font, Length, Padding, Pixels, Point, Rectangle, Renderer,
    Size, Theme,
};
use mahzenterm_core::syntax::{self, Token};
use mahzenterm_ui_kit::{fonts, mix, Metrics, Tokens};
use unicode_width::UnicodeWidthChar;

use crate::state::{EditorState, Mode, Severity, TAB_WIDTH};

/// How many lines before the first visible one are scanned to know whether a `/* */` comment is open.
const BLOCK_LOOKBACK: usize = 200;
/// Double click window.
const DOUBLE_MS: u128 = 400;

/// What the pane tells the app about the mouse and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Click {
        line: usize,
        col: usize,
        extend: bool,
    },
    DoubleClick {
        line: usize,
        col: usize,
    },
    Drag {
        line: usize,
        col: usize,
    },
    Scroll(isize),
    /// The number of text lines that fit changed.
    Rows(usize),
}

/// An editor state plus its drawing cache.
#[derive(Debug)]
pub struct EditorPane {
    pub state: EditorState,
    cache: Cache,
    drawn_revision: Cell<u64>,
}

impl EditorPane {
    pub fn new(state: EditorState) -> Self {
        Self {
            state,
            cache: Cache::new(),
            drawn_revision: Cell::new(u64::MAX),
        }
    }

    /// Redraw on the next frame (the theme or the font changed).
    pub fn invalidate(&self) {
        self.cache.clear();
    }
}

/// Terminal columns taken by the first `chars` characters of a line (a tab is [`TAB_WIDTH`] wide).
pub fn visual_col(line: &str, chars: usize) -> usize {
    line.chars()
        .take(chars)
        .map(|c| {
            if c == '\t' {
                TAB_WIDTH
            } else {
                c.width().unwrap_or(0)
            }
        })
        .sum()
}

/// The character index of a line that is drawn at visual column `vcol` (the nearest boundary).
pub fn char_at_visual(line: &str, vcol: usize) -> usize {
    let mut acc = 0;
    for (i, c) in line.chars().enumerate() {
        let w = if c == '\t' {
            TAB_WIDTH
        } else {
            c.width().unwrap_or(0)
        };
        if acc + w / 2 >= vcol && acc + w > vcol - vcol.min(acc) || acc >= vcol {
            return i;
        }
        acc += w;
    }
    line.chars().count()
}

/// Digits needed for line numbers.
fn gutter_digits(lines: usize) -> usize {
    lines.max(1).to_string().len().max(3)
}

fn token_color(tok: Token, t: Tokens) -> Color {
    match tok {
        Token::Plain => t.text,
        Token::Keyword => t.accent,
        Token::String => t.ok,
        Token::Number => t.warn,
        Token::Comment => t.text_dim,
        Token::Type => mix(t.accent, t.text, 0.5),
    }
}

struct Canvas<'a, M> {
    pane: &'a EditorPane,
    t: Tokens,
    m: Metrics,
    focused: bool,
    on: Box<dyn Fn(Msg) -> M + 'a>,
}

#[derive(Default)]
struct Interaction {
    dragging: bool,
    last_click: Option<Instant>,
    rows: usize,
}

impl<M> Canvas<'_, M> {
    fn gutter(&self) -> f32 {
        (gutter_digits(self.pane.state.buffer.line_count()) + 2) as f32 * self.m.cell_width
    }

    /// Line and character column under a point inside the canvas.
    fn locate(&self, bounds: Rectangle, p: Point) -> (usize, usize) {
        let st = &self.pane.state;
        let row = ((p.y - bounds.y).max(0.0) / self.m.cell_height) as usize;
        let line = (st.scroll + row).min(st.buffer.line_count().saturating_sub(1));
        let vcol = ((p.x - bounds.x - self.gutter()).max(0.0) / self.m.cell_width).round() as usize;
        (line, char_at_visual(&st.buffer.line(line), vcol))
    }
}

impl<M> canvas::Program<M> for Canvas<'_, M> {
    type State = Interaction;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<M>> {
        let rows = (bounds.height / self.m.cell_height).floor().max(1.0) as usize;
        if rows != state.rows && rows != self.pane.state.rows {
            state.rows = rows;
            return Some(canvas::Action::publish((self.on)(Msg::Rows(rows))));
        }
        state.rows = rows;
        let iced::Event::Mouse(ev) = event else {
            return None;
        };
        let publish = |m: Msg| Some(canvas::Action::publish((self.on)(m)).and_capture());
        match ev {
            mouse::Event::WheelScrolled { delta } if cursor.is_over(bounds) => {
                let lines = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => -(y * 3.0).round() as isize,
                    mouse::ScrollDelta::Pixels { y, .. } => {
                        -(y / self.m.cell_height).round() as isize
                    }
                };
                (lines != 0).then(|| publish(Msg::Scroll(lines))).flatten()
            }
            mouse::Event::ButtonPressed(mouse::Button::Left) if cursor.is_over(bounds) => {
                let (line, col) = self.locate(bounds, cursor.position()?);
                state.dragging = true;
                let double = state
                    .last_click
                    .is_some_and(|t| t.elapsed().as_millis() < DOUBLE_MS);
                state.last_click = Some(Instant::now());
                if double {
                    publish(Msg::DoubleClick { line, col })
                } else {
                    publish(Msg::Click {
                        line,
                        col,
                        extend: false,
                    })
                }
            }
            mouse::Event::CursorMoved { position } if state.dragging => {
                let (line, col) = self.locate(bounds, *position);
                publish(Msg::Drag { line, col })
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                state.dragging = false;
                None
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if self.pane.drawn_revision.get() != self.pane.state.revision {
            self.pane.cache.clear();
            self.pane.drawn_revision.set(self.pane.state.revision);
        }
        let g = self.pane.cache.draw(renderer, bounds.size(), |frame| {
            self.paint(frame, bounds.size())
        });
        vec![g]
    }
}

impl<M> Canvas<'_, M> {
    /// The suggestion list or the hover text, in a box next to the cursor (below it, or above near the bottom).
    fn paint_popups(&self, frame: &mut Frame, size: Size, gutter: f32, cw: f32, ch: f32) {
        let (st, t, m) = (&self.pane.state, self.t, self.m);
        let rows: Vec<(String, bool)> = if let Some(c) = &st.completion {
            let first = c.selected.saturating_sub(POPUP_ROWS - 1);
            c.items()
                .enumerate()
                .skip(first)
                .take(POPUP_ROWS)
                .map(|(i, it)| {
                    let line = if it.detail.is_empty() {
                        it.label.clone()
                    } else {
                        format!("{}  {}", it.label, it.detail)
                    };
                    (line.chars().take(POPUP_COLS).collect(), i == c.selected)
                })
                .collect()
        } else if let Some(h) = &st.hover {
            h.iter()
                .take(POPUP_ROWS + 4)
                .map(|l| (l.chars().take(POPUP_COLS + 20).collect(), false))
                .collect()
        } else {
            return;
        };
        if rows.is_empty() {
            return;
        }
        let buf = &st.buffer;
        let (line, col) = buf.line_col(buf.cursor());
        if line < st.scroll || line >= st.scroll + st.rows.max(1) {
            return;
        }
        let text_line = buf.line(line);
        let (x0, y0) = (
            gutter + visual_col(&text_line, col) as f32 * cw,
            (line - st.scroll) as f32 * ch,
        );
        let chars = rows
            .iter()
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(1);
        let (w, h) = ((chars as f32 + 2.0) * cw, rows.len() as f32 * ch + 6.0);
        let x = x0.min((size.width - w).max(0.0));
        let y = if y0 + ch + h <= size.height {
            y0 + ch
        } else {
            (y0 - h).max(0.0)
        };
        frame.fill_rectangle(Point::new(x, y), Size::new(w, h), t.surface);
        frame.stroke(
            &Path::rectangle(Point::new(x + 0.5, y + 0.5), Size::new(w - 1.0, h - 1.0)),
            canvas::Stroke::default()
                .with_color(t.border)
                .with_width(1.0),
        );
        for (i, (line, selected)) in rows.iter().enumerate() {
            let ry = y + 3.0 + i as f32 * ch;
            if *selected {
                frame.fill_rectangle(
                    Point::new(x + 1.0, ry),
                    Size::new(w - 2.0, ch),
                    mix(t.surface, t.accent, 0.35),
                );
            }
            frame.fill_text(Text {
                content: line.clone(),
                position: Point::new(x + cw, ry),
                color: if *selected || st.hover.is_some() {
                    t.text
                } else {
                    t.text_dim
                },
                size: Pixels(m.glyph_size()),
                line_height: iced::widget::text::LineHeight::Absolute(Pixels(ch)),
                font: fonts::MONO,
                align_y: alignment::Vertical::Top,
                ..Text::default()
            });
        }
    }

    fn paint(&self, frame: &mut Frame, size: Size) {
        let (t, m) = (self.t, self.m);
        let st = &self.pane.state;
        let buf = &st.buffer;
        let (cw, ch) = (m.cell_width, m.cell_height);
        frame.fill_rectangle(Point::ORIGIN, size, t.bg);
        let gutter = self.gutter();
        let digits = gutter_digits(buf.line_count());
        let rows = (size.height / ch).floor() as usize + 1;
        let (cur_line, _) = buf.line_col(buf.cursor());
        // Is a block comment open where the screen starts?
        let mut in_block = false;
        for l in st.scroll.saturating_sub(BLOCK_LOOKBACK)..st.scroll.min(buf.line_count()) {
            syntax::highlight_line(st.lang, &buf.line(l), &mut in_block);
        }
        let find: Vec<(usize, usize, bool)> = match &st.mode {
            Mode::Find {
                matches, current, ..
            } => matches
                .iter()
                .enumerate()
                .map(|(i, mt)| (mt.start, mt.end, i == *current))
                .collect(),
            _ => Vec::new(),
        };
        let sels: Vec<(usize, usize)> = buf.selections().iter().map(|s| s.range()).collect();
        for r in 0..rows {
            let line = st.scroll + r;
            if line >= buf.line_count() {
                break;
            }
            let y = r as f32 * ch;
            let (ls, le) = buf.line_range(line);
            let text_line = buf.line(line);
            if line == cur_line && self.focused {
                frame.fill_rectangle(
                    Point::new(gutter, y),
                    Size::new(size.width - gutter, ch),
                    mix(t.bg, t.surface_hi, 0.6),
                );
            }
            let rect = |a: usize, b: usize| {
                let (x0, x1) = (
                    visual_col(&text_line, a - ls),
                    visual_col(&text_line, b - ls),
                );
                (gutter + x0 as f32 * cw, ((x1 - x0).max(1)) as f32 * cw)
            };
            // Find matches under the selection, selection under the text.
            for (a, b, current) in &find {
                if *a < le + 1 && *b > ls && a <= &le {
                    let (x, w) = rect((*a).max(ls), (*b).min(le));
                    frame.fill_rectangle(
                        Point::new(x, y),
                        Size::new(w, ch),
                        mix(t.bg, t.warn, if *current { 0.55 } else { 0.28 }),
                    );
                }
            }
            for (a, b) in &sels {
                if a < b && *a <= le && *b > ls {
                    let (x, mut w) = rect((*a).max(ls), (*b).min(le));
                    if *b > le {
                        w += cw; // the selection runs on past the end of the line
                    }
                    frame.fill_rectangle(
                        Point::new(x, y),
                        Size::new(w, ch),
                        mix(t.bg, t.accent, 0.35),
                    );
                }
            }
            // Text.
            let expanded: String = text_line.replace('\t', &" ".repeat(TAB_WIDTH));
            let mut col = 0usize;
            for (tok, piece) in syntax::highlight_line(st.lang, &expanded, &mut in_block) {
                if piece.trim().is_empty() {
                    col += piece.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>();
                    continue;
                }
                frame.fill_text(Text {
                    content: piece.clone(),
                    position: Point::new(gutter + col as f32 * cw, y),
                    color: token_color(tok, t),
                    size: Pixels(m.glyph_size()),
                    line_height: iced::widget::text::LineHeight::Absolute(Pixels(ch)),
                    font: Font {
                        style: if tok == Token::Comment {
                            font::Style::Italic
                        } else {
                            font::Style::Normal
                        },
                        ..fonts::MONO
                    },
                    align_y: alignment::Vertical::Top,
                    ..Text::default()
                });
                col += piece.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>();
            }
            // Diagnostics: a wavy-looking underline in the severity colour.
            for d in st.diagnostics.iter().filter(|d| d.line == line) {
                let colr = match d.severity {
                    Severity::Error => t.danger,
                    Severity::Warning => t.warn,
                    Severity::Info | Severity::Hint => t.accent,
                };
                let x0 = visual_col(&text_line, d.start_col) as f32 * cw;
                let w = (visual_col(&text_line, d.end_col.max(d.start_col + 1)) as f32 * cw - x0)
                    .max(cw);
                frame.fill_rectangle(
                    Point::new(gutter + x0, y + ch - 2.0),
                    Size::new(w, 2.0),
                    colr,
                );
            }
            // Line number.
            let num = format!("{:>digits$}", line + 1);
            frame.fill_text(Text {
                content: num,
                position: Point::new(cw, y),
                color: if line == cur_line { t.text } else { t.text_dim },
                size: Pixels(m.glyph_size()),
                line_height: iced::widget::text::LineHeight::Absolute(Pixels(ch)),
                font: fonts::MONO,
                align_y: alignment::Vertical::Top,
                ..Text::default()
            });
            // Cursors.
            for s in buf.selections() {
                if s.head >= ls && s.head <= le {
                    let x = gutter + visual_col(&text_line, s.head - ls) as f32 * cw;
                    if self.focused {
                        frame.fill_rectangle(Point::new(x, y), Size::new(2.0, ch), t.accent);
                    } else {
                        frame.stroke(
                            &Path::rectangle(
                                Point::new(x + 0.5, y + 0.5),
                                Size::new(cw - 1.0, ch - 1.0),
                            ),
                            canvas::Stroke::default()
                                .with_color(t.accent)
                                .with_width(1.0),
                        );
                    }
                }
            }
        }
        self.paint_popups(frame, size, gutter, cw, ch);
        // The gutter separator.
        frame.fill_rectangle(
            Point::new(gutter - 4.0, 0.0),
            Size::new(1.0, size.height),
            t.border,
        );
        // A thin scrollbar.
        let total = buf.line_count().max(1) as f32;
        let visible = (size.height / ch).max(1.0);
        if total > visible {
            let h = (visible / total * size.height).max(16.0);
            let y = st.scroll as f32 / total * size.height;
            frame.fill_rectangle(
                Point::new(size.width - 4.0, y),
                Size::new(3.0, h),
                mix(t.bg, t.text_dim, 0.5),
            );
        }
    }
}

/// Rows of a popup list and columns of its text.
const POPUP_ROWS: usize = 10;
const POPUP_COLS: usize = 70;

/// The editor pane: the text area and the status line.
pub fn view<'a, M: Clone + 'a>(
    pane: &'a EditorPane,
    t: Tokens,
    metrics: Metrics,
    focused: bool,
    on: impl Fn(Msg) -> M + 'a,
) -> Element<'a, M> {
    let st = &pane.state;
    let area = canvas::Canvas::new(Canvas {
        pane,
        t,
        m: metrics,
        focused,
        on: Box::new(on),
    })
    .width(Length::Fill)
    .height(Length::Fill);
    let name = st.buffer.path().and_then(|p| p.file_name()).map_or_else(
        || "untitled".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let dirty = if st.buffer.is_dirty() {
        "\u{25cf} "
    } else {
        ""
    };
    let middle: Element<'a, M> = match &st.mode {
        Mode::Find {
            query,
            replace,
            show_replace,
            in_replace,
            matches,
            current,
        } => {
            let count = if query.is_empty() {
                String::new()
            } else if matches.is_empty() {
                "  no results".to_owned()
            } else {
                format!("  {}/{}", current + 1, matches.len())
            };
            let field = |label: &str, value: &str, active: bool| {
                text(format!(
                    "{label} {value}{}",
                    if active { "\u{258f}" } else { "" }
                ))
                .size(12)
                .color(if active { t.text } else { t.text_dim })
            };
            let mut r = row![
                field("find", query, !*in_replace),
                text(count).size(12).color(t.text_dim)
            ]
            .spacing(12);
            if *show_replace {
                r = r.push(field("replace", replace, *in_replace));
                r = r.push(
                    text("Tab switch \u{b7} \u{23ce} replace \u{b7} Ctrl+Alt+\u{23ce} all")
                        .size(11)
                        .color(t.text_dim),
                );
            } else {
                r = r.push(
                    text("\u{23ce} next \u{b7} Shift+\u{23ce} previous \u{b7} Esc close")
                        .size(11)
                        .color(t.text_dim),
                );
            }
            r.into()
        }
        Mode::Goto(d) => text(format!("go to line: {d}\u{258f}"))
            .size(12)
            .color(t.accent)
            .into(),
        Mode::SaveAs(n) => text(format!(
            "save as: {n}\u{258f}   \u{23ce} save \u{b7} Esc cancel"
        ))
        .size(12)
        .color(t.accent)
        .into(),
        Mode::Edit => text(st.notice.clone().unwrap_or_default())
            .size(12)
            .color(t.warn)
            .into(),
    };
    let problems = {
        let e = st
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let w = st
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        if e + w == 0 {
            String::new()
        } else {
            format!("{e} errors {w} warnings   ")
        }
    };
    let bar = container(
        row![
            text(format!("{dirty}{name}"))
                .size(12)
                .font(fonts::MONO_BOLD)
                .color(if st.buffer.is_dirty() { t.warn } else { t.text }),
            middle,
            Space::new().width(Length::Fill),
            text(problems).size(12).color(t.danger),
            text(format!(
                "{}   {}   {}",
                st.position_label(),
                if st.lang.is_empty() { "text" } else { st.lang },
                match st.buffer.line_ending() {
                    crate::LineEnding::Lf => "LF",
                    crate::LineEnding::CrLf => "CRLF",
                }
            ))
            .size(12)
            .color(t.text_dim),
        ]
        .spacing(16)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([4.0, 12.0]))
    .style(move |_| container::background(t.surface));
    column![area, bar].into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_account_for_tabs_and_wide_characters() {
        assert_eq!(visual_col("ab\tc", 3), 2 + TAB_WIDTH);
        assert_eq!(
            visual_col("\u{4e2d}\u{6587}x", 2),
            4,
            "CJK characters take two cells"
        );
        assert_eq!(visual_col("abc", 99), 3);
        assert_eq!(gutter_digits(5), 3);
        assert_eq!(gutter_digits(12_345), 5);
    }

    #[test]
    fn clicks_land_on_the_nearest_character_boundary() {
        let line = "abc\u{4e2d}d";
        assert_eq!(char_at_visual(line, 0), 0);
        assert_eq!(char_at_visual(line, 2), 2);
        assert_eq!(char_at_visual(line, 3), 3);
        assert_eq!(char_at_visual(line, 5), 4, "after the wide character");
        assert_eq!(char_at_visual(line, 99), 5, "past the end: the end");
        assert_eq!(char_at_visual("", 4), 0);
        // A click in the second half of a tab lands after it.
        assert_eq!(char_at_visual("\tx", TAB_WIDTH), 1);
    }
}
