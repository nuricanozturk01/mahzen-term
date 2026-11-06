//! TUI store: a catalog of terminal apps with confirmed, visible installation.
//!
//! The catalog is data (embedded TOML plus user files). The module only decides *what* to run (the install
//! command for this platform) and keeps the browsing state; the app shows the command, asks, and runs it in a pane.

mod catalog;
mod install;
mod store;
mod view;

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};

pub use catalog::{load, parse_app, App, CatalogError, InstallSpec, Loaded, Platforms};
pub use install::{find_program, is_installed, plan_install, InstallPlan, Manager};
pub use store::{Entry, Launch, Mode, Outcome, Store, StoreKey, VISIBLE_ROWS};
pub use view::view;

/// Command ids of this module.
pub mod id {
    pub const OPEN: &str = "store.open";
}

/// The module registration.
pub struct TuiStoreModule;

impl Module for TuiStoreModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "store",
            name: "TUI store",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![
                Capability::ProcessSpawn,
                Capability::Pty,
                Capability::FsRead,
            ],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![CommandSpec::new(id::OPEN, "Open TUI Store", "Store").key("Ctrl+Alt+S")]
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_is_valid() {
        mahzenterm_core::module::validate(&super::TuiStoreModule).unwrap();
    }
}
