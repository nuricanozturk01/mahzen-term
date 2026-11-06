//! The store's state machine: search, selection and the install confirmation. Pure data, unit tested.

use std::ffi::OsStr;

use mahzenterm_core::command::fuzzy_score;

use crate::catalog::App;
use crate::install::{is_installed, plan_install, InstallPlan};

/// Rows shown at once.
pub const VISIBLE_ROWS: usize = 9;

/// One app with what this machine can do about it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub app: App,
    pub installed: bool,
    pub plan: Option<InstallPlan>,
    /// Screenshots that exist on disk.
    pub shots: Vec<std::path::PathBuf>,
}

/// What the panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    /// Asking before running the install command of `entries[index]`.
    Confirm(usize),
    /// Looking at screenshot `.1` of `entries[.0]`.
    Gallery(usize, usize),
}

/// A key press reduced to what the store cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreKey<'a> {
    Escape,
    Enter,
    Up,
    Down,
    PageUp,
    PageDown,
    Left,
    Right,
    Backspace,
    ClearLine,
    /// Ctrl plus a letter (lowercase): `g` shows screenshots.
    Ctrl(char),
    Text(&'a str),
    Other,
}

/// What the app should run in a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Start an installed app.
    Open { program: String },
    /// Run the confirmed install command.
    Install { plan: InstallPlan, app_name: String },
}

/// What a key press did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Idle,
    Changed,
    Close,
    Run(Launch),
}

/// An open store.
#[derive(Debug)]
pub struct Store {
    pub entries: Vec<Entry>,
    pub query: String,
    /// Indices into `entries`, best match first.
    pub results: Vec<usize>,
    pub selected: usize,
    pub mode: Mode,
    /// A short message shown under the list (why nothing happened).
    pub notice: Option<String>,
    /// Catalog entries that were skipped, with the reason.
    pub problems: Vec<String>,
}

impl Store {
    /// Build from catalog apps; `os` and `path_var` decide installed state and install commands.
    pub fn new(apps: Vec<App>, problems: Vec<String>, os: &str, path_var: &OsStr) -> Self {
        let entries: Vec<Entry> = apps
            .into_iter()
            .map(|app| Entry {
                installed: is_installed(&app, path_var),
                plan: plan_install(&app, os, path_var),
                shots: app.screenshot_paths(),
                app,
            })
            .collect();
        let mut s = Self {
            entries,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            mode: Mode::Browse,
            notice: None,
            problems,
        };
        s.refilter();
        s
    }

    pub fn current(&self) -> Option<&Entry> {
        self.results.get(self.selected).map(|i| &self.entries[*i])
    }

    /// First visible row so the selection stays on screen.
    pub fn first_visible(&self) -> usize {
        self.selected.saturating_sub(VISIBLE_ROWS - 1)
    }

    /// The screenshot being shown: path, 1-based position and count, and the app's name.
    pub fn gallery(&self) -> Option<(&std::path::Path, usize, usize, &str)> {
        let Mode::Gallery(e, i) = self.mode else {
            return None;
        };
        let entry = self.entries.get(e)?;
        let path = entry.shots.get(i)?;
        Some((path, i + 1, entry.shots.len(), &entry.app.name))
    }

    /// The entry being confirmed.
    pub fn confirming(&self) -> Option<&Entry> {
        match self.mode {
            Mode::Confirm(i) => self.entries.get(i),
            Mode::Browse | Mode::Gallery(..) => None,
        }
    }

    fn refilter(&mut self) {
        let q = self.query.trim().to_owned();
        if q.is_empty() {
            self.results = (0..self.entries.len()).collect();
        } else {
            let mut scored: Vec<(i64, usize)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    let hay = format!(
                        "{} {} {}",
                        e.app.name,
                        e.app.categories.join(" "),
                        e.app.summary
                    );
                    fuzzy_score(&q, &hay).map(|s| (s, i))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.results = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.selected = 0;
    }

    fn move_by(&mut self, delta: isize) {
        let n = self.results.len();
        if n > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(n as isize) as usize;
        }
    }

    /// Select the row `i` (a mouse click) and act like Enter.
    pub fn click(&mut self, i: usize) -> Outcome {
        if i < self.results.len() && self.mode == Mode::Browse {
            self.selected = i;
            return self.on_key(StoreKey::Enter);
        }
        Outcome::Idle
    }

    /// Apply a key.
    pub fn on_key(&mut self, key: StoreKey<'_>) -> Outcome {
        self.notice = None;
        if let Mode::Gallery(e, i) = self.mode {
            let n = self.entries.get(e).map_or(0, |x| x.shots.len());
            return match key {
                StoreKey::Left | StoreKey::Up if n > 0 => {
                    self.mode = Mode::Gallery(e, (i + n - 1) % n);
                    Outcome::Changed
                }
                StoreKey::Right | StoreKey::Down if n > 0 => {
                    self.mode = Mode::Gallery(e, (i + 1) % n);
                    Outcome::Changed
                }
                StoreKey::Escape | StoreKey::Enter | StoreKey::Ctrl('g') => {
                    self.mode = Mode::Browse;
                    Outcome::Changed
                }
                _ => Outcome::Idle,
            };
        }
        if let Mode::Confirm(i) = self.mode {
            return match key {
                StoreKey::Enter => match self.entries.get(i).and_then(|e| {
                    e.plan.clone().map(|plan| Launch::Install {
                        plan,
                        app_name: e.app.name.clone(),
                    })
                }) {
                    Some(l) => Outcome::Run(l),
                    None => Outcome::Idle,
                },
                StoreKey::Escape => {
                    self.mode = Mode::Browse;
                    Outcome::Changed
                }
                _ => Outcome::Idle,
            };
        }
        match key {
            StoreKey::Escape => Outcome::Close,
            StoreKey::Enter => {
                let Some(&i) = self.results.get(self.selected) else {
                    return Outcome::Idle;
                };
                let e = &self.entries[i];
                if e.installed {
                    Outcome::Run(Launch::Open {
                        program: e.app.command.clone(),
                    })
                } else if e.plan.is_some() {
                    self.mode = Mode::Confirm(i);
                    Outcome::Changed
                } else {
                    self.notice = Some(format!(
                        "{}: no install command for this system (no supported package manager found). Install it manually.",
                        e.app.name
                    ));
                    Outcome::Changed
                }
            }
            StoreKey::Up => {
                self.move_by(-1);
                Outcome::Changed
            }
            StoreKey::Down => {
                self.move_by(1);
                Outcome::Changed
            }
            StoreKey::PageUp => {
                self.move_by(-(VISIBLE_ROWS as isize));
                Outcome::Changed
            }
            StoreKey::PageDown => {
                self.move_by(VISIBLE_ROWS as isize);
                Outcome::Changed
            }
            StoreKey::Ctrl('g') => {
                let Some(&i) = self.results.get(self.selected) else {
                    return Outcome::Idle;
                };
                if self.entries[i].shots.is_empty() {
                    self.notice = Some(format!(
                        "{} has no screenshots (add them to <config>/store/screenshots and list them in its catalog file)",
                        self.entries[i].app.name
                    ));
                } else {
                    self.mode = Mode::Gallery(i, 0);
                }
                Outcome::Changed
            }
            StoreKey::Left | StoreKey::Right | StoreKey::Ctrl(_) => Outcome::Idle,
            StoreKey::Backspace => {
                if self.query.pop().is_some() {
                    self.refilter();
                    Outcome::Changed
                } else {
                    Outcome::Idle
                }
            }
            StoreKey::ClearLine => {
                if self.query.is_empty() {
                    Outcome::Idle
                } else {
                    self.query.clear();
                    self.refilter();
                    Outcome::Changed
                }
            }
            StoreKey::Text(t) => {
                let clean: String = t.chars().filter(|c| !c.is_control()).collect();
                if clean.is_empty() {
                    return Outcome::Idle;
                }
                self.query.push_str(&clean);
                self.refilter();
                Outcome::Changed
            }
            StoreKey::Other => Outcome::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::load;
    use std::fs;

    fn store(tools: &[&str], os: &str) -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        for t in tools {
            fs::write(d.path().join(t), "").unwrap();
        }
        let p = std::env::join_paths([d.path()]).unwrap();
        let l = load(None);
        let s = Store::new(l.apps, l.problems, os, &p);
        (d, s)
    }

    fn type_text(s: &mut Store, text: &str) {
        for c in text.chars() {
            s.on_key(StoreKey::Text(&c.to_string()));
        }
    }

    #[test]
    fn typing_filters_and_ranks() {
        let (_d, mut s) = store(&[], "linux");
        let all = s.results.len();
        type_text(&mut s, "yaz");
        assert!(s.results.len() < all);
        assert_eq!(s.current().unwrap().app.id, "yazi");
        s.on_key(StoreKey::ClearLine);
        assert_eq!(s.results.len(), all);
        type_text(&mut s, "zzzzqq");
        assert!(s.current().is_none());
    }

    #[test]
    fn installing_needs_a_confirmation_that_shows_the_command() {
        let (_d, mut s) = store(&["apt"], "linux");
        type_text(&mut s, "btop");
        assert_eq!(s.on_key(StoreKey::Enter), Outcome::Changed);
        let e = s.confirming().unwrap();
        assert_eq!(
            e.plan.as_ref().unwrap().display(),
            "sudo apt install -y btop"
        );
        // Esc goes back; nothing ran.
        assert_eq!(s.on_key(StoreKey::Escape), Outcome::Changed);
        assert_eq!(s.mode, Mode::Browse);
        s.on_key(StoreKey::Enter);
        match s.on_key(StoreKey::Enter) {
            Outcome::Run(Launch::Install { plan, app_name }) => {
                assert_eq!(app_name, "btop");
                assert_eq!(plan.program, "sudo");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn installed_apps_open_directly() {
        let (_d, mut s) = store(&["btop"], "linux");
        type_text(&mut s, "btop");
        assert!(s.current().unwrap().installed);
        assert_eq!(
            s.on_key(StoreKey::Enter),
            Outcome::Run(Launch::Open {
                program: "btop".into()
            })
        );
    }

    #[test]
    fn explains_when_nothing_can_install_it() {
        let (_d, mut s) = store(&[], "linux");
        type_text(&mut s, "btop");
        s.on_key(StoreKey::Enter);
        assert_eq!(s.mode, Mode::Browse);
        assert!(s.notice.as_deref().unwrap().contains("no install command"));
        // The next key clears the notice.
        s.on_key(StoreKey::Down);
        assert!(s.notice.is_none());
    }

    #[test]
    fn screenshots_open_in_a_gallery_that_wraps() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("screenshots")).unwrap();
        for n in ["a.png", "b.png", "c.png"] {
            std::fs::write(d.path().join("screenshots").join(n), "x").unwrap();
        }
        std::fs::write(
            d.path().join("demo.toml"),
            "id = \"demo\"\nname = \"Demo\"\nsummary = \"x\"\ncommand = \"demo\"\nscreenshots = [\"a.png\", \"b.png\", \"c.png\"]\n",
        )
        .unwrap();
        let l = load(Some(d.path()));
        let mut s = Store::new(l.apps, l.problems, "linux", std::ffi::OsStr::new(""));
        type_text(&mut s, "demo");
        assert_eq!(s.on_key(StoreKey::Ctrl('g')), Outcome::Changed);
        let (path, at, of, name) = s.gallery().unwrap();
        assert!(path.ends_with("a.png"));
        assert_eq!((at, of, name), (1, 3, "Demo"));
        s.on_key(StoreKey::Left);
        assert_eq!(s.gallery().unwrap().1, 3, "wraps backwards");
        s.on_key(StoreKey::Right);
        assert_eq!(s.gallery().unwrap().1, 1);
        assert_eq!(
            s.on_key(StoreKey::Text("x")),
            Outcome::Idle,
            "typing does nothing in the gallery"
        );
        assert_eq!(s.on_key(StoreKey::Escape), Outcome::Changed);
        assert!(s.gallery().is_none() && s.mode == Mode::Browse);
        // An app without screenshots explains how to add some.
        s.on_key(StoreKey::ClearLine);
        type_text(&mut s, "btop");
        s.on_key(StoreKey::Ctrl('g'));
        assert!(s.gallery().is_none());
        assert!(s.notice.as_deref().unwrap().contains("no screenshots"));
    }

    #[test]
    fn selection_wraps_and_escape_closes() {
        let (_d, mut s) = store(&[], "linux");
        let n = s.results.len();
        s.on_key(StoreKey::Up);
        assert_eq!(s.selected, n - 1);
        s.on_key(StoreKey::Down);
        assert_eq!(s.selected, 0);
        assert_eq!(s.on_key(StoreKey::Escape), Outcome::Close);
    }

    #[test]
    fn clicking_a_row_acts_like_enter() {
        let (_d, mut s) = store(&["apt"], "linux");
        type_text(&mut s, "btop");
        assert_eq!(
            s.click(0),
            Outcome::Changed,
            "first row asks for confirmation"
        );
        assert!(matches!(s.mode, Mode::Confirm(_)));
        assert_eq!(
            s.click(1),
            Outcome::Idle,
            "clicks are ignored while confirming"
        );
    }
}
