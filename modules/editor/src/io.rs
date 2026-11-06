//! Reading and writing files: line endings, byte-order mark, size guard and atomic, conflict-safe saving.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use thiserror::Error;

/// Largest file the editor opens (bigger ones belong in the viewer).
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// How lines end in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }

    /// The ending most lines use (LF when there are no lines or it is a tie).
    pub fn detect(text: &str) -> Self {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        if crlf > lf {
            Self::CrLf
        } else {
            Self::Lf
        }
    }
}

/// Why a file could not be opened.
#[derive(Debug, Error)]
pub enum OpenError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is {size} MB; files over 32 MB open in the viewer, not the editor")]
    TooLarge { path: String, size: u64 },
    #[error("{path} is not valid UTF-8; it can be viewed but not edited here")]
    NotUtf8 { path: String },
}

/// Why a file could not be saved.
#[derive(Debug, Error)]
pub enum SaveError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} changed on disk since it was opened; reload it or save with overwrite")]
    Conflict { path: String },
    #[error("this buffer has no file name yet")]
    NoPath,
}

/// What the disk looked like when the file was read, to notice outside changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiskStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

/// What the disk looked like right after a save (hand it back to [`crate::Buffer::finish_save`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedStamp(pub(crate) Option<DiskStamp>);

/// A save that can run off the UI thread: the bytes to write and what the file looked like when it was read.
#[derive(Debug)]
pub struct SaveJob {
    pub path: std::path::PathBuf,
    pub(crate) bytes: Vec<u8>,
    pub(crate) expect: Option<DiskStamp>,
    pub(crate) overwrite: bool,
}

impl SaveJob {
    /// Write the file (blocking: call it in a background task). Refuses when the file changed on disk since it was
    /// read, unless the job was prepared with `overwrite`.
    pub fn run(self) -> Result<SavedStamp, SaveError> {
        if !self.overwrite {
            if let (Some(then), Some(now)) = (self.expect, stamp(&self.path)) {
                if then != now {
                    return Err(SaveError::Conflict {
                        path: self.path.display().to_string(),
                    });
                }
            }
        }
        write_atomic(&self.path, &self.bytes)?;
        Ok(SavedStamp(stamp(&self.path)))
    }
}

pub(crate) fn stamp(path: &Path) -> Option<DiskStamp> {
    let m = fs::metadata(path).ok()?;
    Some(DiskStamp {
        modified: m.modified().ok(),
        len: m.len(),
    })
}

/// A file read: text normalised to `\n`, plus what is needed to write it back the way it was.
pub(crate) struct Loaded {
    pub text: String,
    pub ending: LineEnding,
    pub bom: bool,
    pub stamp: Option<DiskStamp>,
}

pub(crate) fn read(path: &Path) -> Result<Loaded, OpenError> {
    let p = path.display().to_string();
    let io = |source| OpenError::Io {
        path: p.clone(),
        source,
    };
    let meta = fs::metadata(path).map_err(io)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(OpenError::TooLarge {
            path: p,
            size: meta.len() / (1024 * 1024),
        });
    }
    let bytes = fs::read(path).map_err(|source| OpenError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, &bytes[..]),
    };
    let text = std::str::from_utf8(body).map_err(|_| OpenError::NotUtf8 {
        path: path.display().to_string(),
    })?;
    let ending = LineEnding::detect(text);
    Ok(Loaded {
        text: text.replace("\r\n", "\n"),
        ending,
        bom,
        stamp: stamp(path),
    })
}

/// Write `data` next to `path` and rename it over: a crash never leaves a half-written file.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<(), SaveError> {
    let p = path.display().to_string();
    let io = |source| SaveError::Io {
        path: p.clone(),
        source,
    };
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.mz-save-{}", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_are_detected_by_majority() {
        assert_eq!(LineEnding::detect("a\nb\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\n"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect("a\r\nb\nc\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("no newline"), LineEnding::Lf);
    }

    #[test]
    fn files_are_normalised_and_guarded() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();
        let l = read(&f).unwrap();
        assert_eq!(
            (l.text.as_str(), l.ending, l.bom),
            ("one\ntwo\n", LineEnding::CrLf, true)
        );
        fs::write(&f, [0xFF, 0xFE, 0x00]).unwrap();
        assert!(matches!(read(&f), Err(OpenError::NotUtf8 { .. })));
        assert!(matches!(
            read(&d.path().join("missing")),
            Err(OpenError::Io { .. })
        ));
        let big = d.path().join("big");
        fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(matches!(read(&big), Err(OpenError::TooLarge { .. })));
    }

    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temp_files() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.txt");
        fs::write(&f, "old").unwrap();
        write_atomic(&f, b"new").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "new");
        assert_eq!(
            fs::read_dir(d.path()).unwrap().count(),
            1,
            "only x.txt remains"
        );
        // A folder that does not exist fails cleanly.
        assert!(write_atomic(&d.path().join("no/such/dir/y.txt"), b"x").is_err());
    }
}
