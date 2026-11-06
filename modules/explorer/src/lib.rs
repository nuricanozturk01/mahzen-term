//! File explorer: a lazily read directory tree with keyboard and mouse navigation.
//!
//! [`Explorer`] is pure state (testable without a window); [`view`] draws it with theme tokens.

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
        };
        e.rebuild();
        e
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
            capabilities: vec![Capability::FsRead],
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
