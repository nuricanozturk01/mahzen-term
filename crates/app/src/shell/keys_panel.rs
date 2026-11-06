//! The shortcut editor (`Ctrl+K Ctrl+S`, palette: "Keyboard Shortcuts"): pick a command, press the new keys.
//!
//! Choices are written to the `[[keys]]` part of `config.toml` (see `keyfile`), so they are the same entries a user
//! would type by hand, and the config watcher applies them live. A chord that another command already has asks for a
//! second press before it is taken over; chords that would swallow plain typing are refused.

use std::path::PathBuf;

use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{Background, Border, Element, Length, Padding, Task};
use mahzenterm_core::command::Resolution;
use mahzenterm_ui_kit::Tokens;

use super::{commands, Message, Shell};
use crate::keyfile::{self, Change};

/// Rows shown at once.
const VISIBLE: usize = 12;

/// What the panel waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Typing filters, arrows choose, Enter starts capturing, Delete restores the default.
    Browse,
    /// The next key press is the new shortcut. `again` is a chord that needs a second press to take over.
    Capture { again: Option<(String, String)> },
}

#[derive(Debug)]
pub struct KeysPanel {
    pub query: String,
    pub selected: usize,
    pub mode: Mode,
    pub notice: Option<(String, bool)>,
}

impl KeysPanel {
    fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            mode: Mode::Browse,
            notice: None,
        }
    }
}

/// The config file the editor writes.
fn config_path() -> Option<PathBuf> {
    mahzenterm_platform::config_dir().map(|d| d.join("config.toml"))
}

/// Read, change and atomically write the config file (blocking: call from a background task).
fn write_change(
    path: &std::path::Path,
    command: &str,
    defaults: &[String],
    change: &Change,
) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let new = keyfile::apply(&text, command, defaults, change);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, new).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

impl Shell {
    pub(super) fn open_keys_panel(&mut self) {
        self.palette = None;
        self.keys_panel = Some(KeysPanel::new());
    }

    /// The commands the list shows for the current filter.
    fn keys_rows(&self) -> Vec<String> {
        let query = self.keys_panel.as_ref().map_or("", |p| p.query.as_str());
        self.registry
            .search(query, &|_| true, &[])
            .into_iter()
            .map(|c| c.id.clone())
            .collect()
    }

    pub(super) fn keys_key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> Task<Message> {
        let rows = self.keys_rows();
        let Some(panel) = self.keys_panel.as_mut() else {
            return Task::none();
        };
        let n = rows.len();
        if let Mode::Capture { again } = panel.mode.clone() {
            return self.keys_capture(key, modifiers, &rows, again);
        }
        match key {
            Key::Named(Named::Escape) => self.keys_panel = None,
            Key::Named(Named::ArrowUp) if n > 0 => panel.selected = (panel.selected + n - 1) % n,
            Key::Named(Named::ArrowDown) if n > 0 => panel.selected = (panel.selected + 1) % n,
            Key::Named(Named::Enter) if n > 0 => {
                panel.mode = Mode::Capture { again: None };
                panel.notice = Some((
                    "press the new shortcut  (Esc cancels, Backspace removes it)".into(),
                    false,
                ));
            }
            Key::Named(Named::Delete) if n > 0 => {
                let id = rows[panel.selected.min(n - 1)].clone();
                return self.keys_apply(&id, Change::Reset);
            }
            Key::Named(Named::Backspace) => {
                panel.query.pop();
                panel.selected = 0;
            }
            _ => {
                if let Some(t) = text.filter(|t| {
                    !modifiers.control() && !modifiers.alt() && !t.chars().any(char::is_control)
                }) {
                    panel.query.push_str(t);
                    panel.selected = 0;
                }
            }
        }
        Task::none()
    }

    fn keys_capture(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        rows: &[String],
        again: Option<(String, String)>,
    ) -> Task<Message> {
        let Some(panel) = self.keys_panel.as_mut() else {
            return Task::none();
        };
        let Some(id) = rows.get(panel.selected).cloned() else {
            panel.mode = Mode::Browse;
            return Task::none();
        };
        match key {
            Key::Named(Named::Escape) => {
                panel.mode = Mode::Browse;
                panel.notice = Some(("cancelled".into(), false));
                return Task::none();
            }
            Key::Named(Named::Backspace) if !modifiers.control() && !modifiers.alt() => {
                panel.mode = Mode::Browse;
                return self.keys_apply(&id, Change::Unbind);
            }
            _ => {}
        }
        let Some(chord) = commands::chord_from(key, modifiers) else {
            return Task::none(); // a modifier on its own: keep waiting
        };
        if !keyfile::bindable(&chord) {
            panel.notice = Some((
                format!("{chord} would take over plain typing: add Ctrl, Alt or Super"),
                true,
            ));
            return Task::none();
        }
        let text = chord.to_string();
        let owner = match self.keymap.resolve(std::slice::from_ref(&chord), &|_| true) {
            Resolution::Matched(other) if other != id => Some(other),
            Resolution::Pending => {
                let Some(panel) = self.keys_panel.as_mut() else {
                    return Task::none();
                };
                panel.notice = Some((
                    format!("{text} starts a longer key sequence: choose other keys"),
                    true,
                ));
                return Task::none();
            }
            _ => None,
        };
        let Some(panel) = self.keys_panel.as_mut() else {
            return Task::none();
        };
        if let Some(other) = owner {
            let confirmed = again
                .as_ref()
                .is_some_and(|(c, o)| *c == text && *o == other);
            if !confirmed {
                let title = self
                    .registry
                    .get(&other)
                    .map_or(other.clone(), |s| s.title.clone());
                panel.mode = Mode::Capture {
                    again: Some((text.clone(), other)),
                };
                panel.notice = Some((format!("{text} is used by \"{title}\": press it again to take it over, or other keys"), true));
                return Task::none();
            }
            panel.mode = Mode::Browse;
            return self.keys_apply(
                &id,
                Change::Set {
                    chord: text,
                    takeover: vec![other],
                },
            );
        }
        panel.mode = Mode::Browse;
        self.keys_apply(
            &id,
            Change::Set {
                chord: text,
                takeover: Vec::new(),
            },
        )
    }

    /// Write the change in the background; the config watcher reloads the keymap.
    fn keys_apply(&mut self, command: &str, change: Change) -> Task<Message> {
        let defaults: Vec<String> = self
            .registry
            .get(command)
            .map(|s| s.default_keys.clone())
            .unwrap_or_default();
        let Some(path) = config_path() else {
            if let Some(p) = self.keys_panel.as_mut() {
                p.notice = Some(("no config folder is known on this system".into(), true));
            }
            return Task::none();
        };
        let command = command.to_owned();
        Task::perform(
            async move { write_change(&path, &command, &defaults, &change) },
            Message::KeysSaved,
        )
    }

    pub(super) fn on_keys_saved(&mut self, r: Result<(), String>) {
        if let Some(p) = self.keys_panel.as_mut() {
            p.notice = Some(match r {
                Ok(()) => ("saved to config.toml; active now".into(), false),
                Err(e) => (e, true),
            });
        }
    }

    pub(super) fn keys_view(&self) -> Option<Element<'_, Message>> {
        let p = self.keys_panel.as_ref()?;
        let t: Tokens = self.tokens;
        let rows = self.keys_rows();
        let capturing = matches!(p.mode, Mode::Capture { .. });
        let query = if p.query.is_empty() {
            row![
                text("\u{203a}").size(16).color(t.accent),
                text("Filter commands...").size(15).color(t.text_dim)
            ]
        } else {
            row![
                text("\u{203a}").size(16).color(t.accent),
                text(format!("{}\u{258f}", p.query)).size(15).color(t.text)
            ]
        }
        .spacing(10)
        .align_y(iced::Alignment::Center);
        let mut list = column![].spacing(1);
        let first = p.selected.saturating_sub(VISIBLE - 1);
        for (i, id) in rows.iter().enumerate().skip(first).take(VISIBLE) {
            let Some(spec) = self.registry.get(id) else {
                continue;
            };
            let selected = i == p.selected;
            let keys = if selected && capturing {
                "press keys...".to_owned()
            } else {
                self.keymap.shortcut_for(id).unwrap_or_default()
            };
            let line = row![
                text(spec.title.clone())
                    .size(13)
                    .color(if selected { t.text } else { t.text_dim }),
                text(spec.category.clone()).size(11).color(t.text_dim),
                Space::new().width(Length::Fill),
                text(keys).size(12).color(if selected && capturing {
                    t.warn
                } else {
                    t.accent
                }),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center);
            list = list.push(
                mouse_area(
                    container(line)
                        .width(Length::Fill)
                        .padding(Padding::from([5.0, 12.0]))
                        .style(move |_| container::Style {
                            background: selected.then_some(Background::Color(t.surface_hi)),
                            border: Border::default().rounded(6.0),
                            ..container::Style::default()
                        }),
                )
                .on_press(Message::KeysRow(i)),
            );
        }
        if rows.is_empty() {
            list = list.push(
                container(text("no matching command").size(13).color(t.text_dim))
                    .padding(Padding::from([10.0, 14.0])),
            );
        }
        let notice: Element<'_, Message> = match &p.notice {
            Some((m, err)) => {
                container(
                    text(m.clone())
                        .size(12)
                        .color(if *err { t.warn } else { t.ok }),
                )
                .padding(Padding::from([4.0, 8.0]))
                .into()
            }
            None => Space::new().height(Length::Fixed(0.0)).into(),
        };
        let hint = if capturing {
            "press the new keys   esc cancel   backspace remove"
        } else {
            "type filter   \u{2191}\u{2193} choose   enter change   delete restore default   esc close"
        };
        let card = container(
            column![
                container(crate::chrome::tag("KEYBOARD SHORTCUTS", t, true))
                    .padding(Padding::from([2.0, 8.0])),
                container(query).padding(Padding::from([4.0, 8.0])),
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fixed(1.0))
                    .style(move |_| container::background(t.border)),
                list,
                notice,
                container(text(hint).size(10).color(t.text_dim)).padding(Padding::from([4.0, 8.0])),
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
        .on_press(Message::KeysClose);
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

    pub(super) fn keys_pick(&mut self, i: usize) {
        if let Some(p) = self.keys_panel.as_mut() {
            p.selected = i;
            p.mode = Mode::Capture { again: None };
            p.notice = Some((
                "press the new shortcut  (Esc cancels, Backspace removes it)".into(),
                false,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(s: &mut Shell, keys: &str) -> Task<Message> {
        let chord = Chord::parse(keys).unwrap();
        let mods = (if chord.ctrl {
            Modifiers::CTRL
        } else {
            Modifiers::empty()
        }) | (if chord.alt {
            Modifiers::ALT
        } else {
            Modifiers::empty()
        }) | (if chord.shift {
            Modifiers::SHIFT
        } else {
            Modifiers::empty()
        });
        let key = Key::Character(chord.key.as_str().into());
        s.keys_key(&key, mods, None)
    }

    use mahzenterm_core::command::Chord;

    fn named(s: &mut Shell, n: Named) {
        let _ = s.keys_key(&Key::Named(n), Modifiers::empty(), None);
    }

    fn type_text(s: &mut Shell, t: &str) {
        for c in t.chars() {
            let tx = c.to_string();
            let _ = s.keys_key(
                &Key::Character(tx.as_str().into()),
                Modifiers::empty(),
                Some(&tx),
            );
        }
    }

    #[test]
    fn the_editor_filters_captures_and_asks_before_taking_a_chord() {
        let mut s = super::super::tests::shell();
        let _ = s.run_command(commands::id::KEYS_EDIT);
        assert!(s.keys_panel.is_some());
        type_text(&mut s, "toggle pane zoom");
        assert_eq!(
            s.keys_rows().first().map(String::as_str),
            Some(commands::id::PANE_ZOOM)
        );
        // Enter starts capturing; a plain letter would take over typing and is refused.
        named(&mut s, Named::Enter);
        assert!(matches!(
            s.keys_panel.as_ref().unwrap().mode,
            Mode::Capture { .. }
        ));
        let _ = s.keys_key(&Key::Character("a".into()), Modifiers::empty(), Some("a"));
        assert!(s
            .keys_panel
            .as_ref()
            .unwrap()
            .notice
            .as_ref()
            .unwrap()
            .0
            .contains("plain typing"));
        // Ctrl+Shift+T belongs to "new tab": the first press only warns, the second takes it over.
        let _ = press(&mut s, "Ctrl+Shift+T");
        let p = s.keys_panel.as_ref().unwrap();
        assert!(
            matches!(&p.mode, Mode::Capture { again: Some((c, o)) } if c == "Ctrl+Shift+T" && o == commands::id::TAB_NEW)
        );
        assert!(p.notice.as_ref().unwrap().0.contains("press it again"));
        let _ = press(&mut s, "Ctrl+Shift+T");
        assert_eq!(
            s.keys_panel.as_ref().unwrap().mode,
            Mode::Browse,
            "the change went to the writer"
        );
        // A free chord is accepted at once; Escape cancels a capture without a change.
        named(&mut s, Named::Enter);
        let _ = press(&mut s, "Ctrl+Alt+Q");
        assert_eq!(s.keys_panel.as_ref().unwrap().mode, Mode::Browse);
        named(&mut s, Named::Enter);
        named(&mut s, Named::Escape);
        assert_eq!(s.keys_panel.as_ref().unwrap().mode, Mode::Browse);
        named(&mut s, Named::Escape);
        assert!(
            s.keys_panel.is_none(),
            "Escape in the list closes the editor"
        );
    }

    #[test]
    fn writing_changes_goes_through_a_temporary_file_and_keeps_user_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        write_change(
            &path,
            "tab.new",
            &["Ctrl+Shift+T".into()],
            &Change::Set {
                chord: "Ctrl+Alt+T".into(),
                takeover: vec![],
            },
        )
        .unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("Ctrl+Alt+T"));
        std::fs::write(&path, format!("# mine\n{first}")).unwrap();
        write_change(&path, "tab.new", &["Ctrl+Shift+T".into()], &Change::Reset).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "# mine");
        assert!(!path.with_extension("toml.tmp").exists());
    }
}
