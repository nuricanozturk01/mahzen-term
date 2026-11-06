//! Editor bridge: open a file (optionally at a line) in the user's terminal editor.
//!
//! Pure logic only: parse `path:line:col` text, detect installed editors, and build the argument list for each
//! editor's own "go to line" syntax. The app starts the resulting command in a pane.

use std::path::{Path, PathBuf};

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};

/// Command ids of this module.
pub mod id {
    /// Open the focused pane's directory in the preferred editor.
    pub const OPEN_HERE: &str = "editor.open_here";
}

/// `[modules.editor]` in the config.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Editor command, for example `"hx"` or `"nvim"`; empty uses `$VISUAL`, `$EDITOR`, then a detected editor.
    pub command: Option<String>,
}

/// A file and an optional position, 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

impl Location {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            line: None,
            col: None,
        }
    }
}

/// Parse `path`, `path:line` or `path:line:col`. Relative paths are resolved against `cwd`.
/// Only existing files qualify, so prose such as `note:3` is not mistaken for a location.
pub fn parse_location(text: &str, cwd: &Path) -> Option<Location> {
    let text = text
        .trim()
        .trim_end_matches([':', ',', '.', ')', '"', '\'', '`'])
        .trim_start_matches(['(', '"', '\'', '`']);
    let num = |s: &str| s.parse::<u32>().ok().filter(|n| *n > 0);
    let mut tries: Vec<(&str, Option<u32>, Option<u32>)> = Vec::new();
    // path:line:col, then path:line, then the bare path (a Windows drive colon stays in the path).
    if let Some((rest, col)) = text.rsplit_once(':') {
        if let Some((path, line)) = rest.rsplit_once(':') {
            if let (Some(l), Some(c)) = (num(line), num(col)) {
                tries.push((path, Some(l), Some(c)));
            }
        }
        if let Some(l) = num(col) {
            tries.push((rest, Some(l), None));
        }
    }
    tries.push((text, None, None));
    tries.into_iter().find_map(|(p, line, col)| {
        let path = Path::new(p);
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        (!p.is_empty() && full.is_file()).then_some(Location {
            path: full,
            line,
            col,
        })
    })
}

/// Editors the bridge knows how to address by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor {
    Helix,
    Neovim,
    Micro,
    Nano,
    Other,
}

impl Editor {
    /// Classify a program name (`hx`, `/usr/bin/nvim`, `nvim.exe`).
    pub fn from_program(program: &str) -> Self {
        let name = Path::new(program)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(program)
            .to_ascii_lowercase();
        match name.as_str() {
            "hx" | "helix" => Self::Helix,
            "nvim" => Self::Neovim,
            "micro" => Self::Micro,
            "nano" => Self::Nano,
            _ => Self::Other,
        }
    }
}

/// Programs probed in order when the user has no preference.
pub const KNOWN: &[&str] = &["hx", "nvim", "micro", "nano"];

/// A resolved editor command: program plus leading arguments from `$EDITOR` (for example `code -w`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCmd {
    pub program: String,
    pub args: Vec<String>,
}

impl EditorCmd {
    /// Split `"code -w"` into program and arguments. `None` for empty text.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut it = raw.split_whitespace().map(str::to_owned);
        let program = it.next()?;
        Some(Self {
            program,
            args: it.collect(),
        })
    }

    /// Full argument list (after the program) to open `loc`.
    pub fn argv(&self, loc: &Location) -> Vec<String> {
        let mut args = self.args.clone();
        let file = loc.path.display().to_string();
        let line = loc.line;
        let col = loc.col;
        match (Editor::from_program(&self.program), line) {
            (Editor::Helix | Editor::Micro, Some(l)) => {
                args.push(format!(
                    "{file}:{l}{}",
                    col.map_or(String::new(), |c| format!(":{c}"))
                ));
            }
            (Editor::Neovim, Some(l)) => {
                args.push(format!("+{l}"));
                args.push(file);
            }
            (Editor::Nano, Some(l)) => {
                args.push(format!(
                    "+{l}{}",
                    col.map_or(String::new(), |c| format!(",{c}"))
                ));
                args.push(file);
            }
            _ => args.push(file),
        }
        args
    }
}

/// First executable called `name` on `path_var` (a `PATH`-style list).
pub fn find_program(name: &str, path_var: &std::ffi::OsStr) -> Option<PathBuf> {
    let suffix = std::env::consts::EXE_SUFFIX;
    std::env::split_paths(path_var).find_map(|dir| {
        [name.to_owned(), format!("{name}{suffix}")]
            .into_iter()
            .map(|n| dir.join(n))
            .find(|p| p.is_file())
    })
}

/// Pick the editor: the configured one, else `$VISUAL`, `$EDITOR`, else the first known editor on `path_var`.
pub fn choose(
    configured: Option<&str>,
    visual: Option<&str>,
    editor: Option<&str>,
    path_var: &std::ffi::OsStr,
) -> Option<EditorCmd> {
    [configured, visual, editor]
        .into_iter()
        .flatten()
        .filter_map(EditorCmd::parse)
        .next()
        .or_else(|| {
            KNOWN
                .iter()
                .find(|n| find_program(n, path_var).is_some())
                .map(|n| EditorCmd {
                    program: (*n).to_owned(),
                    args: Vec::new(),
                })
        })
}

/// Editors from [`KNOWN`] that are installed, for the "install an editor" suggestion in the store.
pub fn installed(path_var: &std::ffi::OsStr) -> Vec<&'static str> {
    KNOWN
        .iter()
        .copied()
        .filter(|n| find_program(n, path_var).is_some())
        .collect()
}

/// The module registration.
pub struct EditorBridgeModule;

impl Module for EditorBridgeModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "editor",
            name: "Editor bridge",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![
                Capability::ProcessSpawn,
                Capability::Pty,
                Capability::FsRead,
            ],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![CommandSpec::new(id::OPEN_HERE, "Open Editor Here", "Editor").key("Ctrl+Shift+O")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn loc(path: &str, line: Option<u32>, col: Option<u32>) -> Location {
        Location {
            path: PathBuf::from(path),
            line,
            col,
        }
    }

    #[test]
    fn parses_line_and_column_for_existing_files() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/main.rs"), "x").unwrap();
        let p = d.path().join("src/main.rs");
        let got = parse_location("src/main.rs:12:5", d.path()).unwrap();
        assert_eq!(
            got,
            Location {
                path: p.clone(),
                line: Some(12),
                col: Some(5)
            }
        );
        let got = parse_location("src/main.rs:12", d.path()).unwrap();
        assert_eq!((got.line, got.col), (Some(12), None));
        let got = parse_location("src/main.rs", d.path()).unwrap();
        assert_eq!((got.line, got.col), (None, None));
        // Trailing punctuation from compiler output.
        let got = parse_location("(src/main.rs:12:5):", d.path()).unwrap();
        assert_eq!(got.line, Some(12));
        assert_eq!(
            parse_location(&format!("{}:3", p.display()), d.path())
                .unwrap()
                .line,
            Some(3)
        );
    }

    #[test]
    fn rejects_missing_files_and_zero_lines() {
        let d = tempfile::tempdir().unwrap();
        assert!(parse_location("nope.rs:3", d.path()).is_none());
        fs::write(d.path().join("a.rs"), "x").unwrap();
        let got = parse_location("a.rs:0", d.path());
        assert!(got.is_none(), "a.rs:0 is not a file and not a valid line");
    }

    #[test]
    fn each_editor_gets_its_own_goto_syntax() {
        let l = loc("/p/f.rs", Some(7), Some(2));
        let argv = |prog: &str| EditorCmd::parse(prog).unwrap().argv(&l);
        assert_eq!(argv("hx"), ["/p/f.rs:7:2"]);
        assert_eq!(argv("/usr/bin/micro"), ["/p/f.rs:7:2"]);
        assert_eq!(argv("nvim"), ["+7", "/p/f.rs"]);
        assert_eq!(argv("nano"), ["+7,2", "/p/f.rs"]);
        assert_eq!(argv("code -w"), ["-w", "/p/f.rs"]);
        assert_eq!(
            EditorCmd::parse("nvim")
                .unwrap()
                .argv(&loc("/p/f.rs", None, None)),
            ["/p/f.rs"]
        );
    }

    #[test]
    fn choice_prefers_config_then_env_then_installed() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("micro"), "").unwrap();
        let path = std::env::join_paths([d.path()]).unwrap();
        let name = |c: Option<EditorCmd>| c.map(|c| c.program);
        assert_eq!(
            name(choose(Some("hx"), Some("emacs"), None, &path)),
            Some("hx".into())
        );
        assert_eq!(
            name(choose(None, Some("emacs"), Some("nano"), &path)),
            Some("emacs".into())
        );
        assert_eq!(
            name(choose(None, Some("  "), None, &path)),
            Some("micro".into())
        );
        assert_eq!(
            name(choose(None, None, None, std::ffi::OsStr::new(""))),
            None
        );
        assert_eq!(installed(&path), ["micro"]);
    }

    #[test]
    fn module_is_valid() {
        mahzenterm_core::module::validate(&EditorBridgeModule).unwrap();
    }
}
