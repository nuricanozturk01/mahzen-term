//! A small, dependency-free source highlighter shared by the file viewer (`tf cat`) and the built-in editor.
//!
//! It knows enough about common languages to colour keywords, strings, numbers, comments and type names, one line at a
//! time (the only state carried between lines is "inside a block comment"). It never loses or adds text: the pieces of a
//! line, joined, are the line. Colours are the caller's business: a [`Token`] is only a class.

/// What a piece of source text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Plain,
    Keyword,
    String,
    Number,
    Comment,
    Type,
}

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

/// The line comment marker of a language (`//`, `#` ...), if it has one.
pub fn line_comment(lang: &str) -> Option<&'static str> {
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
pub fn highlight_line(lang: &str, line: &str, in_block: &mut bool) -> Vec<(Token, String)> {
    if lang.is_empty() {
        return vec![(Token::Plain, line.to_owned())];
    }
    let kws = keywords(lang);
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(Token, String)> = Vec::new();
    let mut i = 0;
    let push = |out: &mut Vec<(Token, String)>, s: String, st: Token| {
        if !s.is_empty() {
            out.push((st, s));
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
            push(&mut out, chars[start..i].iter().collect(), Token::Comment);
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
            push(&mut out, chars[start..i].iter().collect(), Token::Comment);
            continue;
        }
        if let Some(lc) = line_comment(lang) {
            if starts(i, lc) {
                push(&mut out, chars[i..].iter().collect(), Token::Comment);
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
                push(&mut out, chars[start..i].iter().collect(), Token::String);
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
            push(&mut out, chars[start..i].iter().collect(), Token::Number);
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let style = if kws.contains(&word.as_str()) {
                Token::Keyword
            } else if word.chars().next().is_some_and(char::is_uppercase)
                && word.chars().any(char::is_lowercase)
            {
                Token::Type
            } else {
                Token::Plain
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
        push(&mut out, chars[start..i].iter().collect(), Token::Plain);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(lang: &str, line: &str) -> Vec<(String, Token)> {
        let mut b = false;
        highlight_line(lang, line, &mut b)
            .into_iter()
            .map(|(t, s)| (s, t))
            .collect()
    }

    #[test]
    fn rust_keywords_strings_comments_numbers() {
        let k = pieces("rust", "let x = \"hi\"; // note 42");
        assert!(k.contains(&("let".into(), Token::Keyword)));
        assert!(k.contains(&("\"hi\"".into(), Token::String)));
        assert!(k
            .iter()
            .any(|(t, c)| t.starts_with("// note") && *c == Token::Comment));
        assert!(pieces("rust", "x = 42;").contains(&("42".into(), Token::Number)));
        assert!(pieces("rust", "let v: Vec<String>").contains(&("Vec".into(), Token::Type)));
    }

    #[test]
    fn highlighting_never_loses_or_adds_text() {
        for lang in ["rust", "python", "c", "shell", "toml", "unknown", ""] {
            for line in [
                "fn main() { println!(\"a\\\"b\"); }",
                "'a: loop {}",
                "# comment",
                "x /* y */ z",
                "",
                "\u{e9}t\u{e9} = 'x' + 12.5e3",
            ] {
                let mut b = false;
                let joined: String = highlight_line(lang, line, &mut b)
                    .into_iter()
                    .map(|(_, s)| s)
                    .collect();
                assert_eq!(joined, line, "{lang}: {line}");
            }
        }
    }

    #[test]
    fn block_comments_span_lines() {
        let mut b = false;
        highlight_line("c", "a /* start", &mut b);
        assert!(b);
        let spans = highlight_line("c", "still */ int", &mut b);
        assert!(!b);
        assert_eq!(spans[0].0, Token::Comment);
    }

    #[test]
    fn extensions_map_to_languages() {
        assert_eq!(language_for_extension("rs"), Some("rust"));
        assert_eq!(language_for_extension("yml"), Some("yaml"));
        assert_eq!(language_for_extension("zzz"), None);
    }
}
