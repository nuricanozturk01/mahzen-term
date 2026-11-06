//! CSV/TSV as a box-drawn table.

use std::path::Path;

use crate::ansi::{paint, truncate, width, Style, DIM};
use crate::Options;

/// Split one record on `delim`, honouring double quotes (`""` is a literal quote).
pub fn split_record(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delim && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Draw `rows` (first row is the header) as a table fitting `max_width`.
pub fn table(rows: &[Vec<String>], opts: &Options) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let mut widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .map(|r| r.get(c).map_or(0, |s| width(s)))
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    // Shrink the widest columns until the table fits.
    let chrome = cols * 3 + 1;
    while widths.iter().sum::<usize>() + chrome > opts.width.max(chrome + cols) {
        let Some(w) = widths.iter_mut().max() else {
            break;
        };
        if *w <= 3 {
            break;
        }
        *w -= 1;
    }
    let line = |l: char, m: char, r: char| {
        let inner: Vec<String> = widths.iter().map(|w| "\u{2500}".repeat(w + 2)).collect();
        paint(
            &format!("{l}{}{r}", inner.join(&m.to_string())),
            DIM,
            opts.color,
        )
    };
    let mut out = String::new();
    out.push_str(&line('\u{250c}', '\u{252c}', '\u{2510}'));
    out.push('\n');
    for (ri, row) in rows.iter().enumerate() {
        let bar = paint("\u{2502}", DIM, opts.color);
        out.push_str(&bar);
        for (c, w) in widths.iter().enumerate() {
            let cell = truncate(row.get(c).map_or("", String::as_str), *w);
            let pad = " ".repeat(w.saturating_sub(width(&cell)));
            let style = if ri == 0 {
                Style::fg(6).bold()
            } else {
                Style::default()
            };
            out.push(' ');
            out.push_str(&paint(&cell, style, opts.color));
            out.push_str(&pad);
            out.push(' ');
            out.push_str(&bar);
        }
        out.push('\n');
        if ri == 0 && rows.len() > 1 {
            out.push_str(&line('\u{251c}', '\u{253c}', '\u{2524}'));
            out.push('\n');
        }
    }
    out.push_str(&line('\u{2514}', '\u{2534}', '\u{2518}'));
    out.push('\n');
    out
}

pub fn render(src: &str, path: &Path, opts: &Options) -> String {
    let delim = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("tsv"))
    {
        '\t'
    } else {
        ','
    };
    let rows: Vec<Vec<String>> = src
        .lines()
        .filter(|l| !l.is_empty())
        .take(10_000)
        .map(|l| split_record(l, delim))
        .collect();
    table(&rows, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::strip_ansi;

    #[test]
    fn quotes_and_commas() {
        assert_eq!(
            split_record("a,\"b,c\",\"d\"\"e\"", ','),
            ["a", "b,c", "d\"e"]
        );
        assert_eq!(split_record("x\ty", '\t'), ["x", "y"]);
        assert_eq!(split_record("", ','), [""]);
    }

    #[test]
    fn table_fits_the_width_and_aligns_columns() {
        let rows = vec![
            vec!["name".to_owned(), "n".to_owned()],
            vec!["alpha".to_owned(), "1".to_owned()],
        ];
        let o = Options {
            color: false,
            width: 40,
            ..Options::default()
        };
        let t = table(&rows, &o);
        let widths: Vec<usize> = t.lines().map(width).collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "{t}");
        assert!(t.contains("\u{2502} alpha \u{2502} 1 \u{2502}"));
        let narrow = table(
            &[vec!["a".repeat(60), "b".repeat(60)]],
            &Options {
                width: 30,
                color: false,
                ..Options::default()
            },
        );
        assert!(
            narrow.lines().all(|l| width(&strip_ansi(l)) <= 30),
            "{narrow}"
        );
    }
}
