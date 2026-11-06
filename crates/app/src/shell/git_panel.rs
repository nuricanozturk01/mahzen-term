//! The Git panel in the shell (Ctrl+Alt+G): opens on the focused pane's repository, runs git operations in the
//! background, and shows diffs and logs in terminal panes.

use std::path::{Path, PathBuf};

use iced::keyboard::{key::Named, Key as IcedKey, Modifiers};
use iced::{Element, Task};
use mahzenterm_core::workspace::PaneKind;
use mahzenterm_git as git;

use super::finder::project_root;
use super::{Message, Shell};

/// The result of a background git call that changes things.
type Done = Result<String, String>;

fn map_key(key: &IcedKey, modifiers: Modifiers, text: Option<&str>) -> Option<git::Key> {
    Some(match key {
        IcedKey::Named(Named::ArrowUp) => git::Key::Up,
        IcedKey::Named(Named::ArrowDown) => git::Key::Down,
        IcedKey::Named(Named::Enter) => git::Key::Enter,
        IcedKey::Named(Named::Escape) => git::Key::Escape,
        IcedKey::Named(Named::Backspace) => git::Key::Backspace,
        IcedKey::Named(Named::Space) => git::Key::Space,
        _ => {
            let t = text.filter(|t| {
                !modifiers.control() && !modifiers.alt() && !t.chars().any(char::is_control)
            })?;
            git::Key::Text(t.to_owned())
        }
    })
}

impl Shell {
    pub(super) fn open_git_panel(&mut self) -> Task<Message> {
        let dir = self
            .focused_cwd()
            .or_else(mahzenterm_platform::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let root = project_root(&dir);
        self.palette = None;
        self.git_panel = Some(git::Panel::new(root.clone()));
        self.git_branch.clear();
        self.git_reload(root)
    }

    /// Re-read the changed files and the branch name.
    fn git_reload(&self, root: PathBuf) -> Task<Message> {
        Task::perform(
            async move {
                let entries = git::entries(&root);
                let branch = git::fetch(&root)
                    .ok()
                    .flatten()
                    .map(|r| r.branch)
                    .unwrap_or_default();
                (root, entries, branch)
            },
            |(root, entries, branch)| Message::GitEntries(root, entries, branch),
        )
    }

    pub(super) fn on_git_entries(
        &mut self,
        root: &Path,
        entries: Result<Vec<git::Entry>, String>,
        branch: String,
    ) {
        let Some(p) = self.git_panel.as_mut().filter(|p| p.root == root) else {
            return;
        };
        p.set_entries(entries);
        self.git_branch = branch;
    }

    pub(super) fn on_git_branches(&mut self, r: Result<Vec<git::Branch>, String>) {
        if let Some(p) = self.git_panel.as_mut() {
            p.set_branches(r);
        }
    }

    pub(super) fn on_git_done(&mut self, result: Done) -> Task<Message> {
        let Some(p) = self.git_panel.as_mut() else {
            return Task::none();
        };
        p.finish(result);
        p.busy = true;
        let root = p.root.clone();
        self.git_reload(root)
    }

    pub(super) fn git_key(
        &mut self,
        key: &IcedKey,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> Task<Message> {
        let Some(k) = map_key(key, modifiers, text) else {
            return Task::none();
        };
        let Some(p) = self.git_panel.as_mut() else {
            return Task::none();
        };
        match p.key(k) {
            git::Outcome::None => Task::none(),
            git::Outcome::Close => {
                self.git_panel = None;
                Task::none()
            }
            git::Outcome::Do(a) => self.git_action(a),
        }
    }

    fn git_action(&mut self, action: git::Action) -> Task<Message> {
        let Some(root) = self.git_panel.as_ref().map(|p| p.root.clone()) else {
            return Task::none();
        };
        let done = |f: Box<dyn FnOnce() -> Done + Send>| {
            Task::perform(async move { f() }, Message::GitDone)
        };
        match action {
            git::Action::Stage(paths) => done(Box::new(move || {
                git::stage(&root, &paths).map(|()| "staged".into())
            })),
            git::Action::Unstage(paths) => done(Box::new(move || {
                git::unstage(&root, &paths).map(|()| "unstaged".into())
            })),
            git::Action::Discard(e) => done(Box::new(move || {
                git::discard(&root, &e).map(|()| "changes discarded".into())
            })),
            git::Action::Commit(msg) => done(Box::new(move || git::commit(&root, &msg))),
            git::Action::Switch(name) => done(Box::new(move || {
                git::switch_branch(&root, &name).map(|()| format!("switched to {name}"))
            })),
            git::Action::Push => done(Box::new(move || git::push(&root))),
            git::Action::Pull => done(Box::new(move || git::pull(&root))),
            git::Action::Refresh => done(Box::new(|| Ok("refreshed".into()))),
            git::Action::LoadBranches => {
                Task::perform(async move { git::branches(&root) }, Message::GitBranches)
            }
            git::Action::Diff(e) => {
                if e.is_untracked() {
                    self.git_panel = None;
                    self.open_editor_file(&root.join(&e.path), None);
                } else {
                    let mut args = vec!["diff".to_owned()];
                    if e.is_staged() && e.worktree == ' ' {
                        args.push("--cached".into());
                    }
                    args.extend(["--".to_owned(), e.path]);
                    self.git_pane(&root, args);
                }
                Task::none()
            }
            git::Action::Log => {
                let args = ["log", "--oneline", "--graph", "--decorate", "-n", "300"]
                    .map(String::from)
                    .to_vec();
                self.git_pane(&root, args);
                Task::none()
            }
        }
    }

    /// Run `git args...` in a pane beside (git pages long output itself).
    fn git_pane(&mut self, root: &Path, args: Vec<String>) {
        self.git_panel = None;
        self.split_with(PaneKind::Terminal {
            cwd: Some(root.to_path_buf()),
            shell: Some("git".into()),
            args,
        });
    }

    pub(super) fn git_view(&self) -> Option<Element<'_, Message>> {
        let p = self.git_panel.as_ref()?;
        Some(git::view(
            p,
            &self.git_branch,
            self.tokens,
            Message::GitClose,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_and_typing_never_includes_shortcuts() {
        assert_eq!(
            map_key(&IcedKey::Named(Named::Enter), Modifiers::empty(), None),
            Some(git::Key::Enter)
        );
        assert_eq!(
            map_key(
                &IcedKey::Character("a".into()),
                Modifiers::empty(),
                Some("a")
            ),
            Some(git::Key::Text("a".into()))
        );
        assert_eq!(
            map_key(&IcedKey::Character("a".into()), Modifiers::CTRL, Some("a")),
            None
        );
    }

    #[test]
    fn panel_opens_loads_and_diff_goes_to_a_pane() {
        let d = tempfile::tempdir().unwrap();
        let g = |a: &[&str]| {
            std::process::Command::new("git")
                .args(a)
                .current_dir(d.path())
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !g(&["init", "-q"]) {
            return;
        }
        std::fs::write(d.path().join("a.txt"), "x\n").unwrap();
        let mut s = super::super::tests::shell();
        s.git_panel = Some(git::Panel::new(d.path().to_path_buf()));
        s.on_git_entries(d.path(), git::entries(d.path()), "main".into());
        assert_eq!(s.git_panel.as_ref().unwrap().entries.len(), 1);
        // Stale answers for another folder are ignored.
        s.on_git_entries(Path::new("/elsewhere"), Ok(Vec::new()), "x".into());
        assert_eq!(s.git_panel.as_ref().unwrap().entries.len(), 1);
        // Diff of a tracked, modified file opens `git diff` in a pane and closes the panel.
        g(&["add", "a.txt"]);
        g(&[
            "-c",
            "user.email=t@e.c",
            "-c",
            "user.name=T",
            "commit",
            "-qm",
            "i",
        ]);
        std::fs::write(d.path().join("a.txt"), "y\n").unwrap();
        s.on_git_entries(d.path(), git::entries(d.path()), "main".into());
        let entry = s.git_panel.as_ref().unwrap().entries[0].clone();
        let before = s.window.active_tab().pane_ids().len();
        let _ = s.git_action(git::Action::Diff(entry));
        assert!(s.git_panel.is_none());
        assert_eq!(s.window.active_tab().pane_ids().len(), before + 1);
        let tab = s.window.active_tab();
        let PaneKind::Terminal { args, shell, .. } = &tab.pane(tab.focus).unwrap().kind else {
            panic!("a terminal pane")
        };
        assert_eq!(shell.as_deref(), Some("git"));
        assert_eq!(args[0], "diff");
    }
}
