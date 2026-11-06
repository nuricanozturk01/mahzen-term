//! Office documents as terminal text: Word (`.docx`), PowerPoint (`.pptx`), OpenDocument text (`.odt`) and
//! spreadsheets (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods`).
//!
//! These files are zip archives of XML (or old binary formats) from unknown sources, so everything is bounded: entry
//! sizes while unpacking, entry counts, rows, columns, sheets and slides. A damaged or hostile file produces a short
//! message, never a panic. Formatting that does not survive in a terminal (fonts, colours, images) is dropped; headings,
//! bold and italic, lists and tables are kept.

use std::io::{Cursor, Read};

use calamine::{open_workbook_auto_from_rs, Data, Reader as _};
use quick_xml::events::{BytesEnd, BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use zip::ZipArchive;

use crate::ansi::{paint, render_spans, wrap, Span, Style, DIM, PLAIN};
use crate::csv;
use crate::Options;

/// Most bytes read from one archive entry.
const MAX_ENTRY: u64 = 32 * 1024 * 1024;
/// Most entries an archive may hold.
const MAX_ENTRIES: usize = 5_000;
const MAX_SHEETS: usize = 20;
const MAX_ROWS: usize = 2_000;
const MAX_COLS: usize = 60;
const MAX_SLIDES: usize = 500;

const H1: Style = Style::fg(14).bold();
const H2: Style = Style::fg(12).bold();
const H3: Style = Style::fg(13).bold();
const H4: Style = Style::fg(7).bold();

type Archive<'a> = ZipArchive<Cursor<&'a [u8]>>;

fn problem(what: &str, why: &str, opts: &Options) -> String {
    format!(
        "{}\n",
        paint(&format!("cannot read {what}: {why}"), DIM, opts.color)
    )
}

fn open_zip(bytes: &[u8]) -> Result<Archive<'_>, String> {
    let z =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a valid archive ({e})"))?;
    if z.len() > MAX_ENTRIES {
        return Err(format!(
            "the archive has {} entries (limit {MAX_ENTRIES})",
            z.len()
        ));
    }
    Ok(z)
}

/// Read one entry, at most `limit` bytes; `None` when missing or larger than the limit.
fn read_entry(z: &mut Archive<'_>, name: &str, limit: u64) -> Option<Vec<u8>> {
    let entry = z.by_name(name).ok()?;
    if entry.size() > limit {
        return None;
    }
    let mut out = Vec::new();
    entry.take(limit + 1).read_to_end(&mut out).ok()?;
    (out.len() as u64 <= limit).then_some(out)
}

/// Local name of a start or empty element (`w:p` -> `p`).
fn start_name(e: &BytesStart<'_>) -> String {
    e.local_name().as_ref().to_owned()
}

/// Local name of an end element.
fn end_name(e: &BytesEnd<'_>) -> String {
    e.local_name().as_ref().to_owned()
}

/// Value of the attribute with this local name (`w:val` -> `val`).
fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// The text an entity reference stands for (`&amp;`, `&#233;`); unknown entities become nothing.
fn entity(r: &BytesRef<'_>) -> String {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return c.to_string();
    }
    match &**r {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    }
    .to_owned()
}

fn heading_style(level: u8) -> Style {
    match level {
        1 => H1,
        2 => H2,
        3 => H3,
        _ => H4,
    }
}

/// Collects blocks of text (headings, paragraphs, list items, tables) into the final output.
struct Out<'a> {
    text: String,
    opts: &'a Options,
}

impl<'a> Out<'a> {
    fn new(opts: &'a Options) -> Self {
        Self {
            text: String::new(),
            opts,
        }
    }

    fn blank(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with("\n\n") {
            self.text.push('\n');
        }
    }

    /// A paragraph of styled runs; `\n` inside a run starts a new line. `heading` colours and bolds it.
    fn paragraph(&mut self, spans: &[Span], heading: Option<u8>, bullet: bool) {
        let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
        if plain.trim().is_empty() {
            self.blank();
            return;
        }
        let spans: Vec<Span> = match heading {
            Some(level) => {
                let st = heading_style(level);
                vec![Span::new(plain.replace('\n', " ").trim().to_owned(), st)]
            }
            None => spans.to_vec(),
        };
        if heading.is_some() {
            self.blank();
        }
        let indent = if bullet { 2 } else { 0 };
        // Split the runs at `\n` into separate lines, then wrap each.
        let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
        for s in &spans {
            let mut parts = s.text.split('\n');
            if let Some(first) = parts.next() {
                if let Some(l) = lines.last_mut() {
                    l.push(Span::new(first, s.style));
                }
            }
            for p in parts {
                lines.push(vec![Span::new(p, s.style)]);
            }
        }
        let mut first_line = true;
        for line in lines {
            for w in wrap(&line, self.opts.width.saturating_sub(indent).max(10)) {
                let prefix = if bullet {
                    if first_line {
                        paint("\u{2022} ", Style::fg(11), self.opts.color)
                    } else {
                        "  ".to_owned()
                    }
                } else {
                    String::new()
                };
                first_line = false;
                self.text.push_str(&prefix);
                self.text.push_str(&render_spans(&w, self.opts.color));
                self.text.push('\n');
            }
        }
        if heading.is_some() || !bullet {
            self.blank();
        }
    }

    fn table(&mut self, rows: &[Vec<String>]) {
        if rows.is_empty() {
            return;
        }
        self.text.push_str(&csv::table(rows, self.opts));
        self.blank();
    }

    fn finish(mut self) -> String {
        while self.text.ends_with("\n\n") {
            self.text.pop();
        }
        if self.text.is_empty() {
            self.text = paint("(no text)\n", DIM, self.opts.color);
        }
        self.text
    }
}

// ---- Word ----

#[derive(Default)]
struct RunFmt {
    bold: bool,
    italic: bool,
    underline: bool,
}

impl RunFmt {
    fn style(&self) -> Style {
        Style {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            ..PLAIN
        }
    }
}

/// `w:b`, `w:i` and `w:u` switch a property on unless their value says off.
fn on(e: &BytesStart<'_>) -> bool {
    !matches!(
        attr(e, "val").as_deref(),
        Some("0" | "false" | "none" | "off")
    )
}

/// A table being collected: finished rows, the current row and cell, and the nesting depth.
#[derive(Default)]
struct Tbl {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    depth: usize,
}

impl Tbl {
    fn add_text(&mut self, s: &str) {
        self.cell.push_str(s);
    }

    /// Paragraphs of one cell are joined with a space.
    fn end_paragraph(&mut self) {
        if !self.cell.is_empty() && !self.cell.ends_with(' ') {
            self.cell.push(' ');
        }
    }

    fn end_cell(&mut self) {
        self.row.push(self.cell.trim().to_owned());
        self.cell.clear();
    }

    fn end_row(&mut self) {
        if self.rows.len() < MAX_ROWS {
            self.rows.push(std::mem::take(&mut self.row));
        } else {
            self.row.clear();
        }
    }
}

/// Render a `.docx` file.
pub fn render_docx(bytes: &[u8], opts: &Options) -> String {
    let mut z = match open_zip(bytes) {
        Ok(z) => z,
        Err(e) => return problem("the Word document", &e, opts),
    };
    let Some(xml) = read_entry(&mut z, "word/document.xml", MAX_ENTRY) else {
        return problem(
            "the Word document",
            "word/document.xml is missing or too large",
            opts,
        );
    };
    let mut reader = Reader::from_reader(&xml[..]);
    let mut out = Out::new(opts);
    let mut buf = Vec::new();
    let (mut spans, mut heading, mut listed) = (Vec::<Span>::new(), None::<u8>, false);
    let mut fmt = RunFmt::default();
    let mut in_text = false;
    let mut tbl = Tbl::default();
    loop {
        let ev = reader.read_event_into(&mut buf);
        let (start, empty) = match &ev {
            Ok(Event::Start(e)) => (Some(e), false),
            Ok(Event::Empty(e)) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            let is_start = !empty;
            match start_name(e).as_str() {
                "p" if is_start => {
                    spans.clear();
                    heading = None;
                    listed = false;
                }
                "pStyle" => {
                    let v = attr(e, "val").unwrap_or_default().to_ascii_lowercase();
                    heading = if let Some(n) = v.strip_prefix("heading") {
                        n.trim().parse::<u8>().ok().or(Some(2))
                    } else if v == "title" {
                        Some(1)
                    } else if v == "subtitle" {
                        Some(2)
                    } else {
                        None
                    };
                }
                "numPr" => listed = true,
                "r" if is_start => fmt = RunFmt::default(),
                "b" => fmt.bold = on(e),
                "i" => fmt.italic = on(e),
                "u" => fmt.underline = on(e),
                "t" if is_start => in_text = true,
                "tab" => spans.push(Span::new("    ", PLAIN)),
                "br" | "cr" => spans.push(Span::new("\n", PLAIN)),
                "tbl" if is_start => tbl.depth += 1,
                "tr" if is_start => tbl.row.clear(),
                "tc" if is_start => tbl.cell.clear(),
                _ => {}
            }
        } else {
            match &ev {
                Err(e) => {
                    out.text
                        .push_str(&problem("part of the document", &e.to_string(), opts));
                    break;
                }
                Ok(Event::Eof) => break,
                Ok(Event::Text(t)) if in_text => {
                    let s: &str = t;
                    if tbl.depth > 0 {
                        tbl.add_text(s);
                    } else {
                        spans.push(Span::new(s, fmt.style()));
                    }
                }
                Ok(Event::GeneralRef(r)) if in_text => {
                    let s = entity(r);
                    if tbl.depth > 0 {
                        tbl.add_text(&s);
                    } else {
                        spans.push(Span::new(s, fmt.style()));
                    }
                }
                Ok(Event::End(e)) => match end_name(e).as_str() {
                    "t" => in_text = false,
                    "p" => {
                        if tbl.depth > 0 {
                            tbl.end_paragraph();
                        } else {
                            out.paragraph(&spans, heading, listed);
                        }
                        spans.clear();
                    }
                    "tc" => tbl.end_cell(),
                    "tr" => tbl.end_row(),
                    "tbl" => {
                        tbl.depth = tbl.depth.saturating_sub(1);
                        if tbl.depth == 0 {
                            out.table(&tbl.rows);
                            tbl.rows.clear();
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        buf.clear();
    }
    out.finish()
}

// ---- PowerPoint ----

fn slide_number(name: &str) -> Option<usize> {
    name.strip_prefix("ppt/slides/slide")?
        .strip_suffix(".xml")?
        .parse()
        .ok()
}

/// Render a `.pptx` file: the text of every slide.
pub fn render_pptx(bytes: &[u8], opts: &Options) -> String {
    let mut z = match open_zip(bytes) {
        Ok(z) => z,
        Err(e) => return problem("the presentation", &e, opts),
    };
    let mut slides: Vec<(usize, String)> = z
        .file_names()
        .filter_map(Result::ok)
        .filter_map(|n| slide_number(&n).map(|i| (i, n.into_owned())))
        .collect();
    slides.sort();
    let total = slides.len();
    let mut out = Out::new(opts);
    for (n, name) in slides.into_iter().take(MAX_SLIDES) {
        out.blank();
        out.paragraph(&[Span::new(format!("Slide {n}"), PLAIN)], Some(2), false);
        let Some(xml) = read_entry(&mut z, &name, MAX_ENTRY) else {
            out.paragraph(
                &[Span::new("(slide too large or damaged)", DIM)],
                None,
                false,
            );
            continue;
        };
        let mut reader = Reader::from_reader(&xml[..]);
        let mut buf = Vec::new();
        let (mut para, mut in_t) = (String::new(), false);
        loop {
            match &reader.read_event_into(&mut buf) {
                Ok(Event::Eof) | Err(_) => break,
                Ok(Event::Start(e)) => {
                    if start_name(e) == "t" {
                        in_t = true;
                    }
                }
                Ok(Event::Text(t)) if in_t => {
                    let s: &str = t;
                    para.push_str(s);
                }
                Ok(Event::GeneralRef(r)) if in_t => para.push_str(&entity(r)),
                Ok(Event::End(e)) => match end_name(e).as_str() {
                    "t" => in_t = false,
                    "p" => {
                        let line = std::mem::take(&mut para);
                        if !line.trim().is_empty() {
                            out.paragraph(&[Span::new(line, PLAIN)], None, true);
                        }
                    }
                    _ => {}
                },
                Ok(Event::Empty(e)) if start_name(e) == "br" => para.push(' '),
                _ => {}
            }
            buf.clear();
        }
    }
    if total > MAX_SLIDES {
        out.paragraph(
            &[Span::new(
                format!("(\u{2026} {} more slides)", total - MAX_SLIDES),
                DIM,
            )],
            None,
            false,
        );
    }
    out.finish()
}

// ---- OpenDocument text ----

/// Render an `.odt` file.
pub fn render_odt(bytes: &[u8], opts: &Options) -> String {
    let mut z = match open_zip(bytes) {
        Ok(z) => z,
        Err(e) => return problem("the document", &e, opts),
    };
    let Some(xml) = read_entry(&mut z, "content.xml", MAX_ENTRY) else {
        return problem("the document", "content.xml is missing or too large", opts);
    };
    let mut reader = Reader::from_reader(&xml[..]);
    let mut buf = Vec::new();
    let mut out = Out::new(opts);
    let (mut spans, mut heading, mut in_item) = (Vec::<Span>::new(), None::<u8>, 0usize);
    let mut tbl = Tbl::default();
    let mut in_para = false;
    loop {
        let ev = reader.read_event_into(&mut buf);
        let (start, empty) = match &ev {
            Ok(Event::Start(e)) => (Some(e), false),
            Ok(Event::Empty(e)) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            let is_start = !empty;
            match start_name(e).as_str() {
                "h" if is_start => {
                    in_para = true;
                    spans.clear();
                    heading = Some(
                        attr(e, "outline-level")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(1),
                    );
                }
                "p" if is_start => {
                    in_para = true;
                    spans.clear();
                    heading = None;
                }
                "list-item" if is_start => in_item += 1,
                "tab" => spans.push(Span::new("    ", PLAIN)),
                "line-break" => spans.push(Span::new("\n", PLAIN)),
                "s" => {
                    let n = attr(e, "c")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(1)
                        .min(80);
                    spans.push(Span::new(" ".repeat(n), PLAIN));
                }
                "table" if is_start => tbl.depth += 1,
                "table-row" if is_start => tbl.row.clear(),
                "table-cell" if is_start => tbl.cell.clear(),
                _ => {}
            }
        } else {
            match &ev {
                Err(_) | Ok(Event::Eof) => break,
                Ok(Event::Text(t)) if in_para => {
                    let s: &str = t;
                    if tbl.depth > 0 {
                        tbl.add_text(s);
                    } else {
                        spans.push(Span::new(s, PLAIN));
                    }
                }
                Ok(Event::GeneralRef(r)) if in_para => {
                    let s = entity(r);
                    if tbl.depth > 0 {
                        tbl.add_text(&s);
                    } else {
                        spans.push(Span::new(s, PLAIN));
                    }
                }
                Ok(Event::End(e)) => match end_name(e).as_str() {
                    "h" | "p" => {
                        in_para = false;
                        if tbl.depth == 0 {
                            out.paragraph(&spans, heading, in_item > 0);
                        } else {
                            tbl.end_paragraph();
                        }
                        spans.clear();
                    }
                    "list-item" => in_item = in_item.saturating_sub(1),
                    "table-cell" => tbl.end_cell(),
                    "table-row" => tbl.end_row(),
                    "table" => {
                        tbl.depth = tbl.depth.saturating_sub(1);
                        if tbl.depth == 0 {
                            out.table(&tbl.rows);
                            tbl.rows.clear();
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        buf.clear();
    }
    out.finish()
}

// ---- spreadsheets ----

fn cell_text(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        other => other.to_string().replace(['\n', '\r'], " "),
    }
}

/// Render a workbook: one table per sheet (first row styled as the header).
pub fn render_spreadsheet(bytes: &[u8], opts: &Options) -> String {
    let mut wb = match open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())) {
        Ok(wb) => wb,
        Err(e) => return problem("the spreadsheet", &e.to_string(), opts),
    };
    let names = wb.sheet_names();
    let mut out = Out::new(opts);
    for name in names.iter().take(MAX_SHEETS) {
        let Ok(range) = wb.worksheet_range(name) else {
            continue;
        };
        let (h, w) = range.get_size();
        out.paragraph(
            &[Span::new(
                format!("{name}  ({h} rows \u{d7} {w} columns)"),
                PLAIN,
            )],
            Some(2),
            false,
        );
        let rows: Vec<Vec<String>> = range
            .rows()
            .take(MAX_ROWS)
            .map(|r| r.iter().take(MAX_COLS).map(cell_text).collect())
            .collect();
        if rows.is_empty() {
            out.paragraph(&[Span::new("(empty sheet)", DIM)], None, false);
        } else {
            out.table(&rows);
        }
        if h > MAX_ROWS || w > MAX_COLS {
            out.paragraph(
                &[Span::new(
                    format!(
                        "(showing the first {} rows and {} columns)",
                        h.min(MAX_ROWS),
                        w.min(MAX_COLS)
                    ),
                    DIM,
                )],
                None,
                false,
            );
        }
    }
    if names.len() > MAX_SHEETS {
        out.paragraph(
            &[Span::new(
                format!("(\u{2026} {} more sheets)", names.len() - MAX_SHEETS),
                DIM,
            )],
            None,
            false,
        );
    }
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn plain() -> Options {
        Options {
            width: 60,
            color: false,
            line_numbers: false,
        }
    }

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            for (name, body) in files {
                w.start_file(*name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn docx(body: &str) -> Vec<u8> {
        zip_of(&[(
            "word/document.xml",
            &format!(r#"<w:document {NS}><w:body>{body}</w:body></w:document>"#),
        )])
    }

    #[test]
    fn docx_headings_paragraphs_lists_and_runs() {
        let bytes = docx(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly report</w:t></w:r></w:p>
               <w:p><w:r><w:t xml:space="preserve">Sales were </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>up</w:t></w:r><w:r><w:t>.</w:t></w:r></w:p>
               <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>first point</w:t></w:r></w:p>
               <w:p><w:r><w:t>line one</w:t></w:r><w:r><w:br/></w:r><w:r><w:t>line two</w:t></w:r></w:p>"#,
        );
        let t = render_docx(&bytes, &plain());
        assert!(t.contains("Quarterly report"), "{t}");
        assert!(t.contains("Sales were up."), "runs join: {t}");
        assert!(t.contains("\u{2022} first point"), "{t}");
        assert!(
            t.contains("line one\nline two"),
            "a break starts a new line: {t}"
        );
        // Colour keeps the bold run and the heading.
        let c = render_docx(
            &bytes,
            &Options {
                color: true,
                ..plain()
            },
        );
        assert!(c.contains("\x1b[1m") || c.contains("\x1b[1;"), "{c:?}");
    }

    #[test]
    fn docx_tables_become_tables() {
        let bytes = docx(
            r#"<w:tbl>
                <w:tr><w:tc><w:p><w:r><w:t>Name</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Qty</w:t></w:r></w:p></w:tc></w:tr>
                <w:tr><w:tc><w:p><w:r><w:t>Bolt</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>12</w:t></w:r></w:p><w:p><w:r><w:t>pcs</w:t></w:r></w:p></w:tc></w:tr>
               </w:tbl>
               <w:p><w:r><w:t>after the table</w:t></w:r></w:p>"#,
        );
        let t = render_docx(&bytes, &plain());
        assert!(
            t.contains("Name") && t.contains("Bolt") && t.contains("12 pcs"),
            "{t}"
        );
        assert!(
            t.contains('\u{2502}') || t.contains('|'),
            "drawn as a table: {t}"
        );
        assert!(t.contains("after the table"));
    }

    #[test]
    fn broken_and_hostile_files_give_short_messages() {
        for (name, text) in [
            ("docx", render_docx(b"not a zip", &plain())),
            ("pptx", render_pptx(b"", &plain())),
            ("odt", render_odt(b"PK\x03\x04junk", &plain())),
            ("xlsx", render_spreadsheet(b"nope", &plain())),
        ] {
            assert!(text.starts_with("cannot read"), "{name}: {text}");
            assert!(text.lines().count() == 1, "{name}: one line");
        }
        let missing = zip_of(&[("other.xml", "x")]);
        assert!(render_docx(&missing, &plain()).contains("word/document.xml is missing"));
        // Garbage XML inside a valid archive does not panic.
        let junk = zip_of(&[("word/document.xml", "<w:p><w:t>unclosed")]);
        assert!(!render_docx(&junk, &plain()).is_empty());
    }

    #[test]
    fn entries_over_the_limit_are_refused() {
        let big = zip_of(&[("a.xml", &"x".repeat(1000))]);
        let mut z = open_zip(&big).unwrap();
        assert_eq!(
            read_entry(&mut z, "a.xml", 2000).map(|v| v.len()),
            Some(1000)
        );
        assert!(
            read_entry(&mut z, "a.xml", 999).is_none(),
            "bigger than the limit"
        );
        assert!(read_entry(&mut z, "missing.xml", 2000).is_none());
    }

    #[test]
    fn pptx_lists_slide_text_in_order() {
        let a = r#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="p"><p:txBody><a:p><a:r><a:t>Welcome</a:t></a:r></a:p><a:p><a:r><a:t>Agenda</a:t></a:r></a:p></p:txBody></p:sld>"#;
        let b = r#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="p"><a:p><a:r><a:t>Thanks</a:t></a:r></a:p></p:sld>"#;
        let bytes = zip_of(&[("ppt/slides/slide10.xml", b), ("ppt/slides/slide2.xml", a)]);
        let t = render_pptx(&bytes, &plain());
        let (s2, s10) = (t.find("Slide 2").unwrap(), t.find("Slide 10").unwrap());
        assert!(s2 < s10, "numeric order, not alphabetical: {t}");
        assert!(t.contains("Welcome") && t.contains("Agenda") && t.contains("Thanks"));
    }

    #[test]
    fn odt_headings_paragraphs_and_lists() {
        let xml = r#"<office:document-content xmlns:office="o" xmlns:text="t"><office:body><office:text>
            <text:h text:outline-level="1">Title here</text:h>
            <text:p>Body <text:span>text</text:span>.<text:line-break/>next</text:p>
            <text:list><text:list-item><text:p>an item</text:p></text:list-item></text:list>
            </office:text></office:body></office:document-content>"#;
        let t = render_odt(&zip_of(&[("content.xml", xml)]), &plain());
        assert!(
            t.contains("Title here") && t.contains("Body text.\nnext"),
            "{t}"
        );
        assert!(t.contains("\u{2022} an item"), "{t}");
    }

    /// A minimal real workbook: two rows, text, a number and a second sheet.
    fn xlsx() -> Vec<u8> {
        zip_of(&[
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sales" sheetId="1" r:id="rId1"/><sheet name="Notes" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Item</t></is></c><c r="B1" t="inlineStr"><is><t>Total</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Bolts</t></is></c><c r="B2"><v>42</v></c></row></sheetData></worksheet>"#,
            ),
            (
                "xl/worksheets/sheet2.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#,
            ),
        ])
    }

    #[test]
    fn xlsx_sheets_become_tables() {
        let t = render_spreadsheet(&xlsx(), &plain());
        assert!(t.contains("Sales") && t.contains("Notes"), "{t}");
        assert!(
            t.contains("Item") && t.contains("Bolts") && t.contains("42"),
            "{t}"
        );
        assert!(t.contains("(empty sheet)") || t.contains("0 rows"), "{t}");
    }
}
