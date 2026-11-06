use std::path::PathBuf;
use std::process::ExitCode;

use xtask::{graph, rules};

fn workspace_root() -> PathBuf {
    // xtask lives at <root>/xtask, independent of the current directory.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check-deps") => match graph::load(&workspace_root()) {
            Ok(g) => {
                let v = rules::check(&g);
                if v.is_empty() {
                    println!("check-deps: ok");
                    ExitCode::SUCCESS
                } else {
                    for x in &v {
                        eprintln!("{} -> {}: {}", x.pkg, x.dep, x.reason);
                    }
                    ExitCode::FAILURE
                }
            }
            Err(e) => {
                eprintln!("xtask: {e:#}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: cargo xtask check-deps");
            ExitCode::FAILURE
        }
    }
}
