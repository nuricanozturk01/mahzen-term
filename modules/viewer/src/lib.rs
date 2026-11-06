//! File rendering for the terminal: markdown, source code, CSV, Office documents (Word, PowerPoint, spreadsheets,
//! OpenDocument) and plain text as styled ANSI text.
//!
//! Pure functions with no UI or OS dependencies, so the `tf` command and in-app previews share one renderer.
//! Colours use the terminal's 16 ANSI slots, so every MahzenTerm theme restyles the output.

mod ansi;
pub mod archive;
mod code;
mod csv;
pub mod docs;
pub mod ebook;
pub mod hexview;
pub mod lint;
pub mod logview;
mod markdown;
pub mod notebook;

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
    /// Excel, OpenDocument or legacy spreadsheet.
    Spreadsheet,
    /// Word document.
    Docx,
    /// PowerPoint presentation.
    Pptx,
    /// OpenDocument text.
    Odt,
    /// Jupyter notebook.
    Notebook,
    /// EPUB e-book.
    Epub,
    /// Rich Text Format.
    Rtf,
    /// zip, tar or tar.gz archive.
    Archive,
    /// A log file.
    Log,
    /// Source or config text; the string is the language name used for highlighting.
    Code(String),
    Text,
    Binary,
}

/// Decide how to show a file from its name and first bytes.
pub fn detect(path: &Path, head: &[u8]) -> Kind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    // Office files are binary on the inside (zip or old formats) but have a renderer: check them first.
    match ext.as_str() {
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => return Kind::Spreadsheet,
        "docx" | "docm" => return Kind::Docx,
        "pptx" | "pptm" => return Kind::Pptx,
        "odt" => return Kind::Odt,
        "ipynb" => return Kind::Notebook,
        "epub" => return Kind::Epub,
        "rtf" => return Kind::Rtf,
        "log" if !head.contains(&0) => return Kind::Log,
        _ if archive::format_of(path).is_some() => return Kind::Archive,
        _ => {}
    }
    if head.contains(&0) {
        return Kind::Binary;
    }
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
        Kind::Binary => {
            // Say what it is when the first bytes tell, and which command shows more.
            let what = hexview::identify(bytes).map_or_else(String::new, |d| format!(" ({d})"));
            let hint = if what.contains("SQLite") {
                "; browse it with `tf sql FILE`"
            } else if what.is_empty() {
                ""
            } else {
                "; look inside with `tf hex FILE`"
            };
            format!(
                "{}\n",
                ansi::paint(
                    &format!("binary file, {} bytes{what}{hint}", bytes.len()),
                    ansi::DIM,
                    opts.color
                )
            )
        }
        Kind::Markdown => markdown::render(&String::from_utf8_lossy(bytes), opts),
        Kind::Csv => csv::render(&String::from_utf8_lossy(bytes), path, opts),
        Kind::Spreadsheet => docs::render_spreadsheet(bytes, opts),
        Kind::Docx => docs::render_docx(bytes, opts),
        Kind::Pptx => docs::render_pptx(bytes, opts),
        Kind::Odt => docs::render_odt(bytes, opts),
        Kind::Notebook => notebook::render(bytes, opts),
        Kind::Epub => ebook::render_epub(bytes, opts),
        Kind::Rtf => ebook::render_rtf(bytes, opts),
        Kind::Archive => archive::render_list(bytes, path, opts),
        Kind::Log => logview::render(&String::from_utf8_lossy(bytes), None, opts),
        Kind::Code(lang) => code::render(&String::from_utf8_lossy(bytes), &lang, opts),
        Kind::Text => code::render(&String::from_utf8_lossy(bytes), "", opts),
    }
}

/// Draw rows as a table (the first row is the header) fitting `opts.width`.
pub fn render_table(rows: &[Vec<String>], opts: &Options) -> String {
    csv::table(rows, opts)
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
        // Office files contain NUL bytes but are not "binary": they have renderers.
        assert_eq!(detect(&p("a.XLSX"), b"PK\0\0"), Kind::Spreadsheet);
        assert_eq!(detect(&p("a.docx"), b"PK\0\0"), Kind::Docx);
        assert_eq!(detect(&p("a.pptx"), b"PK\0\0"), Kind::Pptx);
        assert_eq!(detect(&p("a.odt"), b"PK\0\0"), Kind::Odt);
        assert_eq!(detect(&p("a.ods"), b"PK\0\0"), Kind::Spreadsheet);
        assert_eq!(detect(&p("n.ipynb"), b"{}"), Kind::Notebook);
        assert_eq!(detect(&p("b.EPUB"), b"PK\0\0"), Kind::Epub);
        assert_eq!(detect(&p("d.rtf"), b"{\\rtf"), Kind::Rtf);
        assert_eq!(detect(&p("a.tar.gz"), b"\x1f\x8b\0"), Kind::Archive);
        assert_eq!(detect(&p("a.zip"), b"PK\0\0"), Kind::Archive);
        assert_eq!(detect(&p("server.log"), b"INFO x"), Kind::Log);
        assert_eq!(
            detect(&p("weird.log"), b"\0\x01"),
            Kind::Binary,
            "a binary .log is not rendered as text"
        );
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
        let db = render(
            Path::new("x.db"),
            b"SQLite format 3\0abc",
            &Options {
                color: false,
                ..Options::default()
            },
        );
        assert!(
            db.contains("SQLite 3 database") && db.contains("tf sql"),
            "{db}"
        );
        let elf = render(
            Path::new("x"),
            b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x03\0\x3e\0",
            &Options {
                color: false,
                ..Options::default()
            },
        );
        assert!(
            elf.contains("ELF 64-bit") || elf.contains("binary file"),
            "{elf}"
        );
    }
}
