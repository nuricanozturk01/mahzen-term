//! Move files to the user's trash instead of deleting them.
//!
//! Linux/BSD follow the freedesktop.org trash spec (`Trash/files` + `Trash/info/*.trashinfo`, so file managers can
//! restore the item); macOS uses `~/.Trash`. Windows is not supported yet and says so. Only the home trash on the
//! same file system is used: a file on another drive is refused with an explanation rather than copied.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Why a file could not be trashed.
#[derive(Debug, Error)]
pub enum TrashError {
    #[error("moving to the trash is not supported on this system yet")]
    Unsupported,
    #[error("cannot find the trash folder (no home directory)")]
    NoTrash,
    #[error("{0} is on another drive than the trash; delete it from a shell if you are sure")]
    OtherDevice(String),
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
}

fn io_err(path: &Path, source: io::Error) -> TrashError {
    TrashError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Percent-encode a path for `.trashinfo` (keeps `/` and unreserved characters).
fn encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A name not yet used in `dir` (and not in `info` for freedesktop layouts): `name`, `name.2`, `name.3`, ...
fn unique(dir: &Path, info: Option<&Path>, name: &str) -> String {
    let taken = |n: &str| {
        dir.join(n).symlink_metadata().is_ok()
            || info.is_some_and(|i| i.join(format!("{n}.trashinfo")).exists())
    };
    if !taken(name) {
        return name.to_owned();
    }
    (2..)
        .map(|i| format!("{name}.{i}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| name.to_owned())
}

fn rename_checked(from: &Path, to: &Path) -> Result<(), TrashError> {
    fs::rename(from, to).map_err(|e| {
        // EXDEV (18 on Unix) means another file system.
        if e.raw_os_error() == Some(18) {
            TrashError::OtherDevice(from.display().to_string())
        } else {
            io_err(from, e)
        }
    })
}

/// Trash `path` into a freedesktop trash rooted at `trash` (`files/` and `info/` below it).
/// `deleted_at` is the local time as `YYYY-MM-DDThh:mm:ss`. Returns where the item now lives.
pub fn trash_freedesktop(
    trash: &Path,
    path: &Path,
    deleted_at: &str,
) -> Result<PathBuf, TrashError> {
    let abs = fs::canonicalize(path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| io_err(path, e))?
        .join(path.file_name().ok_or(TrashError::Io {
            path: path.display().to_string(),
            source: io::Error::other("no file name"),
        })?);
    abs.symlink_metadata().map_err(|e| io_err(&abs, e))?;
    let (files, info) = (trash.join("files"), trash.join("info"));
    for d in [&files, &info] {
        fs::create_dir_all(d).map_err(|e| io_err(d, e))?;
    }
    let name = abs
        .file_name()
        .map_or_else(|| "item".to_owned(), |n| n.to_string_lossy().into_owned());
    let name = unique(&files, Some(&info), &name);
    let record = format!(
        "[Trash Info]\nPath={}\nDeletionDate={deleted_at}\n",
        encode(&abs.to_string_lossy())
    );
    let info_file = info.join(format!("{name}.trashinfo"));
    fs::write(&info_file, record).map_err(|e| io_err(&info_file, e))?;
    let dest = files.join(&name);
    if let Err(e) = rename_checked(&abs, &dest) {
        let _ = fs::remove_file(&info_file);
        return Err(e);
    }
    Ok(dest)
}

/// Trash into macOS's `~/.Trash`.
fn trash_flat(trash: &Path, path: &Path) -> Result<PathBuf, TrashError> {
    fs::create_dir_all(trash).map_err(|e| io_err(trash, e))?;
    let name = path
        .file_name()
        .map_or_else(|| "item".to_owned(), |n| n.to_string_lossy().into_owned());
    let dest = trash.join(unique(trash, None, &name));
    rename_checked(path, &dest)?;
    Ok(dest)
}

/// Move `path` to the user's trash. `deleted_at` is local time `YYYY-MM-DDThh:mm:ss` (used on Linux/BSD).
pub fn move_to_trash(path: &Path, deleted_at: &str) -> Result<PathBuf, TrashError> {
    if cfg!(windows) {
        return Err(TrashError::Unsupported);
    }
    let home = crate::home_dir().ok_or(TrashError::NoTrash)?;
    if cfg!(target_os = "macos") {
        return trash_flat(&home.join(".Trash"), path);
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from);
    trash_freedesktop(&data.join("Trash"), path, deleted_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_move_with_a_restorable_record() {
        let d = tempfile::tempdir().unwrap();
        let trash = d.path().join("Trash");
        let f = d.path().join("my file.txt");
        fs::write(&f, "hi").unwrap();
        let dest = trash_freedesktop(&trash, &f, "2026-11-06T18:04:00").unwrap();
        assert!(!f.exists());
        assert_eq!(fs::read_to_string(dest).unwrap(), "hi");
        let info = fs::read_to_string(trash.join("info/my file.txt.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="));
        assert!(info.contains("my%20file.txt"), "{info}");
        assert!(info.contains("DeletionDate=2026-11-06T18:04:00"));
    }

    #[test]
    fn same_names_do_not_collide_and_folders_work() {
        let d = tempfile::tempdir().unwrap();
        let trash = d.path().join("Trash");
        for round in 0..3 {
            let dir = d.path().join("a");
            fs::create_dir_all(dir.join("sub")).unwrap();
            fs::write(dir.join("sub/f"), round.to_string()).unwrap();
            trash_freedesktop(&trash, &dir, "2026-11-06T00:00:00").unwrap();
        }
        let mut names: Vec<_> = fs::read_dir(trash.join("files"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a", "a.2", "a.3"]);
        assert_eq!(fs::read_dir(trash.join("info")).unwrap().count(), 3);
    }

    #[test]
    fn missing_files_fail_without_leaving_records() {
        let d = tempfile::tempdir().unwrap();
        let trash = d.path().join("Trash");
        let err = trash_freedesktop(&trash, &d.path().join("nope"), "x").unwrap_err();
        assert!(matches!(err, TrashError::Io { .. }));
        assert!(
            !trash.join("info").exists() || fs::read_dir(trash.join("info")).unwrap().count() == 0
        );
    }

    #[test]
    fn flat_trash_renames_uniquely() {
        let d = tempfile::tempdir().unwrap();
        let trash = d.path().join(".Trash");
        for _ in 0..2 {
            let f = d.path().join("x.txt");
            fs::write(&f, "1").unwrap();
            trash_flat(&trash, &f).unwrap();
        }
        assert!(trash.join("x.txt").exists() && trash.join("x.txt.2").exists());
    }

    #[test]
    fn encoding_keeps_slashes_and_escapes_the_rest() {
        assert_eq!(encode("/home/u/a b/é.txt"), "/home/u/a%20b/%C3%A9.txt");
    }
}
