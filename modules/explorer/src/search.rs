//! Fuzzy file search: a bounded file index (built off the UI thread) ranked with the shared fuzzy scorer.

use std::path::Path;

use mahzenterm_core::command::fuzzy_score;

use crate::NOISE;

/// Most files indexed.
pub const MAX_FILES: usize = 40_000;
/// Deepest folder level indexed.
const MAX_DEPTH: usize = 14;
/// Most results shown.
pub const MAX_RESULTS: usize = 200;

/// Relative paths (with `/`) of files below `root`, skipping build noise (and dotfiles unless `show_hidden`).
/// Blocking: call it off the UI thread. Symlinked folders are not followed.
pub fn collect(root: &Path, show_hidden: bool) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, "", 0, show_hidden, &mut out);
    out.sort_unstable();
    out
}

fn walk(dir: &Path, prefix: &str, depth: usize, show_hidden: bool, out: &mut Vec<String>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if !show_hidden && (name.starts_with('.') || NOISE.contains(&name.as_str())) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        let rel = format!("{prefix}{name}");
        if ft.is_dir() {
            walk(&e.path(), &format!("{rel}/"), depth + 1, show_hidden, out);
        } else {
            out.push(rel);
        }
    }
}

/// Best matches of `query` in `index`: higher score first, shorter paths on ties. An empty query lists the start.
pub fn rank(index: &[String], query: &str) -> Vec<String> {
    let q = query.trim();
    if q.is_empty() {
        return index.iter().take(MAX_RESULTS).cloned().collect();
    }
    let mut scored: Vec<(i64, &String)> = index
        .iter()
        .filter_map(|p| fuzzy_score(q, p).map(|s| (s, p)))
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.len().cmp(&b.1.len()))
            .then(a.1.cmp(b.1))
    });
    scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, p)| p.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn index_skips_noise_hidden_and_lists_nested_files() {
        let d = tempfile::tempdir().unwrap();
        for f in [
            "src/main.rs",
            "src/ui/view.rs",
            "target/debug/x",
            ".git/config",
            "README.md",
            ".env",
        ] {
            let p = d.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        }
        assert_eq!(
            collect(d.path(), false),
            ["README.md", "src/main.rs", "src/ui/view.rs"]
        );
        let all = collect(d.path(), true);
        assert!(all.contains(&".env".to_owned()) && all.contains(&"target/debug/x".to_owned()));
    }

    #[test]
    fn ranking_prefers_tight_matches_and_short_paths() {
        let idx: Vec<String> = [
            "docs/guide/viewer.md",
            "crates/ui/view.rs",
            "view.rs",
            "src/overview.txt",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let r = rank(&idx, "view.rs");
        assert_eq!(r[0], "view.rs");
        assert!(r.contains(&"crates/ui/view.rs".to_owned()));
        assert!(rank(&idx, "zzzz").is_empty());
        assert_eq!(rank(&idx, "  ").len(), 4);
    }
}
