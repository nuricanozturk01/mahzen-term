//! ANSI styling helpers and visible-width measurement.

use unicode_width::UnicodeWidthStr;

/// A text style. Colours are ANSI palette indexes (0-15), so themes restyle them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub dim: bool,
    pub fg: Option<u8>,
    pub bg: Option<u8>,
}

pub const PLAIN: Style = Style {
    bold: false,
    italic: false,
    underline: false,
    strike: false,
    dim: false,
    fg: None,
    bg: None,
};
pub const DIM: Style = Style { dim: true, ..PLAIN };

impl Style {
    pub const fn fg(c: u8) -> Self {
        Style {
            fg: Some(c),
            ..PLAIN
        }
    }
    pub const fn bold(self) -> Self {
        Style { bold: true, ..self }
    }
    fn is_plain(self) -> bool {
        self == PLAIN
    }
    fn sgr(self) -> String {
        let mut p: Vec<String> = Vec::new();
        if self.bold {
            p.push("1".into());
        }
        if self.dim {
            p.push("2".into());
        }
        if self.italic {
            p.push("3".into());
        }
        if self.underline {
            p.push("4".into());
        }
        if self.strike {
            p.push("9".into());
        }
        if let Some(c) = self.fg {
            p.push(if c < 8 {
                format!("{}", 30 + c)
            } else {
                format!("{}", 90 + c - 8)
            });
        }
        if let Some(c) = self.bg {
            p.push(if c < 8 {
                format!("{}", 40 + c)
            } else {
                format!("{}", 100 + c - 8)
            });
        }
        format!("\x1b[{}m", p.join(";"))
    }
}

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

/// Wrap `text` in `style` (unchanged when colour is off or the style is plain).
pub fn paint(text: &str, style: Style, color: bool) -> String {
    if !color || style.is_plain() || text.is_empty() {
        text.to_owned()
    } else {
        format!("{}{text}\x1b[0m", style.sgr())
    }
}

/// Render spans to one string.
pub fn render_spans(spans: &[Span], color: bool) -> String {
    spans
        .iter()
        .map(|s| paint(&s.text, s.style, color))
        .collect()
}

/// Terminal columns taken by `s` (no ANSI inside).
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Remove ANSI escape sequences (for width checks and tests).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Cut `s` to at most `max` columns, ending in `…` when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Greedy word wrap of styled text into lines of at most `max` columns. Overlong words are split.
pub fn wrap(spans: &[Span], max: usize) -> Vec<Vec<Span>> {
    let max = max.max(1);
    let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
    let mut used = 0usize;
    let mut pending_space: Option<Style> = None;
    for span in spans {
        let mut words = span.text.split(' ').peekable();
        let mut first = true;
        while let Some(word) = words.next() {
            if !first || span.text.starts_with(' ') && word.is_empty() {
                pending_space = Some(span.style);
            }
            first = false;
            if word.is_empty() {
                continue;
            }
            let ww = width(word);
            let space = usize::from(pending_space.is_some() && used > 0);
            if used > 0 && used + space + ww > max {
                lines.push(Vec::new());
                used = 0;
            } else if space == 1 {
                if let (Some(st), Some(line)) = (pending_space, lines.last_mut()) {
                    line.push(Span::new(" ", st));
                }
                used += 1;
            }
            pending_space = None;
            // Split a word wider than the line.
            let mut rest = word;
            while width(rest) > max {
                let mut cut = 0;
                let mut w = 0;
                for (i, c) in rest.char_indices() {
                    let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                    if w + cw > max.saturating_sub(used).max(1) {
                        break;
                    }
                    w += cw;
                    cut = i + c.len_utf8();
                }
                let (head, tail) =
                    rest.split_at(cut.max(rest.chars().next().map_or(0, char::len_utf8)));
                if let Some(line) = lines.last_mut() {
                    line.push(Span::new(head, span.style));
                }
                lines.push(Vec::new());
                used = 0;
                rest = tail;
            }
            if let Some(line) = lines.last_mut() {
                line.push(Span::new(rest, span.style));
            }
            used += width(rest);
            if words.peek().is_some() {
                pending_space = Some(span.style);
            }
        }
    }
    if lines.len() > 1 && lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Vec<Span>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn wraps_on_words_and_keeps_styles() {
        let spans = vec![
            Span::new("hello big ", PLAIN),
            Span::new("bold world", Style::default().bold()),
        ];
        assert_eq!(plain(&wrap(&spans, 10)), ["hello big", "bold world"]);
        assert_eq!(plain(&wrap(&spans, 9)), ["hello big", "bold", "world"]);
        assert_eq!(plain(&wrap(&spans, 80)), ["hello big bold world"]);
    }

    #[test]
    fn overlong_words_are_split() {
        let spans = vec![Span::new("abcdefghij", PLAIN)];
        assert_eq!(plain(&wrap(&spans, 4)), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn width_counts_wide_characters_and_strip_removes_styling() {
        assert_eq!(width("日本"), 4);
        assert_eq!(strip_ansi(&paint("x", Style::fg(1).bold(), true)), "x");
        assert_eq!(paint("x", Style::fg(1), false), "x");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
