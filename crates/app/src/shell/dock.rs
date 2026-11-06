//! The file explorer dock on the left edge: opening it, keys while it has focus, previews and editor hand-off.

use std::path::{Path, PathBuf};

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use iced::widget::container;
use iced::{Element, Length};
use mahzenterm_core::workspace::{Axis, PaneKind};
use mahzenterm_editor_bridge::{self as editor_bridge, Location};
use mahzenterm_explorer::{
    view as explorer_view, Action, Event, Explorer, PromptOutcome, PromptWhat,
};

use super::{Message, Shell};

/// Width of the dock in pixels, divider included.
pub(super) const DOCK_W: f32 = mahzenterm_explorer::PANEL_W;

/// Where the `tf` helper lives: next to this executable when installed together, else on the PATH.
fn tf_program() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|d| d.join(if cfg!(windows) { "tf.exe" } else { "tf" }))
        })
        .filter(|p| p.is_file())
        .map_or_else(|| "tf".to_owned(), |p| p.display().to_string())
}

/// Whether `tf img` can show this file, going by its extension.
fn is_image(path: &Path) -> bool {
    const EXT: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "tga", "pnm", "pbm",
        "pgm", "ppm", "qoi",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXT.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

impl Shell {
    /// Rows that fit in the dock below its header.
    pub(super) fn dock_capacity(&self) -> usize {
        let h = self.content_area().height + self.status_h() - mahzenterm_explorer::HEADER_H;
        (h / mahzenterm_explorer::ROW_H).floor().max(1.0) as usize
    }

    /// Width taken by the dock (0 when closed).
    pub(super) fn dock_w(&self) -> f32 {
        if self.explorer.is_some() {
            DOCK_W
        } else {
            0.0
        }
    }

    pub(super) fn toggle_explorer(&mut self) {
        if self.explorer.take().is_some() {
            self.explorer_focus = false;
        } else {
            let root = self
                .focused_cwd()
                .or_else(mahzenterm_platform::home_dir)
                .unwrap_or_else(|| PathBuf::from("/"));
            self.explorer = Some(Explorer::new(root));
            self.explorer_focus = true;
        }
        self.sync();
        self.invalidate_active();
    }

    pub(super) fn focus_explorer(&mut self) {
        if self.explorer.is_none() {
            self.toggle_explorer();
        } else {
            self.explorer_focus = !self.explorer_focus;
        }
    }

    /// Show a file in a new pane to the right: images are drawn by `tf img`, everything else rendered by `tf view`.
    fn preview(&mut self, path: &Path) {
        let cwd = path.parent().map(Path::to_path_buf);
        let args = if is_image(path) {
            vec!["img".into(), "--wait".into(), path.display().to_string()]
        } else {
            vec!["view".into(), path.display().to_string()]
        };
        self.split_with(PaneKind::Terminal {
            cwd,
            shell: Some(tf_program()),
            args,
        });
    }

    /// Open `loc` in the preferred editor in a split pane. Reports a problem when no editor is found.
    pub(super) fn open_in_editor(&mut self, loc: &Location) {
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let Some(cmd) = editor_bridge::choose(
            self.editor_pref.as_deref(),
            env("VISUAL").as_deref(),
            env("EDITOR").as_deref(),
            &path_var,
        ) else {
            self.problem = Some(
                "no editor found: set $EDITOR or [modules.editor] command, or install one (hx, nvim, micro)"
                    .into(),
            );
            return;
        };
        let args = cmd.argv(loc);
        self.split_with(PaneKind::Terminal {
            cwd: loc.path.parent().map(Path::to_path_buf),
            shell: Some(cmd.program),
            args,
        });
    }

    /// `editor.open_here`: the focused pane's directory in the editor.
    pub(super) fn open_editor_here(&mut self) {
        let dir = self
            .focused_cwd()
            .or_else(mahzenterm_platform::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let loc = Location::file(dir.join("."));
        self.open_in_editor(&loc);
    }

    /// Ctrl+click on a word: open it when it names an existing file (`path`, `path:line`, `path:line:col`).
    pub(super) fn open_token(&mut self, token: &str) -> bool {
        let cwd = self.focused_cwd().unwrap_or_else(|| PathBuf::from("."));
        match editor_bridge::parse_location(token, &cwd) {
            Some(loc) => {
                self.open_in_editor(&loc);
                true
            }
            None => false,
        }
    }

    pub(super) fn split_with(&mut self, kind: PaneKind) {
        let tab = self.window.active_tab_mut();
        let target = tab.focus;
        if tab.split(target, Axis::Horizontal, kind).is_ok() {
            self.explorer_focus = false;
            self.sync();
            self.invalidate_active();
        }
    }

    pub(super) fn on_explorer(&mut self, event: Event) {
        let cap = self.dock_capacity();
        let Some(ex) = self.explorer.as_mut() else {
            return;
        };
        self.explorer_focus = true;
        match event {
            Event::Click(i) => {
                ex.select(i);
                if ex.rows().get(i).is_some_and(|r| r.is_dir) {
                    ex.activate(i);
                }
            }
            Event::Open(i) => {
                if let Action::Open(p) = ex.activate(i) {
                    self.preview(&p);
                }
            }
            Event::Scroll(d) => ex.scroll_by(d, cap),
            Event::Up => ex.up(),
            Event::ToggleHidden => ex.toggle_hidden(),
            Event::Close => self.toggle_explorer(),
        }
    }

    /// Keys while the dock has focus. Returns true when the key was used (it never reaches the terminal).
    pub(super) fn explorer_key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> bool {
        let cap = self.dock_capacity();
        let Some(ex) = self.explorer.as_mut() else {
            return false;
        };
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return false; // shortcuts keep working
        }
        let sel = ex.selected_row().cloned();
        if ex.prompting() {
            match key {
                Key::Named(Named::Escape) => ex.prompt_cancel(),
                Key::Named(Named::Backspace) => ex.prompt_backspace(),
                Key::Named(Named::Enter) => {
                    if let PromptOutcome::Trash(path) = ex.prompt_submit() {
                        let when = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
                        match mahzenterm_platform::trash::move_to_trash(&path, &when) {
                            Ok(_) => {
                                ex.refresh();
                                self.git_root = None;
                            }
                            Err(e) => self.problem = Some(format!("cannot trash: {e}")),
                        }
                    } else {
                        self.git_root = None; // a created or renamed file changes git status
                    }
                }
                _ => match text {
                    Some(t) => ex.prompt_push(t),
                    None => return false,
                },
            }
            return true;
        }
        if matches!(key, Key::Named(Named::F2)) {
            ex.prompt_start(PromptWhat::Rename);
            return true;
        }
        if matches!(key, Key::Named(Named::Delete)) {
            ex.prompt_start(PromptWhat::Trash);
            return true;
        }
        if ex.searching() {
            match key {
                Key::Named(Named::Escape) => ex.search_exit(),
                Key::Named(Named::ArrowUp) => ex.move_selection(-1, cap),
                Key::Named(Named::ArrowDown) => ex.move_selection(1, cap),
                Key::Named(Named::PageUp) => ex.move_selection(-(cap as isize), cap),
                Key::Named(Named::PageDown) => ex.move_selection(cap as isize, cap),
                Key::Named(Named::Backspace) => {
                    if ex.search_query().is_some_and(str::is_empty) {
                        ex.search_exit();
                    } else {
                        ex.search_backspace();
                    }
                }
                Key::Named(Named::Enter) => {
                    let i = ex.selected();
                    if let Action::Open(p) = ex.activate(i) {
                        ex.search_exit();
                        self.preview(&p);
                    }
                }
                _ => match text {
                    Some(t) => ex.search_push(t),
                    None => return false,
                },
            }
            return true;
        }
        match key {
            Key::Named(Named::ArrowUp) => ex.move_selection(-1, cap),
            Key::Named(Named::ArrowDown) => ex.move_selection(1, cap),
            Key::Named(Named::PageUp) => ex.move_selection(-(cap as isize), cap),
            Key::Named(Named::PageDown) => ex.move_selection(cap as isize, cap),
            Key::Named(Named::Home) => ex.move_selection(isize::MIN / 2, cap),
            Key::Named(Named::End) => ex.move_selection(isize::MAX / 2, cap),
            Key::Named(Named::ArrowLeft) => ex.left(cap),
            Key::Named(Named::ArrowRight) => ex.right(cap),
            Key::Named(Named::Backspace) => ex.up(),
            Key::Named(Named::Escape) => self.explorer_focus = false,
            Key::Named(Named::Enter | Named::Space) => {
                let i = ex.selected();
                if let Action::Open(p) = ex.activate(i) {
                    self.preview(&p);
                }
            }
            _ => match text {
                Some("/") => ex.search_start(),
                Some("n") => ex.prompt_start(PromptWhat::NewFile),
                Some("N") => ex.prompt_start(PromptWhat::NewDir),
                Some(".") => ex.toggle_hidden(),
                Some("r") => {
                    ex.refresh();
                    self.git_root = None; // marks are read again after the next update
                }
                Some("t") => {
                    let dir = ex.selected_dir();
                    self.window.new_tab(PaneKind::Terminal {
                        cwd: Some(dir),
                        shell: None,
                        args: Vec::new(),
                    });
                    self.explorer_focus = false;
                    self.sync();
                }
                Some("s") => {
                    let dir = ex.selected_dir();
                    self.split_with(PaneKind::Terminal {
                        cwd: Some(dir),
                        shell: None,
                        args: Vec::new(),
                    });
                }
                Some("e") => {
                    if let Some(r) = sel.filter(|r| !r.is_dir) {
                        self.open_in_editor(&Location::file(r.path));
                    }
                }
                Some("q") => self.toggle_explorer(),
                Some(_) => {} // typed text is swallowed, not sent to the shell
                None => return false,
            },
        }
        true
    }

    /// The dock as a view element, or `None` while closed.
    pub(super) fn dock_view(&self) -> Option<Element<'_, Message>> {
        let ex = self.explorer.as_ref()?;
        let body = explorer_view(
            ex,
            self.dock_capacity(),
            self.explorer_focus,
            self.tokens,
            Message::Explorer,
        );
        let t = self.tokens;
        let edge = container(iced::widget::Space::new())
            .width(Length::Fixed(1.0))
            .height(Length::Fill)
            .style(move |_| container::background(t.border));
        Some(
            iced::widget::row![container(body).width(Length::Fixed(DOCK_W - 1.0)), edge]
                .height(Length::Fill)
                .into(),
        )
    }
}
