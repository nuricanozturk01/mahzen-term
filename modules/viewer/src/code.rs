//! Source code and plain text: line numbers and a small, dependency-free highlighter.

use mahzenterm_core::syntax::{self, Token};

use crate::ansi::{self, paint, Span, Style, DIM, PLAIN};
use crate::Options;

const KEYWORD: Style = Style::fg(5);
const STRING: Style = Style::fg(2);
const NUMBER: Style = Style::fg(3);
const COMMENT: Style = Style {
    dim: true,
    italic: true,
    ..Style::fg(7)
};
const TYPE: Style = Style::fg(6);

/// The terminal style of a token class (the 16 ANSI slots, so every theme restyles it).
fn style_of(t: Token) -> Style {
    match t {
        Token::Plain => PLAIN,
        Token::Keyword => KEYWORD,
        Token::String => STRING,
        Token::Number => NUMBER,
        Token::Comment => COMMENT,
        Token::Type => TYPE,
    }
}

/// Language name for a file extension, if the highlighter knows it.
pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    syntax::language_for_extension(ext)
}

/// Highlight one line. `in_block` carries `/* ... */` state across lines.
pub fn highlight_line(lang: &str, line: &str, in_block: &mut bool) -> Vec<Span> {
    syntax::highlight_line(lang, line, in_block)
        .into_iter()
        .map(|(t, s)| Span::new(s, style_of(t)))
        .collect()
}

/// Render source text with a line-number gutter.
pub fn render(src: &str, lang: &str, opts: &Options) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let gutter = lines.len().max(1).to_string().len();
    let mut out = String::new();
    let mut in_block = false;
    for (n, line) in lines.iter().enumerate() {
        let line = line.replace('\t', "    ");
        if opts.line_numbers {
            out.push_str(&paint(
                &format!("{:>gutter$} \u{2502} ", n + 1),
                DIM,
                opts.color,
            ));
        }
        let spans = highlight_line(lang, &line, &mut in_block);
        out.push_str(&ansi::render_spans(&spans, opts.color));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: &str, line: &str) -> Vec<(String, Option<u8>)> {
        let mut b = false;
        highlight_line(lang, line, &mut b)
            .into_iter()
            .map(|s| (s.text, s.style.fg))
            .collect()
    }

    #[test]
    fn rust_keywords_strings_comments_numbers() {
        let k = kinds("rust", "let x = \"hi\"; // note 42");
        assert!(k.contains(&("let".into(), Some(5))));
        assert!(k.contains(&("\"hi\"".into(), Some(2))));
        assert!(k
            .iter()
            .any(|(t, c)| t.starts_with("// note") && *c == Some(7)));
        assert!(kinds("rust", "x = 42;").contains(&("42".into(), Some(3))));
    }

    #[test]
    fn highlighting_never_loses_or_adds_text() {
        for line in [
            "fn main() { println!(\"a\\\"b\"); }",
            "'a: loop {}",
            "# comment",
            "x /* y */ z",
            "",
        ] {
            let mut b = false;
            let joined: String = highlight_line("rust", line, &mut b)
                .into_iter()
                .map(|s| s.text)
                .collect();
            assert_eq!(joined, line);
        }
    }

    #[test]
    fn block_comments_span_lines() {
        let mut b = false;
        highlight_line("c", "a /* start", &mut b);
        assert!(b);
        let spans = highlight_line("c", "still */ int", &mut b);
        assert!(!b);
        assert_eq!(spans[0].style, COMMENT);
    }

    #[test]
    fn numbered_output_has_a_gutter_and_plain_mode_has_no_escapes() {
        let o = Options {
            color: false,
            ..Options::default()
        };
        assert_eq!(render("a\nb", "", &o), "1 \u{2502} a\n2 \u{2502} b\n");
    }
}
