use xtask::graph::{Graph, Pkg};
use xtask::rules::check;

fn ws(name: &str, dir: &str, deps: &[&str]) -> Pkg {
    Pkg {
        name: name.into(),
        dir: dir.into(),
        workspace: true,
        deps: deps.iter().map(|s| s.to_string()).collect(),
    }
}
fn ext(name: &str, deps: &[&str]) -> Pkg {
    Pkg {
        name: name.into(),
        dir: String::new(),
        workspace: false,
        deps: deps.iter().map(|s| s.to_string()).collect(),
    }
}
fn base() -> Vec<Pkg> {
    vec![
        ws("mahzenterm-core", "crates/core", &[]),
        ws("mahzenterm-platform", "crates/platform", &[]),
        ws(
            "mahzenterm-ui-kit",
            "crates/ui-kit",
            &["mahzenterm-core", "mahzenterm-term"],
        ),
        ws(
            "mahzenterm-term",
            "crates/term",
            &["mahzenterm-core", "mahzenterm-platform"],
        ),
        ws(
            "mahzenterm-app",
            "crates/app",
            &[
                "mahzenterm-core",
                "mahzenterm-term",
                "mahzenterm-ui-kit",
                "mahzenterm-platform",
            ],
        ),
    ]
}
fn with(mut v: Vec<Pkg>, extra: Pkg) -> Graph {
    v.retain(|p| p.name != extra.name);
    v.push(extra);
    Graph { pkgs: v }
}

#[test]
fn clean_graph_has_no_violations() {
    assert!(check(&Graph { pkgs: base() }).is_empty());
}

#[test]
fn core_depending_on_term_is_violation() {
    let g = with(
        base(),
        ws("mahzenterm-core", "crates/core", &["mahzenterm-term"]),
    );
    let v = check(&g);
    assert_eq!(v.len(), 1);
    assert_eq!(
        (v[0].pkg.as_str(), v[0].dep.as_str()),
        ("mahzenterm-core", "mahzenterm-term")
    );
}

#[test]
fn core_transitively_reaching_iced_is_violation() {
    let mut pk = base();
    pk.retain(|p| p.name != "mahzenterm-core");
    pk.push(ws("mahzenterm-core", "crates/core", &["x"]));
    pk.push(ext("x", &["iced"]));
    pk.push(ext("iced", &[]));
    let v = check(&Graph { pkgs: pk });
    assert!(v.iter().any(|x| x.dep == "iced"));
}

#[test]
fn ui_kit_may_use_term_but_not_app() {
    let bad = with(
        base(),
        ws("mahzenterm-ui-kit", "crates/ui-kit", &["mahzenterm-app"]),
    );
    assert_eq!(check(&bad).len(), 1);
}

#[test]
fn module_may_use_core_and_ui_kit_only() {
    let ok = with(
        base(),
        ws(
            "mahzenterm-k8s",
            "modules/k8s",
            &["mahzenterm-core", "mahzenterm-ui-kit"],
        ),
    );
    assert!(check(&ok).is_empty());
    let bad = with(
        base(),
        ws("mahzenterm-k8s", "modules/k8s", &["mahzenterm-term"]),
    );
    assert_eq!(check(&bad).len(), 1);
    let mut pk = base();
    pk.push(ws("mahzenterm-a", "modules/a", &[]));
    pk.push(ws("mahzenterm-b", "modules/b", &["mahzenterm-a"]));
    assert_eq!(check(&Graph { pkgs: pk }).len(), 1);
}

#[test]
fn unknown_module_crate_is_governed_by_glob() {
    let g = with(
        base(),
        ws(
            "mahzenterm-brand-new",
            "modules/brand-new",
            &["mahzenterm-app"],
        ),
    );
    assert_eq!(check(&g).len(), 1);
}

#[test]
fn dev_dependency_edge_counts() {
    // dev/build edges are folded into `deps` by the loader.
    let g = with(
        base(),
        ws("mahzenterm-core", "crates/core", &["mahzenterm-app"]),
    );
    assert!(!check(&g).is_empty());
}

#[test]
fn external_deps_are_ignored_except_banned() {
    let mut pk = base();
    pk.retain(|p| p.name != "mahzenterm-core");
    pk.push(ws("mahzenterm-core", "crates/core", &["serde"]));
    pk.push(ext("serde", &[]));
    assert!(check(&Graph { pkgs: pk }).is_empty());
}

#[test]
fn real_workspace_has_no_violations() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let g = xtask::graph::load(&root).unwrap();
    assert_eq!(check(&g), vec![]);
}
