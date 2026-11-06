//! `tf`: the MahzenTerm command line helper. `tf cat <file>...` shows files rendered for the terminal.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mahzenterm_viewer::{render, Options};

const USAGE: &str = "\
tf - MahzenTerm helper

usage:
  tf cat [--plain] [--width N] [--no-numbers] <file>...   show files rendered (markdown, code, CSV)
  tf cat -                                                 read standard input as plain text
  tf view [options] <file>...                              like cat, in a pager ($PAGER, default `less -R`)
  tf --version | --help

Colour is used only when writing to a terminal; --plain forces plain text.";

/// Files larger than this are cut, so a stray `tf cat huge.log` does not flood the terminal.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

struct Args {
    plain: bool,
    width: Option<usize>,
    numbers: bool,
    files: Vec<PathBuf>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut out = Args {
        plain: false,
        width: None,
        numbers: true,
        files: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--plain" => out.plain = true,
            "--no-numbers" => out.numbers = false,
            "--width" => {
                let v = it.next().ok_or("--width needs a number")?;
                out.width = Some(
                    v.parse::<usize>()
                        .ok()
                        .filter(|w| (10..=500).contains(w))
                        .ok_or("--width must be a number from 10 to 500")?,
                );
            }
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f => out.files.push(PathBuf::from(f)),
        }
    }
    if out.files.is_empty() {
        return Err("cat needs at least one file".into());
    }
    Ok(out)
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    if path == Path::new("-") {
        let mut buf = Vec::new();
        std::io::stdin()
            .take(MAX_BYTES)
            .read_to_end(&mut buf)
            .map_err(|e| format!("cannot read standard input: {e}"))?;
        return Ok(buf);
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        return Err(format!("{}: is a directory", path.display()));
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_BYTES).read_to_end(&mut buf))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(buf)
}

/// Show `text` in the user's pager; without one, print it and wait for Enter so a pane does not vanish.
fn page(text: &str) -> Result<(), String> {
    let pager = std::env::var("PAGER")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "less -R".to_owned());
    let mut parts = pager.split_whitespace();
    let program = parts.next().unwrap_or("less");
    let spawned = std::process::Command::new(program)
        .args(parts)
        .stdin(std::process::Stdio::piped())
        .spawn();
    match spawned {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                // A pager that quits early closes the pipe: that is fine.
                let _ = stdin.write_all(text.as_bytes());
            }
            child.wait().map(|_| ()).map_err(|e| e.to_string())
        }
        Err(_) => {
            print!("{text}");
            println!("\n-- press Enter to close --");
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            Ok(())
        }
    }
}

fn cat(args: &[String], paged: bool) -> Result<(), String> {
    let a = parse(args)?;
    let tty = std::io::stdout().is_terminal();
    let width = a.width.unwrap_or_else(|| {
        terminal_size::terminal_size().map_or(100, |(w, _)| usize::from(w.0).clamp(20, 200))
    });
    let opts = Options {
        width,
        color: tty && !a.plain,
        line_numbers: a.numbers,
    };
    let mut collected = String::new();
    let mut out = std::io::stdout().lock();
    let many = a.files.len() > 1;
    for f in &a.files {
        let bytes = read(f)?;
        if many {
            let header = format!("==> {} <==\n", f.display());
            if paged && tty {
                collected.push_str(&header);
            } else {
                write!(out, "{header}").map_err(|e| e.to_string())?;
            }
        }
        // `-` (standard input) has no extension: show it as plain text.
        let name = if f == Path::new("-") {
            Path::new("stdin.txt")
        } else {
            f.as_path()
        };
        let text = render(name, &bytes, &opts);
        if paged && tty {
            collected.push_str(&text);
            continue;
        }
        if out.write_all(text.as_bytes()).is_err() {
            return Ok(()); // closed pipe (`| head`): not an error
        }
    }
    drop(out);
    if paged && tty {
        return page(&collected);
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some(cmd @ ("cat" | "view")) => match cat(&args[1..], cmd == "view") {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("tf: {e}");
                ExitCode::FAILURE
            }
        },
        Some("--version" | "-V") => {
            println!("tf {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") | None => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("tf: unknown command `{other}`\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn arguments_are_validated() {
        let a = parse(&v(&["--plain", "--width", "50", "a.md", "b.rs"])).expect("valid");
        assert!(a.plain && a.width == Some(50) && a.files.len() == 2);
        assert!(parse(&v(&[])).is_err());
        assert!(parse(&v(&["--width", "5", "x"])).is_err());
        assert!(parse(&v(&["--width"])).is_err());
        assert!(parse(&v(&["--nope", "x"])).is_err());
    }

    #[test]
    fn missing_files_and_directories_are_errors_with_the_path() {
        assert!(read(Path::new("/definitely/not/here"))
            .unwrap_err()
            .contains("/definitely/not/here"));
        assert!(read(Path::new("/tmp"))
            .unwrap_err()
            .contains("is a directory"));
    }
}
