//! Kubernetes: context and namespace at a glance, a pod browser, logs and shells in panes.
//!
//! The first slice drives the user's own `kubectl` (read-only queries as JSON; logs and shells run in panes so
//! the user watches them). That keeps the user's auth plugins working, adds no heavy client dependency, and never
//! reads credentials (`kubectl config view` redacts them). Destructive actions are not offered; switching to or
//! opening a shell on a production context asks first.

mod kubectl;
mod panel;
mod parse;
mod view;

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use serde::Deserialize;

pub use kubectl::{
    config_view, describe, exec_shell, get_namespaces, get_pods, logs, run, safe_arg,
    set_namespace, use_context, Scope,
};
pub use panel::{Action, Confirm, Item, Key, Load, Mode, Outcome, Panel, Tone, VISIBLE_ROWS};
pub use parse::{
    age_label, parse_config, parse_namespaces, parse_pods, Context, Health, KubeConfig, Pod,
};
pub use view::view;

/// Command ids of this module.
pub mod id {
    pub const PODS: &str = "kube.pods";
    pub const CONTEXTS: &str = "kube.contexts";
    pub const NAMESPACES: &str = "kube.namespaces";
    pub const REFRESH: &str = "kube.refresh";
}

/// `[modules.kubernetes]` in the config.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Program to run (a bare name looked up on `PATH`).
    pub kubectl: String,
    /// A context whose name contains any of these (case-insensitive) is treated as production.
    pub prod_patterns: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            kubectl: "kubectl".into(),
            prod_patterns: vec!["prod".into(), "production".into(), "prd".into()],
        }
    }
}

impl Settings {
    /// Whether `context` looks like production.
    pub fn is_prod(&self, context: &str) -> bool {
        let c = context.to_ascii_lowercase();
        self.prod_patterns
            .iter()
            .filter(|p| !p.trim().is_empty())
            .any(|p| c.contains(&p.to_ascii_lowercase()))
    }

    /// The program name, falling back to `kubectl` when the setting is unusable.
    pub fn program(&self) -> &str {
        if self.kubectl.contains(['/', '\\']) || !safe_arg(&self.kubectl) {
            "kubectl"
        } else {
            &self.kubectl
        }
    }
}

/// The module registration.
pub struct KubernetesModule;

impl Module for KubernetesModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "kube",
            name: "Kubernetes",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::ProcessSpawn, Capability::Pty],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(id::PODS, "Kubernetes: Pods", "Kubernetes").key("Ctrl+Alt+K"),
            CommandSpec::new(id::CONTEXTS, "Kubernetes: Switch Context", "Kubernetes"),
            CommandSpec::new(id::NAMESPACES, "Kubernetes: Switch Namespace", "Kubernetes"),
            CommandSpec::new(
                id::REFRESH,
                "Kubernetes: Refresh Context Info",
                "Kubernetes",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_is_matched_by_name_pattern() {
        let s = Settings::default();
        assert!(s.is_prod("gke_acme_prod-eu"));
        assert!(s.is_prod("Production"));
        assert!(!s.is_prod("dev-cluster"));
        let custom = Settings {
            prod_patterns: vec!["live".into(), " ".into()],
            ..Settings::default()
        };
        assert!(custom.is_prod("eks-live-1") && !custom.is_prod("eks-prod-1"));
        assert!(!Settings {
            prod_patterns: Vec::new(),
            ..Settings::default()
        }
        .is_prod("prod"));
    }

    #[test]
    fn the_program_setting_cannot_point_at_a_path_or_option() {
        assert_eq!(Settings::default().program(), "kubectl");
        for bad in ["/tmp/evil", "..\\evil", "-v", "a b", ""] {
            let s = Settings {
                kubectl: bad.into(),
                ..Settings::default()
            };
            assert_eq!(s.program(), "kubectl", "{bad:?}");
        }
        let ok = Settings {
            kubectl: "k3s-kubectl".into(),
            ..Settings::default()
        };
        assert_eq!(ok.program(), "k3s-kubectl");
    }

    #[test]
    fn module_and_settings_are_valid() {
        mahzenterm_core::module::validate(&KubernetesModule).unwrap();
        assert!(serde_json::from_str::<Settings>(r#"{"bogus":1}"#).is_err());
    }
}
