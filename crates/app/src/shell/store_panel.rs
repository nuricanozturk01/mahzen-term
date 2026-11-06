//! The TUI store overlay: loading the catalog off the UI thread, keys while it is open, and launching.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iced::{Element, Task};
use mahzenterm_core::command::Resolution;
use mahzenterm_core::workspace::PaneKind;
use mahzenterm_tui_store::{self as tui_store, Launch, Outcome, Store, StoreKey};

use super::{commands, palette, Message, Shell};

/// A freshly built store handed from the loader task to the UI thread.
pub(super) type Loaded = Arc<Mutex<Option<Store>>>;

/// Read the catalog and probe the machine (file system and `PATH` lookups, so never on the UI thread).
fn build_store(user_dir: Option<PathBuf>) -> Store {
    let loaded = tui_store::load(user_dir.as_deref());
    let path = std::env::var_os("PATH").unwrap_or_default();
    Store::new(loaded.apps, loaded.problems, std::env::consts::OS, &path)
}

/// The command the install pane runs: the plan, then a pause so the result stays readable.
/// Every token comes from validated catalog fields (no spaces, quotes or shell syntax).
fn install_script(display: &str) -> (String, Vec<String>) {
    if std::env::consts::OS == "windows" {
        (
            "cmd".into(),
            vec!["/c".into(), format!("{display} & pause")],
        )
    } else {
        (
            "sh".into(),
            vec![
                "-c".into(),
                format!(
                    "{display}; code=$?; printf '\\n[finished with exit code %s] Press Enter to close. ' \"$code\"; read _"
                ),
            ],
        )
    }
}

fn map_key(k: palette::PaletteKey<'_>) -> StoreKey<'_> {
    use palette::PaletteKey as P;
    match k {
        P::Escape => StoreKey::Escape,
        P::Enter => StoreKey::Enter,
        P::Up => StoreKey::Up,
        P::Down => StoreKey::Down,
        P::PageUp => StoreKey::PageUp,
        P::PageDown => StoreKey::PageDown,
        P::Backspace => StoreKey::Backspace,
        P::ClearLine => StoreKey::ClearLine,
        P::Text(t) => StoreKey::Text(t),
        P::Other => StoreKey::Other,
    }
}

impl Shell {
    /// `store.open`: close the store if it is open, else load it in the background.
    pub(super) fn toggle_store(&mut self) -> Task<Message> {
        if self.store.take().is_some() {
            return Task::none();
        }
        if self.store_loading {
            return Task::none();
        }
        self.store_loading = true;
        let dir = mahzenterm_platform::config_dir().map(|d| d.join("store"));
        Task::perform(async move { build_store(dir) }, |s| {
            Message::StoreLoaded(Arc::new(Mutex::new(Some(s))))
        })
    }

    pub(super) fn on_store_loaded(&mut self, loaded: &Loaded) {
        self.store_loading = false;
        let store = loaded.lock().ok().and_then(|mut g| g.take());
        if let Some(store) = store {
            if let Some(first) = store.problems.first() {
                self.problem = Some(format!("store catalog: {first}"));
            }
            self.store = Some(store);
        }
    }

    pub(super) fn store_key(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
        text: Option<&str>,
    ) -> Task<Message> {
        // The store shortcut toggles it closed.
        if let Some(chord) = commands::chord_from(key, modifiers) {
            if self.keymap.resolve(&[chord], &|n| self.flag(n))
                == Resolution::Matched(tui_store::id::OPEN.into())
            {
                self.store = None;
                return Task::none();
            }
        }
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let outcome = store.on_key(map_key(palette::classify(key, modifiers, text)));
        self.apply_store_outcome(outcome)
    }

    pub(super) fn store_click(&mut self, row: usize) -> Task<Message> {
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let outcome = store.click(row);
        self.apply_store_outcome(outcome)
    }

    fn apply_store_outcome(&mut self, outcome: Outcome) -> Task<Message> {
        match outcome {
            Outcome::Idle | Outcome::Changed => {}
            Outcome::Close => self.store = None,
            Outcome::Run(launch) => {
                self.store = None;
                let (shell, args) = match launch {
                    Launch::Open { program } => (program, Vec::new()),
                    Launch::Install { plan, .. } => install_script(&plan.display()),
                };
                let cwd = self.focused_cwd();
                self.window.new_tab(PaneKind::Terminal {
                    cwd,
                    shell: Some(shell),
                    args,
                });
                self.sync();
                self.invalidate_active();
            }
        }
        Task::none()
    }

    /// The store overlay, or `None` while closed.
    pub(super) fn store_view(&self) -> Option<Element<'_, Message>> {
        let store = self.store.as_ref()?;
        Some(tui_store::view(
            store,
            self.tokens,
            Message::StoreRow,
            Message::StoreClose,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::install_script;

    #[test]
    fn the_install_pane_shows_the_result_before_closing() {
        let (prog, args) = install_script("sudo apt install -y btop");
        if std::env::consts::OS == "windows" {
            assert_eq!(prog, "cmd");
        } else {
            assert_eq!(prog, "sh");
            assert!(args[1].starts_with("sudo apt install -y btop; code=$?"));
            assert!(args[1].ends_with("read _"));
        }
    }
}
