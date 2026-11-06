//! Git at a glance: branch, ahead/behind, change counts and the last commit of the focused pane's repository,
//! as an info-card widget (card, tab or window). Read-only: it runs `git status` and `git log` and never changes
//! the repository.

mod ops;
mod panel;
mod view;

use std::path::Path;
use std::time::Duration;

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use mahzenterm_core::proc::{self, RunError};
use mahzenterm_ui_kit::{InfoCard, InfoRow, InfoTone};

/// Command ids of this module.
pub mod id {
    pub const TOGGLE: &str = "git.toggle";
    pub const PANEL: &str = "git.panel";
}

const TIMEOUT: Duration = Duration::from_secs(4);
const MAX_OUTPUT: usize = 1024 * 1024;
/// Changed files listed on the card.
const MAX_FILES: usize = 6;

/// What `git status` and `git log` said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Repo {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub conflicts: u32,
    /// `(status code, path)` of the first changed files.
    pub files: Vec<(String, String)>,
    /// Subject and relative age of the last commit.
    pub last: Option<(String, String)>,
}

/// Parse `git status --porcelain=v1 -b` output.
pub fn parse_status(text: &str) -> Repo {
    let mut repo = Repo::default();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            parse_branch(head, &mut repo);
            continue;
        }
        let (code, path) = match (line.get(..2), line.get(3..)) {
            (Some(c), Some(p)) => (c, p),
            _ => continue,
        };
        let mut chars = code.chars();
        let (x, y) = (chars.next().unwrap_or(' '), chars.next().unwrap_or(' '));
        if code == "??" {
            repo.untracked += 1;
        } else if matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')) {
            repo.conflicts += 1;
        } else {
            if x != ' ' {
                repo.staged += 1;
            }
            if y != ' ' {
                repo.modified += 1;
            }
        }
        if repo.files.len() < MAX_FILES {
            repo.files.push((code.trim().to_owned(), path.to_owned()));
        }
    }
    repo
}

fn parse_branch(head: &str, repo: &mut Repo) {
    if let Some(name) = head.strip_prefix("No commits yet on ") {
        repo.branch = name.to_owned();
        return;
    }
    if head.starts_with("HEAD (no branch)") {
        repo.branch = "(detached)".into();
        return;
    }
    let (names, counts) = match head.split_once(" [") {
        Some((n, c)) => (n, c.trim_end_matches(']')),
        None => (head, ""),
    };
    match names.split_once("...") {
        Some((b, u)) => {
            repo.branch = b.to_owned();
            repo.upstream = Some(u.to_owned());
        }
        None => repo.branch = names.to_owned(),
    }
    for part in counts.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            repo.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            repo.behind = n.parse().unwrap_or(0);
        }
    }
}

/// Query the repository containing `dir`. `Ok(None)`: not a repository. Blocking: call from a background task.
pub fn fetch(dir: &Path) -> Result<Option<Repo>, String> {
    let run = |args: &[&str]| {
        proc::capture("git", args, Some(dir), TIMEOUT, MAX_OUTPUT).map_err(|e| match e {
            RunError::NotFound => "git not found on PATH".to_owned(),
            RunError::Spawn(m) => format!("cannot start git: {m}"),
            RunError::Timeout => "git did not answer in time".to_owned(),
        })
    };
    let status = run(&["status", "--porcelain=v1", "-b", "--no-renames"])?;
    if !status.success {
        return Ok(None);
    }
    let mut repo = parse_status(&status.stdout);
    if let Ok(log) = run(&["log", "-1", "--format=%s%x1f%cr"]) {
        if log.success {
            if let Some((subject, age)) = log.stdout.trim_end().split_once('\u{1f}') {
                repo.last = Some((subject.to_owned(), age.to_owned()));
            }
        }
    }
    Ok(Some(repo))
}

/// The card for a query result.
pub fn card(result: &Result<Option<Repo>, String>) -> InfoCard {
    let mut card = InfoCard {
        title: "Git".into(),
        ..InfoCard::default()
    };
    let repo = match result {
        Err(e) => {
            card.badge = "error".into();
            card.empty = e.clone();
            return card;
        }
        Ok(None) => {
            card.empty = "not a git repository".into();
            return card;
        }
        Ok(Some(r)) => r,
    };
    card.badge = repo.branch.clone();
    let clean = repo.staged + repo.modified + repo.untracked + repo.conflicts == 0;
    if let Some(up) = &repo.upstream {
        let (text, tone) = match (repo.ahead, repo.behind) {
            (0, 0) => ("in sync".to_owned(), InfoTone::Good),
            (a, 0) => (format!("\u{2191}{a} to push"), InfoTone::Warn),
            (0, b) => (format!("\u{2193}{b} to pull"), InfoTone::Warn),
            (a, b) => (format!("\u{2191}{a} \u{2193}{b} diverged"), InfoTone::Bad),
        };
        card.rows.push(InfoRow::new(up.clone(), text, tone));
    } else {
        card.rows
            .push(InfoRow::new("upstream", "none", InfoTone::Dim));
    }
    if clean {
        card.rows
            .push(InfoRow::new("working tree", "clean", InfoTone::Good));
    } else {
        for (label, n, tone) in [
            ("conflicts", repo.conflicts, InfoTone::Bad),
            ("staged", repo.staged, InfoTone::Good),
            ("modified", repo.modified, InfoTone::Warn),
            ("untracked", repo.untracked, InfoTone::Dim),
        ] {
            if n > 0 {
                card.rows.push(InfoRow::new(label, n.to_string(), tone));
            }
        }
        for (code, path) in repo.files.iter().take(4) {
            card.rows
                .push(InfoRow::new(path.clone(), code.clone(), InfoTone::Dim));
        }
    }
    if let Some((subject, age)) = &repo.last {
        card.rows
            .push(InfoRow::new(subject.clone(), age.clone(), InfoTone::Accent));
    }
    card
}

pub use ops::{
    branches, commit, discard, entries, pull, push, stage, switch_branch, unstage, Branch, Entry,
};
pub use panel::{Action, Confirm, Key, Mode, Outcome, Panel};
pub use view::view;

/// The module registration.
pub struct GitModule;

impl Module for GitModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "git",
            name: "Git",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::ProcessSpawn],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(id::TOGGLE, "Widgets: Git", "Widgets"),
            CommandSpec::new(id::PANEL, "Git: Changes, Commit, Branches", "Git").key("Ctrl+Alt+G"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_parsed() {
        let r = parse_status(
            "## main...origin/main [ahead 2, behind 1]\n M a.rs\nA  b.rs\n?? c.txt\nUU d.rs\n",
        );
        assert_eq!(
            (r.branch.as_str(), r.upstream.as_deref()),
            ("main", Some("origin/main"))
        );
        assert_eq!((r.ahead, r.behind), (2, 1));
        assert_eq!(
            (r.staged, r.modified, r.untracked, r.conflicts),
            (1, 1, 1, 1)
        );
        assert_eq!(r.files.len(), 4);
    }

    #[test]
    fn odd_branch_lines_do_not_panic() {
        assert_eq!(parse_status("## No commits yet on dev\n").branch, "dev");
        assert_eq!(parse_status("## HEAD (no branch)\n").branch, "(detached)");
        assert_eq!(parse_status("## solo\n").upstream, None);
        assert_eq!(parse_status("x\n\n").files.len(), 0);
    }

    #[test]
    fn card_shows_sync_state_and_changes() {
        let c = card(&Ok(Some(parse_status(
            "## main...origin/main [behind 3]\n M a.rs\n",
        ))));
        assert_eq!(c.badge, "main");
        assert!(c.rows[0].value.contains("3 to pull"));
        assert!(c.rows.iter().any(|r| r.label == "modified"));
        let clean = card(&Ok(Some(parse_status("## main...origin/main\n"))));
        assert!(clean.rows.iter().any(|r| r.value == "clean"));
        assert_eq!(card(&Ok(None)).empty, "not a git repository");
    }

    #[test]
    fn real_repository_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            return; // no git here
        }
        std::fs::write(dir.path().join("f"), "x").unwrap();
        let repo = fetch(dir.path()).unwrap().unwrap();
        assert_eq!(repo.untracked, 1);
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(fetch(plain.path()).unwrap(), None);
    }

    #[test]
    fn module_is_valid() {
        mahzenterm_core::module::validate(&GitModule).unwrap();
    }
}
