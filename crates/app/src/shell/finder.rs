//! "Go to File" (Ctrl+Alt+P) and "Find in Files" (Ctrl+Alt+F): one overlay with a query line and a result list.
//!
//! The file index is built in the background when the overlay opens. File-name ranking is instant; text search runs
//! in the background after a short pause in typing, and answers for an older query are dropped. Enter opens the
//! result in the built-in editor (at the matching line for text hits).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{Background, Border, Element, Length, Padding, Task};
use mahzenterm_project_search as ps;
use mahzenterm_ui_kit::Tokens;

use super::{Message, Shell};

/// Rows shown at once.
const VISIBLE: usize = 12;
/// Pause after the last keystroke before a text search starts.
const PAUSE: Duration = Duration::from_millis(180);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Files,
    Text,
}

/// One result row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    /// 1-based line and column for text hits.
    pub at: Option<(usize, usize)>,
    pub detail: String,
}

/// The overlay state.
#[derive(Debug)]
pub struct Finder {
    pub mode: Mode,
    pub query: String,
    pub selected: usize,
    pub root: PathBuf,
    /// `None` while the index is being built.
    pub index: Option<Vec<String>>,
    pub items: Vec<Item>,
    pub regex: bool,
    pub case_sensitive: bool,
    pub error: Option<String>,
    pub busy: bool,
    /// Latest query generation, shared with the background searches so stale ones stop early.
    pub generation: Arc<AtomicU64>,
}

impl Finder {
    fn new(mode: Mode, root: PathBuf) -> Self {
        Self {
            mode,
            query: String::new(),
            selected: 0,
            root,
            index: None,
            items: Vec::new(),
            regex: false,
            case_sensitive: false,
            error: None,
            busy: true,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    fn first_visible(&self) -> usize {
        self.selected.saturating_sub(VISIBLE - 1)
    }

    /// Recompute file results from the index (instant).
    fn rank_files(&mut self) {
        let Some(index) = &self.index else { return };
        self.items = ps::rank(index, &self.query)
            .into_iter()
            .map(|p| Item {
                path: p,
                at: None,
                detail: String::new(),
            })
            .collect();
        self.selected = 0;
    }
}

/// The project folder of `dir`: the nearest ancestor with a `.git`, else `dir` itself.
pub fn project_root(dir: &Path) -> PathBuf {
    dir.ancestors()
        .find(|a| a.join(".git").exists())
        .unwrap_or(dir)
        .to_path_buf()
}

impl Shell {
    pub(super) fn open_finder(&mut self, mode: Mode) -> Task<Message> {
        let dir = self
            .explorer
            .as_ref()
            .map(|e| e.root().to_path_buf())
            .or_else(|| self.focused_cwd())
            .or_else(mahzenterm_platform::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let root = project_root(&dir);
        self.palette = None;
        self.finder = Some(Finder::new(mode, root.clone()));
        Task::perform(
            async move { (root.clone(), ps::collect(&root)) },
            |(root, idx)| Message::FinderIndex(root, idx),
        )
    }

    pub(super) fn on_finder_index(&mut self, root: &Path, index: Vec<String>) -> Task<Message> {
        let Some(f) = self.finder.as_mut().filter(|f| f.root == root) else {
            return Task::none();
        };
        f.index = Some(index);
        f.busy = false;
        match f.mode {
            Mode::Files => {
                f.rank_files();
                Task::none()
            }
            Mode::Text => self.finder_search(),
        }
    }

    /// Start a text search for the current query (after a pause; older searches are dropped).
    fn finder_search(&mut self) -> Task<Message> {
        let Some(f) = self.finder.as_mut() else {
            return Task::none();
        };
        let (Some(index), Mode::Text) = (f.index.clone(), f.mode) else {
            return Task::none();
        };
        let generation = f.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = Arc::clone(&f.generation);
        let (root, query) = (
            f.root.clone(),
            ps::Query {
                pattern: f.query.clone(),
                regex: f.regex,
                case_sensitive: f.case_sensitive,
            },
        );
        f.busy = !query.pattern.is_empty();
        Task::perform(
            async move {
                std::thread::sleep(PAUSE);
                if latest.load(Ordering::SeqCst) != generation {
                    return (generation, None);
                }
                (generation, Some(ps::grep(&root, &index, &query)))
            },
            |(g, r)| Message::FinderHits(g, r),
        )
    }

    pub(super) fn on_finder_hits(
        &mut self,
        generation: u64,
        result: Option<Result<Vec<ps::Hit>, String>>,
    ) {
        let Some(f) = self.finder.as_mut() else {
            return;
        };
        if f.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        f.busy = false;
        let Some(result) = result else { return };
        match result {
            Ok(hits) => {
                f.error = None;
                f.items = hits
                    .into_iter()
                    .map(|h| Item {
                        detail: format!("{}: {}", h.line, h.text),
                        at: Some((h.line, h.col)),
                        path: h.path,
                    })
                    .collect();
            }
            Err(e) => {
                f.error = Some(e);
                f.items.clear();
            }
        }
        f.selected = 0;
    }

    /// Keys while the overlay is open.
    pub(super) fn finder_key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> Task<Message> {
        let Some(f) = self.finder.as_mut() else {
            return Task::none();
        };
        let n = f.items.len();
        let mut changed = false;
        match key {
            Key::Named(Named::Escape) => {
                self.finder = None;
                return Task::none();
            }
            Key::Named(Named::Enter) => return self.finder_open(None),
            Key::Named(Named::ArrowUp) if n > 0 => f.selected = (f.selected + n - 1) % n,
            Key::Named(Named::ArrowDown) if n > 0 => f.selected = (f.selected + 1) % n,
            Key::Named(Named::Backspace) => changed = f.query.pop().is_some(),
            Key::Character(c) if modifiers.alt() && c.eq_ignore_ascii_case("r") => {
                f.regex = !f.regex;
                changed = true;
            }
            Key::Character(c) if modifiers.alt() && c.eq_ignore_ascii_case("c") => {
                f.case_sensitive = !f.case_sensitive;
                changed = true;
            }
            _ => {
                if let Some(t) = text.filter(|t| {
                    !modifiers.control() && !modifiers.alt() && !t.chars().any(char::is_control)
                }) {
                    f.query.push_str(t);
                    changed = true;
                }
            }
        }
        if !changed {
            return Task::none();
        }
        match f.mode {
            Mode::Files => {
                f.rank_files();
                Task::none()
            }
            Mode::Text => self.finder_search(),
        }
    }

    /// Open the chosen (or given) row in the editor and close the overlay.
    pub(super) fn finder_open(&mut self, row: Option<usize>) -> Task<Message> {
        let Some(f) = self.finder.take() else {
            return Task::none();
        };
        let Some(item) = f.items.get(row.unwrap_or(f.selected)) else {
            self.finder = Some(f);
            return Task::none();
        };
        let path = f.root.join(&item.path);
        self.open_editor_file(&path, item.at);
        Task::none()
    }

    pub(super) fn finder_view(&self, t: Tokens) -> Option<Element<'_, Message>> {
        let f = self.finder.as_ref()?;
        let title = match f.mode {
            Mode::Files => "GO TO FILE",
            Mode::Text => "FIND IN FILES",
        };
        let placeholder = match f.mode {
            Mode::Files => "File name...",
            Mode::Text => "Text to find...",
        };
        let query = if f.query.is_empty() {
            row![
                text("\u{203a}").size(16).color(t.accent),
                text(placeholder).size(15).color(t.text_dim)
            ]
        } else {
            row![
                text("\u{203a}").size(16).color(t.accent),
                text(format!("{}\u{258f}", f.query)).size(15).color(t.text)
            ]
        }
        .spacing(10)
        .align_y(iced::Alignment::Center);
        let mut list = column![].spacing(1);
        for (i, item) in f
            .items
            .iter()
            .enumerate()
            .skip(f.first_visible())
            .take(VISIBLE)
        {
            let selected = i == f.selected;
            let line = row![
                text(item.path.clone())
                    .size(13)
                    .color(if selected { t.text } else { t.text_dim }),
                text(item.detail.clone()).size(12).color(t.text_dim),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center);
            let rowbox = container(line)
                .width(Length::Fill)
                .padding(Padding::from([6.0, 12.0]))
                .style(move |_| container::Style {
                    background: selected.then_some(Background::Color(t.surface_hi)),
                    border: Border::default().rounded(6.0),
                    ..container::Style::default()
                });
            list = list.push(mouse_area(rowbox).on_press(Message::FinderRow(i)));
        }
        if f.items.is_empty() {
            let msg = if let Some(e) = &f.error {
                e.clone()
            } else if f.busy {
                "searching...".to_owned()
            } else if f.query.is_empty() && f.mode == Mode::Text {
                "type to search the project".to_owned()
            } else {
                "no matches".to_owned()
            };
            list = list.push(
                container(text(msg).size(13).color(t.text_dim))
                    .padding(Padding::from([10.0, 14.0])),
            );
        }
        let flags = if f.mode == Mode::Text {
            format!(
                "alt+r regex {}  alt+c case {}",
                if f.regex { "on" } else { "off" },
                if f.case_sensitive { "on" } else { "off" }
            )
        } else {
            String::new()
        };
        let footer = row![
            text("\u{2191}\u{2193} navigate   \u{23ce} open   esc close")
                .size(11)
                .color(t.text_dim),
            Space::new().width(Length::Fill),
            text(flags).size(11).color(t.text_dim),
            text(format!(
                "  {}",
                f.root
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            ))
            .size(11)
            .color(t.accent),
        ];
        let card = container(
            column![
                container(crate::chrome::tag(title, t, true)).padding(Padding::from([2.0, 8.0])),
                container(query).padding(Padding::from([4.0, 8.0])),
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fixed(1.0))
                    .style(move |_| container::background(t.border)),
                list,
                container(footer).padding(Padding::from([4.0, 8.0])),
            ]
            .spacing(8),
        )
        .width(Length::Fixed(760.0))
        .padding(10)
        .style(move |_| container::Style {
            background: Some(Background::Color(t.surface)),
            border: Border {
                color: t.border,
                width: 1.0,
                radius: 12.0.into(),
            },
            ..container::Style::default()
        });
        let backdrop = mouse_area(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press(Message::FinderClose);
        Some(
            iced::widget::stack![
                backdrop,
                container(card)
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center)
                    .padding(Padding {
                        top: 64.0,
                        ..Padding::ZERO
                    })
            ]
            .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_into(s: &mut Shell, text: &str) {
        for ch in text.chars() {
            let t = ch.to_string();
            let _ = s.finder_key(
                &Key::Character(t.as_str().into()),
                Modifiers::empty(),
                Some(&t),
            );
        }
    }

    #[test]
    fn go_to_file_ranks_names_and_enter_opens_the_editor() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(d.path().join("README.md"), "hi\n").unwrap();
        let mut s = super::super::tests::shell();
        s.finder = Some(Finder::new(Mode::Files, d.path().to_path_buf()));
        let _ = s.on_finder_index(d.path(), ps::collect(d.path()));
        type_into(&mut s, "main");
        assert_eq!(s.finder.as_ref().unwrap().items[0].path, "src/main.rs");
        let tabs = s.window.tabs.len();
        let _ = s.finder_key(&Key::Named(Named::Enter), Modifiers::empty(), None);
        assert!(s.finder.is_none(), "the overlay closes");
        assert_eq!(s.window.tabs.len(), tabs + 1, "an editor tab opened");
    }

    #[test]
    fn text_hits_open_at_their_line_and_stale_answers_are_dropped() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "one\ntwo needle\n").unwrap();
        let mut s = super::super::tests::shell();
        s.finder = Some(Finder::new(Mode::Text, d.path().to_path_buf()));
        let _ = s.on_finder_index(d.path(), vec!["a.txt".into()]);
        type_into(&mut s, "needle");
        let g = s.finder.as_ref().unwrap().generation.load(Ordering::SeqCst);
        // An answer for an older query changes nothing.
        s.on_finder_hits(g - 1, Some(Ok(Vec::new())));
        assert!(s.finder.as_ref().unwrap().busy);
        let q = ps::Query {
            pattern: "needle".into(),
            regex: false,
            case_sensitive: false,
        };
        let hits = ps::grep(d.path(), &["a.txt".into()], &q);
        s.on_finder_hits(g, Some(hits));
        let f = s.finder.as_ref().unwrap();
        assert_eq!(f.items[0].at, Some((2, 5)));
        let _ = s.finder_open(None);
        let tab = s.window.active_tab();
        assert!(super::super::editor_pane::path_of(&tab.pane(tab.focus).unwrap().kind).is_some());
        // Bad patterns show a message instead of results.
        s.finder = Some(Finder::new(Mode::Text, d.path().to_path_buf()));
        s.finder.as_mut().unwrap().regex = true;
        let g = s
            .finder
            .as_ref()
            .unwrap()
            .generation
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        s.on_finder_hits(g, Some(Err("bad pattern: x".into())));
        assert_eq!(
            s.finder.as_ref().unwrap().error.as_deref(),
            Some("bad pattern: x")
        );
    }

    #[test]
    fn project_root_is_the_nearest_git_folder() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(d.path().join("repo/src/deep")).unwrap();
        assert_eq!(
            project_root(&d.path().join("repo/src/deep")),
            d.path().join("repo")
        );
        let plain = d.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        // No repository above (the temp dir itself has none): the folder itself.
        assert_eq!(project_root(&plain).file_name(), plain.file_name());
    }
}
