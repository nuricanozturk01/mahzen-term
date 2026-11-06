//! Git status marks for the tree: parse `git status --porcelain=v1 -z` and roll changes up to parent folders.
//!
//! [`status`] runs `git` and blocks, so the app calls it off the UI thread. Output is external input: it is only
//! parsed, never executed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A change shown next to a file or folder. Declared from least to most urgent: a folder shows its worst child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    Untracked,
    Renamed,
    Added,
    Modified,
    Deleted,
    Conflict,
}

impl Mark {
    /// One-letter badge.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Untracked => "U",
            Self::Renamed => "R",
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Conflict => "!",
        }
    }
}

/// Marks of one repository, keyed by absolute path (folders carry the worst mark of anything inside).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    pub root: PathBuf,
    pub marks: HashMap<PathBuf, Mark>,
    /// Folders git reports as a whole (`?? dir/`): everything inside is untracked too.
    pub untracked_dirs: Vec<PathBuf>,
}

impl GitStatus {
    /// The mark of `path`: its own, else untracked when it lies inside an untracked folder.
    pub fn mark(&self, path: &Path) -> Option<Mark> {
        self.marks.get(path).copied().or_else(|| {
            self.untracked_dirs
                .iter()
                .any(|d| path.starts_with(d))
                .then_some(Mark::Untracked)
        })
    }
}

fn classify(x: u8, y: u8) -> Mark {
    match (x, y) {
        (b'?', b'?') => Mark::Untracked,
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => Mark::Conflict,
        (b'D', _) | (_, b'D') => Mark::Deleted,
        (b'A', _) => Mark::Added,
        (b'R' | b'C', _) => Mark::Renamed,
        _ => Mark::Modified,
    }
}

/// Parse porcelain v1 `-z` output into `(path relative to the repository root, mark)`.
pub fn parse_porcelain(out: &[u8]) -> Vec<(String, Mark)> {
    let mut fields = out.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut res = Vec::new();
    while let Some(f) = fields.next() {
        if f.len() < 4 || f[2] != b' ' {
            continue;
        }
        let mark = classify(f[0], f[1]);
        if matches!(f[0], b'R' | b'C') || matches!(f[1], b'R' | b'C') {
            fields.next(); // the original name of a rename or copy
        }
        // A trailing slash is kept: it marks a whole untracked folder.
        let path = String::from_utf8_lossy(&f[3..]).into_owned();
        if !path.trim_matches('/').is_empty() {
            res.push((path, mark));
        }
    }
    res
}

/// Build the mark table from parsed entries: every ancestor folder up to `root` inherits the worst mark.
pub fn build(root: &Path, entries: &[(String, Mark)]) -> GitStatus {
    let mut marks: HashMap<PathBuf, Mark> = HashMap::new();
    let mut untracked_dirs = Vec::new();
    for (rel, mark) in entries {
        if rel.ends_with('/') {
            untracked_dirs.push(root.join(rel.trim_end_matches('/')));
        }
        let mut p = root.join(rel.trim_end_matches('/'));
        loop {
            let slot = marks.entry(p.clone()).or_insert(*mark);
            *slot = (*slot).max(*mark);
            if !p.pop() || p == root || !p.starts_with(root) {
                break;
            }
        }
    }
    marks.remove(root);
    GitStatus {
        root: root.to_path_buf(),
        marks,
        untracked_dirs,
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Status of the repository containing `dir`; `None` outside a repository or without `git`.
pub fn status(dir: &Path) -> Option<GitStatus> {
    let top = git(dir, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(String::from_utf8_lossy(&top).trim_end().to_owned());
    let out = git(
        dir,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )?;
    Some(build(&top, &parse_porcelain(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_codes_and_renames() {
        let out =
            b" M src/lib.rs\0A  new.rs\0?? notes/\0 D gone.txt\0R  moved.rs\0old.rs\0UU clash.rs\0";
        let got = parse_porcelain(out);
        assert_eq!(
            got,
            vec![
                ("src/lib.rs".into(), Mark::Modified),
                ("new.rs".into(), Mark::Added),
                ("notes/".into(), Mark::Untracked),
                ("gone.txt".into(), Mark::Deleted),
                ("moved.rs".into(), Mark::Renamed),
                ("clash.rs".into(), Mark::Conflict),
            ]
        );
        assert!(parse_porcelain(b"").is_empty());
        assert!(parse_porcelain(b"xx\0").is_empty(), "junk is ignored");
    }

    #[test]
    fn folders_show_their_worst_child() {
        let root = Path::new("/repo");
        let g = build(
            root,
            &[
                ("a/b/c.rs".into(), Mark::Untracked),
                ("a/d.rs".into(), Mark::Modified),
                ("z.rs".into(), Mark::Added),
            ],
        );
        assert_eq!(g.marks[Path::new("/repo/a")], Mark::Modified);
        assert_eq!(g.marks[Path::new("/repo/a/b")], Mark::Untracked);
        assert_eq!(g.marks[Path::new("/repo/a/b/c.rs")], Mark::Untracked);
        assert_eq!(g.marks[Path::new("/repo/z.rs")], Mark::Added);
        assert!(!g.marks.contains_key(root));
    }

    #[test]
    fn status_of_a_real_repository() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(d.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(run(&["init", "-q"]));
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/f.txt"), "x").unwrap();
        let g = status(&d.path().join("sub")).expect("inside a repo");
        let top = g.root.clone();
        assert_eq!(g.marks[&top.join("sub")], Mark::Untracked);
        assert_eq!(g.mark(&top.join("sub/f.txt")), Some(Mark::Untracked));
        assert_eq!(g.mark(&top.join("other.txt")), None);
        let outside = tempfile::tempdir().unwrap();
        let _ = status(outside.path()); // must not panic outside a repository
    }
}
