//! Reading the tools' session logs: line parsers, an incremental file tailer and the summary.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::{Record, Snapshot, Tool, Usage};

/// Records older than this are dropped (enough for "today" in any time zone).
const KEEP_SECS: i64 = 36 * 3600;
/// Most bytes read from one file per poll, so a huge log cannot stall a scan.
const CHUNK: u64 = 16 * 1024 * 1024;
/// Deepest directory level searched below a root.
const MAX_DEPTH: usize = 5;
/// Most files followed at once.
const MAX_FILES: usize = 2000;

fn unix(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.timestamp())
}

fn num(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// A Claude Code transcript line: `assistant` entries carry `message.usage`.
pub fn parse_claude_line(line: &str) -> Option<Record> {
    if !line.contains("\"usage\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let msg = v.get("message")?;
    let u = msg.get("usage")?;
    let id = msg
        .get("id")
        .or_else(|| v.get("uuid"))?
        .as_str()?
        .to_owned();
    Some(Record {
        tool: Tool::Claude,
        id,
        at: unix(v.get("timestamp")?.as_str()?)?,
        usage: Usage {
            input: num(u, "input_tokens"),
            output: num(u, "output_tokens"),
            cache_read: num(u, "cache_read_input_tokens"),
            cache_write: num(u, "cache_creation_input_tokens"),
        },
    })
}

/// A Codex rollout line: `token_usage_record` carries the usage of one response.
pub fn parse_codex_line(line: &str) -> Option<Record> {
    if !line.contains("token_usage_record") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "token_usage_record" {
        return None;
    }
    let p = v.get("payload")?;
    let u = p.get("usage")?;
    let id = p.get("response_id")?.as_str()?.to_owned();
    let cached = num(u, "cached_input_tokens");
    Some(Record {
        tool: Tool::Codex,
        id,
        at: unix(v.get("timestamp")?.as_str()?)?,
        usage: Usage {
            // Codex counts cached tokens inside `input_tokens`.
            input: num(u, "input_tokens").saturating_sub(cached),
            output: num(u, "output_tokens"),
            cache_read: cached,
            cache_write: num(u, "cache_write_input_tokens"),
        },
    })
}

#[derive(Default)]
struct FileState {
    offset: u64,
    /// Bytes of an unfinished last line.
    partial: Vec<u8>,
}

/// Follows the session logs and keeps the recent records.
pub struct Scanner {
    claude: Option<PathBuf>,
    codex: Option<PathBuf>,
    files: HashMap<PathBuf, FileState>,
    seen: HashSet<(Tool, String)>,
    records: Vec<Record>,
}

impl Scanner {
    /// `claude` is the Claude config dir (holds `projects/`), `codex` the Codex dir (holds `sessions/`).
    pub fn new(claude: Option<PathBuf>, codex: Option<PathBuf>) -> Self {
        Self {
            claude,
            codex,
            files: HashMap::new(),
            seen: HashSet::new(),
            records: Vec::new(),
        }
    }

    /// Default locations: `CLAUDE_CONFIG_DIR` or `~/.claude`, and `CODEX_HOME` or `~/.codex`.
    pub fn default_dirs(home: Option<PathBuf>) -> (Option<PathBuf>, Option<PathBuf>) {
        let env = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        (
            env("CLAUDE_CONFIG_DIR").or_else(|| home.as_ref().map(|h| h.join(".claude"))),
            env("CODEX_HOME").or_else(|| home.map(|h| h.join(".codex"))),
        )
    }

    /// Read what was appended since the last call. Returns true when new records arrived.
    pub fn poll(&mut self, now: i64) -> bool {
        let cutoff = now - KEEP_SECS;
        let mut found: Vec<(PathBuf, Tool)> = Vec::new();
        if let Some(root) = &self.claude {
            collect(&root.join("projects"), 0, cutoff, Tool::Claude, &mut found);
        }
        if let Some(root) = &self.codex {
            collect(&root.join("sessions"), 0, cutoff, Tool::Codex, &mut found);
        }
        found.truncate(MAX_FILES);
        let live: HashSet<&PathBuf> = found.iter().map(|(p, _)| p).collect();
        self.files.retain(|p, _| live.contains(p));
        let mut changed = false;
        for (path, tool) in &found {
            let st = self.files.entry(path.clone()).or_default();
            for line in read_new_lines(path, st) {
                let rec = match tool {
                    Tool::Claude => parse_claude_line(&line),
                    Tool::Codex => parse_codex_line(&line),
                };
                if let Some(r) = rec.filter(|r| r.at >= cutoff) {
                    if self.seen.insert((r.tool, r.id.clone())) {
                        self.records.push(r);
                        changed = true;
                    }
                }
            }
        }
        let before = self.records.len();
        self.records.retain(|r| r.at >= cutoff);
        if self.records.len() != before {
            self.seen
                .retain(|(t, id)| self.records.iter().any(|r| r.tool == *t && &r.id == id));
        }
        changed
    }

    /// Sums as of `now`; `day_start` is the unix time of local midnight.
    pub fn snapshot(&self, now: i64, day_start: i64) -> Snapshot {
        let mut snap = Snapshot::default();
        let mut recent = [Usage::default(); 2];
        for r in &self.records {
            let (s, idx) = match r.tool {
                Tool::Claude => (&mut snap.claude, 0),
                Tool::Codex => (&mut snap.codex, 1),
            };
            s.last_at = s.last_at.max(Some(r.at));
            if r.at >= day_start {
                s.today.add(&r.usage);
                s.responses_today += 1;
            }
            if r.at >= now - 3600 {
                s.last_hour.add(&r.usage);
            }
            if r.at >= now - 300 {
                recent[idx].add(&r.usage);
            }
        }
        let per_min = |u: &Usage| u.billable() as f64 / 5.0;
        snap.claude.per_min = per_min(&recent[0]);
        snap.codex.per_min = per_min(&recent[1]);
        snap
    }
}

fn mtime(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_secs()).ok()
}

/// Recent `.jsonl` files below `dir`.
fn collect(dir: &Path, depth: usize, cutoff: i64, tool: Tool, out: &mut Vec<(PathBuf, Tool)>) {
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
            collect(&path, depth + 1, cutoff, tool, out);
        } else if path.extension().is_some_and(|x| x == "jsonl")
            && mtime(&path).is_some_and(|m| m >= cutoff)
        {
            out.push((path, tool));
        }
    }
}

/// Complete new lines of `path` since `st.offset`; a shrunk file is read again from the start.
fn read_new_lines(path: &Path, st: &mut FileState) -> Vec<String> {
    let Ok(mut f) = File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    if len < st.offset {
        *st = FileState::default();
    }
    if len == st.offset || f.seek(SeekFrom::Start(st.offset)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.take(CHUNK).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    st.offset += buf.len() as u64;
    let mut data = std::mem::take(&mut st.partial);
    data.extend_from_slice(&buf);
    let mut lines = Vec::new();
    let mut start = 0;
    while let Some(i) = data[start..].iter().position(|b| *b == b'\n') {
        lines.push(String::from_utf8_lossy(&data[start..start + i]).into_owned());
        start += i + 1;
    }
    st.partial = data[start..].to_vec();
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const NOW: i64 = 1_793_600_000;

    fn ts(at: i64) -> String {
        chrono::DateTime::from_timestamp(at, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    fn claude_line(id: &str, at: i64, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"u-{id}","timestamp":"{}","message":{{"id":"{id}","usage":{{"input_tokens":2,"output_tokens":{out},"cache_read_input_tokens":100,"cache_creation_input_tokens":10}}}}}}"#,
            ts(at)
        )
    }

    fn codex_line(id: &str, at: i64) -> String {
        format!(
            r#"{{"type":"token_usage_record","timestamp":"{}","payload":{{"response_id":"{id}","usage":{{"input_tokens":1000,"cached_input_tokens":400,"cache_write_input_tokens":0,"output_tokens":50}}}}}}"#,
            ts(at)
        )
    }

    #[test]
    fn claude_usage_is_read_from_assistant_lines_only() {
        let r = parse_claude_line(&claude_line("m1", NOW, 30)).unwrap();
        assert_eq!(r.tool, Tool::Claude);
        assert_eq!(r.id, "m1");
        assert_eq!(r.at, NOW);
        assert_eq!(
            r.usage,
            Usage {
                input: 2,
                output: 30,
                cache_read: 100,
                cache_write: 10
            }
        );
        assert!(parse_claude_line(r#"{"type":"user","usage":1}"#).is_none());
        assert!(parse_claude_line("not json \"usage\"").is_none());
    }

    #[test]
    fn codex_cached_tokens_are_split_out_of_input() {
        let r = parse_codex_line(&codex_line("resp_1", NOW)).unwrap();
        assert_eq!(r.usage.input, 600);
        assert_eq!(r.usage.cache_read, 400);
        assert_eq!(r.usage.output, 50);
        assert!(parse_codex_line(r#"{"type":"event_msg"}"#).is_none());
    }

    fn write(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    #[test]
    fn scanner_sums_windows_and_drops_duplicates() {
        let d = tempfile::tempdir().unwrap();
        let claude = d.path().join("c");
        let codex = d.path().join("x");
        // The same message id appears in two transcripts (a resumed session).
        let now = std::time::SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        write(
            &claude.join("projects/p1/a.jsonl"),
            &[
                claude_line("m1", now - 60, 30),
                claude_line("m2", now - 7200, 20),
            ],
        );
        write(
            &claude.join("projects/p2/b.jsonl"),
            &[claude_line("m1", now - 60, 30)],
        );
        write(
            &codex.join("sessions/2026/11/06/r.jsonl"),
            &[codex_line("r1", now - 10)],
        );
        let mut s = Scanner::new(Some(claude), Some(codex));
        assert!(s.poll(now));
        let snap = s.snapshot(now, now - 86_400);
        assert_eq!(snap.claude.responses_today, 2, "m1 counted once");
        assert_eq!(snap.claude.today.output, 50);
        assert_eq!(snap.claude.last_hour.output, 30);
        assert!(snap.claude.per_min > 0.0);
        assert_eq!(snap.codex.today.input, 600);
        // Only messages after local midnight count as today.
        let snap = s.snapshot(now, now - 100);
        assert_eq!(snap.claude.responses_today, 1);
    }

    #[test]
    fn appended_lines_are_read_once_and_partial_lines_wait() {
        let d = tempfile::tempdir().unwrap();
        let claude = d.path().to_path_buf();
        let path = claude.join("projects/p/a.jsonl");
        let now = std::time::SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        write(&path, &[claude_line("m1", now, 5)]);
        let mut s = Scanner::new(Some(claude), None);
        assert!(s.poll(now));
        assert!(!s.poll(now), "nothing new");
        let second = claude_line("m2", now, 7);
        let (head, tail) = second.split_at(40);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(f, "{head}").unwrap();
        assert!(!s.poll(now), "half a line is not a record yet");
        writeln!(f, "{tail}").unwrap();
        assert!(s.poll(now));
        assert_eq!(s.snapshot(now, now - 10).claude.today.output, 12);
    }

    #[test]
    fn missing_dirs_are_fine() {
        let mut s = Scanner::new(Some("/nonexistent/x".into()), None);
        assert!(!s.poll(NOW));
        assert_eq!(s.snapshot(NOW, 0), Snapshot::default());
    }
}
