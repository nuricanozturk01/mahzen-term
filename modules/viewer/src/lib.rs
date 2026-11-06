//! File rendering for the terminal: markdown, source code, CSV and plain text as styled ANSI text.
//!
//! Pure functions with no UI or OS dependencies, so the `tf` command and in-app previews share one renderer.
//! Colours use the terminal's 16 ANSI slots, so every MahzenTerm theme restyles the output.

mod ansi;
mod code;
mod csv;
mod markdown;

use std::path::Path;

pub use ansi::strip_ansi;

/// How to render.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Output width in columns (text wraps to it).
    pub width: usize,
    /// Emit ANSI styling; `false` gives plain text for pipes.
    pub color: bool,
    /// Number the lines of source files.
    pub line_numbers: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 80,
            color: true,
            line_numbers: true,
        }
    }
}

/// What kind of content a file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Csv,
    /// Source or config text; the string is the language name used for highlighting.
    Code(String),
    Text,
    Binary,
}

/// Decide how to show a file from its name and first bytes.
pub fn detect(path: &Path, head: &[u8]) -> Kind {
    if head.contains(&0) {
        return Kind::Binary;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" | "mdown" => Kind::Markdown,
        "csv" | "tsv" => Kind::Csv,
        "" => match path.file_name().and_then(|n| n.to_str()) {
            Some("Makefile" | "makefile") => Kind::Code("make".into()),
            Some("Dockerfile") => Kind::Code("docker".into()),
            _ => Kind::Text,
        },
        e => match code::language_for_extension(e) {
            Some(l) => Kind::Code(l.to_owned()),
            None => Kind::Text,
        },
    }
}

/// Render a file's bytes. Invalid UTF-8 is replaced, never an error.
pub fn render(path: &Path, bytes: &[u8], opts: &Options) -> String {
    let head = &bytes[..bytes.len().min(8192)];
    match detect(path, head) {
        Kind::Binary => format!(
            "{}\n",
            ansi::paint(
                &format!("binary file, {} bytes", bytes.len()),
                ansi::DIM,
                opts.color
            )
        ),
        Kind::Markdown => markdown::render(&String::from_utf8_lossy(bytes), opts),
        Kind::Csv => csv::render(&String::from_utf8_lossy(bytes), path, opts),
        Kind::Code(lang) => code::render(&String::from_utf8_lossy(bytes), &lang, opts),
        Kind::Text => code::render(&String::from_utf8_lossy(bytes), "", opts),
    }
}

/// Render markdown text directly.
pub fn render_markdown(src: &str, opts: &Options) -> String {
    markdown::render(src, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn kinds_follow_extension_and_content() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(detect(&p("a.md"), b"# hi"), Kind::Markdown);
        assert_eq!(detect(&p("a.CSV"), b"a,b"), Kind::Csv);
        assert_eq!(detect(&p("a.rs"), b"fn x(){}"), Kind::Code("rust".into()));
        assert_eq!(detect(&p("notes"), b"hello"), Kind::Text);
        assert_eq!(detect(&p("a.rs"), b"\0\x01"), Kind::Binary);
        assert_eq!(detect(&p("Makefile"), b"all:"), Kind::Code("make".into()));
    }

    #[test]
    fn binary_files_are_summarised() {
        let out = render(
            Path::new("x.bin"),
            &[0, 1, 2],
            &Options {
                color: false,
                ..Options::default()
            },
        );
        assert_eq!(out, "binary file, 3 bytes\n");
    }
}
