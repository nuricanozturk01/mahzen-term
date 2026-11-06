//! PDF support for `tf`, through poppler's command line tools (`pdfinfo`, `pdftotext`, `pdftoppm`).
//!
//! PDF is a huge format with a long history of parser bugs, so it is read by a tool that is built for it, in a separate
//! process (`tf` itself is one: a crash or a hostile file can only end this pane). Nothing is downloaded; if poppler is
//! missing the message says how to install it. Every call has a time limit and an output cap.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Longest a poppler call may run.
const TIMEOUT: Duration = Duration::from_secs(30);
/// Most bytes read from a tool's output.
const MAX_OUTPUT: usize = 64 * 1024 * 1024;
/// Resolution used for page images.
pub const DEFAULT_DPI: u32 = 110;

/// How to install poppler, shown when a tool is missing.
const INSTALL_HINT: &str = "install poppler: Debian/Ubuntu `sudo apt install poppler-utils`, Fedora `sudo dnf install poppler-utils`, macOS `brew install poppler`";

/// Whether the file name says PDF.
pub fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Run a tool and collect its standard output (capped), killing it after the time limit.
fn run(tool: &str, args: &[&str], path: &Path, after: &[&str]) -> Result<Vec<u8>, String> {
    let mut child = Command::new(tool)
        .args(args)
        .arg(path)
        .args(after)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("{tool} was not found; {INSTALL_HINT}")
            } else {
                format!("cannot start {tool}: {e}")
            }
        })?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.take(MAX_OUTPUT as u64).read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.take(16 * 1024).read_to_end(&mut buf);
        }
        buf
    });
    let end = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{tool} took longer than 30 seconds"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("{tool}: {e}")),
        }
    };
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        let msg = String::from_utf8_lossy(&stderr);
        let first = msg
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed");
        Err(format!(
            "{tool}: {}",
            first
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect::<String>()
        ))
    }
}

/// The page count from `pdfinfo` output (`Pages:   12`).
pub fn parse_pages(info: &str) -> Option<u32> {
    info.lines()
        .find_map(|l| l.strip_prefix("Pages:"))
        .and_then(|v| v.trim().parse().ok())
        .filter(|n| *n > 0)
}

/// Number of pages in the file.
pub fn page_count(path: &Path) -> Result<u32, String> {
    let out = run("pdfinfo", &[], path, &[])?;
    parse_pages(&String::from_utf8_lossy(&out))
        .ok_or_else(|| "this does not look like a PDF (no page count)".to_owned())
}

/// The text of the whole document, laid out like the page.
pub fn text(path: &Path) -> Result<String, String> {
    // The trailing `-` sends the text to standard output; without it pdftotext writes `file.txt` next to the PDF.
    let out = run("pdftotext", &["-layout", "-nopgbrk", "-q"], path, &["-"])?;
    let text = String::from_utf8_lossy(&out).into_owned();
    // Pages that are only pictures (scans) have no text layer.
    if text.trim().is_empty() {
        return Ok(
            "(no text layer: this PDF holds pictures; use `tf pdf` to look at the pages)\n"
                .to_owned(),
        );
    }
    Ok(text)
}

/// One page as PNG bytes. poppler writes files, so a private folder holds the result.
pub fn page_png(path: &Path, page: u32, dpi: u32) -> Result<Vec<u8>, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!("mz-pdf-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).map_err(|e| format!("cannot create a temporary folder: {e}"))?;
    let root = dir.join("page");
    let (p, d) = (page.to_string(), dpi.to_string());
    let result = Command::new("pdftoppm")
        .args(["-png", "-singlefile", "-r", &d, "-f", &p, "-l", &p])
        .arg(path)
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("pdftoppm was not found; {INSTALL_HINT}")
            } else {
                format!("cannot start pdftoppm: {e}")
            }
        });
    let png = result.and_then(|o| {
        if o.status.success() {
            std::fs::read(root.with_extension("png"))
                .map_err(|e| format!("pdftoppm made no image: {e}"))
        } else {
            let msg = String::from_utf8_lossy(&o.stderr);
            Err(format!(
                "pdftoppm: {}",
                msg.lines()
                    .next()
                    .unwrap_or("failed")
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(200)
                    .collect::<String>()
            ))
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
    png
}

/// What the user typed at the page prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Goto(u32),
    Quit,
    Help,
}

/// Interpret a line: Enter or `n` next, `p` previous, `f`/`l` first/last, a number goes to that page, `q` quits.
pub fn parse_command(line: &str, current: u32, total: u32) -> Cmd {
    let l = line.trim().to_ascii_lowercase();
    let clamp = |p: u32| p.clamp(1, total.max(1));
    match l.as_str() {
        "" | "n" | "next" | "j" | "space" => Cmd::Goto(clamp(current + 1)),
        "p" | "prev" | "k" | "b" => Cmd::Goto(clamp(current.saturating_sub(1))),
        "f" | "first" | "g" => Cmd::Goto(1),
        "l" | "last" => Cmd::Goto(total.max(1)),
        "q" | "quit" | "exit" => Cmd::Quit,
        other => other
            .parse::<u32>()
            .map_or(Cmd::Help, |n| Cmd::Goto(clamp(n))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with the text "Hello poppler", built by hand with correct cross-reference offsets.
    fn tiny_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 24 Tf 20 100 Td ({text}) Tj ET");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    fn poppler() -> bool {
        ["pdfinfo", "pdftotext", "pdftoppm"].iter().all(|t| {
            Command::new(t)
                .arg("-v")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
        })
    }

    #[test]
    fn pdf_names_page_counts_and_commands() {
        assert!(
            is_pdf(Path::new("A.PDF")) && !is_pdf(Path::new("a.pdfx")) && !is_pdf(Path::new("pdf"))
        );
        assert_eq!(
            parse_pages("Title: x\nPages:           12\nEncrypted: no"),
            Some(12)
        );
        assert_eq!(parse_pages("Pages: 0"), None);
        assert_eq!(parse_pages("nothing"), None);
        assert_eq!(parse_command("", 2, 5), Cmd::Goto(3));
        assert_eq!(
            parse_command("n", 5, 5),
            Cmd::Goto(5),
            "stays on the last page"
        );
        assert_eq!(
            parse_command("p", 1, 5),
            Cmd::Goto(1),
            "stays on the first page"
        );
        assert_eq!(parse_command("42", 1, 5), Cmd::Goto(5), "clamped");
        assert_eq!(parse_command("q", 1, 5), Cmd::Quit);
        assert_eq!(parse_command("what", 1, 5), Cmd::Help);
    }

    #[test]
    fn poppler_reads_text_counts_pages_and_renders_a_page() {
        if !poppler() {
            return; // not installed here: the message path is tested below
        }
        let dir = std::env::temp_dir().join(format!("mz-pdf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.pdf");
        std::fs::write(&f, tiny_pdf("Hello poppler")).unwrap();
        assert_eq!(page_count(&f).unwrap(), 1);
        assert!(text(&f).unwrap().contains("Hello poppler"));
        assert!(
            !dir.join("t.txt").exists(),
            "no stray text file is written next to the PDF"
        );
        let png = page_png(&f, 1, 50).unwrap();
        assert!(png.starts_with(b"\x89PNG"), "a PNG came back");
        assert!(
            page_png(&f, 9, 50).is_err(),
            "a page that does not exist is an error"
        );
        // Not a PDF: explained, not a crash. The temporary folder is gone afterwards.
        std::fs::write(dir.join("bad.pdf"), "not a pdf").unwrap();
        assert!(page_count(&dir.join("bad.pdf")).is_err());
        assert!(!std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .any(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("mz-pdf-{}-", std::process::id()))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_tool_says_how_to_install_it() {
        let err = run("definitely-not-poppler-xyz", &[], Path::new("x.pdf"), &[]).unwrap_err();
        assert!(
            err.contains("was not found") && err.contains("poppler-utils"),
            "{err}"
        );
    }
}
