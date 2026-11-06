//! Process table: which processes are alive and who descends from whom (to find the pane a program runs in).

use std::collections::HashMap;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// A snapshot of `pid -> parent pid` for every process.
#[derive(Debug, Clone, Default)]
pub struct ProcTable {
    parents: HashMap<u32, Option<u32>>,
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
            ProcessRefreshKind::nothing(),
        );
        Self {
            parents: sys
                .processes()
                .iter()
                .map(|(pid, p)| (pid.as_u32(), p.parent().map(sysinfo::Pid::as_u32)))
                .collect(),
        }
    }

    /// Build a table from explicit `(pid, parent)` pairs (tests and callers that already have the data).
    pub fn from_pairs(pairs: &[(u32, Option<u32>)]) -> Self {
        Self {
            parents: pairs.iter().copied().collect(),
        }
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
    fn the_real_table_knows_this_process_and_its_parent() {
        let t = ProcTable::snapshot();
        let me = std::process::id();
        assert!(t.is_alive(me));
        assert!(t.lineage(me).len() >= 2, "{:?}", t.lineage(me));
    }
}
