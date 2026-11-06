//! File operations with name validation. Names come from the user's keyboard, so they are checked before any path
//! is built: no separators, no `.`/`..`, nothing that already exists.

use std::fs;
use std::path::{Path, PathBuf};

/// Longest file name accepted (bytes).
const MAX_NAME: usize = 255;

/// Check a new file or folder name; `Err` carries the reason in words.
pub fn validate_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("name is empty".into());
    }
    if n == "." || n == ".." {
        return Err("`.` and `..` are not names".into());
    }
    if n.len() > MAX_NAME {
        return Err("name is longer than 255 bytes".into());
    }
    if n.chars()
        .any(|c| matches!(c, '/' | '\\' | '\0') || c.is_control())
    {
        return Err("name cannot contain / \\ or control characters".into());
    }
    Ok(())
}

fn target(dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let path = dir.join(name.trim());
    if path.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", name.trim()));
    }
    Ok(path)
}

/// Create an empty file `name` in `dir`.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = target(dir, name)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Create folder `name` in `dir`.
pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = target(dir, name)?;
    fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Rename `path` to `new_name` within the same folder.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let dir = path.parent().ok_or("cannot rename the root")?;
    let to = target(dir, new_name)?;
    fs::rename(path, &to).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_validated() {
        for ok in ["a.txt", ".env", "my file", "ünï.rs"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "  ",
            ".",
            "..",
            "a/b",
            "a\\b",
            "x\0y",
            "tab\there",
            &"n".repeat(300),
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn create_and_rename_refuse_to_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let f = create_file(d.path(), "a.txt").unwrap();
        assert!(f.is_file());
        assert!(create_file(d.path(), "a.txt")
            .unwrap_err()
            .contains("already exists"));
        let sub = create_dir(d.path(), "sub").unwrap();
        assert!(sub.is_dir());
        let g = rename(&f, "b.txt").unwrap();
        assert!(g.is_file() && !f.exists());
        fs::write(d.path().join("c.txt"), "keep").unwrap();
        assert!(rename(&g, "c.txt").is_err());
        assert_eq!(fs::read_to_string(d.path().join("c.txt")).unwrap(), "keep");
        assert!(rename(&g, "../escape").is_err());
    }
}
