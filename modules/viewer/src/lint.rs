//! Syntax check for config and data files (TOML and JSON): where the first error is, with a source excerpt.

use std::path::Path;

/// A problem in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// 1-based line and column.
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// Formats that can be checked, by file name.
pub fn supported(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("toml" | "json" | "jsonc" | "geojson" | "yaml" | "yml")
    )
}

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..text.floor_char_boundary(offset)];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// Check `text` as the format the file name says. `Ok(None)` means it parses; an unknown format is an error message.
pub fn check(path: &Path, text: &str) -> Result<Option<Problem>, String> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("toml") => Ok(toml::from_str::<toml::Table>(text).err().map(|e| {
            let (line, col) = e.span().map_or((1, 1), |s| line_col(text, s.start));
            Problem {
                line,
                col,
                message: e.message().to_owned(),
            }
        })),
        Some("json" | "geojson") => {
            Ok(serde_json::from_str::<serde_json::Value>(text)
                .err()
                .map(|e| Problem {
                    line: e.line().max(1),
                    col: e.column().max(1),
                    message: e
                        .to_string()
                        .split(" at line ")
                        .next()
                        .unwrap_or("invalid JSON")
                        .to_owned(),
                }))
        }
        Some("yaml" | "yml") => {
            Ok(yaml_rust2::YamlLoader::load_from_str(text)
                .err()
                .map(|e| Problem {
                    line: e.marker().line().max(1),
                    col: e.marker().col() + 1,
                    message: e.info().to_owned(),
                }))
        }
        _ => Err("only TOML, JSON and YAML files can be checked".to_owned()),
    }
}

/// The offending line with a caret under the column.
pub fn excerpt(text: &str, p: &Problem) -> String {
    let line = text.lines().nth(p.line - 1).unwrap_or("");
    let shown: String = line.chars().take(120).collect();
    format!(
        "{:>5} | {shown}\n      | {}^",
        p.line,
        " ".repeat(p.col.saturating_sub(1).min(120))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_errors_have_a_position_and_a_message() {
        let text = "name = \"x\"\n[section]\nvalue = = 3\n";
        let p = check(Path::new("a.toml"), text)
            .unwrap()
            .expect("a problem");
        assert_eq!(p.line, 3, "{p:?}");
        assert!(!p.message.is_empty());
        assert!(check(Path::new("a.toml"), "a = 1\n[t]\nb = \"ok\"\n")
            .unwrap()
            .is_none());
        let ex = excerpt(text, &p);
        assert!(ex.contains("value = = 3") && ex.contains('^'), "{ex}");
    }

    #[test]
    fn json_errors_have_a_position() {
        let p = check(Path::new("a.json"), "{\n  \"a\": 1,\n  \"b\": ,\n}\n")
            .unwrap()
            .expect("a problem");
        assert_eq!(p.line, 3, "{p:?}");
        assert!(check(Path::new("a.json"), "{\"a\": [1, 2, {\"b\": null}]}")
            .unwrap()
            .is_none());
        assert!(
            check(Path::new("a.json"), "").unwrap().is_some(),
            "empty input is not JSON"
        );
    }

    #[test]
    fn yaml_errors_have_a_position() {
        let p = check(Path::new("a.yaml"), "a: 1\nb: [1, 2\nc: 3\n")
            .unwrap()
            .expect("a problem");
        assert!(p.line >= 2, "{p:?}");
        assert!(!p.message.is_empty());
        assert!(check(Path::new("a.yml"), "a: 1\nlist:\n  - x\n  - y\n")
            .unwrap()
            .is_none());
        assert!(
            check(Path::new("a.yaml"), "a: 1\n\tb: 2\n")
                .unwrap()
                .is_some(),
            "a tab as indentation is invalid"
        );
        assert!(supported(Path::new("c.YML")));
    }

    #[test]
    fn unknown_formats_and_odd_text_are_handled() {
        assert!(check(Path::new("a.txt"), "x").is_err());
        assert!(supported(Path::new("A.TOML")) && !supported(Path::new("a.rs")));
        // Multi-byte characters before the error do not break the column.
        let p = check(Path::new("a.toml"), "k = \"\u{e9}\u{e9}\u{e9}\" x\n")
            .unwrap()
            .expect("a problem");
        assert_eq!(p.line, 1);
        assert!(line_col("\u{e9}\u{e9}\nab", 5) == (2, 1));
    }
}
