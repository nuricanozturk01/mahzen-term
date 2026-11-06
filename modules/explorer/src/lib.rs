//! File explorer: a lazily read directory tree with keyboard and mouse navigation.
//!
//! [`Explorer`] is pure state (testable without a window); [`view`] draws it with theme tokens.

pub mod git;
pub mod ops;
pub mod search;
mod view;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};

pub use view::{fit_name, view, Event, HEADER_H, PANEL_W, ROW_H};

/// Command ids of this module.
pub mod id {
    pub const TOGGLE: &str = "explorer.toggle";
    pub const FOCUS: &str = "explorer.focus";
}

/// Directories hidden unless hidden files are shown (build output and caches are noise in a tree).
const NOISE: &[&str] = &["target", "node_modules", "__pycache__", ".git"];
/// Most entries read from one directory.
const MAX_ENTRIES: usize = 5000;

/// One entry of a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// One visible line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub path: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

/// Which prompt to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptWhat {
    NewFile,
    NewDir,
    Rename,
    Trash,
}

/// What activating a row asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A file was activated: show it.
    Open(PathBuf),
    Nothing,
}

/// The explorer's state.
#[derive(Debug)]
pub struct Explorer {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    cache: HashMap<PathBuf, Vec<Entry>>,
    rows: Vec<Row>,
    selected: usize,
    scroll: usize,
    show_hidden: bool,
    error: Option<String>,
    git: git::GitStatus,
    search: Option<SearchState>,
    prompt: Option<Prompt>,
}

/// What a text prompt in the panel is asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    NewFile(PathBuf),
    NewDir(PathBuf),
    Rename(PathBuf),
    /// Confirm moving this path to the trash.
    Trash(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Prompt {
    kind: PromptKind,
    input: String,
}

/// What submitting a prompt did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptOutcome {
    /// Still open (for example a bad name; see [`Explorer::error`]).
    Open,
    /// Done; the new or renamed path is selected.
    Done,
    /// The user confirmed: the app moves this path to the trash.
    Trash(PathBuf),
}

/// Fuzzy file search in progress: the query and the (lazily built) index.
#[derive(Debug, Default)]
struct SearchState {
    query: String,
    index: Option<Vec<String>>,
}

/// Read and sort one directory: folders first, then names ignoring case.
pub fn list_dir(dir: &Path, show_hidden: bool) -> Result<Vec<Entry>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out: Vec<Entry> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !show_hidden && (name.starts_with('.') || NOISE.contains(&name.as_str())) {
                return None;
            }
            let path = e.path();
            // Follow symlinks to decide folder vs file; a broken link is a file.
            let is_dir = path.is_dir();
            Some(Entry { name, path, is_dir })
        })
        .take(MAX_ENTRIES)
        .collect();
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

impl Explorer {
    pub fn new(root: PathBuf) -> Self {
        let mut e = Self {
            root,
            expanded: HashSet::new(),
            cache: HashMap::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            show_hidden: false,
            error: None,
            git: git::GitStatus::default(),
            search: None,
            prompt: None,
        };
        e.rebuild();
        e
    }

    /// Replace the git marks (computed off the UI thread). Ignored when they belong to another repository.
    pub fn set_git(&mut self, status: Option<git::GitStatus>) {
        self.git = match status {
            Some(s) if self.root.starts_with(&s.root) || s.root.starts_with(&self.root) => s,
            _ => git::GitStatus::default(),
        };
    }

    /// The git mark of a path, if it changed.
    pub fn mark(&self, path: &Path) -> Option<git::Mark> {
        self.git.mark(path)
    }

    /// Start fuzzy search. The app then builds the index off the UI thread ([`search::collect`]) and hands it to
    /// [`Explorer::set_index`].
    pub fn search_start(&mut self) {
        self.search = Some(SearchState::default());
        self.rebuild();
    }

    pub fn searching(&self) -> bool {
        self.search.is_some()
    }

    /// The query while searching.
    pub fn search_query(&self) -> Option<&str> {
        self.search.as_ref().map(|s| s.query.as_str())
    }

    /// Whether the search still waits for its file index.
    pub fn index_pending(&self) -> bool {
        self.search.as_ref().is_some_and(|s| s.index.is_none())
    }

    /// Folders whose listing is on screen: the root and every open folder (watch these for changes).
    pub fn watched_dirs(&self) -> Vec<PathBuf> {
        let mut v = vec![self.root.clone()];
        v.extend(
            self.expanded
                .iter()
                .filter(|p| p.starts_with(&self.root))
                .cloned(),
        );
        v
    }

    /// Hand over the file index built by [`search::collect`]; results appear for the current query.
    pub fn set_index(&mut self, index: Vec<String>) {
        if let Some(s) = self.search.as_mut() {
            s.index = Some(index);
            self.rebuild();
        }
    }

    pub fn search_push(&mut self, text: &str) {
        if let Some(s) = self.search.as_mut() {
            s.query.extend(text.chars().filter(|c| !c.is_control()));
            self.rebuild();
        }
    }

    pub fn search_backspace(&mut self) {
        if let Some(s) = self.search.as_mut() {
            s.query.pop();
            self.rebuild();
        }
    }

    pub fn search_exit(&mut self) {
        if self.search.take().is_some() {
            self.selected = 0;
            self.scroll = 0;
            self.rebuild();
        }
    }

    /// Ask for a name: new file or folder in the selected folder (or the selection's folder), or confirm a trash.
    pub fn prompt_start(&mut self, what: PromptWhat) {
        let Some(row) = self.selected_row().cloned() else {
            if matches!(what, PromptWhat::NewFile | PromptWhat::NewDir) {
                self.begin_prompt(what, self.root.clone(), String::new());
            }
            return;
        };
        let dir = self.selected_dir();
        match what {
            PromptWhat::NewFile | PromptWhat::NewDir => self.begin_prompt(what, dir, String::new()),
            PromptWhat::Rename => self.begin_prompt(what, row.path.clone(), row.name),
            PromptWhat::Trash => self.begin_prompt(what, row.path, String::new()),
        }
    }

    fn begin_prompt(&mut self, what: PromptWhat, path: PathBuf, input: String) {
        self.error = None;
        let kind = match what {
            PromptWhat::NewFile => PromptKind::NewFile(path),
            PromptWhat::NewDir => PromptKind::NewDir(path),
            PromptWhat::Rename => PromptKind::Rename(path),
            PromptWhat::Trash => PromptKind::Trash(path),
        };
        self.prompt = Some(Prompt { kind, input });
    }

    pub fn prompting(&self) -> bool {
        self.prompt.is_some()
    }

    /// The prompt as a line of text for the header, e.g. `new file: abc▏`.
    pub fn prompt_line(&self) -> Option<String> {
        let p = self.prompt.as_ref()?;
        Some(match &p.kind {
            PromptKind::NewFile(_) => format!("new file: {}\u{258f}", p.input),
            PromptKind::NewDir(_) => format!("new folder: {}\u{258f}", p.input),
            PromptKind::Rename(_) => format!("rename: {}\u{258f}", p.input),
            PromptKind::Trash(path) => format!(
                "trash {}? Enter yes, Esc no",
                path.file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            ),
        })
    }

    pub fn prompt_push(&mut self, text: &str) {
        if let Some(p) = self
            .prompt
            .as_mut()
            .filter(|p| !matches!(p.kind, PromptKind::Trash(_)))
        {
            p.input.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn prompt_backspace(&mut self) {
        if let Some(p) = self.prompt.as_mut() {
            p.input.pop();
        }
    }

    pub fn prompt_cancel(&mut self) {
        self.prompt = None;
        self.error = None;
    }

    /// Enter in a prompt: run the operation.
    pub fn prompt_submit(&mut self) -> PromptOutcome {
        let Some(p) = self.prompt.clone() else {
            return PromptOutcome::Open;
        };
        let result = match &p.kind {
            PromptKind::NewFile(dir) => ops::create_file(dir, &p.input),
            PromptKind::NewDir(dir) => ops::create_dir(dir, &p.input),
            PromptKind::Rename(path) => ops::rename(path, &p.input),
            PromptKind::Trash(path) => {
                self.prompt = None;
                return PromptOutcome::Trash(path.clone());
            }
        };
        match result {
            Ok(path) => {
                self.prompt = None;
                self.after_change(&path);
                PromptOutcome::Done
            }
            Err(e) => {
                self.error = Some(e);
                PromptOutcome::Open
            }
        }
    }

    /// Re-read after a change and select (and reveal) `path` when it is visible.
    pub fn after_change(&mut self, path: &Path) {
        if let Some(parent) = path.parent() {
            if parent != self.root {
                self.expanded.insert(parent.to_path_buf());
            }
        }
        self.refresh();
        if let Some(i) = self.rows.iter().position(|r| r.path == path) {
            self.selected = i;
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Last read problem, in words.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// The folder a new terminal for the selection should open in.
    pub fn selected_dir(&self) -> PathBuf {
        match self.selected_row() {
            Some(r) if r.is_dir => r.path.clone(),
            Some(r) => r
                .path
                .parent()
                .map_or_else(|| self.root.clone(), Path::to_path_buf),
            None => self.root.clone(),
        }
    }

    /// Show another folder as the root.
    pub fn set_root(&mut self, root: PathBuf) {
        if root == self.root {
            return;
        }
        self.root = root;
        self.expanded.clear();
        self.cache.clear();
        self.selected = 0;
        self.scroll = 0;
        self.rebuild();
    }

    /// Go to the parent folder, keeping the old root selected.
    pub fn up(&mut self) {
        let Some(parent) = self.root.parent().map(Path::to_path_buf) else {
            return;
        };
        let old = self.root.clone();
        self.set_root(parent);
        if let Some(i) = self.rows.iter().position(|r| r.path == old) {
            self.selected = i;
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refresh();
    }

    /// Re-read every opened folder from disk.
    pub fn refresh(&mut self) {
        self.cache.clear();
        self.rebuild();
    }

    fn entries(&mut self, dir: &Path) -> Vec<Entry> {
        if let Some(c) = self.cache.get(dir) {
            return c.clone();
        }
        let list = match list_dir(dir, self.show_hidden) {
            Ok(l) => l,
            Err(e) => {
                self.error = Some(e);
                Vec::new()
            }
        };
        self.cache.insert(dir.to_path_buf(), list.clone());
        list
    }

    fn rebuild(&mut self) {
        if let Some(s) = &self.search {
            // Search results replace the tree: one row per matching file, shown by its relative path.
            let found = s
                .index
                .as_deref()
                .map_or_else(Vec::new, |i| search::rank(i, &s.query));
            self.rows = found
                .into_iter()
                .map(|rel| Row {
                    path: self.root.join(&rel),
                    name: rel,
                    depth: 0,
                    is_dir: false,
                    expanded: false,
                })
                .collect();
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        let keep = self.selected_row().map(|r| r.path.clone());
        self.error = None;
        let root = self.root.clone();
        self.rows = self.subtree(&root, 0);
        self.selected = keep
            .and_then(|p| self.rows.iter().position(|r| r.path == p))
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
    }

    fn subtree(&mut self, dir: &Path, depth: usize) -> Vec<Row> {
        let mut out = Vec::new();
        for e in self.entries(dir) {
            let expanded = e.is_dir && self.expanded.contains(&e.path);
            let path = e.path.clone();
            out.push(Row {
                name: e.name,
                path: e.path,
                depth,
                is_dir: e.is_dir,
                expanded,
            });
            if expanded {
                out.extend(self.subtree(&path, depth + 1));
            }
        }
        out
    }

    /// Move the selection by `delta` rows (clamped) and keep it inside a window of `capacity` rows.
    pub fn move_selection(&mut self, delta: isize, capacity: usize) {
        let last = self.rows.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.reveal(capacity);
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    /// Scroll the window by `delta` rows without moving the selection.
    pub fn scroll_by(&mut self, delta: isize, capacity: usize) {
        let max = self.rows.len().saturating_sub(capacity.max(1));
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    /// Adjust the scroll offset so the selected row is visible in a window of `capacity` rows.
    pub fn reveal(&mut self, capacity: usize) {
        let cap = capacity.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + cap {
            self.scroll = self.selected + 1 - cap;
        }
        let max = self.rows.len().saturating_sub(cap);
        self.scroll = self.scroll.min(max);
    }

    pub fn expand(&mut self, index: usize) {
        if let Some(r) = self.rows.get(index).filter(|r| r.is_dir && !r.expanded) {
            self.expanded.insert(r.path.clone());
            self.rebuild();
        }
    }

    pub fn collapse(&mut self, index: usize) {
        if let Some(r) = self.rows.get(index).filter(|r| r.is_dir && r.expanded) {
            self.expanded.remove(&r.path.clone());
            self.rebuild();
        }
    }

    /// Left arrow: collapse an open folder, else move to the parent row.
    pub fn left(&mut self, capacity: usize) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir && row.expanded {
            self.collapse(self.selected);
        } else if let Some(i) = self.rows[..self.selected]
            .iter()
            .rposition(|r| r.depth < row.depth)
        {
            self.selected = i;
            self.reveal(capacity);
        }
    }

    /// Right arrow: open a folder, or step into its first child.
    pub fn right(&mut self, capacity: usize) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir && !row.expanded {
            self.expand(self.selected);
        } else if row.is_dir {
            self.move_selection(1, capacity);
        }
    }

    /// Enter / double click: toggle a folder, open a file.
    pub fn activate(&mut self, index: usize) -> Action {
        self.select(index);
        match self.rows.get(index).cloned() {
            Some(r) if r.is_dir && r.expanded => {
                self.collapse(index);
                Action::Nothing
            }
            Some(r) if r.is_dir => {
                self.expand(index);
                Action::Nothing
            }
            Some(r) => Action::Open(r.path),
            None => Action::Nothing,
        }
    }
}

/// The explorer as an app module.
pub struct ExplorerModule;

impl Module for ExplorerModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "explorer",
            name: "File Explorer",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::FsRead, Capability::FsWrite],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(id::TOGGLE, "Toggle File Explorer", "View").key("Ctrl+B"),
            CommandSpec::new(id::FOCUS, "Focus File Explorer", "View").key("Ctrl+Alt+B"),
        ]
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn watched_dirs_are_the_root_and_open_folders() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        let mut ex = Explorer::new(d.path().to_path_buf());
        assert_eq!(ex.watched_dirs(), vec![d.path().to_path_buf()]);
        ex.expand(0);
        let dirs = ex.watched_dirs();
        assert_eq!(dirs.len(), 2);
        assert!(dirs.contains(&d.path().join("a")));
    }

    #[test]
    fn prompts_create_rename_and_ask_before_trashing() {
        let d = tempfile::tempdir().unwrap();
        let mut ex = Explorer::new(d.path().to_path_buf());
        ex.prompt_start(PromptWhat::NewFile);
        ex.prompt_push("a.txt");
        assert_eq!(ex.prompt_line().unwrap(), "new file: a.txt\u{258f}");
        assert_eq!(ex.prompt_submit(), PromptOutcome::Done);
        assert!(d.path().join("a.txt").is_file());
        assert_eq!(
            ex.selected_row().unwrap().name,
            "a.txt",
            "the new file is selected"
        );
        // A clash keeps the prompt open and explains.
        ex.prompt_start(PromptWhat::NewFile);
        ex.prompt_push("a.txt");
        assert_eq!(ex.prompt_submit(), PromptOutcome::Open);
        assert!(ex.error().unwrap().contains("already exists"));
        ex.prompt_cancel();
        assert!(!ex.prompting() && ex.error().is_none());
        // Rename starts with the current name.
        ex.prompt_start(PromptWhat::Rename);
        assert_eq!(ex.prompt_line().unwrap(), "rename: a.txt\u{258f}");
        for _ in 0..5 {
            ex.prompt_backspace();
        }
        ex.prompt_push("b.txt");
        assert_eq!(ex.prompt_submit(), PromptOutcome::Done);
        assert!(d.path().join("b.txt").is_file() && !d.path().join("a.txt").exists());
        // Trash only asks; typing is ignored and the file stays until the app acts.
        ex.prompt_start(PromptWhat::Trash);
        ex.prompt_push("zzz");
        assert!(ex.prompt_line().unwrap().starts_with("trash b.txt?"));
        assert_eq!(
            ex.prompt_submit(),
            PromptOutcome::Trash(d.path().join("b.txt"))
        );
        assert!(d.path().join("b.txt").is_file());
        // New folder inside a selected folder expands it.
        ex.prompt_start(PromptWhat::NewDir);
        ex.prompt_push("sub");
        assert_eq!(ex.prompt_submit(), PromptOutcome::Done);
        ex.prompt_start(PromptWhat::NewFile);
        ex.prompt_push("in.txt");
        assert_eq!(ex.prompt_submit(), PromptOutcome::Done);
        assert!(d.path().join("sub/in.txt").is_file());
        assert!(
            ex.rows().iter().any(|r| r.name == "in.txt"),
            "the folder opened to show it"
        );
    }

    #[test]
    fn search_replaces_the_tree_with_ranked_files_and_restores_it() {
        let d = tempfile::tempdir().unwrap();
        for f in ["src/main.rs", "src/lib.rs", "README.md"] {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let mut ex = Explorer::new(d.path().to_path_buf());
        let tree_rows = ex.rows().len();
        ex.search_start();
        assert!(ex.searching() && ex.index_pending() && ex.rows().is_empty());
        ex.set_index(search::collect(d.path(), false));
        assert!(!ex.index_pending());
        assert_eq!(ex.rows().len(), 3);
        ex.search_push("main");
        assert_eq!(ex.rows()[0].name, "src/main.rs");
        assert_eq!(ex.rows()[0].path, d.path().join("src/main.rs"));
        assert_eq!(
            ex.activate(0),
            Action::Open(d.path().join("src/main.rs")),
            "a result opens like a file"
        );
        ex.search_backspace();
        assert_eq!(ex.search_query(), Some("mai"));
        ex.search_exit();
        assert!(!ex.searching());
        assert_eq!(ex.rows().len(), tree_rows, "the tree is back");
    }

    use super::*;
    use std::fs;

    fn sample() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        let p = d.path();
        fs::create_dir_all(p.join("src/inner")).expect("mkdir");
        fs::create_dir_all(p.join("docs")).expect("mkdir");
        fs::create_dir_all(p.join(".git")).expect("mkdir");
        fs::create_dir_all(p.join("target")).expect("mkdir");
        fs::write(p.join("README.md"), "# hi").expect("write");
        fs::write(p.join("Cargo.toml"), "").expect("write");
        fs::write(p.join(".env"), "").expect("write");
        fs::write(p.join("src/main.rs"), "").expect("write");
        fs::write(p.join("src/inner/deep.rs"), "").expect("write");
        d
    }

    fn names(e: &Explorer) -> Vec<String> {
        e.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    #[test]
    fn folders_first_sorted_and_noise_hidden() {
        let d = sample();
        let e = Explorer::new(d.path().to_path_buf());
        assert_eq!(names(&e), ["docs", "src", "Cargo.toml", "README.md"]);
    }

    #[test]
    fn hidden_files_can_be_shown() {
        let d = sample();
        let mut e = Explorer::new(d.path().to_path_buf());
        e.toggle_hidden();
        let n = names(&e);
        assert!(
            n.contains(&".git".to_owned())
                && n.contains(&".env".to_owned())
                && n.contains(&"target".to_owned())
        );
    }

    #[test]
    fn expanding_nests_in_place_and_collapsing_restores() {
        let d = sample();
        let mut e = Explorer::new(d.path().to_path_buf());
        e.activate(1); // src
        assert_eq!(
            names(&e),
            [
                "docs",
                "src",
                "  inner",
                "  main.rs",
                "Cargo.toml",
                "README.md"
            ]
        );
        e.activate(2); // src/inner
        assert!(names(&e).contains(&"    deep.rs".to_owned()));
        e.activate(1); // collapse src
        assert_eq!(names(&e), ["docs", "src", "Cargo.toml", "README.md"]);
    }

    #[test]
    fn activating_a_file_asks_to_open_it() {
        let d = sample();
        let mut e = Explorer::new(d.path().to_path_buf());
        assert_eq!(e.activate(3), Action::Open(d.path().join("README.md")));
    }

    #[test]
    fn arrows_walk_the_tree() {
        let d = sample();
        let mut e = Explorer::new(d.path().to_path_buf());
        e.move_selection(1, 10); // src
        e.right(10); // open
        e.right(10); // into inner
        assert_eq!(e.selected_row().map(|r| r.name.as_str()), Some("inner"));
        e.left(10); // inner is closed: go to the parent
        assert_eq!(e.selected_row().map(|r| r.name.as_str()), Some("src"));
        e.left(10); // collapse src
        assert_eq!(names(&e).len(), 4);
        e.move_selection(100, 10);
        assert_eq!(e.selected_row().map(|r| r.name.as_str()), Some("README.md"));
        e.move_selection(-100, 10);
        assert_eq!(e.selected(), 0);
    }

    #[test]
    fn window_follows_the_selection_and_is_clamped() {
        let d = tempfile::tempdir().expect("tempdir");
        for i in 0..30 {
            fs::write(d.path().join(format!("f{i:02}")), "").expect("write");
        }
        let mut e = Explorer::new(d.path().to_path_buf());
        e.move_selection(12, 10);
        assert_eq!(e.scroll(), 3);
        e.move_selection(-12, 10);
        assert_eq!(e.scroll(), 0);
        e.scroll_by(1000, 10);
        assert_eq!(e.scroll(), 20);
        e.scroll_by(-1000, 10);
        assert_eq!(e.scroll(), 0);
    }

    #[test]
    fn root_changes_and_parent_keeps_the_old_root_selected() {
        let d = sample();
        let mut e = Explorer::new(d.path().join("src"));
        assert_eq!(names(&e), ["inner", "main.rs"]);
        e.up();
        assert_eq!(e.root(), d.path());
        assert_eq!(e.selected_row().map(|r| r.name.as_str()), Some("src"));
        assert_eq!(e.selected_dir(), d.path().join("src"));
    }

    #[test]
    fn unreadable_roots_report_a_problem_not_a_panic() {
        let e = Explorer::new(PathBuf::from("/definitely/not/here"));
        assert!(e.rows().is_empty());
        assert!(e
            .error()
            .is_some_and(|m| m.contains("/definitely/not/here")));
    }

    #[test]
    fn module_contract_holds() {
        assert!(mahzenterm_core::module::validate(&ExplorerModule).is_ok());
    }
}
