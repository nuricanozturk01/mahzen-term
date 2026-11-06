//! The smaller `tf` commands: `hex`, `log`, `lint` and `sql`.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use mahzenterm_viewer::hexview;
use mahzenterm_viewer::lint;
use mahzenterm_viewer::logview::{self, Level};
use mahzenterm_viewer::Options;

use crate::{read, sql};

/// Output options for the terminal: its width, colour only on a terminal.
fn opts() -> Options {
    let tty = std::io::stdout().is_terminal();
    Options {
        width: terminal_size::terminal_size().map_or(100, |(w, _)| usize::from(w.0).clamp(20, 240)),
        color: tty,
        line_numbers: false,
    }
}

/// A number written as `123` or `0x7b`.
pub fn parse_number(s: &str) -> Option<usize> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => usize::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

fn print(text: &str) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// `tf hex [--offset N] [--length N] <file>`
pub fn hex(args: &[String]) -> Result<(), String> {
    let (mut offset, mut length, mut file) = (0usize, 0usize, None::<PathBuf>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--offset" => {
                offset = it
                    .next()
                    .and_then(|v| parse_number(v))
                    .ok_or("--offset needs a number (decimal or 0x...)")?
            }
            "--length" => {
                length = it
                    .next()
                    .and_then(|v| parse_number(v))
                    .ok_or("--length needs a number")?
            }
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f if file.is_none() => file = Some(PathBuf::from(f)),
            _ => return Err("hex takes one file".into()),
        }
    }
    let file = file.ok_or("hex needs a file")?;
    let bytes = read(&file)?;
    print(&hexview::render(&bytes, offset, length, &opts()))
}

/// `tf log [--level L] [--tail N] <file>`
pub fn log(args: &[String]) -> Result<(), String> {
    let (mut min, mut tail, mut file) = (None::<Level>, None::<usize>, None::<PathBuf>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--level" => {
                min = Some(
                    it.next()
                        .and_then(|v| Level::parse(v))
                        .ok_or("--level must be trace, debug, info, warn or error")?,
                );
            }
            "--tail" => {
                tail = Some(
                    it.next()
                        .and_then(|v| parse_number(v))
                        .filter(|n| *n > 0)
                        .ok_or("--tail needs a number of lines")?,
                )
            }
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f if file.is_none() => file = Some(PathBuf::from(f)),
            _ => return Err("log takes one file".into()),
        }
    }
    let file = file.ok_or("log needs a file (or - for standard input)")?;
    let text = String::from_utf8_lossy(&read(&file)?).into_owned();
    let text = match tail {
        Some(n) => {
            let lines: Vec<&str> = text.lines().collect();
            lines[lines.len().saturating_sub(n)..].join("\n")
        }
        None => text,
    };
    print(&logview::render(&text, min, &opts()))
}

/// `tf lint <file>...`: exit with an error when any file has a problem.
pub fn lint(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("lint needs at least one TOML or JSON file".into());
    }
    let mut bad = 0;
    for a in args {
        let path = Path::new(a);
        let text = String::from_utf8_lossy(&read(path)?).into_owned();
        match lint::check(path, &text)? {
            None => println!("{a}: ok"),
            Some(p) => {
                bad += 1;
                println!(
                    "{a}:{}:{}: {}\n{}",
                    p.line,
                    p.col,
                    p.message,
                    lint::excerpt(&text, &p)
                );
            }
        }
    }
    if bad > 0 {
        Err(format!("{bad} file(s) with problems"))
    } else {
        Ok(())
    }
}

/// `tf parquet <file> [--limit N]`
pub fn parquet_cmd(args: &[String]) -> Result<(), String> {
    let (mut limit, mut file) = (50usize, None::<PathBuf>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--limit" => {
                limit = it
                    .next()
                    .and_then(|v| parse_number(v))
                    .filter(|n| *n > 0)
                    .ok_or("--limit needs a number")?
            }
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f if file.is_none() => file = Some(PathBuf::from(f)),
            _ => return Err("parquet takes one file".into()),
        }
    }
    let file = file.ok_or("parquet needs a file")?;
    print(&crate::parquet_view::render(&file, limit, &opts())?)
}

/// `tf sql <db> [table] [--limit N]`
pub fn sql_cmd(args: &[String]) -> Result<(), String> {
    let (mut limit, mut pos) = (50usize, Vec::<String>::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--limit" => {
                limit = it
                    .next()
                    .and_then(|v| parse_number(v))
                    .filter(|n| *n > 0)
                    .ok_or("--limit needs a number")?
            }
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f => pos.push(f.to_owned()),
        }
    }
    let db = pos.first().ok_or("sql needs a database file")?;
    let o = opts();
    let text = match pos.get(1) {
        Some(table) => sql::table(Path::new(db), table, limit, &o)?,
        None => sql::overview(Path::new(db), &o)?,
    };
    print(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_may_be_hex() {
        assert_eq!(parse_number("42"), Some(42));
        assert_eq!(parse_number("0x2A"), Some(42));
        assert_eq!(parse_number("0xzz"), None);
        assert_eq!(parse_number("-1"), None);
        assert_eq!(parse_number(""), None);
    }

    #[test]
    fn bad_arguments_are_explained() {
        let v = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(hex(&v(&[])).unwrap_err().contains("needs a file"));
        assert!(hex(&v(&["--offset"])).unwrap_err().contains("--offset"));
        assert!(log(&v(&["--level", "loud", "x"]))
            .unwrap_err()
            .contains("--level"));
        assert!(lint(&v(&[])).unwrap_err().contains("needs at least one"));
        assert!(sql_cmd(&v(&[])).unwrap_err().contains("needs a database"));
        assert!(sql_cmd(&v(&["--limit", "0", "x.db"]))
            .unwrap_err()
            .contains("--limit"));
    }
}
