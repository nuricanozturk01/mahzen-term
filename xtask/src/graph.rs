//! Dependency graph model built from `cargo metadata`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One package in the graph.
#[derive(Debug, Clone)]
pub struct Pkg {
    pub name: String,
    /// Workspace-relative directory with `/` separators; empty for external packages.
    pub dir: String,
    pub workspace: bool,
    /// Direct dependency names: normal, dev and build edges.
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub pkgs: Vec<Pkg>,
}

/// All packages reachable from `name` (excluding `name` itself).
pub fn transitive(g: &Graph, name: &str) -> BTreeSet<String> {
    let index: BTreeMap<&str, &Pkg> = g.pkgs.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut seen = BTreeSet::new();
    let mut stack = vec![name.to_string()];
    while let Some(cur) = stack.pop() {
        let Some(pkg) = index.get(cur.as_str()) else {
            continue;
        };
        for d in &pkg.deps {
            if seen.insert(d.clone()) {
                stack.push(d.clone());
            }
        }
    }
    seen
}

/// Load the graph by running `cargo metadata` for the workspace at `workspace_root`.
pub fn load(workspace_root: &Path) -> anyhow::Result<Graph> {
    let meta = cargo_metadata::MetadataCommand::new()
        .manifest_path(workspace_root.join("Cargo.toml"))
        .exec()?;
    let ws_root = meta.workspace_root.as_std_path().to_path_buf();
    let members: BTreeSet<_> = meta.workspace_members.iter().collect();
    let pkgs = meta
        .packages
        .iter()
        .map(|p| {
            let workspace = members.contains(&p.id);
            let dir = if workspace {
                let manifest_dir = p
                    .manifest_path
                    .as_std_path()
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                manifest_dir
                    .strip_prefix(&ws_root)
                    .map(|r| {
                        r.components()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            };
            Pkg {
                name: p.name.to_string(),
                dir,
                workspace,
                deps: p.dependencies.iter().map(|d| d.name.clone()).collect(),
            }
        })
        .collect();
    Ok(Graph { pkgs })
}
