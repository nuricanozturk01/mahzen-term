//! Project search: a bounded file index for "Go to File" and a text search across it ("Find in Files").
//!
//! Everything here is blocking and meant for a background task. Limits keep untrusted trees safe: file count,
//! depth, file size and hit count are capped, binary files are skipped, symlinked folders are not followed and a
//! user-supplied regular expression is compiled with a size limit.

use std::path::Path;

use mahzenterm_core::command::{fuzzy_score, CommandSpec};
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use regex::RegexBuilder;

/// Command ids of this module.
pub mod id {
    pub const QUICK_OPEN: &str = "search.quick_open";
    pub const IN_FILES: &str = "search.in_files";
}

/// Most files indexed.
pub const MAX_FILES: usize = 40_000;
const MAX_DEPTH: usize = 16;
/// Largest file searched for text (bigger ones are skipped).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Most hits returned.
pub const MAX_HITS: usize = 300;
/// Most results shown for file names.
pub const MAX_RESULTS: usize = 200;
/// Longest hit line kept.
const MAX_LINE: usize = 240;
/// Folders never indexed.
const NOISE: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    "venv",
    "vendor",
];

/// Relative paths (with `/`) of files below `root`, skipping VCS folders, build noise and dotfolders.
pub fn collect(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, "", 0, &mut out);
    out.sort_unstable();
    out
}

fn walk(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
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
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if name.starts_with('.') || NOISE.contains(&name.as_str()) {
                continue;
            }
            walk(&e.path(), &format!("{prefix}{name}/"), depth + 1, out);
        } else if ft.is_file() && !name.starts_with('.')
            || name == ".gitignore"
            || name == ".env.example"
        {
            out.push(format!("{prefix}{name}"));
        }
    }
}

/// Best file matches of `query`: a better fuzzy score first, shorter paths on ties; an empty query lists the start.
pub fn rank(index: &[String], query: &str) -> Vec<String> {
    let q = query.trim();
    if q.is_empty() {
        return index.iter().take(MAX_RESULTS).cloned().collect();
    }
    let mut scored: Vec<(i64, &String)> = index
        .iter()
        .filter_map(|p| {
            // Matching the file name counts more than matching the folders.
            let name = p.rsplit('/').next().unwrap_or(p);
            let by_name = fuzzy_score(q, name).map(|s| s + 40);
            let by_path = fuzzy_score(q, p);
            by_name.max(by_path).map(|s| (s, p))
        })
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

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Path relative to the search root.
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 1-based column of the match start.
    pub col: usize,
    /// The line, trimmed and cut.
    pub text: String,
}

/// What to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
}

/// Search the text of `files` (relative to `root`). Binary and oversized files are skipped. Returns at most
/// [`MAX_HITS`] hits in file order. An empty pattern gives no hits; a bad regular expression gives an error message.
pub fn grep(root: &Path, files: &[String], q: &Query) -> Result<Vec<Hit>, String> {
    if q.pattern.is_empty() {
        return Ok(Vec::new());
    }
    let source = if q.regex {
        q.pattern.clone()
    } else {
        regex::escape(&q.pattern)
    };
    let re = RegexBuilder::new(&source)
        .case_insensitive(!q.case_sensitive)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| {
            format!(
                "bad pattern: {}",
                e.to_string().lines().last().unwrap_or("invalid")
            )
        })?;
    let mut hits = Vec::new();
    'files: for rel in files {
        let path = root.join(rel);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES || meta.len() == 0 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.iter().take(8000).any(|b| *b == 0) {
            continue; // binary
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            if let Some(m) = re.find(line) {
                let col = line[..m.start()].chars().count() + 1;
                let trimmed = line.trim();
                let shown: String = trimmed.chars().take(MAX_LINE).collect();
                hits.push(Hit {
                    path: rel.clone(),
                    line: i + 1,
                    col,
                    text: shown,
                });
                if hits.len() >= MAX_HITS {
                    break 'files;
                }
            }
        }
    }
    Ok(hits)
}

/// The module registration.
pub struct ProjectSearchModule;

impl Module for ProjectSearchModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "search",
            name: "Project Search",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::FsRead],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(id::QUICK_OPEN, "Go to File...", "Search").key("Ctrl+Alt+P"),
            CommandSpec::new(id::IN_FILES, "Find in Files...", "Search").key("Ctrl+Alt+F"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let w = |p: &str, c: &[u8]| {
            let f = d.path().join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        };
        w("src/main.rs", b"fn main() {\n    println!(\"Hello\");\n}\n");
        w("src/lib.rs", b"pub fn hello() {}\n");
        w("README.md", b"# Title\nhello world\n");
        w("target/debug/x.rs", b"hello\n");
        w(".git/config", b"hello\n");
        w("img.bin", b"\0\0hello\0");
        d
    }

    #[test]
    fn index_skips_noise_and_hidden_folders() {
        let d = tree();
        let idx = collect(d.path());
        assert_eq!(idx, ["README.md", "img.bin", "src/lib.rs", "src/main.rs"]);
    }

    #[test]
    fn file_names_rank_above_folder_matches() {
        let idx: Vec<String> = ["docs/main/notes.txt", "src/main.rs", "src/domain.rs"]
            .map(String::from)
            .to_vec();
        assert_eq!(rank(&idx, "main")[0], "src/main.rs");
        assert!(rank(&idx, "zzz").is_empty());
        assert_eq!(rank(&idx, "").len(), 3);
    }

    #[test]
    fn grep_finds_lines_with_positions_and_skips_binaries() {
        let d = tree();
        let files = collect(d.path());
        let q = Query {
            pattern: "hello".into(),
            regex: false,
            case_sensitive: false,
        };
        let hits = grep(d.path(), &files, &q).unwrap();
        let at: Vec<(&str, usize)> = hits.iter().map(|h| (h.path.as_str(), h.line)).collect();
        assert_eq!(
            at,
            [("README.md", 2), ("src/lib.rs", 1), ("src/main.rs", 2)]
        );
        assert_eq!(hits[2].col, 15);
        let exact = Query {
            case_sensitive: true,
            ..q
        };
        assert_eq!(
            grep(d.path(), &files, &exact).unwrap().len(),
            2,
            "capital Hello is not hello"
        );
    }

    #[test]
    fn regex_mode_and_bad_patterns() {
        let d = tree();
        let files = collect(d.path());
        let re = Query {
            pattern: r"fn\s+\w+".into(),
            regex: true,
            case_sensitive: true,
        };
        assert_eq!(grep(d.path(), &files, &re).unwrap().len(), 2);
        let bad = Query {
            pattern: "(".into(),
            regex: true,
            case_sensitive: true,
        };
        assert!(grep(d.path(), &files, &bad)
            .unwrap_err()
            .starts_with("bad pattern"));
        // The same text in plain mode is just a character.
        let plain = Query {
            pattern: "(".into(),
            regex: false,
            case_sensitive: true,
        };
        assert_eq!(grep(d.path(), &files, &plain).unwrap().len(), 3);
        assert!(grep(
            d.path(),
            &files,
            &Query {
                pattern: String::new(),
                ..plain
            }
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn hits_are_capped() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "x\n".repeat(1000)).unwrap();
        let q = Query {
            pattern: "x".into(),
            regex: false,
            case_sensitive: true,
        };
        assert_eq!(
            grep(d.path(), &["a.txt".into()], &q).unwrap().len(),
            MAX_HITS
        );
    }

    #[test]
    fn module_is_valid() {
        mahzenterm_core::module::validate(&ProjectSearchModule).unwrap();
    }
}
