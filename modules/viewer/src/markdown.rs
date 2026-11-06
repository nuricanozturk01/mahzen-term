//! Markdown to styled terminal text (headings, lists, quotes, code blocks, tables, links).

use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, Options as MdOptions, Parser, Tag, TagEnd,
};

use crate::ansi::{self, paint, render_spans, wrap, Span, Style, DIM, PLAIN};
use crate::code::{highlight_line, language_for_extension};
use crate::csv;
use crate::Options;

const H1: Style = Style::fg(14).bold();
const H2: Style = Style::fg(12).bold();
const H3: Style = Style::fg(13).bold();
const CODE: Style = Style::fg(11);
const LINK: Style = Style {
    underline: true,
    ..Style::fg(4)
};

/// Line prefix of one container level: the bullet or marker on its first line, indentation afterwards.
struct Ctx {
    first: Option<String>,
    rest: String,
    style: Style,
}

struct Renderer<'a> {
    opts: &'a Options,
    out: String,
    ctx: Vec<Ctx>,
    spans: Vec<Span>,
    style_stack: Vec<Style>,
    lists: Vec<Option<u64>>,
    code: Option<(String, String)>,
    table: Option<Table>,
    /// Target of the open link and where its text starts in `spans`.
    link: Option<(String, usize)>,
    /// A blank line is owed before the next block.
    gap: bool,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    cur: Vec<String>,
    cell: String,
}

impl<'a> Renderer<'a> {
    fn new(opts: &'a Options) -> Self {
        Self {
            opts,
            out: String::new(),
            ctx: Vec::new(),
            spans: Vec::new(),
            style_stack: vec![PLAIN],
            lists: Vec::new(),
            code: None,
            table: None,
            link: None,
            gap: false,
        }
    }

    fn style(&self) -> Style {
        self.style_stack.last().copied().unwrap_or(PLAIN)
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let s = f(self.style());
        self.style_stack.push(s);
    }

    fn prefix_width(&self) -> usize {
        self.ctx
            .iter()
            .map(|c| ansi::width(c.first.as_deref().unwrap_or(&c.rest)).max(ansi::width(&c.rest)))
            .sum()
    }

    /// Write one output line with the container prefixes in front.
    fn emit(&mut self, spans: &[Span]) {
        let mut line = String::new();
        for c in &mut self.ctx {
            let text = c.first.take().unwrap_or_else(|| c.rest.clone());
            line.push_str(&paint(&text, c.style, self.opts.color));
        }
        line.push_str(&render_spans(spans, self.opts.color));
        self.out.push_str(line.trim_end_matches(' '));
        self.out.push('\n');
    }

    /// Blank separator line between blocks (carries quote bars so quotes stay connected).
    fn blank(&mut self) {
        let mut line = String::new();
        for c in &self.ctx {
            line.push_str(&paint(c.rest.trim_end(), c.style, self.opts.color));
            line.push(' ');
        }
        self.out.push_str(line.trim_end());
        self.out.push('\n');
    }

    fn begin_block(&mut self) {
        if self.gap {
            self.blank();
            self.gap = false;
        }
    }

    fn flush_paragraph(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let avail = self.opts.width.saturating_sub(self.prefix_width()).max(10);
        for line in wrap(&spans, avail) {
            self.emit(&line);
        }
    }

    fn text(&mut self, t: &str) {
        if let Some((_, buf)) = self.code.as_mut() {
            buf.push_str(t);
        } else if let Some(tb) = self.table.as_mut() {
            tb.cell.push_str(t);
        } else {
            let st = self.style();
            self.spans.push(Span::new(t, st));
        }
    }

    fn heading_prefix(level: HeadingLevel) -> (&'static str, Style) {
        match level {
            HeadingLevel::H1 => ("", H1),
            HeadingLevel::H2 => ("", H2),
            HeadingLevel::H3 => ("\u{258d} ", H3),
            _ => ("\u{258f} ", Style::fg(7).bold()),
        }
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(c) => {
                if let Some(tb) = self.table.as_mut() {
                    tb.cell.push_str(&c);
                } else {
                    self.spans.push(Span::new(
                        format!(" {c} "),
                        Style {
                            bg: Some(0),
                            ..CODE
                        },
                    ));
                }
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => {
                self.flush_paragraph();
            }
            Event::Rule => {
                self.begin_block();
                let w = self
                    .opts
                    .width
                    .saturating_sub(self.prefix_width())
                    .clamp(4, 100);
                self.emit(&[Span::new("\u{2500}".repeat(w), DIM)]);
                self.gap = true;
            }
            Event::TaskListMarker(done) => {
                let (mark, st) = if done {
                    ("\u{2611} ", Style::fg(2))
                } else {
                    ("\u{2610} ", DIM)
                };
                self.spans.push(Span::new(mark, st));
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                let st = Style { dim: true, ..PLAIN };
                self.spans.push(Span::new(h.trim_end().to_owned(), st));
            }
            Event::FootnoteReference(n) => self.spans.push(Span::new(format!("[{n}]"), DIM)),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.begin_block(),
            Tag::Heading { level, .. } => {
                self.begin_block();
                let (marker, st) = Self::heading_prefix(level);
                self.style_stack.push(st);
                if !marker.is_empty() {
                    self.spans.push(Span::new(marker, st));
                }
            }
            Tag::BlockQuote(_) => {
                self.begin_block();
                self.ctx.push(Ctx {
                    first: Some("\u{2502} ".into()),
                    rest: "\u{2502} ".into(),
                    style: Style::fg(8),
                });
            }
            Tag::CodeBlock(kind) => {
                self.begin_block();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.begin_block();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush_paragraph();
                let depth = self.lists.len();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => ["\u{2022} ", "\u{25e6} ", "\u{25aa} "][(depth - 1).min(2)].to_owned(),
                };
                let pad = " ".repeat(ansi::width(&marker));
                self.ctx.push(Ctx {
                    first: Some(marker),
                    rest: pad,
                    style: Style::fg(6),
                });
            }
            Tag::Emphasis => self.push_style(|s| Style { italic: true, ..s }),
            Tag::Strong => self.push_style(|s| Style { bold: true, ..s }),
            Tag::Strikethrough => self.push_style(|s| Style { strike: true, ..s }),
            Tag::Link { dest_url, .. } => {
                self.link = Some((dest_url.to_string(), self.spans.len()));
                self.push_style(|_| LINK);
            }
            Tag::Image { dest_url, .. } => {
                self.link = Some((dest_url.to_string(), self.spans.len()));
                self.push_style(|_| DIM);
                self.spans.push(Span::new("\u{1f5bc} ", DIM));
            }
            Tag::Table(_) => {
                self.begin_block();
                self.table = Some(Table::default());
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.cur.clear();
                }
            }
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cell.clear();
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_paragraph();
                if self
                    .ctx
                    .last()
                    .is_none_or(|c| c.first.is_some() || !self.lists.is_empty())
                    || true
                {
                    self.gap = self.lists.is_empty();
                }
            }
            TagEnd::Heading(level) => {
                self.flush_paragraph();
                self.style_stack.pop();
                if level == HeadingLevel::H1 {
                    let w = self
                        .opts
                        .width
                        .saturating_sub(self.prefix_width())
                        .clamp(4, 100);
                    self.emit(&[Span::new("\u{2550}".repeat(w), Style::fg(6))]);
                }
                self.gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.ctx.pop();
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                if let Some((lang, body)) = self.code.take() {
                    self.code_block(&lang, &body);
                }
                self.gap = true;
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.gap = true;
                }
            }
            TagEnd::Item => {
                self.flush_paragraph();
                self.ctx.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.style_stack.pop();
            }
            TagEnd::Link | TagEnd::Image => {
                self.style_stack.pop();
                if let Some((url, from)) = self.link.take() {
                    let shown: String = self.spans[from.min(self.spans.len())..]
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect();
                    // Tables keep plain cell text, and a link whose text is its address needs no repeat.
                    if self.table.is_none() && shown.trim() != url {
                        self.spans.push(Span::new(format!(" ({url})"), DIM));
                    }
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let c = std::mem::take(&mut t.cell);
                    t.cur.push(c);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.cur);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    let width = self.opts.width.saturating_sub(self.prefix_width());
                    let rendered = csv::table(
                        &t.rows,
                        &Options {
                            width,
                            ..*self.opts
                        },
                    );
                    for l in rendered.lines() {
                        self.emit(&[Span::new(l, PLAIN)]);
                    }
                }
                self.gap = true;
            }
            _ => {}
        }
    }

    fn code_block(&mut self, lang: &str, body: &str) {
        let lang = language_for_extension(lang).map_or(lang, |l| l);
        let mut in_block = false;
        let body = body.trim_end_matches('\n');
        let width = self.opts.width.saturating_sub(self.prefix_width()).max(8);
        for line in body.split('\n') {
            let line = line.replace('\t', "    ");
            let mut spans = vec![Span::new("\u{2502} ", Style::fg(8))];
            spans.extend(highlight_line(lang, &line, &mut in_block));
            // Long lines are cut, not wrapped, so code keeps its shape.
            let text: String = spans.iter().map(|s| s.text.as_str()).collect();
            if ansi::width(&text) > width {
                let mut budget = width.saturating_sub(1);
                let mut cut = Vec::new();
                for s in spans {
                    if budget == 0 {
                        break;
                    }
                    let t = ansi::truncate(&s.text, budget + 1);
                    let w = ansi::width(&t).min(budget);
                    budget -= w.min(budget);
                    let end = t.ends_with('\u{2026}');
                    cut.push(Span::new(t, s.style));
                    if end {
                        break;
                    }
                }
                self.emit(&cut);
            } else {
                self.emit(&spans);
            }
        }
    }
}

pub fn render(src: &str, opts: &Options) -> String {
    let md = MdOptions::ENABLE_TABLES
        | MdOptions::ENABLE_STRIKETHROUGH
        | MdOptions::ENABLE_TASKLISTS
        | MdOptions::ENABLE_FOOTNOTES;
    let mut r = Renderer::new(opts);
    for ev in Parser::new_ext(src, md) {
        r.event(ev);
    }
    r.flush_paragraph();
    r.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::strip_ansi;

    fn plain(src: &str, width: usize) -> String {
        render(
            src,
            &Options {
                width,
                color: false,
                line_numbers: false,
            },
        )
    }

    #[test]
    fn headings_paragraphs_and_wrapping() {
        let out = plain("# Title\n\nsome words here that wrap around\n", 20);
        assert!(out.starts_with("Title\n\u{2550}"));
        assert!(out.lines().all(|l| ansi::width(l) <= 20), "{out}");
        assert!(out.contains("some words here that\nwrap around"));
    }

    #[test]
    fn lists_nest_and_number() {
        let out = plain("- a\n- b\n  - c\n\n1. x\n2. y\n", 40);
        assert_eq!(out, "\u{2022} a\n\u{2022} b\n  \u{25e6} c\n\n1. x\n2. y\n");
    }

    #[test]
    fn quote_code_rule_and_link() {
        let out = plain(
            "> quoted\n\n```rust\nlet x = 1;\n```\n\n---\n\n[go](http://x.y)\n",
            30,
        );
        assert!(out.contains("\u{2502} quoted"));
        assert!(out.contains("\u{2502} let x = 1;"));
        assert!(out.contains(&"\u{2500}".repeat(30)));
        assert!(out.contains("go (http://x.y)"));
        assert_eq!(plain("<http://a.b>\n", 40), "http://a.b\n");
    }

    #[test]
    fn tables_render_as_boxes() {
        let out = plain("| a | b |\n|---|---|\n| 1 | 2 |\n", 40);
        assert!(
            out.contains('\u{250c}') && out.contains("\u{2502} 1 \u{2502} 2 \u{2502}"),
            "{out}"
        );
    }

    #[test]
    fn task_items_inline_styles_and_color_output() {
        let out = plain("- [x] done\n- [ ] todo\n\n**b** *i* `c`\n", 40);
        assert!(out.contains("\u{2611} done") && out.contains("\u{2610} todo"));
        let colored = render("**b**", &Options::default());
        assert!(colored.contains("\x1b[1m"));
        assert_eq!(strip_ansi(&colored).trim(), "b");
    }

    #[test]
    fn no_line_exceeds_the_width_for_realistic_input() {
        let src = "# H\n\nA long paragraph with `code`, **bold**, a [link](https://example.com/very/long/url/path) and more text to wrap.\n\n> quote that is also quite long and has to wrap over several lines to be tested\n\n- item one that is long enough to wrap onto a second line for sure\n";
        for w in [24, 40, 80] {
            let out = plain(src, w);
            assert!(
                out.lines().all(|l| ansi::width(l) <= w),
                "width {w}:\n{out}"
            );
        }
    }
}
