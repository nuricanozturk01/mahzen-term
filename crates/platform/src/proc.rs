//! Process table: which processes are alive and who descends from whom (to find the pane a program runs in).

use std::collections::HashMap;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// A snapshot of `pid -> parent pid` for every process.
#[derive(Debug, Clone, Default)]
pub struct ProcTable {
    parents: HashMap<u32, Option<u32>>,
    /// Process name and the first few command-line words, to recognise programs (`codex`, or `node .../codex`).
    names: HashMap<u32, Vec<String>>,
}

/// Longest ancestor chain followed (guards against cycles in odd process tables).
const MAX_DEPTH: usize = 64;

impl ProcTable {
    /// Read the process table now. Empty when the OS does not allow it.
    pub fn snapshot() -> Self {
        let Ok(mut sys) = System::new() else {
            return Self::default();
        };
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::OnlyIfNotSet),
        );
        let mut parents = HashMap::new();
        let mut names = HashMap::new();
        for (pid, p) in sys.processes() {
            let pid = pid.as_u32();
            parents.insert(pid, p.parent().map(sysinfo::Pid::as_u32));
            let mut words = vec![p.name().to_string_lossy().into_owned()];
            words.extend(
                p.cmd()
                    .iter()
                    .take(3)
                    .map(|c| c.to_string_lossy().into_owned()),
            );
            names.insert(pid, words);
        }
        Self { parents, names }
    }

    /// Build a table from explicit `(pid, parent)` pairs (tests and callers that already have the data).
    pub fn from_pairs(pairs: &[(u32, Option<u32>)]) -> Self {
        Self {
            parents: pairs.iter().copied().collect(),
            names: HashMap::new(),
        }
    }

    /// Same, with names: `(pid, parent, [name, arg1, arg2, ...])`.
    pub fn from_named(rows: &[(u32, Option<u32>, &[&str])]) -> Self {
        Self {
            parents: rows.iter().map(|(p, par, _)| (*p, *par)).collect(),
            names: rows
                .iter()
                .map(|(p, _, w)| (*p, w.iter().map(|s| (*s).to_owned()).collect()))
                .collect(),
        }
    }

    /// Pids of processes that run `program`: the process itself is called that, or a word of its command line is
    /// a path whose file name is that (a `node /usr/lib/.../codex` script). Sorted.
    pub fn find_program(&self, program: &str) -> Vec<u32> {
        let file = |w: &str| w.rsplit(['/', '\\']).next().unwrap_or(w).to_owned();
        let mut out: Vec<u32> = self
            .names
            .iter()
            .filter(|(_, words)| words.iter().any(|w| file(w) == program))
            .map(|(pid, _)| *pid)
            .collect();
        out.sort_unstable();
        out
    }

    pub fn is_alive(&self, pid: u32) -> bool {
        self.parents.contains_key(&pid)
    }

    /// `pid` itself followed by its parent, grandparent, ... (nearest first).
    pub fn lineage(&self, pid: u32) -> Vec<u32> {
        let mut out = vec![pid];
        let mut cur = pid;
        while out.len() < MAX_DEPTH {
            match self.parents.get(&cur).copied().flatten() {
                Some(p) if p != 0 && !out.contains(&p) => {
                    out.push(p);
                    cur = p;
                }
                _ => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lineage_walks_up_and_stops_at_cycles() {
        let t = ProcTable::from_pairs(&[
            (1, None),
            (10, Some(1)),
            (20, Some(10)),
            (30, Some(20)),
            (7, Some(8)),
            (8, Some(7)),
        ]);
        assert_eq!(t.lineage(30), [30, 20, 10, 1]);
        assert_eq!(t.lineage(999), [999], "unknown pids have no parents");
        assert_eq!(t.lineage(7), [7, 8], "a cycle ends the walk");
        assert!(t.is_alive(20) && !t.is_alive(21));
    }

    #[test]
    fn programs_are_found_by_name_or_script_path() {
        let t = ProcTable::from_named(&[
            (1, None, &["init"]),
            (10, Some(1), &["codex"]),
            (
                11,
                Some(1),
                &[
                    "node",
                    "/usr/lib/node_modules/@openai/codex/bin/codex",
                    "--x",
                ],
            ),
            (12, Some(1), &["codex-helper"]),
            (13, Some(1), &["bash", "-c", "echo codex"]),
        ]);
        assert_eq!(t.find_program("codex"), [10, 11]);
        assert!(t.find_program("nothing").is_empty());
    }

    #[test]
    fn the_real_table_knows_this_process_and_its_parent() {
        let t = ProcTable::snapshot();
        let me = std::process::id();
        assert!(t.is_alive(me));
        assert!(t.lineage(me).len() >= 2, "{:?}", t.lineage(me));
    }
}
