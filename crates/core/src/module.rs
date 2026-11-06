//! Contract between the app and feature modules (`modules/*`).
//!
//! A module declares what it is and which commands it adds; the app registers them in the shared command
//! registry, so every module action is searchable in the palette and rebindable. Modules never call each other:
//! they talk through the bus and commands.

use serde::{Deserialize, Serialize};

use crate::command::CommandSpec;

/// What a module may do. In-tree modules are trusted; the list is recorded so later sandboxed modules
/// (WASM or external process) can be held to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    FsRead,
    FsWrite,
    Net,
    ProcessSpawn,
    Pty,
    Clipboard,
}

/// Identity and requirements of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleManifest {
    /// Stable id, `[a-z0-9_]`; also the key of its `[modules.<id>]` config section.
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub capabilities: Vec<Capability>,
}

/// A feature module.
pub trait Module: Send + 'static {
    fn manifest(&self) -> ModuleManifest;

    /// Commands this module adds (ids start with the module id).
    fn commands(&self) -> Vec<CommandSpec>;
}

/// Check a module's manifest and commands: id format and command id prefix.
pub fn validate(module: &dyn Module) -> Result<(), String> {
    let m = module.manifest();
    if m.id.is_empty()
        || !m
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(format!("module id `{}` must use a-z, 0-9 and _", m.id));
    }
    for c in module.commands() {
        if !c.id.starts_with(&format!("{}.", m.id)) {
            return Err(format!(
                "command `{}` of module `{}` must start with `{}.`",
                c.id, m.id, m.id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(&'static str, &'static str);

    impl Module for Fake {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest {
                id: self.0,
                name: "Fake",
                version: "0.1.0",
                capabilities: vec![Capability::FsRead],
            }
        }
        fn commands(&self) -> Vec<CommandSpec> {
            vec![CommandSpec::new(self.1, "Do", "Fake")]
        }
    }

    #[test]
    fn ids_and_prefixes_are_checked() {
        assert!(validate(&Fake("fake", "fake.do")).is_ok());
        assert!(validate(&Fake("Fake!", "fake.do")).is_err());
        assert!(validate(&Fake("fake", "other.do"))
            .unwrap_err()
            .contains("must start with"));
    }
}
