//! E-books and rich text: EPUB (a zip of XHTML chapters) and RTF, shown as plain styled text.
//! Both are untrusted files: chapters and archive entries are size-limited, the HTML is never interpreted beyond
//! turning tags into line breaks and headings (no scripts, no external loads).

use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::ansi::{paint, Span, Style, DIM, PLAIN};
use crate::Options;

const MAX_CHAPTERS: usize = 300;
const MAX_ENTRY: u64 = 8 * 1024 * 1024;
const H1: Style = Style::fg(14).bold();
const H2: Style = Style::fg(12).bold();

fn note(msg: &str, opts: &Options) -> String {
    format!("{}\n", paint(msg, DIM, opts.color))
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match tail.find(';').filter(|e| *e <= 10) {
            Some(end) => {
                let name = &tail[1..end];
                let ch = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    n if n.starts_with("#x") || n.starts_with("#X") => {
                        u32::from_str_radix(&n[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                    }
                    n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32),
                    _ => None,
                };
                match ch {
                    Some(c) => {
                        out.push(c);
                        rest = &tail[end + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &tail[1..];
                    }
                }
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Turn (X)HTML into blocks of text: `(heading level, text)`. Tags only decide line breaks and headings.
pub fn html_blocks(html: &str) -> Vec<(Option<u8>, String)> {
    let mut blocks = Vec::new();
    let (mut cur, mut heading) = (String::new(), None::<u8>);
    let flush =
        |cur: &mut String, heading: &mut Option<u8>, blocks: &mut Vec<(Option<u8>, String)>| {
            // Entities first (`&nbsp;` is a space), then collapse every run of white space.
            let text = decode_entities(cur)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !text.is_empty() {
                blocks.push((*heading, text));
            }
            cur.clear();
            *heading = None;
        };
    let mut chars = html.char_indices().peekable();
    let mut skip_until: Option<&str> = None;
    while let Some((i, c)) = chars.next() {
        if let Some(close) = skip_until {
            if c == '<' && html[i..].to_ascii_lowercase().starts_with(close) {
                skip_until = None;
                // Drop the rest of the closing tag too.
                if let Some(end) = html[i..].find('>') {
                    while chars.peek().is_some_and(|(j, _)| *j < i + end + 1) {
                        chars.next();
                    }
                }
            }
            continue;
        }
        if c != '<' {
            cur.push(c);
            continue;
        }
        let Some(end) = html[i..].find('>') else {
            break;
        };
        let tag = html[i + 1..i + end].trim().to_ascii_lowercase();
        while chars.peek().is_some_and(|(j, _)| *j < i + end + 1) {
            chars.next();
        }
        let name = tag
            .trim_start_matches('/')
            .split([' ', '/', '\t', '\n'])
            .next()
            .unwrap_or("");
        let closing = tag.starts_with('/');
        match name {
            "script" | "style" if !closing => {
                skip_until = Some(if name == "script" {
                    "</script"
                } else {
                    "</style"
                })
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                flush(&mut cur, &mut heading, &mut blocks);
                if !closing {
                    heading = name[1..].parse().ok();
                }
            }
            "p" | "div" | "br" | "li" | "tr" | "blockquote" | "section" | "pre" | "table"
            | "ul" | "ol" => {
                flush(&mut cur, &mut heading, &mut blocks);
            }
            _ => {}
        }
    }
    flush(&mut cur, &mut heading, &mut blocks);
    blocks
}

fn read_zip(z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let f = z.by_name(name).ok()?;
    if f.size() > MAX_ENTRY {
        return None;
    }
    let mut out = Vec::new();
    f.take(MAX_ENTRY).read_to_end(&mut out).ok()?;
    Some(out)
}

/// First attribute value with this name in the first element with this local name.
fn find_attr(xml: &[u8], element: &str, attr: &str) -> Option<String> {
    let mut r = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match r.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.local_name().as_ref() == element => {
                return e
                    .attributes()
                    .flatten()
                    .find(|a| a.key.local_name().as_ref() == attr)
                    .and_then(|a| {
                        a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .ok()
                            .map(|v| v.into_owned())
                    });
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

/// `(manifest id -> href)` and the spine order, plus the title, from an OPF package file.
fn parse_opf(xml: &[u8]) -> (Vec<String>, Option<String>) {
    use std::collections::HashMap;
    let mut r = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let (mut manifest, mut spine) = (HashMap::new(), Vec::new());
    let (mut title, mut in_title) = (None::<String>, false);
    loop {
        match r.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "item" => {
                    let get = |k: &str| {
                        e.attributes()
                            .flatten()
                            .find(|a| a.key.local_name().as_ref() == k)
                            .and_then(|a| {
                                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                    .ok()
                                    .map(|v| v.into_owned())
                            })
                    };
                    if let (Some(id), Some(href)) = (get("id"), get("href")) {
                        manifest.insert(id, href);
                    }
                }
                "itemref" => {
                    if let Some(a) = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == "idref")
                    {
                        if let Ok(v) = a.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
                            spine.push(v.into_owned());
                        }
                    }
                }
                "title" => in_title = true,
                _ => {}
            },
            Ok(Event::Text(t)) if in_title && title.is_none() => {
                let s: &str = &t;
                title = Some(s.trim().to_owned()).filter(|s| !s.is_empty());
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "title" => in_title = false,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    let ordered = spine
        .into_iter()
        .filter_map(|id| manifest.get(&id).cloned())
        .collect();
    (ordered, title)
}

fn resolve(base_dir: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or(href);
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn emit(blocks: &[(Option<u8>, String)], opts: &Options, out: &mut String) {
    for (h, text) in blocks {
        match h {
            Some(level) => {
                out.push('\n');
                let st = if *level <= 1 { H1 } else { H2 };
                out.push_str(&crate::ansi::render_spans(
                    &[Span::new(text.clone(), st)],
                    opts.color,
                ));
                out.push_str("\n\n");
            }
            None => {
                for line in crate::ansi::wrap(&[Span::new(text.clone(), PLAIN)], opts.width.max(20))
                {
                    out.push_str(&crate::ansi::render_spans(&line, opts.color));
                    out.push('\n');
                }
                out.push('\n');
            }
        }
    }
}

/// Render an `.epub`: the title, then the chapters in reading order.
pub fn render_epub(bytes: &[u8], opts: &Options) -> String {
    let Ok(mut z) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return note("cannot read the e-book: not a valid EPUB (zip) file", opts);
    };
    let Some(container) = read_zip(&mut z, "META-INF/container.xml") else {
        return note(
            "cannot read the e-book: META-INF/container.xml is missing",
            opts,
        );
    };
    let Some(opf_path) = find_attr(&container, "rootfile", "full-path") else {
        return note("cannot read the e-book: no package file listed", opts);
    };
    let Some(opf) = read_zip(&mut z, &opf_path) else {
        return note(
            "cannot read the e-book: the package file is missing or too large",
            opts,
        );
    };
    let (spine, title) = parse_opf(&opf);
    let dir = opf_path.rsplit_once('/').map_or("", |(d, _)| d);
    let mut out = String::new();
    if let Some(t) = title {
        out.push_str(&crate::ansi::render_spans(&[Span::new(t, H1)], opts.color));
        out.push_str("\n\n");
    }
    for href in spine.iter().take(MAX_CHAPTERS) {
        if let Some(chapter) = read_zip(&mut z, &resolve(dir, href)) {
            emit(
                &html_blocks(&String::from_utf8_lossy(&chapter)),
                opts,
                &mut out,
            );
        }
    }
    if spine.len() > MAX_CHAPTERS {
        out.push_str(&note(
            &format!("(\u{2026} {} more chapters)", spine.len() - MAX_CHAPTERS),
            opts,
        ));
    }
    if out.trim().is_empty() {
        return note("(this e-book has no readable text)", opts);
    }
    out
}

/// Render RTF as text: control words become line breaks, tabs and Unicode characters; font and colour tables, styles and
/// other hidden groups are dropped.
pub fn render_rtf(bytes: &[u8], opts: &Options) -> String {
    let src = String::from_utf8_lossy(bytes);
    if !src.trim_start().starts_with("{\\rtf") {
        return note("cannot read the document: this is not RTF", opts);
    }
    let mut out = String::new();
    // Per group: is its text hidden (a destination such as the font table)?
    let mut skip: Vec<bool> = vec![false];
    let mut chars = src.chars().peekable();
    let mut uc_skip = 0usize;
    while let Some(c) = chars.next() {
        let hidden = *skip.last().unwrap_or(&false);
        match c {
            '{' => skip.push(hidden),
            '}' => {
                if skip.len() > 1 {
                    skip.pop();
                }
            }
            '\\' => {
                let Some(&n) = chars.peek() else { break };
                if n == '\\' || n == '{' || n == '}' {
                    chars.next();
                    if !hidden {
                        out.push(n);
                    }
                    continue;
                }
                if n == '\'' {
                    chars.next();
                    let hex: String = chars.by_ref().take(2).collect();
                    if let (false, Ok(b)) = (hidden, u8::from_str_radix(&hex, 16)) {
                        // Windows-1252 for the common range; Latin-1 elsewhere is close enough.
                        out.push(b as char);
                    }
                    continue;
                }
                if n == '*' {
                    chars.next();
                    if let Some(last) = skip.last_mut() {
                        *last = true; // an optional destination: ignore the group
                    }
                    continue;
                }
                let mut word = String::new();
                while chars.peek().is_some_and(char::is_ascii_alphabetic) {
                    word.push(chars.next().unwrap_or(' '));
                }
                let mut num = String::new();
                if chars.peek() == Some(&'-') {
                    num.push(chars.next().unwrap_or('-'));
                }
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    num.push(chars.next().unwrap_or('0'));
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                match word.as_str() {
                    "par" | "line" | "row" if !hidden => out.push('\n'),
                    "tab" | "cell" if !hidden => out.push('\t'),
                    "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "header"
                    | "footer" | "themedata" => {
                        if let Some(last) = skip.last_mut() {
                            *last = true;
                        }
                    }
                    "uc" => uc_skip = num.parse().unwrap_or(1),
                    "u" if !hidden => {
                        if let Ok(v) = num.parse::<i32>() {
                            let cp = if v < 0 { (v + 65536) as u32 } else { v as u32 };
                            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        }
                        // The fallback characters that follow are not shown.
                        for _ in 0..uc_skip.max(1) {
                            chars.next();
                        }
                    }
                    _ => {}
                }
            }
            '\r' | '\n' => {}
            c if !hidden => out.push(c),
            _ => {}
        }
    }
    let text: String = out
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        return note("(no text)", opts);
    }
    let mut res = String::new();
    for line in text.lines() {
        for l in crate::ansi::wrap(&[Span::new(line.to_owned(), PLAIN)], opts.width.max(20)) {
            res.push_str(&crate::ansi::render_spans(&l, opts.color));
            res.push('\n');
        }
    }
    res
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

    #[test]
    fn html_becomes_headings_and_paragraphs_without_scripts() {
        let b = html_blocks(
            "<html><head><style>p{color:red}</style></head><body><h1>Chapter &amp; One</h1><p>It was a <b>dark</b>\n and   stormy night.</p>\
             <script>alert(1)</script><p>Next &#233;t&#xE9; &nbsp;done</p><br/>tail</body></html>",
        );
        assert_eq!(b[0], (Some(1), "Chapter & One".to_owned()));
        assert_eq!(b[1], (None, "It was a dark and stormy night.".to_owned()));
        assert_eq!(b[2], (None, "Next \u{e9}t\u{e9} done".to_owned()));
        assert_eq!(b.last().unwrap().1, "tail");
        assert!(!b
            .iter()
            .any(|(_, t)| t.contains("alert") || t.contains("color")));
        assert!(
            html_blocks("<p>unterminated <b").len() <= 1,
            "a cut tag never panics"
        );
    }

    fn epub() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            let mut put = |n: &str, b: &str| {
                w.start_file(n, o).unwrap();
                w.write_all(b.as_bytes()).unwrap();
            };
            put(
                "META-INF/container.xml",
                r#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
            );
            put(
                "OEBPS/content.opf",
                r#"<package xmlns:dc="d"><metadata><dc:title>My Book</dc:title></metadata><manifest><item id="c1" href="ch1.xhtml"/><item id="c2" href="text/ch2.xhtml"/></manifest><spine><itemref idref="c2"/><itemref idref="c1"/></spine></package>"#,
            );
            put(
                "OEBPS/ch1.xhtml",
                "<html><body><h1>First</h1><p>one</p></body></html>",
            );
            put(
                "OEBPS/text/ch2.xhtml",
                "<html><body><h2>Second</h2><p>two</p></body></html>",
            );
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn epub_shows_the_title_and_chapters_in_spine_order() {
        let t = render_epub(&epub(), &plain());
        assert!(t.starts_with("My Book"), "{t}");
        assert!(
            t.find("Second").unwrap() < t.find("First").unwrap(),
            "reading order is the spine's: {t}"
        );
        assert!(t.contains("one") && t.contains("two"));
        assert!(render_epub(b"nope", &plain()).starts_with("cannot read the e-book"));
        assert_eq!(
            resolve("OEBPS/text", "../img/../ch1.xhtml#a"),
            "OEBPS/ch1.xhtml"
        );
    }

    #[test]
    fn rtf_text_survives_and_hidden_groups_do_not() {
        let rtf = r"{\rtf1\ansi\deff0{\fonttbl{\f0 Times New Roman;}}{\colortbl;\red255\green0\blue0;}{\*\generator Word;}\pard Hello \b bold\b0  world\par Second line with caf\'e9 and \u8364? euro\tab end\par}";
        let t = render_rtf(rtf.as_bytes(), &plain());
        assert!(t.contains("Hello bold world"), "{t}");
        assert!(
            t.contains("Second line with caf\u{e9} and \u{20ac} euro"),
            "{t}"
        );
        assert!(
            !t.contains("Times") && !t.contains("generator") && !t.contains("255"),
            "{t}"
        );
        assert!(render_rtf(b"plain", &plain()).contains("not RTF"));
        assert!(render_rtf(br"{\rtf1}", &plain()).contains("no text"));
        assert!(
            !render_rtf(br"{\rtf1 \u", &plain()).is_empty(),
            "cut input does not panic"
        );
    }
}
