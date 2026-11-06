//! The editor's key bindings and what each command does to a [`Buffer`].
//!
//! The default table is Visual Studio Code's, embedded from `keymap/vscode.toml`. It is versioned and guarded by a
//! snapshot test, so the controls never shift by accident. The table maps key presses to command ids; [`apply`] runs the
//! commands that only touch the text (movement, selection, deletion, line operations, undo) and tells the caller about
//! the ones that need a window, a clipboard or a file (find, copy, save ...).

use std::collections::BTreeMap;

use mahzenterm_core::command::Chord;
use serde::Deserialize;

use crate::{Buffer, Move};

const VSCODE: &str = include_str!("../keymap/vscode.toml");

#[derive(Debug, Deserialize)]
struct Raw {
    version: u32,
    bindings: Vec<RawBinding>,
}

#[derive(Debug, Deserialize)]
struct RawBinding {
    keys: String,
    command: String,
}

/// A key press to command table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub version: u32,
    bindings: BTreeMap<Chord, String>,
}

impl Keymap {
    /// Parse a table. A chord that is written twice, or cannot be parsed, is an error (never silently dropped).
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.message().to_owned())?;
        let mut bindings = BTreeMap::new();
        for b in raw.bindings {
            let chord = Chord::parse(&b.keys).map_err(|e| e.to_string())?;
            if let Some(prev) = bindings.insert(chord.clone(), b.command.clone()) {
                return Err(format!(
                    "{chord} is bound twice (to {prev} and {})",
                    b.command
                ));
            }
        }
        Ok(Self {
            version: raw.version,
            bindings,
        })
    }

    /// The built-in VS Code keymap.
    pub fn vscode() -> Self {
        Self::parse(VSCODE).expect("the embedded keymap is valid (a test checks it)")
    }

    /// The command a key press runs, if any.
    pub fn resolve(&self, chord: &Chord) -> Option<&str> {
        self.bindings.get(chord).map(String::as_str)
    }

    /// Layer user bindings on top: `command = "-editor.save"` style removal is not needed here, an override simply wins.
    pub fn with_overrides(mut self, extra: &[(String, String)]) -> Result<Self, String> {
        for (keys, command) in extra {
            let chord = Chord::parse(keys).map_err(|e| e.to_string())?;
            self.bindings.insert(chord, command.clone());
        }
        Ok(self)
    }

    /// Every binding as `Keys -> command`, sorted: the text the snapshot test pins.
    pub fn listing(&self) -> String {
        let mut out = format!("version {}\n", self.version);
        for (chord, cmd) in &self.bindings {
            out.push_str(&format!("{chord} -> {cmd}\n"));
        }
        out
    }

    /// Every command id that appears in the table.
    pub fn commands(&self) -> impl Iterator<Item = &str> {
        self.bindings.values().map(String::as_str)
    }
}

/// What running a command did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The text or selection changed (or stayed as it should).
    Done,
    /// The command needs the surrounding window (find, save, clipboard, language server ...).
    NeedsUi,
    /// Not an editor command.
    Unknown,
}

/// Run a command that only touches the buffer.
pub fn apply(buffer: &mut Buffer, command: &str) -> Applied {
    use Move::*;
    let mv = |b: &mut Buffer, m, extend| {
        b.move_cursor(m, extend);
        Applied::Done
    };
    match command {
        "editor.undo" => {
            buffer.undo();
            Applied::Done
        }
        "editor.redo" => {
            buffer.redo();
            Applied::Done
        }
        "editor.select_all" => {
            buffer.select_all();
            Applied::Done
        }
        "editor.newline" => {
            buffer.newline();
            Applied::Done
        }
        "editor.backspace" => {
            buffer.backspace();
            Applied::Done
        }
        "editor.delete" => {
            buffer.delete_forward();
            Applied::Done
        }
        "editor.delete_line" => {
            buffer.delete_lines();
            Applied::Done
        }
        "editor.move_line_up" => {
            buffer.move_lines(-1);
            Applied::Done
        }
        "editor.move_line_down" => {
            buffer.move_lines(1);
            Applied::Done
        }
        "editor.duplicate_line_down" | "editor.duplicate_line_up" => {
            buffer.duplicate_lines();
            Applied::Done
        }
        "editor.cursor_left" => mv(buffer, Left, false),
        "editor.cursor_right" => mv(buffer, Right, false),
        "editor.cursor_up" => mv(buffer, Up, false),
        "editor.cursor_down" => mv(buffer, Down, false),
        "editor.select_left" => mv(buffer, Left, true),
        "editor.select_right" => mv(buffer, Right, true),
        "editor.select_up" => mv(buffer, Up, true),
        "editor.select_down" => mv(buffer, Down, true),
        "editor.cursor_word_left" => mv(buffer, WordLeft, false),
        "editor.cursor_word_right" => mv(buffer, WordRight, false),
        "editor.select_word_left" => mv(buffer, WordLeft, true),
        "editor.select_word_right" => mv(buffer, WordRight, true),
        "editor.cursor_line_start" => mv(buffer, LineStart, false),
        "editor.cursor_line_end" => mv(buffer, LineEnd, false),
        "editor.select_line_start" => mv(buffer, LineStart, true),
        "editor.select_line_end" => mv(buffer, LineEnd, true),
        "editor.cursor_doc_start" => mv(buffer, DocStart, false),
        "editor.cursor_doc_end" => mv(buffer, DocEnd, false),
        "editor.select_doc_start" => mv(buffer, DocStart, true),
        "editor.select_doc_end" => mv(buffer, DocEnd, true),
        "editor.save"
        | "editor.save_as"
        | "editor.cut"
        | "editor.copy"
        | "editor.paste"
        | "editor.find"
        | "editor.replace"
        | "editor.add_next_occurrence"
        | "editor.goto_line"
        | "editor.quick_open"
        | "editor.toggle_comment"
        | "editor.goto_definition"
        | "editor.hover"
        | "editor.suggest" => Applied::NeedsUi,
        _ => Applied::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    /// THE CONTRACT. If this fails, a default editor binding changed. That is only allowed on purpose: bump `version` in
    /// `keymap/vscode.toml`, review the diff below, accept the new snapshot (`cargo insta review`) and add a changelog note.
    #[test]
    fn the_default_keymap_never_shifts() {
        insta::assert_snapshot!("vscode_keymap", Keymap::vscode().listing());
    }

    #[test]
    fn the_embedded_table_is_valid_and_complete() {
        let k = Keymap::vscode();
        assert_eq!(k.version, 1);
        // Every command in the table is one the editor knows (a typo would make a key do nothing).
        let mut probe = Buffer::from_text("x");
        for c in k.commands() {
            assert_ne!(
                apply(&mut probe, c),
                Applied::Unknown,
                "{c} is not an editor command"
            );
        }
        // The VS Code essentials are all there.
        for (keys, cmd) in [
            ("Ctrl+S", "editor.save"),
            ("Ctrl+Z", "editor.undo"),
            ("Ctrl+Shift+Z", "editor.redo"),
            ("Ctrl+Y", "editor.redo"),
            ("Ctrl+F", "editor.find"),
            ("Ctrl+D", "editor.add_next_occurrence"),
            ("Alt+Up", "editor.move_line_up"),
            ("Shift+Alt+Down", "editor.duplicate_line_down"),
            ("Ctrl+Shift+K", "editor.delete_line"),
            ("Ctrl+/", "editor.toggle_comment"),
            ("F12", "editor.goto_definition"),
            ("Home", "editor.cursor_line_start"),
        ] {
            assert_eq!(k.resolve(&chord(keys)), Some(cmd), "{keys}");
        }
        assert_eq!(k.resolve(&chord("Ctrl+Alt+Shift+Q")), None);
    }

    #[test]
    fn broken_tables_are_errors_not_silence() {
        assert!(Keymap::parse("version = 1\n[[bindings]]\nkeys = \"Ctrl+S\"\ncommand = \"a\"\n[[bindings]]\nkeys = \"ctrl+s\"\ncommand = \"b\"\n")
            .unwrap_err()
            .contains("bound twice"));
        assert!(
            Keymap::parse("version = 1\n[[bindings]]\nkeys = \"Hyper+S\"\ncommand = \"a\"\n")
                .is_err()
        );
        assert!(Keymap::parse("nonsense").is_err());
    }

    #[test]
    fn user_bindings_layer_on_top_without_touching_the_rest() {
        let base = Keymap::vscode();
        let mine = base
            .clone()
            .with_overrides(&[
                ("Ctrl+S".into(), "editor.save_as".into()),
                ("Ctrl+K".into(), "editor.delete_line".into()),
            ])
            .unwrap();
        assert_eq!(mine.resolve(&chord("Ctrl+S")), Some("editor.save_as"));
        assert_eq!(mine.resolve(&chord("Ctrl+K")), Some("editor.delete_line"));
        assert_eq!(
            mine.resolve(&chord("Ctrl+Z")),
            Some("editor.undo"),
            "everything else is as shipped"
        );
        assert_eq!(
            base.resolve(&chord("Ctrl+S")),
            Some("editor.save"),
            "the default itself is unchanged"
        );
        assert!(base
            .with_overrides(&[("Nope+S".into(), "x".into())])
            .is_err());
    }

    #[test]
    fn commands_drive_the_buffer() {
        let mut b = Buffer::from_text("one two\nthree\nfour");
        let km = Keymap::vscode();
        let run = |b: &mut Buffer, keys: &str| apply(b, km.resolve(&chord(keys)).unwrap());
        assert_eq!(run(&mut b, "Ctrl+End"), Applied::Done);
        assert_eq!(b.cursor(), b.len_chars());
        run(&mut b, "Ctrl+Home");
        run(&mut b, "Ctrl+Right");
        assert_eq!(b.cursor(), 3, "a word to the right");
        run(&mut b, "Shift+End");
        assert_eq!(b.selected_text(), " two");
        run(&mut b, "Backspace");
        assert_eq!(b.line(0), "one");
        run(&mut b, "Alt+Down");
        assert_eq!(b.line(0), "three");
        run(&mut b, "Ctrl+Z");
        assert_eq!(b.line(0), "one");
        run(&mut b, "Ctrl+A");
        assert_eq!(b.selected_text().chars().count(), b.len_chars());
        assert_eq!(
            apply(&mut b, "editor.find"),
            Applied::NeedsUi,
            "find needs the window"
        );
        assert_eq!(apply(&mut b, "editor.fly"), Applied::Unknown);
    }
}
