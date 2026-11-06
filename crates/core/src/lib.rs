//! UI-independent core: event bus, command registry, config, workspace model.

pub mod bars;
pub mod bus;
pub mod command;
pub mod config;
pub mod module;
pub mod workspace;

/// Product name.
pub const NAME: &str = "MahzenTerm";

/// Product tagline.
pub const SLOGAN: &str = "A terminal with depth";

/// Crate version, exposed for the UI shell.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_is_exposed() {
        assert_eq!(NAME, "MahzenTerm");
        assert_eq!(SLOGAN, "A terminal with depth");
    }

    #[test]
    fn version_is_exposed() {
        assert!(!VERSION.is_empty());
    }
}
