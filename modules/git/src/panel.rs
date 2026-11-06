//! The Git panel model: a list of changed files with one-key actions. Pure state and key handling; the app runs the
//! returned [`Action`]s in the background and feeds the results back. Outward or destructive actions (discard,
//! push) wait for a `y`.

use std::path::PathBuf;

use crate::ops::{Branch, Entry};

/// A key, already reduced by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Escape,
    Backspace,
    Space,
    Text(String),
}

/// Something the app must do for the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Discard(Entry),
    Commit(String),
    LoadBranches,
    Switch(String),
    Push,
    Pull,
    /// Show the diff of a file in a terminal pane.
    Diff(Entry),
    /// Show the log in a terminal pane.
    Log,
    Refresh,
}

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    None,
    Close,
    Do(Action),
}

/// Waiting for a yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Discard(Entry),
    Push,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    List,
    /// Typing a commit message.
    Commit(String),
    Branches {
        list: Vec<Branch>,
        sel: usize,
    },
    Confirm(Confirm),
}

/// The panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub mode: Mode,
    /// Last result: text and whether it is an error.
    pub notice: Option<(String, bool)>,
    pub busy: bool,
}

impl Panel {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            entries: Vec::new(),
            selected: 0,
            mode: Mode::List,
            notice: None,
            busy: true,
        }
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    pub fn staged_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_staged()).count()
    }

    /// New file list (keeps the selection near where it was).
    pub fn set_entries(&mut self, result: Result<Vec<Entry>, String>) {
        self.busy = false;
        match result {
            Ok(list) => {
                self.entries = list;
                self.selected = self.selected.min(self.entries.len().saturating_sub(1));
            }
            Err(e) => {
                self.entries.clear();
                self.notice = Some((e, true));
            }
        }
    }

    pub fn set_branches(&mut self, result: Result<Vec<Branch>, String>) {
        self.busy = false;
        match result {
            Ok(list) if !list.is_empty() => self.mode = Mode::Branches { list, sel: 0 },
            Ok(_) => self.notice = Some(("no branches".into(), true)),
            Err(e) => self.notice = Some((e, true)),
        }
    }

    /// An action finished: show its message and go back to the list.
    pub fn finish(&mut self, result: Result<String, String>) {
        self.busy = false;
        self.mode = Mode::List;
        self.notice = Some(match result {
            Ok(m) => (m, false),
            Err(e) => (e, true),
        });
    }

    fn start(&mut self, a: Action) -> Outcome {
        self.busy = true;
        Outcome::Do(a)
    }

    /// Apply a key.
    pub fn key(&mut self, key: Key) -> Outcome {
        if self.busy && !matches!(key, Key::Escape) {
            return Outcome::None; // one operation at a time
        }
        match self.mode.clone() {
            Mode::List => self.list_key(key),
            Mode::Commit(msg) => self.commit_key(key, msg),
            Mode::Branches { list, sel } => self.branch_key(key, &list, sel),
            Mode::Confirm(c) => self.confirm_key(key, c),
        }
    }

    fn list_key(&mut self, key: Key) -> Outcome {
        let n = self.entries.len();
        match key {
            Key::Escape => return Outcome::Close,
            Key::Up if n > 0 => self.selected = (self.selected + n - 1) % n,
            Key::Down if n > 0 => self.selected = (self.selected + 1) % n,
            Key::Space => {
                if let Some(e) = self.current().cloned() {
                    // A partly staged file is staged completely first.
                    let a = if e.is_staged() && e.worktree == ' ' {
                        Action::Unstage(vec![e.path])
                    } else {
                        Action::Stage(vec![e.path])
                    };
                    return self.start(a);
                }
            }
            Key::Enter => {
                if let Some(e) = self.current().cloned() {
                    return self.start(Action::Diff(e)).tap(|_| self.busy = false);
                }
            }
            Key::Text(t) => return self.list_text(&t),
            _ => {}
        }
        Outcome::None
    }

    fn list_text(&mut self, t: &str) -> Outcome {
        match t {
            "a" => {
                let paths: Vec<String> = self
                    .entries
                    .iter()
                    .filter(|e| !e.is_staged() || e.worktree != ' ')
                    .map(|e| e.path.clone())
                    .collect();
                if !paths.is_empty() {
                    return self.start(Action::Stage(paths));
                }
            }
            "u" => {
                let paths: Vec<String> = self
                    .entries
                    .iter()
                    .filter(|e| e.is_staged())
                    .map(|e| e.path.clone())
                    .collect();
                if !paths.is_empty() {
                    return self.start(Action::Unstage(paths));
                }
            }
            "c" => {
                if self.staged_count() == 0 {
                    self.notice =
                        Some(("nothing staged: stage files with Space or a".into(), true));
                } else {
                    self.mode = Mode::Commit(String::new());
                }
            }
            "b" => return self.start(Action::LoadBranches),
            "x" => {
                if let Some(e) = self.current().cloned() {
                    self.mode = Mode::Confirm(Confirm::Discard(e));
                }
            }
            "p" => self.mode = Mode::Confirm(Confirm::Push),
            "l" => return self.start(Action::Pull),
            "d" => {
                if let Some(e) = self.current().cloned() {
                    return Outcome::Do(Action::Diff(e));
                }
            }
            "g" => return Outcome::Do(Action::Log),
            "r" => return self.start(Action::Refresh),
            "q" => return Outcome::Close,
            _ => {}
        }
        Outcome::None
    }

    fn commit_key(&mut self, key: Key, mut msg: String) -> Outcome {
        match key {
            Key::Escape => self.mode = Mode::List,
            Key::Enter => {
                if msg.trim().is_empty() {
                    self.notice = Some(("write a commit message first".into(), true));
                } else {
                    return self.start(Action::Commit(msg));
                }
            }
            Key::Backspace => {
                msg.pop();
                self.mode = Mode::Commit(msg);
            }
            Key::Space => {
                msg.push(' ');
                self.mode = Mode::Commit(msg);
            }
            Key::Text(t) => {
                msg.push_str(&t);
                self.mode = Mode::Commit(msg);
            }
            _ => {}
        }
        Outcome::None
    }

    fn branch_key(&mut self, key: Key, list: &[Branch], sel: usize) -> Outcome {
        let n = list.len();
        match key {
            Key::Escape => self.mode = Mode::List,
            Key::Up => {
                self.mode = Mode::Branches {
                    list: list.to_vec(),
                    sel: (sel + n - 1) % n,
                }
            }
            Key::Down => {
                self.mode = Mode::Branches {
                    list: list.to_vec(),
                    sel: (sel + 1) % n,
                }
            }
            Key::Enter => {
                let b = &list[sel];
                if b.current {
                    self.mode = Mode::List;
                } else {
                    return self.start(Action::Switch(b.name.clone()));
                }
            }
            _ => {}
        }
        Outcome::None
    }

    fn confirm_key(&mut self, key: Key, c: Confirm) -> Outcome {
        self.mode = Mode::List;
        if key != Key::Text("y".into()) {
            self.notice = Some(("cancelled".into(), false));
            return Outcome::None;
        }
        match c {
            Confirm::Discard(e) => self.start(Action::Discard(e)),
            Confirm::Push => self.start(Action::Push),
        }
    }
}

trait Tap: Sized {
    fn tap(self, f: impl FnOnce(&Self)) -> Self {
        f(&self);
        self
    }
}
impl Tap for Outcome {}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(x: char, y: char, p: &str) -> Entry {
        Entry {
            staged: x,
            worktree: y,
            path: p.into(),
        }
    }

    fn panel() -> Panel {
        let mut p = Panel::new(PathBuf::from("/r"));
        p.set_entries(Ok(vec![
            entry(' ', 'M', "a.rs"),
            entry('A', ' ', "b.rs"),
            entry('?', '?', "c.txt"),
        ]));
        p
    }

    fn t(s: &str) -> Key {
        Key::Text(s.into())
    }

    #[test]
    fn space_stages_or_unstages_the_selected_file() {
        let mut p = panel();
        assert_eq!(
            p.key(Key::Space),
            Outcome::Do(Action::Stage(vec!["a.rs".into()]))
        );
        p.finish(Ok("ok".into()));
        p.key(Key::Down);
        assert_eq!(
            p.key(Key::Space),
            Outcome::Do(Action::Unstage(vec!["b.rs".into()]))
        );
    }

    #[test]
    fn bulk_keys_and_one_operation_at_a_time() {
        let mut p = panel();
        assert_eq!(
            p.key(t("a")),
            Outcome::Do(Action::Stage(vec!["a.rs".into(), "c.txt".into()]))
        );
        assert_eq!(
            p.key(t("u")),
            Outcome::None,
            "busy: keys are ignored until the result arrives"
        );
        p.finish(Ok(String::new()));
        assert_eq!(
            p.key(t("u")),
            Outcome::Do(Action::Unstage(vec!["b.rs".into()]))
        );
    }

    #[test]
    fn discard_and_push_need_a_yes() {
        let mut p = panel();
        assert_eq!(p.key(t("x")), Outcome::None);
        assert!(matches!(p.mode, Mode::Confirm(Confirm::Discard(_))));
        assert_eq!(p.key(t("n")), Outcome::None, "anything but y cancels");
        assert_eq!(p.mode, Mode::List);
        p.key(t("x"));
        assert_eq!(
            p.key(t("y")),
            Outcome::Do(Action::Discard(entry(' ', 'M', "a.rs")))
        );
        p.finish(Ok(String::new()));
        p.key(t("p"));
        assert_eq!(p.key(t("y")), Outcome::Do(Action::Push));
    }

    #[test]
    fn commit_needs_staged_files_and_a_message() {
        let mut p = Panel::new(PathBuf::from("/r"));
        p.set_entries(Ok(vec![entry(' ', 'M', "a.rs")]));
        p.key(t("c"));
        assert_eq!(p.mode, Mode::List, "nothing staged");
        assert!(p.notice.as_ref().unwrap().1);
        p.set_entries(Ok(vec![entry('M', ' ', "a.rs")]));
        p.key(t("c"));
        assert_eq!(p.key(Key::Enter), Outcome::None, "empty message");
        for k in [t("f"), t("i"), t("x"), Key::Space, t("it")] {
            p.key(k);
        }
        p.key(Key::Backspace);
        assert_eq!(p.mode, Mode::Commit("fix i".into()));
        assert_eq!(
            p.key(Key::Enter),
            Outcome::Do(Action::Commit("fix i".into()))
        );
    }

    #[test]
    fn branches_switch_except_the_current_one() {
        let mut p = panel();
        p.set_branches(Ok(vec![
            Branch {
                name: "main".into(),
                current: true,
            },
            Branch {
                name: "dev".into(),
                current: false,
            },
        ]));
        assert_eq!(p.key(Key::Enter), Outcome::None);
        assert_eq!(p.mode, Mode::List);
        p.set_branches(Ok(vec![
            Branch {
                name: "main".into(),
                current: true,
            },
            Branch {
                name: "dev".into(),
                current: false,
            },
        ]));
        p.key(Key::Down);
        assert_eq!(p.key(Key::Enter), Outcome::Do(Action::Switch("dev".into())));
    }

    #[test]
    fn escape_closes_from_the_list_only() {
        let mut p = panel();
        p.key(t("b"));
        p.set_branches(Ok(vec![Branch {
            name: "m".into(),
            current: true,
        }]));
        assert_eq!(p.key(Key::Escape), Outcome::None);
        assert_eq!(p.key(Key::Escape), Outcome::Close);
        assert!(Panel::new(PathBuf::new()).current().is_none());
    }
}
