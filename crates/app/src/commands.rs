//! Built-in commands and key handling for the shell.
//!
//! Every action is a registered command (palette, shortcuts and menus share the registry); this module also
//! converts Iced key events into core key chords.

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use mahzenterm_core::command::{Chord, CommandSpec, Registry};
use mahzenterm_ui_kit::Themes;

/// The feature modules built into this app (one line per module).
fn modules() -> Vec<Box<dyn mahzenterm_core::module::Module>> {
    vec![
        Box::new(mahzenterm_explorer::ExplorerModule),
        Box::new(mahzenterm_editor_bridge::EditorBridgeModule),
        Box::new(mahzenterm_ai_usage::AiUsageModule),
        Box::new(mahzenterm_ai_sessions::AiSessionsModule),
        Box::new(mahzenterm_config_switcher::ConfigSwitcherModule),
        Box::new(mahzenterm_kubernetes::KubernetesModule),
        Box::new(mahzenterm_docker::DockerModule),
        Box::new(mahzenterm_lsp::LspModule),
        Box::new(mahzenterm_plugins::PluginsModule),
        Box::new(mahzenterm_project_search::ProjectSearchModule),
        Box::new(mahzenterm_git::GitModule),
        Box::new(mahzenterm_tui_store::TuiStoreModule),
    ]
}

/// Command ids, so the shell never matches on stray string literals.
pub mod id {
    pub const TAB_NEW: &str = "tab.new";
    pub const TAB_CLOSE: &str = "tab.close";
    pub const TAB_NEXT: &str = "tab.next";
    pub const TAB_PREV: &str = "tab.prev";
    pub const PANE_SPLIT_RIGHT: &str = "pane.split_right";
    pub const PANE_SPLIT_DOWN: &str = "pane.split_down";
    pub const PANE_CLOSE: &str = "pane.close";
    pub const PANE_ZOOM: &str = "pane.zoom";
    pub const PANE_DETACH: &str = "pane.detach";
    pub const PANE_FOCUS_LEFT: &str = "pane.focus_left";
    pub const PANE_FOCUS_RIGHT: &str = "pane.focus_right";
    pub const PANE_FOCUS_UP: &str = "pane.focus_up";
    pub const PANE_FOCUS_DOWN: &str = "pane.focus_down";
    pub const PANE_GROW_LEFT: &str = "pane.resize_left";
    pub const PANE_GROW_RIGHT: &str = "pane.resize_right";
    pub const PANE_GROW_UP: &str = "pane.resize_up";
    pub const PANE_GROW_DOWN: &str = "pane.resize_down";
    pub const PANE_NEXT: &str = "pane.next";
    pub const PANE_PREV: &str = "pane.prev";
    pub const PANE_SWAP_NEXT: &str = "pane.swap_next";
    pub const PALETTE_OPEN: &str = "palette.open";
    pub const WIDGETS_PICK: &str = "widgets.pick";
    pub const KEYS_EDIT: &str = "keys.edit";
    pub const PROMPT_PREV: &str = "terminal.prompt_prev";
    pub const PROMPT_NEXT: &str = "terminal.prompt_next";
    pub const SEARCH_OPEN: &str = "search.open";
    pub const SEARCH_NEXT: &str = "search.next";
    pub const SEARCH_PREV: &str = "search.prev";
    pub const THEME_NEXT: &str = "theme.next";
    pub const THEME_PREV: &str = "theme.prev";
    pub const FONT_LARGER: &str = "view.font_larger";
    pub const FONT_SMALLER: &str = "view.font_smaller";
    pub const FONT_RESET: &str = "view.font_reset";
    pub const APP_QUIT: &str = "app.quit";
    pub const WINDOW_FULLSCREEN: &str = "window.fullscreen";
    pub const WINDOW_MAXIMIZE: &str = "window.maximize";
    /// Prefix of the per-theme commands (`theme.set.tokyo_night`).
    pub const THEME_SET_PREFIX: &str = "theme.set.";
    pub const EDIT_COPY: &str = "edit.copy";
    pub const EDIT_PASTE: &str = "edit.paste";
    /// `pane.goto_1` .. `pane.goto_9`.
    pub fn pane_goto(n: usize) -> String {
        format!("pane.goto_{n}")
    }
    /// `tab.goto_1` .. `tab.goto_9`.
    pub fn tab_goto(n: usize) -> String {
        format!("tab.goto_{n}")
    }
}

/// Command id that selects a theme: ids allow only `[a-z0-9_.]`, so `-` becomes `_`.
pub fn theme_command_id(theme_id: &str) -> String {
    format!("{}{}", id::THEME_SET_PREFIX, theme_id.replace('-', "_"))
}

/// Registry with every built-in command and its default shortcuts, plus one command per theme.
pub fn builtin_registry(themes: &Themes) -> Registry {
    use id::*;
    let mut r = Registry::new();
    let specs = [
        CommandSpec::new(TAB_NEW, "New Tab", "Tab").key("Ctrl+Shift+T"),
        CommandSpec::new(TAB_CLOSE, "Close Tab", "Tab")
            .key("Ctrl+Shift+Q")
            .when("tabs.multiple"),
        CommandSpec::new(TAB_NEXT, "Next Tab", "Tab")
            .key("Ctrl+PageDown")
            .when("tabs.multiple"),
        CommandSpec::new(TAB_PREV, "Previous Tab", "Tab")
            .key("Ctrl+PageUp")
            .when("tabs.multiple"),
        CommandSpec::new(PANE_SPLIT_RIGHT, "Split Right", "Pane").key("Ctrl+Shift+D"),
        CommandSpec::new(PANE_SPLIT_DOWN, "Split Down", "Pane").key("Ctrl+Shift+E"),
        CommandSpec::new(PANE_CLOSE, "Close Pane", "Pane").key("Ctrl+Shift+W"),
        CommandSpec::new(PANE_DETACH, "Move Pane to a New Window", "Pane").key("Ctrl+Alt+D"),
        CommandSpec::new(PANE_ZOOM, "Toggle Pane Zoom", "Pane")
            .key("Ctrl+Shift+Enter")
            .when("panes.multiple || pane.zoomed"),
        CommandSpec::new(PANE_NEXT, "Next Pane", "Pane")
            .key("Ctrl+Alt+PageDown")
            .when("panes.multiple"),
        CommandSpec::new(PANE_PREV, "Previous Pane", "Pane")
            .key("Ctrl+Alt+PageUp")
            .when("panes.multiple"),
        CommandSpec::new(PANE_FOCUS_LEFT, "Focus Pane Left", "Pane")
            .key("Ctrl+Alt+Left")
            .when("panes.multiple"),
        CommandSpec::new(PANE_FOCUS_RIGHT, "Focus Pane Right", "Pane")
            .key("Ctrl+Alt+Right")
            .when("panes.multiple"),
        CommandSpec::new(PANE_FOCUS_UP, "Focus Pane Up", "Pane")
            .key("Ctrl+Alt+Up")
            .when("panes.multiple"),
        CommandSpec::new(PANE_FOCUS_DOWN, "Focus Pane Down", "Pane")
            .key("Ctrl+Alt+Down")
            .when("panes.multiple"),
        CommandSpec::new(PANE_GROW_LEFT, "Resize Pane Left", "Pane")
            .key("Ctrl+Alt+Shift+Left")
            .when("panes.multiple"),
        CommandSpec::new(PANE_GROW_RIGHT, "Resize Pane Right", "Pane")
            .key("Ctrl+Alt+Shift+Right")
            .when("panes.multiple"),
        CommandSpec::new(PANE_GROW_UP, "Resize Pane Up", "Pane")
            .key("Ctrl+Alt+Shift+Up")
            .when("panes.multiple"),
        CommandSpec::new(PANE_GROW_DOWN, "Resize Pane Down", "Pane")
            .key("Ctrl+Alt+Shift+Down")
            .when("panes.multiple"),
        CommandSpec::new(PANE_SWAP_NEXT, "Swap Pane With Next", "Pane")
            .key("Ctrl+Alt+Shift+S")
            .when("panes.multiple"),
        CommandSpec::new(PALETTE_OPEN, "Show Command Palette", "View").key("Ctrl+Shift+P"),
        CommandSpec::new(KEYS_EDIT, "Keyboard Shortcuts...", "View").key("Ctrl+K Ctrl+S"),
        CommandSpec::new(WIDGETS_PICK, "Widgets: Choose a Widget...", "Widgets").key("Ctrl+Alt+W"),
        CommandSpec::new(PROMPT_PREV, "Jump to Previous Prompt", "Terminal").key("Ctrl+Shift+Up"),
        CommandSpec::new(PROMPT_NEXT, "Jump to Next Prompt", "Terminal").key("Ctrl+Shift+Down"),
        CommandSpec::new(SEARCH_OPEN, "Find in Scrollback", "Edit").key("Ctrl+Shift+F"),
        CommandSpec::new(SEARCH_NEXT, "Find Next", "Edit").key("F3"),
        CommandSpec::new(SEARCH_PREV, "Find Previous", "Edit").key("Shift+F3"),
        CommandSpec::new(THEME_NEXT, "Next Theme", "Theme"),
        CommandSpec::new(THEME_PREV, "Previous Theme", "Theme"),
        CommandSpec::new(FONT_LARGER, "Increase Font Size", "View")
            .key("Ctrl+=")
            .key("Ctrl+Shift++"),
        CommandSpec::new(FONT_SMALLER, "Decrease Font Size", "View").key("Ctrl+-"),
        CommandSpec::new(FONT_RESET, "Reset Font Size", "View").key("Ctrl+0"),
        CommandSpec::new(APP_QUIT, "Quit MahzenTerm", "Application"),
        CommandSpec::new(WINDOW_FULLSCREEN, "Toggle Full Screen", "Window").key("F11"),
        CommandSpec::new(WINDOW_MAXIMIZE, "Toggle Maximize", "Window"),
        CommandSpec::new(EDIT_COPY, "Copy Selection", "Edit").key("Ctrl+Shift+C"),
        CommandSpec::new(EDIT_PASTE, "Paste", "Edit").key("Ctrl+Shift+V"),
    ];
    for module in modules() {
        if let Err(e) = mahzenterm_core::module::validate(module.as_ref()) {
            tracing::error!(error = %e, "invalid module");
            continue;
        }
        for spec in module.commands() {
            if let Err(e) = r.register(spec) {
                tracing::error!(error = %e, "invalid module command");
            }
        }
    }
    for spec in specs {
        // Built-in specs are static; a failure here is a programming error caught by the unit tests.
        if let Err(e) = r.register(spec) {
            tracing::error!(error = %e, "invalid built-in command");
        }
    }
    for n in 1..=9usize {
        let spec = CommandSpec::new(&id::tab_goto(n), &format!("Go To Tab {n}"), "Tab")
            .key(&format!("Alt+{n}"))
            .when(&format!("tabs.count_ge_{n}"));
        if let Err(e) = r.register(spec) {
            tracing::error!(error = %e, "invalid built-in command");
        }
    }
    for n in 1..=9usize {
        let spec = CommandSpec::new(&id::pane_goto(n), &format!("Go To Pane {n}"), "Pane")
            .key(&format!("Ctrl+Alt+{n}"))
            .when(&format!("panes.count_ge_{n}"));
        if let Err(e) = r.register(spec) {
            tracing::error!(error = %e, "invalid built-in command");
        }
    }
    for theme in themes.all() {
        let spec = CommandSpec::new(
            &theme_command_id(&theme.id),
            &format!("Color Theme: {}", theme.name),
            "Theme",
        );
        if let Err(e) = r.register(spec) {
            tracing::error!(error = %e, theme = %theme.id, "cannot register theme command");
        }
    }
    r
}

/// Convert an Iced key press into a core chord. Returns `None` for pure modifier presses and unknown keys.
pub fn chord_from(key: &Key, mods: Modifiers) -> Option<Chord> {
    let name = match key {
        Key::Character(c) => c.to_lowercase(),
        Key::Named(n) => match n {
            Named::ArrowLeft => "left",
            Named::ArrowRight => "right",
            Named::ArrowUp => "up",
            Named::ArrowDown => "down",
            Named::PageUp => "pageup",
            Named::PageDown => "pagedown",
            Named::Home => "home",
            Named::End => "end",
            Named::Enter => "enter",
            Named::Tab => "tab",
            Named::Escape => "escape",
            Named::Backspace => "backspace",
            Named::Delete => "delete",
            Named::Insert => "insert",
            Named::Space => "space",
            Named::F1 => "f1",
            Named::F2 => "f2",
            Named::F3 => "f3",
            Named::F4 => "f4",
            Named::F5 => "f5",
            Named::F6 => "f6",
            Named::F7 => "f7",
            Named::F8 => "f8",
            Named::F9 => "f9",
            Named::F10 => "f10",
            Named::F11 => "f11",
            Named::F12 => "f12",
            _ => return None,
        }
        .to_string(),
        Key::Unidentified => return None,
    };
    Some(Chord {
        ctrl: mods.control(),
        alt: mods.alt(),
        shift: mods.shift(),
        logo: mods.logo(),
        key: name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mahzenterm_core::command::{Keymap, Resolution};

    #[test]
    fn builtin_registry_registers_everything_without_key_conflicts() {
        let themes = Themes::builtin();
        let r = builtin_registry(&themes);
        assert_eq!(
            r.len(),
            36 + 2 + 1 + 7 + 7 + 1 + 2 + 5 + 9 + 9 + 4 + 2 + 1 + 1 + 1 + 1 + 1 + themes.all().len()
        );
        let (_, issues) = Keymap::build(&r, &[]);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn new_module_shortcuts_resolve() {
        let (km, _) = Keymap::build(&builtin_registry(&Themes::builtin()), &[]);
        let none = |_: &str| false;
        // Ctrl+Shift+U is taken by IBus (Unicode entry) on many Linux desktops, so the card uses Ctrl+Alt+U.
        for (ch, mods, cmd) in [
            (
                "u",
                Modifiers::CTRL | Modifiers::ALT,
                mahzenterm_ai_usage::id::TOGGLE,
            ),
            (
                "O",
                Modifiers::CTRL | Modifiers::SHIFT,
                mahzenterm_editor_bridge::id::OPEN_HERE,
            ),
        ] {
            let chord = chord_from(&Key::Character(ch.into()), mods).unwrap();
            assert_eq!(km.resolve(&[chord], &none), Resolution::Matched(cmd.into()));
        }
    }

    #[test]
    fn explorer_focus_does_not_collide_with_go_to_pane_keys() {
        let (km, _) = Keymap::build(&builtin_registry(&Themes::builtin()), &[]);
        // Even with many panes and tabs open, Ctrl+Alt+E is the explorer and Ctrl+Alt+1 is still pane 1.
        let all = |_: &str| true;
        let e = chord_from(
            &Key::Character("e".into()),
            Modifiers::CTRL | Modifiers::ALT,
        )
        .unwrap();
        assert_eq!(
            km.resolve(&[e], &all),
            Resolution::Matched(mahzenterm_explorer::id::FOCUS.into())
        );
        let one = chord_from(
            &Key::Character("1".into()),
            Modifiers::CTRL | Modifiers::ALT,
        )
        .unwrap();
        assert_eq!(
            km.resolve(&[one], &all),
            Resolution::Matched(id::pane_goto(1))
        );
    }

    #[test]
    fn ctrl_shift_d_splits_right() {
        let (km, _) = Keymap::build(&builtin_registry(&Themes::builtin()), &[]);
        let chord = chord_from(
            &Key::Character("D".into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .unwrap();
        let none = |_: &str| false;
        assert_eq!(
            km.resolve(&[chord], &none),
            Resolution::Matched(id::PANE_SPLIT_RIGHT.into())
        );
    }

    #[test]
    fn guarded_commands_follow_context() {
        let (km, _) = Keymap::build(&builtin_registry(&Themes::builtin()), &[]);
        let chord = chord_from(
            &Key::Named(Named::ArrowLeft),
            Modifiers::CTRL | Modifiers::ALT,
        )
        .unwrap();
        assert_eq!(
            km.resolve(std::slice::from_ref(&chord), &|_| false),
            Resolution::NoMatch
        );
        assert_eq!(
            km.resolve(&[chord], &|n| n == "panes.multiple"),
            Resolution::Matched(id::PANE_FOCUS_LEFT.into())
        );
    }

    #[test]
    fn every_theme_gets_a_valid_unique_command() {
        let themes = Themes::builtin();
        let r = builtin_registry(&themes);
        for t in themes.all() {
            let cmd = theme_command_id(&t.id);
            assert!(r.get(&cmd).is_some(), "{cmd} missing");
            assert!(r.get(&cmd).unwrap().title.contains(&t.name));
        }
        assert_eq!(theme_command_id("tokyo-night"), "theme.set.tokyo_night");
    }

    #[test]
    fn font_and_palette_shortcuts_resolve() {
        let (km, _) = Keymap::build(&builtin_registry(&Themes::builtin()), &[]);
        let none = |_: &str| false;
        let ctrl = Modifiers::CTRL;
        let eq = chord_from(&Key::Character("=".into()), ctrl).unwrap();
        assert_eq!(
            km.resolve(&[eq], &none),
            Resolution::Matched(id::FONT_LARGER.into())
        );
        let plus = chord_from(&Key::Character("+".into()), ctrl | Modifiers::SHIFT).unwrap();
        assert_eq!(
            km.resolve(&[plus], &none),
            Resolution::Matched(id::FONT_LARGER.into())
        );
        let minus = chord_from(&Key::Character("-".into()), ctrl).unwrap();
        assert_eq!(
            km.resolve(&[minus], &none),
            Resolution::Matched(id::FONT_SMALLER.into())
        );
        let p = chord_from(&Key::Character("P".into()), ctrl | Modifiers::SHIFT).unwrap();
        assert_eq!(
            km.resolve(&[p], &none),
            Resolution::Matched(id::PALETTE_OPEN.into())
        );
    }

    #[test]
    fn plain_typing_is_never_a_command() {
        let (km, _) = Keymap::build(&builtin_registry(&Themes::builtin()), &[]);
        let chord = chord_from(&Key::Character("a".into()), Modifiers::empty()).unwrap();
        assert_eq!(km.resolve(&[chord], &|_| true), Resolution::NoMatch);
    }

    #[test]
    fn modifier_only_press_is_not_a_chord() {
        assert!(chord_from(&Key::Named(Named::Shift), Modifiers::SHIFT).is_none());
    }
}
