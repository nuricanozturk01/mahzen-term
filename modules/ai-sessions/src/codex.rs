//! Codex sessions. Codex has no per-process status file, so this reads its rollout logs
//! (`<codex dir>/sessions/YYYY/MM/DD/rollout-*.jsonl`): the first line names the folder, the end of the file says
//! whether a turn is running. A running Codex process is matched to the rollout of its working folder.
//!
//! Best effort and heuristic: the log format is the tool's own business. "Needs you" is a guess: a tool call that
//! got no answer for a while usually waits for an approval.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::{Session, Status, Tool};

/// Bytes read from the end of a rollout.
const TAIL: u64 = 128 * 1024;
/// Rollouts older than this are not live sessions.
const MAX_AGE_MS: u64 = 6 * 3600 * 1000;
/// A tool call without an answer for this long is taken as an approval prompt.
const WAITING_AFTER_MS: u64 = 8_000;
/// A turn that is still "open" but silent for this long is a dead session.
const STALE_MS: u64 = 10 * 60 * 1000;
const MAX_FILES: usize = 200;
const MAX_DEPTH: usize = 5;

/// What the end of a rollout says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// No turn is running.
    Idle,
    /// A turn runs and the model is working.
    Running,
    /// A turn runs and the last thing is a tool call without a result.
    ToolPending,
}

/// A rollout file read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rollout {
    pub path: PathBuf,
    pub id: String,
    pub cwd: String,
    pub turn: Turn,
    pub modified_ms: u64,
}

/// The folder and id from the first line (`session_meta`).
pub fn parse_head(line: &str) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let p = v.get("payload")?;
    let cwd = p.get("cwd")?.as_str()?.to_owned();
    let id = p
        .get("id")
        .or_else(|| p.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Some((id, cwd))
}

/// Walk the lines at the end of a rollout and decide what is going on.
pub fn parse_turn(tail: &str) -> Turn {
    let (mut open, mut pending) = (false, false);
    for line in tail.lines() {
        if !(line.contains("task_") || line.contains("turn_aborted") || line.contains("_call")) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = v
            .pointer("/payload/type")
            .and_then(Value::as_str)
            .unwrap_or("");
        match (v.get("type").and_then(Value::as_str).unwrap_or(""), kind) {
            ("event_msg", "task_started") => {
                open = true;
                pending = false;
            }
            ("event_msg", "task_complete" | "turn_aborted") => {
                open = false;
                pending = false;
            }
            ("response_item", "custom_tool_call" | "function_call") => pending = true,
            ("response_item", "custom_tool_call_output" | "function_call_output") => {
                pending = false
            }
            _ => {}
        }
    }
    match (open, pending) {
        (false, _) => Turn::Idle,
        (true, false) => Turn::Running,
        (true, true) => Turn::ToolPending,
    }
}

/// The user-facing status for a rollout last written at `modified_ms`.
pub fn status(turn: Turn, modified_ms: u64, now_ms: u64) -> Status {
    let quiet = now_ms.saturating_sub(modified_ms);
    match turn {
        Turn::Idle => Status::Idle,
        _ if quiet > STALE_MS => Status::Idle,
        Turn::ToolPending if quiet >= WAITING_AFTER_MS => Status::Waiting,
        _ => Status::Busy,
    }
}

fn read_ends(path: &Path) -> Option<(String, String)> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = String::new();
    {
        let mut buf = vec![0u8; 64 * 1024];
        let n = f.read(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf[..n]);
        head.push_str(text.lines().next().unwrap_or(""));
    }
    let start = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = String::new();
    let mut raw = Vec::new();
    f.take(TAIL).read_to_end(&mut raw).ok()?;
    tail.push_str(&String::from_utf8_lossy(&raw));
    if start > 0 {
        // The first line is probably cut in the middle.
        tail = tail
            .split_once('\n')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default();
    }
    Some((head, tail))
}

fn ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

fn collect(dir: &Path, depth: usize, min_mtime: u64, out: &mut Vec<(PathBuf, u64)>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            collect(&path, depth + 1, min_mtime, out);
        } else if path.extension().is_some_and(|x| x == "jsonl") {
            let m = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map_or(0, ms);
            if m >= min_mtime {
                out.push((path, m));
            }
        }
    }
}

/// Recent rollouts below `<codex_dir>/sessions`, newest first.
pub fn scan(codex_dir: &Path, now_ms: u64) -> Vec<Rollout> {
    let mut files = Vec::new();
    collect(
        &codex_dir.join("sessions"),
        0,
        now_ms.saturating_sub(MAX_AGE_MS),
        &mut files,
    );
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files
        .into_iter()
        .filter_map(|(path, modified_ms)| {
            let (head, tail) = read_ends(&path)?;
            let (id, cwd) = parse_head(&head)?;
            Some(Rollout {
                turn: parse_turn(&tail),
                path,
                id,
                cwd,
                modified_ms,
            })
        })
        .collect()
}

/// A running Codex process: its pid, working folder (when the OS told) and ancestors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub cwd: Option<String>,
    pub lineage: Vec<u32>,
}

/// Match processes to rollouts by working folder: the most recently written rollout of a folder goes to the first
/// process there, the next one to the next process, so two Codex windows in one folder get two rows.
pub fn sessions(procs: &[Proc], rollouts: &[Rollout], now_ms: u64) -> Vec<Session> {
    let mut used = vec![false; rollouts.len()];
    let mut out = Vec::new();
    for p in procs {
        let Some(cwd) = p.cwd.as_deref() else {
            continue;
        };
        let pick = rollouts
            .iter()
            .enumerate()
            .find(|(i, r)| !used[*i] && r.cwd == cwd);
        let (status, id, updated_ms) = match pick {
            Some((i, r)) => {
                used[i] = true;
                (
                    status(r.turn, r.modified_ms, now_ms),
                    r.id.clone(),
                    r.modified_ms,
                )
            }
            // A Codex process that has not written a rollout yet (just started).
            None => (Status::Idle, String::new(), 0),
        };
        let leaf = cwd.rsplit('/').find(|s| !s.is_empty()).unwrap_or(cwd);
        out.push(Session {
            pid: p.pid,
            id,
            name: format!("codex {leaf}"),
            cwd: cwd.to_owned(),
            status,
            waiting_for: (status == Status::Waiting).then(|| "approval?".to_owned()),
            updated_ms,
            lineage: p.lineage.clone(),
            tool: Tool::Codex,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn ev(kind: &str, payload: &str) -> String {
        format!(r#"{{"type":"{kind}","payload":{{"type":"{payload}"}}}}"#)
    }

    #[test]
    fn the_head_names_folder_and_id() {
        let line = r#"{"type":"session_meta","payload":{"id":"abc","cwd":"/work/app"}}"#;
        assert_eq!(parse_head(line), Some(("abc".into(), "/work/app".into())));
        assert!(parse_head(r#"{"type":"event_msg"}"#).is_none());
        assert!(parse_head("junk").is_none());
    }

    #[test]
    fn the_tail_tells_idle_running_and_pending() {
        let lines = |v: &[String]| v.join("\n");
        let started = ev("event_msg", "task_started");
        let done = ev("event_msg", "task_complete");
        let call = ev("response_item", "custom_tool_call");
        let out = ev("response_item", "custom_tool_call_output");
        assert_eq!(parse_turn(""), Turn::Idle);
        assert_eq!(
            parse_turn(&lines(std::slice::from_ref(&started))),
            Turn::Running
        );
        assert_eq!(
            parse_turn(&lines(&[started.clone(), call.clone()])),
            Turn::ToolPending
        );
        assert_eq!(
            parse_turn(&lines(&[started.clone(), call.clone(), out.clone()])),
            Turn::Running
        );
        assert_eq!(
            parse_turn(&lines(&[started.clone(), call.clone(), done.clone()])),
            Turn::Idle
        );
        assert_eq!(
            parse_turn(&lines(&[started, call, ev("event_msg", "turn_aborted")])),
            Turn::Idle,
            "an aborted turn is over"
        );
        // Junk lines are skipped.
        assert_eq!(parse_turn("not json task_started\n{broken"), Turn::Idle);
    }

    #[test]
    fn status_follows_time() {
        let now = 1_000_000;
        assert_eq!(status(Turn::Idle, now, now), Status::Idle);
        assert_eq!(status(Turn::Running, now - 2_000, now), Status::Busy);
        assert_eq!(
            status(Turn::ToolPending, now - 2_000, now),
            Status::Busy,
            "a fresh tool call is just work"
        );
        assert_eq!(status(Turn::ToolPending, now - 9_000, now), Status::Waiting);
        assert_eq!(
            status(Turn::ToolPending, now - STALE_MS - 1, now),
            Status::Idle,
            "a silent open turn is dead"
        );
    }

    fn write_rollout(dir: &Path, name: &str, cwd: &str, events: &[String]) {
        let day = dir.join("sessions/2026/11/06");
        std::fs::create_dir_all(&day).unwrap();
        let mut f = std::fs::File::create(day.join(name)).unwrap();
        writeln!(
            f,
            r#"{{"type":"session_meta","payload":{{"id":"{name}","cwd":"{cwd}"}}}}"#
        )
        .unwrap();
        for e in events {
            writeln!(f, "{e}").unwrap();
        }
    }

    #[test]
    fn rollouts_are_scanned_and_matched_to_processes_by_folder() {
        let d = tempfile::tempdir().unwrap();
        let started = ev("event_msg", "task_started");
        write_rollout(
            d.path(),
            "a.jsonl",
            "/work/app",
            &[started.clone(), ev("response_item", "function_call")],
        );
        write_rollout(
            d.path(),
            "b.jsonl",
            "/work/other",
            &[started, ev("event_msg", "task_complete")],
        );
        let now = ms(SystemTime::now()) + 20_000; // the tool call has waited 20 s
        let rollouts = scan(d.path(), now);
        assert_eq!(rollouts.len(), 2);
        let procs = [
            Proc {
                pid: 11,
                cwd: Some("/work/app".into()),
                lineage: vec![11, 5],
            },
            Proc {
                pid: 12,
                cwd: Some("/work/other".into()),
                lineage: vec![12, 5],
            },
            Proc {
                pid: 13,
                cwd: Some("/work/app".into()),
                lineage: vec![13, 5],
            },
            Proc {
                pid: 14,
                cwd: None,
                lineage: vec![14],
            },
        ];
        let got = sessions(&procs, &rollouts, now);
        assert_eq!(got.len(), 3, "a process without a known folder is skipped");
        let by = |pid: u32| got.iter().find(|s| s.pid == pid).unwrap();
        assert_eq!(by(11).status, Status::Waiting);
        assert_eq!(by(11).waiting_for.as_deref(), Some("approval?"));
        assert_eq!(by(11).name, "codex app");
        assert_eq!(by(11).tool, Tool::Codex);
        assert_eq!(by(12).status, Status::Idle);
        // The second process in the same folder has no rollout of its own: shown idle, not a copy of the first.
        assert_eq!(by(13).status, Status::Idle);
        assert!(by(13).id.is_empty());
    }

    #[test]
    fn a_huge_log_is_read_only_at_its_ends() {
        let d = tempfile::tempdir().unwrap();
        let day = d.path().join("sessions/2026/11/06");
        std::fs::create_dir_all(&day).unwrap();
        let mut f = std::fs::File::create(day.join("big.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"session_meta","payload":{{"id":"big","cwd":"/w"}}}}"#
        )
        .unwrap();
        for _ in 0..4000 {
            writeln!(f, "{}", ev("response_item", "message")).unwrap();
        }
        writeln!(f, "{}", ev("event_msg", "task_started")).unwrap();
        drop(f);
        let r = scan(d.path(), ms(SystemTime::now()));
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].cwd.as_str(), r[0].turn), ("/w", Turn::Running));
        assert!(scan(Path::new("/nonexistent"), 0).is_empty());
    }
}
