//! Config switcher: named profiles (environment variables and a theme) you switch with one command.
//!
//! A profile is a TOML file in `<config dir>/profiles/`. The active profile applies to **new** tabs and panes; running
//! shells are never changed behind your back. A project can name the profile it wants in `.mahzenterm/profile`
//! (a single profile name, never the definition itself), so entering a project folder can select it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use serde::Deserialize;

/// Command ids of this module.
pub mod id {
    /// Prefix of the per-profile commands (`profile.use.work`).
    pub const USE_PREFIX: &str = "profile.use.";
    pub const CLEAR: &str = "profile.clear";
    pub const RELOAD: &str = "profile.reload";
}

/// Largest profile file read, in bytes.
const MAX_FILE: u64 = 32 * 1024;
const MAX_PROFILES: usize = 100;
const MAX_VARS: usize = 64;
const MAX_VALUE: usize = 4096;
/// Variables that make a program load foreign code at start. Never set from a profile.
const BLOCKED: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "BASH_ENV",
    "ENV",
];

/// `[modules.profile]` in the config.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Switch to the profile a folder names (`.mahzenterm/profile`) when the focused pane enters it.
    pub auto_project: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_project: true }
    }
}

/// Why a profile was rejected.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProfileError {
    #[error("cannot parse: {0}")]
    Parse(String),
    #[error("{field}: {why}")]
    Invalid { field: String, why: String },
}

fn invalid(field: impl Into<String>, why: &str) -> ProfileError {
    ProfileError::Invalid {
        field: field.into(),
        why: why.to_owned(),
    }
}

/// On-disk shape of a profile.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: Option<String>,
    #[serde(default)]
    description: String,
    theme: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// A validated profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// File stem: `work` for `work.toml`; what commands and `.mahzenterm/profile` refer to.
    pub id: String,
    pub name: String,
    pub description: String,
    pub theme: Option<String>,
    pub env: BTreeMap<String, String>,
}

/// Valid profile ids: `a-z`, `0-9`, `-`, `_`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn valid_var(name: &str) -> bool {
    let mut it = name.bytes();
    it.next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && name.len() <= 64
        && it.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Parse and validate one profile; `id` is the file stem.
pub fn parse_profile(id: &str, text: &str) -> Result<Profile, ProfileError> {
    if !valid_id(id) {
        return Err(invalid(
            "file name",
            "use a-z, 0-9, - and _ (for example work.toml)",
        ));
    }
    let raw: Raw = toml::from_str(text).map_err(|e| ProfileError::Parse(e.message().to_owned()))?;
    if raw.env.len() > MAX_VARS {
        return Err(invalid("env", "at most 64 variables"));
    }
    for (k, v) in &raw.env {
        if !valid_var(k) {
            return Err(invalid(format!("env.{k}"), "not a valid variable name"));
        }
        if BLOCKED.contains(&k.to_ascii_uppercase().as_str()) {
            return Err(invalid(
                format!("env.{k}"),
                "this variable makes programs load foreign code and cannot be set from a profile",
            ));
        }
        if v.len() > MAX_VALUE || v.contains('\0') {
            return Err(invalid(
                format!("env.{k}"),
                "value too long or contains NUL",
            ));
        }
    }
    let name = raw
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| id.to_owned());
    if name.len() > 60 {
        return Err(invalid("name", "at most 60 characters"));
    }
    Ok(Profile {
        id: id.to_owned(),
        name,
        description: raw.description.chars().take(160).collect(),
        theme: raw.theme,
        env: raw.env,
    })
}

impl Profile {
    /// Environment for a child process: values with a leading `~/` expanded to `home`, in name order.
    pub fn env_for_spawn(&self, home: Option<&Path>) -> Vec<(String, String)> {
        self.env
            .iter()
            .map(|(k, v)| {
                let value = match (v.strip_prefix("~/"), home) {
                    (Some(rest), Some(h)) => h.join(rest).display().to_string(),
                    _ if v == "~" => home.map_or_else(|| v.clone(), |h| h.display().to_string()),
                    _ => v.clone(),
                };
                (k.clone(), value)
            })
            .collect()
    }
}

/// Profiles read from a folder, and what was skipped.
#[derive(Debug, Default)]
pub struct Loaded {
    pub profiles: Vec<Profile>,
    pub problems: Vec<String>,
}

/// Read `*.toml` profiles from `dir` (a missing folder is normal). Sorted by name.
pub fn load_dir(dir: &Path) -> Loaded {
    let mut out = Loaded::default();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    for path in paths.into_iter().take(MAX_PROFILES) {
        let label = path.display().to_string();
        if std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_FILE) {
            out.problems
                .push(format!("{label}: unreadable or larger than 32 KiB"));
            continue;
        }
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_profile(id, &t).map_err(|e| e.to_string()))
        {
            Ok(p) => out.profiles.push(p),
            Err(e) => out.problems.push(format!("{label}: {e}")),
        }
    }
    out.profiles.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// The profile a project asks for: the first `.mahzenterm/profile` found from `start` upwards (at most 12 levels),
/// which holds only a profile id.
pub fn project_profile(start: &Path) -> Option<String> {
    let mut dir = Some(start);
    for _ in 0..12 {
        let d = dir?;
        let file = d.join(".mahzenterm").join("profile");
        if std::fs::metadata(&file).is_ok_and(|m| m.is_file() && m.len() <= 128) {
            let text = std::fs::read_to_string(&file).ok()?;
            let id = text.trim();
            return valid_id(id).then(|| id.to_owned());
        }
        dir = d.parent();
    }
    None
}

/// Command id that switches to `profile_id`.
pub fn use_command_id(profile_id: &str) -> String {
    format!("{}{}", id::USE_PREFIX, profile_id.replace('-', "_"))
}

/// The module registration (per-profile commands are added by the app from the loaded profiles).
pub struct ConfigSwitcherModule;

impl Module for ConfigSwitcherModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "profile",
            name: "Config switcher",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::FsRead],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(id::CLEAR, "Profile: None (Default Environment)", "Profile"),
            CommandSpec::new(id::RELOAD, "Profile: Reload Profiles From Disk", "Profile"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: &str = r#"
name = "Work"
description = "Company cluster and git identity"
theme = "nord"
[env]
AWS_PROFILE = "work"
KUBECONFIG = "~/.kube/work"
GIT_AUTHOR_EMAIL = "me@corp.example"
"#;

    #[test]
    fn profiles_parse_and_expand_home() {
        let p = parse_profile("work", WORK).unwrap();
        assert_eq!((p.id.as_str(), p.name.as_str()), ("work", "Work"));
        assert_eq!(p.theme.as_deref(), Some("nord"));
        let env = p.env_for_spawn(Some(Path::new("/home/me")));
        assert!(env.contains(&("KUBECONFIG".into(), "/home/me/.kube/work".into())));
        assert!(env.contains(&("AWS_PROFILE".into(), "work".into())));
        let none = p.env_for_spawn(None);
        assert!(
            none.contains(&("KUBECONFIG".into(), "~/.kube/work".into())),
            "no home: left as written"
        );
        // The name falls back to the id.
        assert_eq!(
            parse_profile("plain", "[env]\nA = \"1\"").unwrap().name,
            "plain"
        );
    }

    #[test]
    fn hostile_profiles_are_rejected() {
        for (label, text) in [
            ("preload", "[env]\nLD_PRELOAD = \"/tmp/evil.so\""),
            ("preload lower", "[env]\nld_preload = \"/tmp/evil.so\""),
            ("bash env", "[env]\nBASH_ENV = \"/tmp/x\""),
            ("dyld", "[env]\nDYLD_INSERT_LIBRARIES = \"x\""),
            ("bad name", "[env]\n\"A B\" = \"1\""),
            ("digit first", "[env]\n\"1A\" = \"1\""),
            ("nul", "[env]\nA = \"x\\u0000y\""),
            ("unknown key", "bogus = 1"),
            ("not toml", "= ="),
        ] {
            assert!(parse_profile("p", text).is_err(), "{label}");
        }
        let many: String = (0..70).map(|i| format!("V{i} = \"1\"\n")).collect();
        assert!(parse_profile("p", &format!("[env]\n{many}")).is_err());
        assert!(parse_profile("Bad Id", "").is_err());
        assert!(parse_profile("../x", "").is_err());
    }

    #[test]
    fn folders_load_sorted_with_problems_reported() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("work.toml"), WORK).unwrap();
        std::fs::write(d.path().join("home.toml"), "name = \"Home\"").unwrap();
        std::fs::write(d.path().join("evil.toml"), "[env]\nLD_PRELOAD = \"x\"").unwrap();
        std::fs::write(d.path().join("Big Name.toml"), "").unwrap();
        std::fs::write(d.path().join("huge.toml"), vec![b'#'; 40_000]).unwrap();
        std::fs::write(d.path().join("readme.txt"), "ignored").unwrap();
        let l = load_dir(d.path());
        assert_eq!(
            l.profiles.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["home", "work"]
        );
        assert_eq!(l.problems.len(), 3, "{:?}", l.problems);
        assert!(load_dir(&d.path().join("missing")).profiles.is_empty());
    }

    #[test]
    fn a_project_names_its_profile_from_any_subfolder() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".mahzenterm")).unwrap();
        std::fs::create_dir_all(d.path().join("a/b/c")).unwrap();
        std::fs::write(d.path().join(".mahzenterm/profile"), "work\n").unwrap();
        assert_eq!(
            project_profile(&d.path().join("a/b/c")).as_deref(),
            Some("work")
        );
        // Anything that is not a plain id is ignored (no paths, no definitions).
        std::fs::write(d.path().join(".mahzenterm/profile"), "../../etc/passwd").unwrap();
        assert_eq!(project_profile(d.path()), None);
        std::fs::write(d.path().join(".mahzenterm/profile"), "x".repeat(300)).unwrap();
        assert_eq!(project_profile(d.path()), None);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(project_profile(empty.path()), None);
    }

    #[test]
    fn settings_default_to_automatic_project_profiles() {
        assert!(Settings::default().auto_project);
        let off: Settings = toml::from_str("auto_project = false").unwrap();
        assert!(!off.auto_project);
        assert!(toml::from_str::<Settings>("bogus = 1").is_err());
    }

    #[test]
    fn command_ids_and_module_are_valid() {
        assert_eq!(use_command_id("my-work"), "profile.use.my_work");
        mahzenterm_core::module::validate(&ConfigSwitcherModule).unwrap();
    }
}
