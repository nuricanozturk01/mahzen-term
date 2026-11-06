//! Layering rules from PLAN.md section 3.2.

use crate::graph::{transitive, Graph};

#[derive(Debug, PartialEq, Eq)]
pub struct Violation {
    pub pkg: String,
    pub dep: String,
    pub reason: String,
}

const CORE: &str = "mahzenterm-core";
const PLATFORM: &str = "mahzenterm-platform";
const TERM: &str = "mahzenterm-term";
const UI_KIT: &str = "mahzenterm-ui-kit";

/// Allowed internal (workspace) dependencies for a workspace-relative directory.
/// `None` means the directory is not governed by a rule.
fn allowed(dir: &str) -> Option<Vec<&'static str>> {
    match dir {
        "crates/core" | "crates/platform" => Some(vec![]),
        "crates/ui-kit" => Some(vec![CORE, TERM]),
        "crates/term" => Some(vec![CORE, PLATFORM]),
        "crates/app" => None, // checked separately: app may use every internal crate
        "xtask" => Some(vec![]),
        d if d.starts_with("modules/") => Some(vec![CORE, UI_KIT]),
        _ => None,
    }
}

const CORE_BANNED: [&str; 3] = ["iced", "wgpu", "winit"];

pub fn check(g: &Graph) -> Vec<Violation> {
    let mut out = Vec::new();
    let internal: std::collections::BTreeSet<&str> = g
        .pkgs
        .iter()
        .filter(|p| p.workspace)
        .map(|p| p.name.as_str())
        .collect();
    for p in g.pkgs.iter().filter(|p| p.workspace) {
        if p.dir == "crates/app" {
            continue; // app may depend on any internal crate
        }
        let Some(ok) = allowed(&p.dir) else { continue };
        for d in &p.deps {
            if internal.contains(d.as_str()) && !ok.contains(&d.as_str()) {
                out.push(Violation {
                    pkg: p.name.clone(),
                    dep: d.clone(),
                    reason: format!("`{}` may not depend on `{d}` (PLAN.md 3.2)", p.dir),
                });
            }
        }
        if p.name == CORE {
            let reach = transitive(g, &p.name);
            for b in CORE_BANNED {
                if reach.contains(b) {
                    out.push(Violation {
                        pkg: p.name.clone(),
                        dep: b.to_string(),
                        reason: "core must not reach UI crates".into(),
                    });
                }
            }
        }
    }
    out
}
