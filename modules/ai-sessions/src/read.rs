//! Reading Claude Code's live session records (`<claude dir>/sessions/<pid>.json`).
//!
//! Best effort: these files are an implementation detail of the tool and may change. Records are external input,
//! so they are size-limited and every field is optional.

use std::path::Path;

use serde_json::Value;

use crate::{Session, Status};

/// Largest record read, in bytes.
const MAX_FILE: u64 = 64 * 1024;
/// Most records read.
const MAX_FILES: usize = 200;

fn status_of(s: &str) -> Status {
    match s {
        "busy" => Status::Busy,
        "idle" => Status::Idle,
        "waiting" => Status::Waiting,
        "shell" => Status::Shell,
        _ => Status::Unknown,
    }
}

/// Parse one record. Only the user's own interactive chats count: background helpers, spare processes and
/// sessions hosted by another one are skipped.
pub fn parse_session(text: &str) -> Option<Session> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("kind").and_then(Value::as_str)? != "interactive" {
        return None;
    }
    if v.get("spare").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if v.get("hostSessionId")
        .and_then(Value::as_str)
        .is_some_and(|h| !h.is_empty())
    {
        return None;
    }
    let str_of = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let num = |k: &str| {
        v.get(k).and_then(|n| {
            n.as_u64()
                .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
        })
    };
    let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
    Some(Session {
        pid,
        id: str_of("sessionId").unwrap_or_default(),
        name: str_of("name").unwrap_or_default(),
        cwd: str_of("cwd").unwrap_or_default(),
        status: status_of(v.get("status").and_then(Value::as_str).unwrap_or("")),
        waiting_for: str_of("waitingFor").filter(|w| !w.is_empty()),
        updated_ms: num("statusUpdatedAt")
            .or_else(|| num("updatedAt"))
            .unwrap_or(0),
        lineage: Vec::new(),
    })
}

/// Live sessions below `claude_dir`; `alive` tells whether a pid still runs (stale records are dropped).
/// Most recently active first.
pub fn read_sessions(claude_dir: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<Session> {
    let Ok(rd) = std::fs::read_dir(claude_dir.join("sessions")) else {
        return Vec::new();
    };
    let mut out: Vec<Session> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .take(MAX_FILES)
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_FILE))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|t| parse_session(&t))
        .filter(|s| alive(s.pid))
        .collect();
    out.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then(a.pid.cmp(&b.pid)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(extra: &str) -> String {
        format!(
            r#"{{"pid":4242,"sessionId":"s1","cwd":"/work/app","kind":"interactive","name":"app-1a","status":"busy","statusUpdatedAt":1700000000000{extra}}}"#
        )
    }

    #[test]
    fn interactive_records_are_parsed() {
        let s = parse_session(&rec("")).unwrap();
        assert_eq!(
            (s.pid, s.id.as_str(), s.name.as_str()),
            (4242, "s1", "app-1a")
        );
        assert_eq!(s.status, Status::Busy);
        assert_eq!(s.cwd, "/work/app");
        assert_eq!(s.updated_ms, 1_700_000_000_000);
        let w = parse_session(&rec(r#","status":"waiting","waitingFor":"permission""#
            .replace(r#","status":"waiting""#, "")
            .as_str()));
        assert!(w.is_some());
    }

    #[test]
    fn helpers_spares_and_hosted_sessions_are_skipped() {
        assert!(parse_session(
            &rec(r#","kind":"background""#).replace(r#""kind":"interactive","#, "")
        )
        .is_none());
        assert!(parse_session(&rec(r#","spare":true"#)).is_none());
        assert!(parse_session(&rec(r#","hostSessionId":"other""#)).is_none());
        assert!(parse_session(&rec(r#","hostSessionId":"""#)).is_some());
        assert!(parse_session("not json").is_none());
        assert!(
            parse_session(r#"{"kind":"interactive"}"#).is_none(),
            "a pid is required"
        );
    }

    #[test]
    fn statuses_map_and_unknown_ones_survive() {
        let status = |s: &str| {
            parse_session(&format!(
                r#"{{"pid":1,"kind":"interactive","status":"{s}"}}"#
            ))
            .unwrap()
            .status
        };
        assert_eq!(status("idle"), Status::Idle);
        assert_eq!(status("waiting"), Status::Waiting);
        assert_eq!(status("shell"), Status::Shell);
        assert_eq!(status("brand-new"), Status::Unknown);
    }

    #[test]
    fn dead_pids_are_dropped_and_recent_sessions_come_first() {
        let d = tempfile::tempdir().unwrap();
        let sessions = d.path().join("sessions");
        std::fs::create_dir(&sessions).unwrap();
        let w = |pid: u32, at: u64| {
            std::fs::write(
                sessions.join(format!("{pid}.json")),
                format!(
                    r#"{{"pid":{pid},"kind":"interactive","status":"idle","statusUpdatedAt":{at}}}"#
                ),
            )
            .unwrap();
        };
        w(1, 100);
        w(2, 300);
        w(3, 200);
        std::fs::write(sessions.join("2.key"), "ignored").unwrap();
        std::fs::write(sessions.join("big.json"), vec![b' '; 70_000]).unwrap();
        let got = read_sessions(d.path(), &|pid| pid != 3);
        assert_eq!(got.iter().map(|s| s.pid).collect::<Vec<_>>(), [2, 1]);
        assert!(read_sessions(Path::new("/nonexistent"), &|_| true).is_empty());
    }
}
