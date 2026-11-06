//! Git commands the panel runs. Every function is blocking (call from a background task), takes the repository
//! folder explicitly, passes arguments as a vector and ends paths with `--` so a file name can never be an option.

use std::path::Path;
use std::time::Duration;

use mahzenterm_core::proc::{self, RunError};

const TIMEOUT: Duration = Duration::from_secs(20);
/// Network operations (push, pull) may take longer.
const NET_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT: usize = 2 * 1024 * 1024;
/// Most changed files listed.
const MAX_ENTRIES: usize = 500;

/// A changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Index (staged) status letter, space when unchanged.
    pub staged: char,
    /// Working tree status letter, space when unchanged.
    pub worktree: char,
    pub path: String,
}

impl Entry {
    pub fn is_staged(&self) -> bool {
        self.staged != ' ' && self.staged != '?'
    }

    pub fn is_untracked(&self) -> bool {
        self.staged == '?'
    }

    pub fn is_conflict(&self) -> bool {
        matches!(
            (self.staged, self.worktree),
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')
        )
    }
}

/// A local branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
}

fn run(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let r = proc::capture("git", args, Some(dir), timeout, MAX_OUTPUT).map_err(|e| match e {
        RunError::NotFound => "git not found on PATH".to_owned(),
        RunError::Spawn(m) => format!("cannot start git: {m}"),
        RunError::Timeout => "git did not finish in time".to_owned(),
    })?;
    if r.success {
        Ok(r.stdout)
    } else {
        let msg = if r.stderr.trim().is_empty() {
            r.stdout
        } else {
            r.stderr
        };
        let line = msg
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("git failed");
        Err(line.chars().take(160).collect())
    }
}

/// A path safe to hand to git: not empty, no control characters (it still follows `--`).
fn safe_path(p: &str) -> bool {
    !p.is_empty() && !p.chars().any(char::is_control)
}

/// Parse `git status --porcelain=v1 -z` output.
pub(crate) fn parse_entries(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut parts = text.split('\0');
    while let Some(part) = parts.next() {
        let mut chars = part.chars();
        let (Some(x), Some(y), Some(' ')) = (chars.next(), chars.next(), chars.next()) else {
            continue;
        };
        let path: String = chars.collect();
        if path.is_empty() {
            continue;
        }
        if x == 'R' || x == 'C' {
            parts.next(); // the original name follows as its own field
        }
        out.push(Entry {
            staged: x,
            worktree: y,
            path,
        });
        if out.len() >= MAX_ENTRIES {
            break;
        }
    }
    out
}

/// Changed files of the repository at `dir`.
pub fn entries(dir: &Path) -> Result<Vec<Entry>, String> {
    run(
        dir,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        TIMEOUT,
    )
    .map(|t| parse_entries(&t))
}

/// Stage files (`git add -- paths`).
pub fn stage(dir: &Path, paths: &[String]) -> Result<(), String> {
    with_paths(dir, &["add", "-A", "--"], paths)
}

/// Unstage files (`git restore --staged -- paths`); in a repository without commits `git rm --cached` is used.
pub fn unstage(dir: &Path, paths: &[String]) -> Result<(), String> {
    match with_paths(dir, &["restore", "--staged", "--"], paths) {
        Ok(()) => Ok(()),
        Err(_) => with_paths(dir, &["rm", "--cached", "-r", "-q", "--"], paths),
    }
}

/// Throw away the changes of one file: tracked files are restored from the index, untracked ones are deleted.
/// Destructive: the caller must have asked the user.
pub fn discard(dir: &Path, entry: &Entry) -> Result<(), String> {
    if entry.is_untracked() {
        with_paths(
            dir,
            &["clean", "-f", "--"],
            std::slice::from_ref(&entry.path),
        )
    } else {
        with_paths(dir, &["restore", "--"], std::slice::from_ref(&entry.path))
    }
}

fn with_paths(dir: &Path, base: &[&str], paths: &[String]) -> Result<(), String> {
    if paths.is_empty() || !paths.iter().all(|p| safe_path(p)) {
        return Err("no valid file selected".into());
    }
    let mut args: Vec<&str> = base.to_vec();
    args.extend(paths.iter().map(String::as_str));
    run(dir, &args, TIMEOUT).map(|_| ())
}

/// Commit what is staged. The message is passed as one argument, never through a shell.
pub fn commit(dir: &Path, message: &str) -> Result<String, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("write a commit message first".into());
    }
    let out = run(dir, &["commit", "-m", message], TIMEOUT)?;
    Ok(out.lines().next().unwrap_or("committed").to_owned())
}

/// Local branches, current first.
pub fn branches(dir: &Path) -> Result<Vec<Branch>, String> {
    let text = run(
        dir,
        &["branch", "--format=%(HEAD)%(refname:short)"],
        TIMEOUT,
    )?;
    let mut out: Vec<Branch> = text
        .lines()
        .filter(|l| l.len() > 1)
        .map(|l| Branch {
            current: l.starts_with('*'),
            name: l[1..].trim().to_owned(),
        })
        .filter(|b| !b.name.is_empty() && !b.name.starts_with('('))
        .collect();
    out.sort_by_key(|b| !b.current);
    Ok(out)
}

/// Switch to a local branch. Git refuses when local changes would be lost, and that message is shown.
pub fn switch_branch(dir: &Path, name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('-')
        || name.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("that branch name cannot be used".into());
    }
    run(dir, &["switch", name], TIMEOUT).map(|_| ())
}

/// `git push` (network).
pub fn push(dir: &Path) -> Result<String, String> {
    run(dir, &["push"], NET_TIMEOUT).map(|_| "pushed".to_owned())
}

/// `git pull --ff-only`: never creates a merge commit behind the user's back.
pub fn pull(dir: &Path) -> Result<String, String> {
    run(dir, &["pull", "--ff-only"], NET_TIMEOUT)
        .map(|o| o.lines().last().unwrap_or("pulled").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Option<tempfile::TempDir> {
        let d = tempfile::tempdir().unwrap();
        let g = |a: &[&str]| {
            std::process::Command::new("git")
                .args(a)
                .current_dir(d.path())
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        (g(&["init", "-q", "-b", "main"])
            && g(&["config", "user.email", "t@example.com"])
            && g(&["config", "user.name", "T"])
            && g(&["config", "commit.gpgsign", "false"]))
        .then_some(d)
    }

    #[test]
    fn porcelain_z_is_parsed_including_renames() {
        let e = parse_entries(" M a.rs\0A  b.rs\0?? c.txt\0R  new.rs\0old.rs\0UU d.rs\0");
        let names: Vec<&str> = e.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["a.rs", "b.rs", "c.txt", "new.rs", "d.rs"]);
        assert!(e[1].is_staged() && !e[0].is_staged());
        assert!(e[2].is_untracked());
        assert!(e[4].is_conflict());
        assert!(parse_entries("").is_empty());
        assert!(parse_entries("garbage\0x\0").is_empty());
    }

    #[test]
    fn stage_commit_branch_and_discard_work_on_a_real_repository() {
        let Some(d) = repo() else { return };
        let p = d.path();
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        assert_eq!(entries(p).unwrap()[0].path, "a.txt");
        assert!(entries(p).unwrap()[0].is_untracked());
        stage(p, &["a.txt".into()]).unwrap();
        assert!(entries(p).unwrap()[0].is_staged());
        unstage(p, &["a.txt".into()]).unwrap();
        assert!(entries(p).unwrap()[0].is_untracked());
        stage(p, &["a.txt".into()]).unwrap();
        assert!(commit(p, "  ").is_err(), "an empty message is refused");
        commit(p, "first").unwrap();
        assert!(entries(p).unwrap().is_empty());
        // Discard a modification and an untracked file.
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        std::fs::write(p.join("new.txt"), "x").unwrap();
        for e in entries(p).unwrap() {
            discard(p, &e).unwrap();
        }
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "one\n");
        assert!(!p.join("new.txt").exists());
        // Branches.
        assert!(run(p, &["branch", "topic"], TIMEOUT).is_ok());
        let b = branches(p).unwrap();
        assert!(b[0].current && b[0].name == "main" && b.iter().any(|b| b.name == "topic"));
        switch_branch(p, "topic").unwrap();
        assert!(branches(p).unwrap()[0].name == "topic");
        assert!(switch_branch(p, "--detach").is_err());
        assert!(switch_branch(p, "no such").is_err());
    }

    #[test]
    fn file_names_cannot_become_options() {
        let Some(d) = repo() else { return };
        std::fs::write(d.path().join("-rf"), "x").unwrap();
        stage(d.path(), &["-rf".into()]).unwrap();
        assert_eq!(entries(d.path()).unwrap()[0].path, "-rf");
        assert!(stage(d.path(), &[]).is_err());
        assert!(stage(d.path(), &["a\nb".into()]).is_err());
    }
}
