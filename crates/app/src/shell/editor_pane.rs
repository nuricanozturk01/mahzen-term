//! The built-in editor inside the shell: panes that show a file, loading and saving in the background, keyboard and
//! mouse routing, the clipboard, and the guard against losing unsaved changes.
//!
//! An editor pane is a `PaneKind::Panel { module: "editor", panel: "file:<absolute path>" }`, so it is saved with the layout
//! and restored at the next start like any other pane (the file is read again then).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iced::widget::{container, text};
use iced::{clipboard, Element, Length, Padding, Rectangle, Task};
use mahzenterm_core::workspace::PaneKind;
use mahzenterm_editor::{Buffer, EditorPane, EditorState, Effect, Msg, SavedStamp};

use super::{commands, Message, PaneKey, Shell};

/// A buffer read in the background (taken out of the box once, on the UI thread).
pub(crate) type Loaded = Arc<Mutex<Option<Result<Buffer, String>>>>;

/// The module and panel ids of an editor pane.
const MODULE: &str = "editor";

/// The pane kind that edits `path`.
pub fn kind_for(path: &Path) -> PaneKind {
    PaneKind::Panel {
        module: MODULE.into(),
        panel: format!("file:{}", path.display()),
    }
}

/// The file an editor pane shows.
pub fn path_of(kind: &PaneKind) -> Option<PathBuf> {
    let PaneKind::Panel { module, panel } = kind else {
        return None;
    };
    (module == MODULE).then(|| panel.strip_prefix("file:").map(PathBuf::from))?
}

/// Extensions the file viewer (`tf view`) shows better than a text editor can.
pub fn better_viewed(path: &Path) -> bool {
    const VIEWED: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "tga", "qoi", "pdf",
        "docx", "docm", "pptx", "pptm", "xlsx", "xlsm", "xlsb", "xls", "ods", "odt", "epub", "rtf",
        "zip", "jar", "whl", "apk", "tar", "gz", "tgz", "7z", "parquet", "db", "sqlite", "sqlite3",
        "mp4", "mkv", "webm", "mov", "avi", "mp3", "flac", "ogg", "wav", "m4a", "ipynb", "so",
        "dll", "exe", "bin", "o", "a",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIEWED.iter().any(|v| e.eq_ignore_ascii_case(v)))
}

impl Shell {
    fn editor_key_of(&self, key: PaneKey) -> bool {
        self.editors.contains_key(&key)
    }

    /// The editor pane that has the keyboard, if the focused pane is one.
    fn focused_editor_key(&self) -> Option<PaneKey> {
        let key = self.focused_key();
        self.editor_key_of(key).then_some(key)
    }

    /// Show a file in the built-in editor: an open pane for it is brought forward, otherwise a new tab opens.
    /// `goto` is a 1-based line and column to put the cursor on.
    pub(super) fn open_editor_file(&mut self, path: &Path, goto: Option<(usize, usize)>) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let existing = self.window.tabs.iter().find_map(|t| {
            t.pane_ids()
                .into_iter()
                .find(|p| {
                    t.pane(*p).and_then(|pn| path_of(&pn.kind)).as_deref() == Some(path.as_path())
                })
                .map(|p| (t.id, p))
        });
        let key = match existing {
            Some(key) => {
                self.focus_pane_key(key);
                key
            }
            None => {
                self.window.new_tab(kind_for(&path));
                self.sync();
                self.invalidate_active();
                self.focused_key()
            }
        };
        if let (Some((l, c)), Some(p)) = (goto, self.editors.get_mut(&key)) {
            p.state.goto(l, c);
        } else if let Some(g) = goto {
            // Still loading: remember where to go.
            self.editor_goto.insert(key, g);
        }
        self.state_dirty = true;
    }

    /// Queue a background read for every editor pane that has no buffer yet (new panes and restored ones).
    pub(super) fn queue_editor_loads(&mut self) {
        let mut want = Vec::new();
        for t in &self.window.tabs {
            for p in t.pane_ids() {
                let key = (t.id, p);
                if let Some(path) = t.pane(p).and_then(|pn| path_of(&pn.kind)) {
                    if !self.editors.contains_key(&key)
                        && !self.editor_loading.contains_key(&key)
                        && !self.editor_errors.contains_key(&key)
                    {
                        want.push((key, path));
                    }
                }
            }
        }
        for (key, path) in want {
            self.editor_loading.insert(key, path.clone());
            self.editor_queue.push((key, path));
        }
        // Forget editors whose pane is gone.
        let live: std::collections::HashSet<PaneKey> = self
            .window
            .tabs
            .iter()
            .flat_map(|t| t.pane_ids().into_iter().map(move |p| (t.id, p)))
            .collect();
        self.editors.retain(|k, _| live.contains(k));
        self.editor_loading.retain(|k, _| live.contains(k));
        self.editor_errors.retain(|k, _| live.contains(k));
        self.editor_goto.retain(|k, _| live.contains(k));
        self.editor_warned.retain(|k| live.contains(k));
        self.editor_conflict.retain(|k| live.contains(k));
    }

    /// Start the queued reads.
    pub(super) fn editor_tasks(&mut self) -> Task<Message> {
        let jobs = std::mem::take(&mut self.editor_queue);
        Task::batch(jobs.into_iter().map(|(key, path)| {
            Task::perform(
                async move {
                    let r = Buffer::open(&path).map_err(|e| e.to_string());
                    Arc::new(Mutex::new(Some(r)))
                },
                move |l| Message::EditorLoaded(key, l),
            )
        }))
    }

    pub(super) fn on_editor_loaded(&mut self, key: PaneKey, loaded: &Loaded) {
        self.editor_loading.remove(&key);
        let Some(result) = loaded.lock().ok().and_then(|mut g| g.take()) else {
            return;
        };
        match result {
            Ok(buffer) => {
                let mut pane = EditorPane::new(EditorState::new(buffer));
                pane.state.rows = (self.content_area().height / self.metrics.cell_height)
                    .floor()
                    .max(1.0) as usize;
                if let Some((l, c)) = self.editor_goto.remove(&key) {
                    pane.state.goto(l, c);
                }
                self.editors.insert(key, pane);
                self.lsp_attach(key);
            }
            Err(e) => {
                self.editor_errors.insert(key, e);
            }
        }
    }

    /// Mouse messages from an editor pane.
    pub(super) fn on_editor_msg(&mut self, key: PaneKey, msg: Msg) {
        self.set_focus(key);
        self.explorer_focus = false;
        let Some(p) = self.editors.get_mut(&key) else {
            return;
        };
        match msg {
            Msg::Click { line, col, extend } => p.state.click(line, col, extend),
            Msg::DoubleClick { line, col } => p.state.double_click(line, col),
            Msg::Drag { line, col } => p.state.click(line, col, true),
            Msg::Scroll(d) => p.state.scroll_by(d),
            Msg::Rows(n) => p.state.rows = n,
        }
    }

    /// A key press while an editor pane has focus. `None`: not an editor key, let the app handle it.
    pub(super) fn editor_key(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
        text: Option<&str>,
    ) -> Option<Task<Message>> {
        let pane_key = self.focused_editor_key()?;
        let chord = commands::chord_from(key, modifiers)?;
        let effect = self.editors.get_mut(&pane_key)?.state.key(&chord, text)?;
        Some(self.editor_effect(pane_key, effect))
    }

    /// Carry out what the editor asked for.
    pub(super) fn editor_effect(&mut self, key: PaneKey, effect: Effect) -> Task<Message> {
        match effect {
            Effect::None => Task::none(),
            Effect::Save => {
                let overwrite = self.editor_conflict.remove(&key);
                self.save_editor(key, None, overwrite)
            }
            Effect::SaveAs => Task::none(),
            Effect::SaveAsTo(name) => {
                let target = {
                    let base = self
                        .editors
                        .get(&key)
                        .and_then(|p| {
                            p.state
                                .buffer
                                .path()
                                .and_then(Path::parent)
                                .map(Path::to_path_buf)
                        })
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    let p = PathBuf::from(&name);
                    if p.is_absolute() {
                        p
                    } else {
                        base.join(p)
                    }
                };
                self.save_editor(key, Some(target), false)
            }
            Effect::Copy(t) | Effect::Cut(t) => clipboard::write(t),
            Effect::Paste => clipboard::read().map(move |t| Message::EditorPaste(key, t)),
            Effect::QuickOpen => self.open_finder(super::finder::Mode::Files),
            Effect::GotoDefinition => {
                self.lsp_request(key, super::lsp::Ask::Definition);
                Task::none()
            }
            Effect::Suggest => {
                self.lsp_request(key, super::lsp::Ask::Completion);
                Task::none()
            }
            Effect::Hover => {
                self.lsp_request(key, super::lsp::Ask::Hover);
                Task::none()
            }
        }
    }

    /// Save, optionally under a new name or over changes made on disk. The write runs in the background.
    pub(super) fn save_editor(
        &mut self,
        key: PaneKey,
        to: Option<PathBuf>,
        overwrite: bool,
    ) -> Task<Message> {
        let Some(p) = self.editors.get_mut(&key) else {
            return Task::none();
        };
        match p.state.buffer.prepare_save(to.as_deref(), overwrite) {
            Ok(job) => {
                let path = job.path.clone();
                p.state.notice = Some("saving\u{2026}".into());
                Task::perform(
                    async move { job.run().map_err(|e| e.to_string()) },
                    move |r| Message::EditorSaved(key, path.clone(), r),
                )
            }
            Err(mahzenterm_editor::SaveError::NoPath) => {
                p.state.mode = mahzenterm_editor::Mode::SaveAs(String::new());
                Task::none()
            }
            Err(e) => {
                p.state.notice = Some(e.to_string());
                Task::none()
            }
        }
    }

    pub(super) fn on_editor_saved(
        &mut self,
        key: PaneKey,
        path: &Path,
        result: Result<SavedStamp, String>,
    ) {
        let Some(p) = self.editors.get_mut(&key) else {
            return;
        };
        match result {
            Ok(stamp) => {
                p.state.buffer.finish_save(path, stamp);
                p.state.notice = Some("saved".into());
                self.editor_warned.remove(&key);
                self.editor_conflict.remove(&key);
                // The pane now shows (and restores) the file under its real name.
                self.rename_editor_pane(key, path);
            }
            Err(e) => {
                if e.contains("changed on disk") {
                    self.editor_conflict.insert(key);
                    p.state.notice = Some(format!("{e}: press Ctrl+S again to overwrite"));
                } else {
                    p.state.notice = Some(e);
                }
            }
        }
        if let Some(p) = self.editors.get_mut(&key) {
            p.state.revision = p.state.revision.wrapping_add(1);
        }
    }

    /// After "save as": point the pane at the new file name so the layout is restored with it.
    fn rename_editor_pane(&mut self, key: PaneKey, path: &Path) {
        if let Some(tab) = self.window.tabs.iter_mut().find(|t| t.id == key.0) {
            if let Some(pane) = tab.pane_mut(key.1) {
                pane.kind = kind_for(path);
                self.state_dirty = true;
            }
        }
    }

    pub(super) fn editor_paste(&mut self, key: PaneKey, text: Option<String>) {
        if let (Some(t), Some(p)) = (text, self.editors.get_mut(&key)) {
            p.state.paste(&t);
        }
    }

    /// Unsaved editors among the given panes (closing them would lose work).
    fn dirty_among(&self, keys: &[PaneKey]) -> Vec<PaneKey> {
        keys.iter()
            .copied()
            .filter(|k| {
                self.editors
                    .get(k)
                    .is_some_and(|p| p.state.buffer.is_dirty())
            })
            .collect()
    }

    /// Every pane of the given tabs.
    pub(super) fn panes_of_tabs(&self, tabs: &[mahzenterm_core::workspace::TabId]) -> Vec<PaneKey> {
        self.window
            .tabs
            .iter()
            .filter(|t| tabs.contains(&t.id))
            .flat_map(|t| t.pane_ids().into_iter().map(move |p| (t.id, p)))
            .collect()
    }

    /// Ask before closing unsaved work: the first attempt only warns, the second goes through.
    /// Returns true when the close must be stopped.
    pub(super) fn stop_close_for_unsaved(&mut self, keys: &[PaneKey]) -> bool {
        let dirty = self.dirty_among(keys);
        if dirty.is_empty() {
            return false;
        }
        let first_warning = dirty.iter().any(|k| !self.editor_warned.contains(k));
        for k in &dirty {
            self.editor_warned.insert(*k);
            if let Some(p) = self.editors.get_mut(k) {
                p.state.notice =
                    Some("unsaved changes: save with Ctrl+S, or close again to discard".into());
                p.state.revision = p.state.revision.wrapping_add(1);
            }
        }
        if first_warning {
            self.problem = Some(
                "an editor has unsaved changes: close again to discard them, or save with Ctrl+S"
                    .into(),
            );
        }
        first_warning
    }

    /// The pane for an editor panel.
    pub(super) fn editor_pane_view(
        &self,
        key: PaneKey,
        outer: Rectangle,
        path: &Path,
    ) -> Element<'_, Message> {
        let t = self.tokens;
        let body: Element<'_, Message> = match self.editors.get(&key) {
            Some(p) => {
                let focused = self.focused_key() == key && self.palette.is_none();
                mahzenterm_editor::view(p, t, self.metrics, focused, move |m| {
                    Message::EditorMsg(key, m)
                })
            }
            None => {
                let msg = match self.editor_errors.get(&key) {
                    Some(e) => format!("Could not open {}: {e}", path.display()),
                    None => format!("opening {}\u{2026}", path.display()),
                };
                container(text(msg).size(13).color(t.text_dim))
                    .padding(Padding::from(20.0))
                    .into()
            }
        };
        let framed = container(body)
            .width(Length::Fixed(outer.width))
            .height(Length::Fixed(outer.height))
            .style(move |_| container::background(t.bg));
        iced::widget::mouse_area(framed)
            .on_press(Message::FocusPane(key))
            .into()
    }

    /// Tab title of an editor pane: the file name, with a dot while it has unsaved changes.
    pub(super) fn editor_title(&self, key: PaneKey, path: &Path) -> String {
        let name = path
            .file_name()
            .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
        let dirty = self
            .editors
            .get(&key)
            .is_some_and(|p| p.state.buffer.is_dirty());
        format!("{}{name}", if dirty { "\u{25cf} " } else { "" })
    }
}

/// Used by the state of `Shell`: an empty map of pending go-to positions.
pub(super) type GotoMap = HashMap<PaneKey, (usize, usize)>;

#[cfg(test)]
mod tests {
    use super::*;

    fn load(s: &mut Shell, path: &Path) -> PaneKey {
        s.open_editor_file(path, Some((1, 1)));
        s.queue_editor_loads();
        let (key, p) = s.editor_queue.pop().expect("a load is queued");
        let loaded = Arc::new(Mutex::new(Some(
            Buffer::open(&p).map_err(|e| e.to_string()),
        )));
        s.on_editor_loaded(key, &loaded);
        key
    }

    #[test]
    fn opening_a_file_makes_an_editor_tab_that_guards_unsaved_work() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let mut s = super::super::tests::shell();
        let tabs = s.window.tabs.len();
        let key = load(&mut s, &file);
        assert_eq!(s.window.tabs.len(), tabs + 1);
        assert!(s.editors.contains_key(&key));
        // Opening it again reuses the pane.
        s.open_editor_file(&file, None);
        assert_eq!(s.window.tabs.len(), tabs + 1);
        // Typing marks it dirty; the first close attempt is refused, the second goes through.
        s.editors.get_mut(&key).unwrap().state.type_text("x");
        assert!(s.editors[&key].state.buffer.is_dirty());
        assert!(s.stop_close_for_unsaved(&[key]));
        assert!(!s.stop_close_for_unsaved(&[key]));
        assert!(s.editor_title(key, &file).starts_with('\u{25cf}'));
    }

    #[test]
    fn editor_panels_encode_the_file_path() {
        let p = Path::new("/home/me/a b/main.rs");
        let k = kind_for(p);
        assert_eq!(path_of(&k).as_deref(), Some(p));
        assert_eq!(
            path_of(&PaneKind::Panel {
                module: "ai_usage".into(),
                panel: "tokens".into()
            }),
            None
        );
        assert_eq!(path_of(&PaneKind::terminal()), None);
        assert_eq!(
            path_of(&PaneKind::Panel {
                module: "editor".into(),
                panel: "nonsense".into()
            }),
            None
        );
    }

    #[test]
    fn binary_documents_go_to_the_viewer_and_text_to_the_editor() {
        for f in [
            "a.PNG", "b.pdf", "c.xlsx", "d.tar.gz", "e.sqlite", "f.mp4", "g.ipynb",
        ] {
            assert!(better_viewed(Path::new(f)), "{f}");
        }
        for f in [
            "main.rs",
            "README.md",
            "notes.txt",
            "Makefile",
            "config.toml",
            "x.log",
            "a.json",
        ] {
            assert!(!better_viewed(Path::new(f)), "{f}");
        }
    }
}
