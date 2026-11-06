//! Source code and plain text: line numbers and a small, dependency-free highlighter.

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

/// Language name for a file extension, if the highlighter knows it.
pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "java" => "java",
        "kt" => "kotlin",
        "sh" | "bash" | "zsh" => "shell",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "sql" => "sql",
        "html" | "xml" => "markup",
        "css" => "css",
        "lua" => "lua",
        "rb" => "ruby",
        _ => return None,
    })
}

fn keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" => &[
            "fn", "let", "mut", "pub", "struct", "enum", "impl", "trait", "use", "mod", "match",
            "if", "else", "for", "while", "loop", "return", "self", "Self", "crate", "const",
            "static", "async", "await", "move", "where", "as", "in", "ref", "type", "unsafe",
            "dyn", "true", "false", "break", "continue",
        ],
        "python" => &[
            "def", "class", "import", "from", "return", "if", "elif", "else", "for", "while", "in",
            "not", "and", "or", "is", "None", "True", "False", "with", "as", "try", "except",
            "finally", "raise", "lambda", "yield", "pass", "break", "continue", "async", "await",
            "self",
        ],
        "javascript" | "typescript" => &[
            "function",
            "const",
            "let",
            "var",
            "return",
            "if",
            "else",
            "for",
            "while",
            "class",
            "extends",
            "import",
            "export",
            "from",
            "default",
            "new",
            "this",
            "async",
            "await",
            "try",
            "catch",
            "throw",
            "true",
            "false",
            "null",
            "undefined",
            "typeof",
            "switch",
            "case",
            "break",
            "continue",
            "interface",
            "type",
            "enum",
        ],
        "go" => &[
            "func",
            "package",
            "import",
            "var",
            "const",
            "type",
            "struct",
            "interface",
            "return",
            "if",
            "else",
            "for",
            "range",
            "switch",
            "case",
            "go",
            "defer",
            "chan",
            "map",
            "nil",
            "true",
            "false",
            "select",
        ],
        "c" | "cpp" | "java" | "kotlin" => &[
            "int",
            "char",
            "void",
            "long",
            "short",
            "float",
            "double",
            "unsigned",
            "struct",
            "enum",
            "class",
            "public",
            "private",
            "protected",
            "static",
            "const",
            "return",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "new",
            "null",
            "true",
            "false",
            "this",
            "include",
            "define",
            "fun",
            "val",
            "var",
            "namespace",
            "template",
            "typedef",
        ],
        "shell" | "make" | "docker" => &[
            "if", "then", "else", "elif", "fi", "for", "do", "done", "while", "case", "esac",
            "function", "in", "echo", "export", "local", "return", "FROM", "RUN", "COPY", "CMD",
            "ENV", "WORKDIR", "ARG",
        ],
        "sql" => &[
            "select", "from", "where", "insert", "into", "update", "delete", "create", "table",
            "join", "on", "group", "by", "order", "limit", "and", "or", "not", "null", "as",
            "SELECT", "FROM", "WHERE", "INSERT", "INTO", "UPDATE", "DELETE", "CREATE", "TABLE",
            "JOIN", "ON", "GROUP", "BY", "ORDER", "LIMIT", "AND", "OR", "NOT", "NULL", "AS",
        ],
        "lua" | "ruby" => &[
            "function", "local", "end", "if", "then", "else", "elseif", "for", "while", "do",
            "return", "def", "class", "module", "nil", "true", "false", "require", "end",
        ],
        "toml" | "yaml" | "json" => &["true", "false", "null"],
        _ => &[],
    }
}

fn line_comment(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" | "javascript" | "typescript" | "go" | "c" | "cpp" | "java" | "kotlin" => Some("//"),
        "python" | "shell" | "make" | "docker" | "toml" | "yaml" | "ruby" => Some("#"),
        "sql" | "lua" => Some("--"),
        _ => None,
    }
}

fn has_block_comments(lang: &str) -> bool {
    matches!(
        lang,
        "rust"
            | "javascript"
            | "typescript"
            | "go"
            | "c"
            | "cpp"
            | "java"
            | "kotlin"
            | "css"
            | "sql"
    )
}

/// Highlight one line. `in_block` carries `/* ... */` state across lines.
pub fn highlight_line(lang: &str, line: &str, in_block: &mut bool) -> Vec<Span> {
    if lang.is_empty() {
        return vec![Span::new(line, PLAIN)];
    }
    let kws = keywords(lang);
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<Span> = Vec::new();
    let mut i = 0;
    let push = |out: &mut Vec<Span>, s: String, st: Style| {
        if !s.is_empty() {
            out.push(Span::new(s, st));
        }
    };
    let starts = |i: usize, pat: &str| {
        let p: Vec<char> = pat.chars().collect();
        chars.len() >= i + p.len() && chars[i..i + p.len()] == p[..]
    };
    while i < chars.len() {
        if *in_block {
            let start = i;
            while i < chars.len() && !starts(i, "*/") {
                i += 1;
            }
            if i < chars.len() {
                i += 2;
                *in_block = false;
            }
            push(&mut out, chars[start..i].iter().collect(), COMMENT);
            continue;
        }
        let c = chars[i];
        if has_block_comments(lang) && starts(i, "/*") {
            *in_block = true;
            let start = i;
            i += 2;
            while i < chars.len() && !starts(i, "*/") {
                i += 1;
            }
            if i < chars.len() {
                i += 2;
                *in_block = false;
            }
            push(&mut out, chars[start..i].iter().collect(), COMMENT);
            continue;
        }
        if let Some(lc) = line_comment(lang) {
            if starts(i, lc) {
                push(&mut out, chars[i..].iter().collect(), COMMENT);
                break;
            }
        }
        if c == '"' || c == '\'' || (c == '`' && lang != "shell") {
            // A lifetime or label in Rust ('a) is not a string.
            let is_lifetime = lang == "rust"
                && c == '\''
                && chars.get(i + 2).is_some_and(|n| *n != '\'')
                && chars.get(i + 1).is_some_and(|n| n.is_alphabetic());
            if !is_lifetime {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != c {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i = (i + 1).min(chars.len());
                push(&mut out, chars[start..i].iter().collect(), STRING);
                continue;
            }
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            push(&mut out, chars[start..i].iter().collect(), NUMBER);
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let style = if kws.contains(&word.as_str()) {
                KEYWORD
            } else if word.chars().next().is_some_and(char::is_uppercase)
                && word.chars().any(char::is_lowercase)
            {
                TYPE
            } else {
                PLAIN
            };
            push(&mut out, word, style);
            continue;
        }
        let start = i;
        while i < chars.len()
            && !(chars[i].is_alphanumeric() || matches!(chars[i], '_' | '"' | '\'' | '`'))
            && !(has_block_comments(lang) && starts(i, "/*"))
            && line_comment(lang).is_none_or(|lc| !starts(i, lc))
        {
            i += 1;
        }
        if i == start {
            i += 1;
        }
        push(&mut out, chars[start..i].iter().collect(), PLAIN);
    }
    out
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
