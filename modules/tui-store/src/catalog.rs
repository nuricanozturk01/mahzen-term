//! The catalog: apps described as data (TOML), embedded plus user-added files. External input is untrusted, so
//! every field is validated before it can reach a command line.

use std::path::Path;

use serde::Deserialize;

/// Catalog files shipped inside the binary.
const EMBEDDED: &[(&str, &str)] = &[
    ("yazi", include_str!("../catalog/yazi.toml")),
    ("helix", include_str!("../catalog/helix.toml")),
    ("neovim", include_str!("../catalog/neovim.toml")),
    ("micro", include_str!("../catalog/micro.toml")),
    ("btop", include_str!("../catalog/btop.toml")),
    ("htop", include_str!("../catalog/htop.toml")),
    ("bottom", include_str!("../catalog/bottom.toml")),
    ("ncdu", include_str!("../catalog/ncdu.toml")),
    ("lazygit", include_str!("../catalog/lazygit.toml")),
    ("gitui", include_str!("../catalog/gitui.toml")),
    ("lazydocker", include_str!("../catalog/lazydocker.toml")),
    ("k9s", include_str!("../catalog/k9s.toml")),
    ("glow", include_str!("../catalog/glow.toml")),
    ("zellij", include_str!("../catalog/zellij.toml")),
];

/// Largest user catalog file read, in bytes.
const MAX_FILE: u64 = 64 * 1024;
/// Most user catalog files read.
const MAX_FILES: usize = 200;

/// Package names per package manager.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstallSpec {
    pub apt: Option<String>,
    pub pacman: Option<String>,
    pub brew: Option<String>,
    pub winget: Option<String>,
    pub cargo: Option<String>,
}

/// Platforms an app supports; empty means all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Platforms {
    pub supported: Vec<String>,
}

/// One catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub id: String,
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub categories: Vec<String>,
    /// Program to start (looked up on `PATH`).
    pub command: String,
    #[serde(default)]
    pub install: InstallSpec,
    #[serde(default)]
    pub platforms: Platforms,
}

/// Why a catalog entry was rejected.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("cannot parse: {0}")]
    Parse(String),
    #[error("{field}: {why}")]
    Invalid { field: &'static str, why: String },
}

fn bad(field: &'static str, why: &str) -> CatalogError {
    CatalogError::Invalid {
        field,
        why: why.to_owned(),
    }
}

/// Package names and commands may only use these characters: no spaces, quotes or shell syntax.
fn safe_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.starts_with('-')
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-' | b'/' | b'@')
        })
}

impl App {
    /// Check every field.
    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err(bad("id", "use a-z, 0-9, - and _"));
        }
        if self.name.trim().is_empty() || self.name.len() > 60 {
            return Err(bad("name", "1 to 60 characters"));
        }
        if self.summary.len() > 160 {
            return Err(bad("summary", "at most 160 characters"));
        }
        if !safe_token(&self.command) || self.command.contains('/') {
            return Err(bad(
                "command",
                "a bare program name without spaces or slashes",
            ));
        }
        let pkgs = [
            ("install.apt", &self.install.apt),
            ("install.pacman", &self.install.pacman),
            ("install.brew", &self.install.brew),
            ("install.winget", &self.install.winget),
            ("install.cargo", &self.install.cargo),
        ];
        for (field, v) in pkgs {
            if v.as_deref().is_some_and(|p| !safe_token(p)) {
                return Err(CatalogError::Invalid {
                    field: "install",
                    why: format!("{field} has characters that are not allowed in a package name"),
                });
            }
        }
        for p in &self.platforms.supported {
            if !matches!(p.as_str(), "linux" | "macos" | "windows") {
                return Err(bad("platforms.supported", "linux, macos or windows"));
            }
        }
        Ok(())
    }

    /// Whether the app runs on `os` (`linux`, `macos`, `windows`).
    pub fn supports(&self, os: &str) -> bool {
        self.platforms.supported.is_empty() || self.platforms.supported.iter().any(|p| p == os)
    }
}

/// Parse and validate one TOML entry.
pub fn parse_app(text: &str) -> Result<App, CatalogError> {
    let app: App = toml::from_str(text).map_err(|e| CatalogError::Parse(e.message().to_owned()))?;
    app.validate()?;
    Ok(app)
}

/// The catalog and the entries that were skipped.
#[derive(Debug, Default)]
pub struct Loaded {
    pub apps: Vec<App>,
    pub problems: Vec<String>,
}

/// Embedded entries plus `*.toml` files from `user_dir`. A user entry replaces an embedded one with the same id.
pub fn load(user_dir: Option<&Path>) -> Loaded {
    let mut out = Loaded::default();
    let add = |source: &str, text: &str, out: &mut Loaded| match parse_app(text) {
        Ok(app) => {
            out.apps.retain(|a| a.id != app.id);
            out.apps.push(app);
        }
        Err(e) => out.problems.push(format!("{source}: {e}")),
    };
    for (name, text) in EMBEDDED {
        add(name, text, &mut out);
    }
    if let Some(dir) = user_dir {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut paths: Vec<_> = rd
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            paths.sort();
            for path in paths.into_iter().take(MAX_FILES) {
                let label = path.display().to_string();
                let too_big = std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_FILE);
                if too_big {
                    out.problems
                        .push(format!("{label}: unreadable or larger than 64 KiB"));
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(text) => add(&label, &text, &mut out),
                    Err(e) => out.problems.push(format!("{label}: {e}")),
                }
            }
        }
    }
    out.apps.sort_by_key(|a| a.name.to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
id = "demo"
name = "Demo"
summary = "A demo"
command = "demo"
[install]
cargo = "demo-tui"
"#;

    #[test]
    fn embedded_catalog_is_valid_and_complete() {
        let l = load(None);
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        assert_eq!(l.apps.len(), EMBEDDED.len());
        assert!(l.apps.iter().any(|a| a.id == "yazi"));
        // Sorted by name, ignoring case.
        let names: Vec<_> = l.apps.iter().map(|a| a.name.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn hostile_fields_are_rejected() {
        let with = |k: &str, v: &str| GOOD.replace(k, v);
        assert!(parse_app(GOOD).is_ok());
        for text in [
            with("command = \"demo\"", "command = \"demo; rm -rf ~\""),
            with("command = \"demo\"", "command = \"/bin/demo\""),
            with("demo-tui", "demo tui"),
            with("demo-tui", "$(curl evil)"),
            with("demo-tui", "--git=evil"),
            with("id = \"demo\"", "id = \"De Mo\""),
            format!("{GOOD}\n[platforms]\nsupported = [\"beos\"]\n"),
        ] {
            assert!(parse_app(&text).is_err(), "{text}");
        }
        assert!(matches!(parse_app("id = 3"), Err(CatalogError::Parse(_))));
        assert!(parse_app(&format!("{GOOD}\nbogus = 1\n")).is_err());
    }

    #[test]
    fn user_files_add_replace_and_report_problems() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.toml"), GOOD).unwrap();
        std::fs::write(
            d.path().join("yazi.toml"),
            GOOD.replace("demo\"\nname", "yazi\"\nname"),
        )
        .unwrap();
        std::fs::write(d.path().join("broken.toml"), "id = ").unwrap();
        std::fs::write(d.path().join("big.toml"), vec![b'#'; 70_000]).unwrap();
        std::fs::write(d.path().join("note.txt"), "ignored").unwrap();
        let l = load(Some(d.path()));
        assert!(l.apps.iter().any(|a| a.id == "demo"));
        let yazi = l.apps.iter().find(|a| a.id == "yazi").unwrap();
        assert_eq!(yazi.name, "Demo", "user entry replaced the embedded one");
        assert_eq!(l.problems.len(), 2, "{:?}", l.problems);
    }

    #[test]
    fn platform_filter() {
        let mut a = parse_app(GOOD).unwrap();
        assert!(a.supports("windows"));
        a.platforms.supported = vec!["linux".into()];
        assert!(a.supports("linux") && !a.supports("macos"));
    }
}
