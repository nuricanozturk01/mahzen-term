//! Archive browser: list zip, tar and tar.gz archives without unpacking them, and preview single entries.
//!
//! Archives are untrusted input. Nothing is ever extracted to disk (so a hostile path such as `../../x` cannot write
//! anywhere); entry names are shown with control characters removed and unsafe paths are flagged; sizes are read from the
//! headers but reading an entry's content is capped, so a zip bomb cannot fill memory.

use std::io::{Cursor, Read};
use std::path::Path;

use flate2::read::GzDecoder;

use crate::ansi::{paint, DIM};
use crate::csv;
use crate::Options;

/// Most entries listed.
const MAX_ENTRIES: usize = 5_000;
/// Most bytes of an entry that are previewed.
pub const MAX_PREVIEW: u64 = 8 * 1024 * 1024;
/// Most decompressed bytes read from a `.tar.gz` while listing (tar has no index: it is read from the start).
const MAX_STREAM: u64 = 512 * 1024 * 1024;

/// The archive formats this module understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    SevenZ,
}

/// Decide by file name (`.zip`, `.jar`, `.whl`, `.tar`, `.tar.gz`, `.tgz`).
pub fn format_of(path: &Path) -> Option<Format> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        return Some(Format::TarGz);
    }
    match name.rsplit_once('.')?.1 {
        "zip" | "jar" | "war" | "whl" | "apk" | "xpi" | "nupkg" => Some(Format::Zip),
        "tar" => Some(Format::Tar),
        "7z" => Some(Format::SevenZ),
        _ => None,
    }
}

/// One entry of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    /// Compressed size when the format records it (zip).
    pub packed: Option<u64>,
    pub is_dir: bool,
    pub encrypted: bool,
}

impl Entry {
    /// An absolute path or one that climbs out with `..`: harmless here (nothing is extracted), but worth a warning.
    pub fn unsafe_path(&self) -> bool {
        self.name.starts_with('/')
            || self.name.starts_with('\\')
            || self.name.split(['/', '\\']).any(|p| p == "..")
            || self.name.chars().nth(1) == Some(':')
    }
}

fn clean(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}

/// List the entries. The second value is true when the listing was cut short.
pub fn list(bytes: &[u8], format: Format) -> Result<(Vec<Entry>, bool), String> {
    match format {
        Format::Zip => {
            let mut z = zip::ZipArchive::new(Cursor::new(bytes))
                .map_err(|e| format!("not a valid zip file ({e})"))?;
            let total = z.len();
            let mut out = Vec::new();
            for i in 0..total.min(MAX_ENTRIES) {
                let Ok(f) = z.by_index_raw(i) else { continue };
                out.push(Entry {
                    name: clean(&f.name().map_or_else(|_| "?".to_owned(), |n| n.into_owned())),
                    size: f.size(),
                    packed: Some(f.compressed_size()),
                    is_dir: f.is_dir(),
                    encrypted: f.encrypted(),
                });
            }
            Ok((out, total > MAX_ENTRIES))
        }
        Format::Tar => list_tar(Cursor::new(bytes)),
        Format::SevenZ => {
            // A memory limit (in KiB) keeps a hostile header from asking for gigabytes.
            let mut rd = Cursor::new(bytes);
            let archive = sevenz_rust2::Archive::read_with_memory_limit(
                &mut rd,
                &sevenz_rust2::Password::empty(),
                256 * 1024,
            )
            .map_err(|e| format!("not a valid 7z file or encrypted ({e})"))?;
            let total = archive.files.len();
            let out = archive
                .files
                .iter()
                .take(MAX_ENTRIES)
                .map(|f| Entry {
                    name: clean(f.name()),
                    size: f.size(),
                    packed: None,
                    is_dir: f.is_directory(),
                    encrypted: false,
                })
                .collect();
            Ok((out, total > MAX_ENTRIES))
        }
        Format::TarGz => list_tar(GzDecoder::new(bytes).take(MAX_STREAM)),
    }
}

fn list_tar<R: Read>(reader: R) -> Result<(Vec<Entry>, bool), String> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|e| format!("not a valid tar file ({e})"))?;
    let mut out = Vec::new();
    let mut truncated = false;
    for e in entries {
        let Ok(e) = e else {
            truncated = true; // damaged or cut by the stream limit
            break;
        };
        if out.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let kind = e.header().entry_type();
        let name = e
            .path()
            .map_or_else(|_| "?".to_owned(), |p| p.to_string_lossy().into_owned());
        out.push(Entry {
            name: clean(&name),
            size: e.header().size().unwrap_or(0),
            packed: None,
            is_dir: kind.is_dir(),
            encrypted: false,
        });
    }
    if out.is_empty() {
        return Err("no entries (is this really a tar file?)".into());
    }
    Ok((out, truncated))
}

fn human(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Render the listing as a table with a one-line summary.
pub fn render_list(bytes: &[u8], path: &Path, opts: &Options) -> String {
    let Some(format) = format_of(path) else {
        return format!("{}\n", paint("not a supported archive", DIM, opts.color));
    };
    let (entries, truncated) = match list(bytes, format) {
        Ok(v) => v,
        Err(e) => {
            return format!(
                "{}\n",
                paint(&format!("cannot read the archive: {e}"), DIM, opts.color)
            )
        }
    };
    let files = entries.iter().filter(|e| !e.is_dir).count();
    let size: u64 = entries.iter().map(|e| e.size).sum();
    let packed: u64 = entries.iter().filter_map(|e| e.packed).sum();
    let kind = match format {
        Format::Zip => "zip",
        Format::Tar => "tar",
        Format::TarGz => "tar.gz",
        Format::SevenZ => "7z",
    };
    let mut summary = format!("{kind}: {files} files, {} unpacked", human(size));
    if format == Format::Zip && size > 0 {
        summary.push_str(&format!(
            ", {} stored ({}%)",
            human(packed),
            packed * 100 / size
        ));
    } else if format == Format::TarGz {
        summary.push_str(&format!(", {} compressed", human(bytes.len() as u64)));
    }
    if truncated {
        summary.push_str("  (listing cut short)");
    }
    let mut rows = vec![vec!["Size".to_owned(), "Name".to_owned(), "".to_owned()]];
    for e in &entries {
        let mut note = String::new();
        if e.unsafe_path() {
            note.push_str("unsafe path ");
        }
        if e.encrypted {
            note.push_str("encrypted");
        }
        rows.push(vec![
            if e.is_dir {
                String::new()
            } else {
                human(e.size)
            },
            if e.is_dir && !e.name.ends_with('/') {
                format!("{}/", e.name)
            } else {
                e.name.clone()
            },
            note.trim().to_owned(),
        ]);
    }
    format!(
        "{}\n{}",
        paint(&summary, DIM, opts.color),
        csv::table(&rows, opts)
    )
}

/// The bytes of one entry (at most [`MAX_PREVIEW`]), found by exact name.
pub fn read_entry(bytes: &[u8], format: Format, name: &str) -> Result<Vec<u8>, String> {
    let cap = MAX_PREVIEW;
    match format {
        Format::Zip => {
            let mut z = zip::ZipArchive::new(Cursor::new(bytes))
                .map_err(|e| format!("not a valid zip file ({e})"))?;
            let f = z
                .by_name(name)
                .map_err(|_| format!("no entry named {name}"))?;
            if f.encrypted() {
                return Err("that entry is encrypted".into());
            }
            let mut out = Vec::new();
            f.take(cap)
                .read_to_end(&mut out)
                .map_err(|e| e.to_string())?;
            Ok(out)
        }
        Format::Tar => read_tar_entry(Cursor::new(bytes), name, cap),
        Format::SevenZ => {
            let mut rd = Cursor::new(bytes);
            let archive = sevenz_rust2::Archive::read_with_memory_limit(
                &mut rd,
                &sevenz_rust2::Password::empty(),
                256 * 1024,
            )
            .map_err(|e| format!("not a valid 7z file ({e})"))?;
            let size = archive
                .files
                .iter()
                .find(|f| f.name() == name)
                .map(|f| f.size())
                .ok_or_else(|| format!("no entry named {name}"))?;
            if size > cap {
                return Err(format!("that entry is {size} bytes; only the first {cap} can be previewed from a 7z (it must be unpacked whole)"));
            }
            let mut reader = sevenz_rust2::ArchiveReader::new(
                Cursor::new(bytes),
                sevenz_rust2::Password::empty(),
            )
            .map_err(|e| e.to_string())?;
            reader.read_file(name).map_err(|e| e.to_string())
        }
        Format::TarGz => read_tar_entry(GzDecoder::new(bytes).take(MAX_STREAM), name, cap),
    }
}

fn read_tar_entry<R: Read>(reader: R, name: &str, cap: u64) -> Result<Vec<u8>, String> {
    let mut archive = tar::Archive::new(reader);
    for e in archive.entries().map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        if e.path().is_ok_and(|p| p.to_string_lossy() == name) {
            let mut out = Vec::new();
            e.take(cap)
                .read_to_end(&mut out)
                .map_err(|e| e.to_string())?;
            return Ok(out);
        }
    }
    Err(format!("no entry named {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn plain() -> Options {
        Options {
            width: 80,
            color: false,
            line_numbers: false,
        }
    }

    fn zip_bytes(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            for (n, b) in files {
                if n.ends_with('/') {
                    w.add_directory(*n, o).unwrap();
                } else {
                    w.start_file(*n, o).unwrap();
                    w.write_all(b.as_bytes()).unwrap();
                }
            }
            w.finish().unwrap();
        }
        buf
    }

    fn tar_bytes(files: &[(&str, &str)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (n, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, n, body.as_bytes()).unwrap();
        }
        b.into_inner().unwrap()
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn formats_follow_the_file_name() {
        let f = |s: &str| format_of(Path::new(s));
        assert_eq!(f("a.ZIP"), Some(Format::Zip));
        assert_eq!(f("lib.jar"), Some(Format::Zip));
        assert_eq!(f("a.tar"), Some(Format::Tar));
        assert_eq!(f("a.tar.gz"), Some(Format::TarGz));
        assert_eq!(f("A.TGZ"), Some(Format::TarGz));
        assert_eq!(f("a.txt"), None);
        assert_eq!(f("zip"), None);
    }

    #[test]
    fn zip_listing_shows_sizes_directories_and_a_summary() {
        let z = zip_bytes(&[
            ("src/", ""),
            ("src/main.rs", "fn main() {}"),
            ("README.md", "# hi"),
        ]);
        let (e, cut) = list(&z, Format::Zip).unwrap();
        assert!(!cut);
        assert_eq!(e.len(), 3);
        assert!(e.iter().any(|x| x.name == "src/" && x.is_dir));
        let t = render_list(&z, Path::new("x.zip"), &plain());
        assert!(t.starts_with("zip: 2 files"), "{t}");
        assert!(
            t.contains("src/main.rs") && t.contains("README.md") && t.contains("12 B"),
            "{t}"
        );
    }

    #[test]
    fn tar_and_tar_gz_are_listed_and_entries_read() {
        let t = tar_bytes(&[("a/one.txt", "first"), ("two.txt", "second")]);
        let (e, _) = list(&t, Format::Tar).unwrap();
        assert_eq!(
            e.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["a/one.txt", "two.txt"]
        );
        let tgz = gz(&t);
        let (e, _) = list(&tgz, Format::TarGz).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(
            read_entry(&tgz, Format::TarGz, "two.txt").unwrap(),
            b"second"
        );
        assert_eq!(read_entry(&t, Format::Tar, "a/one.txt").unwrap(), b"first");
        assert!(read_entry(&t, Format::Tar, "missing").is_err());
        let z = zip_bytes(&[("x.txt", "zipped")]);
        assert_eq!(read_entry(&z, Format::Zip, "x.txt").unwrap(), b"zipped");
        assert!(read_entry(&z, Format::Zip, "nope").is_err());
    }

    #[test]
    fn hostile_names_are_flagged_and_cleaned_and_nothing_is_written() {
        let z = zip_bytes(&[
            ("../../etc/evil", "x"),
            ("/abs/path", "x"),
            ("ok/fine.txt", "x"),
        ]);
        let (e, _) = list(&z, Format::Zip).unwrap();
        let flags: Vec<bool> = e.iter().map(Entry::unsafe_path).collect();
        assert_eq!(flags, [true, true, false]);
        let t = render_list(&z, Path::new("x.zip"), &plain());
        assert!(t.contains("unsafe path"), "{t}");
        assert_eq!(
            clean("a\u{1b}[31mb"),
            "a\u{fffd}[31mb",
            "escapes cannot reach the terminal"
        );
        assert!(!Path::new("/tmp/etc/evil").exists() && !Path::new("etc/evil").exists());
    }

    #[test]
    fn seven_zip_archives_are_listed_and_entries_read() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "alpha").unwrap();
        std::fs::write(src.join("sub/b.txt"), "beta beta").unwrap();
        let out = dir.path().join("t.7z");
        sevenz_rust2::compress_to_path(&src, &out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(format_of(Path::new("x.7z")), Some(Format::SevenZ));
        let (e, cut) = list(&bytes, Format::SevenZ).unwrap();
        assert!(!cut);
        assert!(
            e.iter().any(|x| x.name.ends_with("a.txt") && x.size == 5),
            "{e:?}"
        );
        assert!(
            e.iter().any(|x| x.name.ends_with("b.txt") && x.size == 9),
            "{e:?}"
        );
        let t = render_list(&bytes, Path::new("t.7z"), &plain());
        assert!(t.starts_with("7z: "), "{t}");
        let name = e
            .iter()
            .find(|x| x.name.ends_with("b.txt"))
            .unwrap()
            .name
            .clone();
        assert_eq!(
            read_entry(&bytes, Format::SevenZ, &name).unwrap(),
            b"beta beta"
        );
        assert!(read_entry(&bytes, Format::SevenZ, "nope").is_err());
        assert!(list(b"not a 7z at all", Format::SevenZ).is_err());
        assert!(
            render_list(b"7z\xbc\xaf\x27\x1c junk", Path::new("bad.7z"), &plain())
                .starts_with("cannot read the archive")
        );
    }

    #[test]
    fn broken_archives_give_one_line() {
        for (name, bytes) in [
            ("a.zip", &b"not a zip"[..]),
            ("a.tar", b"junk"),
            ("a.tgz", b"\x1f\x8b junk"),
        ] {
            let t = render_list(bytes, Path::new(name), &plain());
            assert!(t.starts_with("cannot read the archive"), "{name}: {t}");
        }
        assert!(render_list(b"", Path::new("a.txt"), &plain()).contains("not a supported archive"));
    }
}
