//! Editing the `[[keys]]` entries of `config.toml` as text, so the user's comments and everything else stay as they
//! are. The shortcut editor writes its entries as small marked blocks and only ever removes blocks it wrote itself.

/// Marks a block written by the shortcut editor.
pub const MARK: &str = "# mahzenterm: set in the shortcut editor";

/// What the user did with a command's shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Use this chord (`Ctrl+Alt+X`); `takeover` lists commands that had it and lose it.
    Set {
        chord: String,
        takeover: Vec<String>,
    },
    /// No shortcut at all.
    Unbind,
    /// Back to the built-in default.
    Reset,
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn block(keys: &str, command: &str) -> String {
    format!(
        "{MARK}\n[[keys]]\nkeys = {}\ncommand = {}\n\n",
        quote(keys),
        quote(command)
    )
}

/// The command named in a managed block that starts at `lines[i]` (the marker line), and the block's length in lines.
fn managed_block(lines: &[&str], i: usize) -> Option<(String, usize)> {
    if lines.get(i)?.trim() != MARK || lines.get(i + 1)?.trim() != "[[keys]]" {
        return None;
    }
    let cmd_line = lines.get(i + 3)?.trim();
    let value = cmd_line
        .strip_prefix("command")?
        .trim()
        .strip_prefix('=')?
        .trim();
    let cmd = value
        .strip_prefix('"')?
        .strip_suffix('"')?
        .replace("\\\"", "\"")
        .replace("\\\\", "\\");
    // A blank line after the block belongs to it.
    let len = if lines.get(i + 4).is_some_and(|l| l.trim().is_empty()) {
        5
    } else {
        4
    };
    Some((cmd, len))
}

/// Remove the editor's blocks for `command` (and its `-command` removals) from `text`.
pub fn remove_blocks(text: &str, command: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let minus = format!("-{command}");
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        if let Some((cmd, len)) = managed_block(&lines, i) {
            if cmd == command || cmd == minus {
                i += len;
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// Apply `change` for `command` (whose built-in shortcuts are `defaults`) to the config text.
pub fn apply(text: &str, command: &str, defaults: &[String], change: &Change) -> String {
    let mut out = remove_blocks(text, command);
    if let Change::Set { takeover, .. } = change {
        for other in takeover {
            out = remove_blocks_keep_others(&out, other);
        }
    }
    let mut add = String::new();
    match change {
        Change::Reset => {}
        Change::Unbind => {
            for d in defaults {
                add.push_str(&block(d, &format!("-{command}")));
            }
        }
        Change::Set { chord, takeover } => {
            // The built-in shortcut stops working, the new one starts; commands that lose the chord are told so.
            for d in defaults.iter().filter(|d| !d.eq_ignore_ascii_case(chord)) {
                add.push_str(&block(d, &format!("-{command}")));
            }
            for other in takeover {
                add.push_str(&block(chord, &format!("-{other}")));
            }
            if !defaults.iter().any(|d| d.eq_ignore_ascii_case(chord)) {
                add.push_str(&block(chord, command));
            }
        }
    }
    if add.is_empty() {
        return out;
    }
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(&add);
    out
}

/// Takeover entries for another command are written under that command's name; an earlier identical removal is
/// replaced instead of duplicated.
fn remove_blocks_keep_others(text: &str, other: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let minus = format!("-{other}");
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some((cmd, len)) = managed_block(&lines, i) {
            if cmd == minus {
                i += len;
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// A chord that is safe to bind: it needs Ctrl, Alt or Super, or be a function key, so that typing is never taken.
pub fn bindable(chord: &mahzenterm_core::command::Chord) -> bool {
    let function_key = chord
        .key
        .strip_prefix('f')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    chord.ctrl || chord.alt || chord.logo || function_key
}

#[cfg(test)]
mod tests {
    use super::*;
    use mahzenterm_core::command::{Keymap, Registry, UserBinding};

    const USER: &str = "# my settings\n[terminal]\nshell = \"/bin/zsh\"\n\n[[keys]]\nkeys = \"Ctrl+Shift+N\"\ncommand = \"tab.new\"\n";

    fn defaults() -> Vec<String> {
        vec!["Ctrl+Shift+T".into()]
    }

    #[test]
    fn setting_a_shortcut_removes_the_default_and_adds_the_new_one_and_keeps_the_rest() {
        let out = apply(
            USER,
            "tab.new",
            &defaults(),
            &Change::Set {
                chord: "Ctrl+Alt+T".into(),
                takeover: vec![],
            },
        );
        assert!(
            out.starts_with("# my settings\n[terminal]"),
            "the user's text is untouched"
        );
        assert!(
            out.contains("keys = \"Ctrl+Shift+N\"\ncommand = \"tab.new\""),
            "their own binding stays"
        );
        assert!(out.contains("keys = \"Ctrl+Shift+T\"\ncommand = \"-tab.new\""));
        assert!(out.contains("keys = \"Ctrl+Alt+T\"\ncommand = \"tab.new\""));
        assert_eq!(out.matches(MARK).count(), 2);
    }

    #[test]
    fn changing_again_replaces_the_editors_blocks_and_reset_removes_them() {
        let once = apply(
            USER,
            "tab.new",
            &defaults(),
            &Change::Set {
                chord: "Ctrl+Alt+T".into(),
                takeover: vec![],
            },
        );
        let twice = apply(
            &once,
            "tab.new",
            &defaults(),
            &Change::Set {
                chord: "Ctrl+Alt+Y".into(),
                takeover: vec![],
            },
        );
        assert!(!twice.contains("Ctrl+Alt+T"));
        assert!(twice.contains("Ctrl+Alt+Y"));
        assert_eq!(twice.matches(MARK).count(), 2);
        let reset = apply(&twice, "tab.new", &defaults(), &Change::Reset);
        assert_eq!(reset.trim_end(), USER.trim_end());
    }

    #[test]
    fn unbinding_removes_every_default_and_takeover_removes_the_chord_from_the_other_command() {
        let out = apply(
            USER,
            "tab.new",
            &["Ctrl+Shift+T".into(), "Ctrl+T".into()],
            &Change::Unbind,
        );
        assert!(out.contains("keys = \"Ctrl+T\"\ncommand = \"-tab.new\""));
        assert_eq!(
            out.matches("command = \"tab.new\"").count(),
            1,
            "only the user's own binding, no new one"
        );
        let out = apply(
            USER,
            "pane.zoom",
            &["Ctrl+Shift+Enter".into()],
            &Change::Set {
                chord: "Ctrl+Shift+T".into(),
                takeover: vec!["tab.new".into()],
            },
        );
        assert!(out.contains("keys = \"Ctrl+Shift+T\"\ncommand = \"-tab.new\""));
        assert!(out.contains("keys = \"Ctrl+Shift+T\"\ncommand = \"pane.zoom\""));
        // Giving the first command its own shortcut back later drops the editor's removal of it.
        let back = apply(&out, "tab.new", &defaults(), &Change::Reset);
        assert!(!back.contains("-tab.new"));
        assert!(back.contains("command = \"pane.zoom\""));
    }

    #[test]
    fn choosing_the_default_again_adds_nothing() {
        let out = apply(
            USER,
            "tab.new",
            &defaults(),
            &Change::Set {
                chord: "ctrl+shift+t".into(),
                takeover: vec![],
            },
        );
        assert_eq!(out.trim_end(), USER.trim_end());
    }

    #[test]
    fn what_the_editor_writes_is_what_the_keymap_understands() {
        let mut reg = Registry::new();
        reg.register(
            mahzenterm_core::command::CommandSpec::new("a.one", "One", "T").key("Ctrl+Shift+T"),
        )
        .unwrap();
        reg.register(
            mahzenterm_core::command::CommandSpec::new("a.two", "Two", "T").key("Ctrl+Alt+Y"),
        )
        .unwrap();
        let text = apply(
            "",
            "a.one",
            &["Ctrl+Shift+T".into()],
            &Change::Set {
                chord: "Ctrl+Alt+Y".into(),
                takeover: vec!["a.two".into()],
            },
        );
        #[derive(serde::Deserialize)]
        struct File {
            keys: Vec<UserBinding>,
        }
        let file: File = toml::from_str(&text).expect("valid toml");
        let (km, issues) = Keymap::build(&reg, &file.keys);
        assert!(issues.is_empty(), "{issues:?}");
        let all = |_: &str| true;
        let chord = |s: &str| vec![mahzenterm_core::command::Chord::parse(s).unwrap()];
        assert_eq!(
            km.resolve(&chord("Ctrl+Alt+Y"), &all),
            mahzenterm_core::command::Resolution::Matched("a.one".into())
        );
        assert_eq!(
            km.resolve(&chord("Ctrl+Shift+T"), &all),
            mahzenterm_core::command::Resolution::NoMatch
        );
    }

    #[test]
    fn only_safe_chords_can_be_bound() {
        let c = |s: &str| mahzenterm_core::command::Chord::parse(s).unwrap();
        assert!(
            bindable(&c("Ctrl+K"))
                && bindable(&c("Alt+X"))
                && bindable(&c("F5"))
                && bindable(&c("Shift+F5"))
        );
        assert!(
            !bindable(&c("A"))
                && !bindable(&c("Shift+A"))
                && !bindable(&c("Enter"))
                && !bindable(&c("Space"))
        );
    }
}
