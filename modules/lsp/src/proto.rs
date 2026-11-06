//! LSP wire format: message framing, request bodies and answer parsing. Pure functions, easy to test.

use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::Event;

/// Largest message accepted from a server.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// Most diagnostics kept per file.
const MAX_DIAGS: usize = 500;

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A problem in a file. Lines are 0-based, columns are UTF-16 units as the server sent them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub severity: DiagSeverity,
    pub message: String,
}

/// A place in a file (0-based line, UTF-16 column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// `Content-Length` framing.
pub fn frame(msg: &Value) -> Vec<u8> {
    let body = msg.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Read one framed message. `Ok(None)` at end of stream or on a message that cannot be accepted.
pub fn read_message<R: BufRead>(r: &mut R) -> std::io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse().ok();
        }
    }
    let Some(len) = len.filter(|l| *l <= MAX_MESSAGE) else {
        return Ok(None);
    };
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body).ok().or(Some(Value::Null)))
}

/// Percent-encode a path into a `file://` URI.
pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The file path of a `file://` URI (`None` for other schemes or bad escapes).
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = rest.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

/// UTF-16 column of character column `col` in `line`.
pub fn utf16_col(line: &str, col: usize) -> usize {
    line.chars().take(col).map(char::len_utf16).sum()
}

/// Character column of UTF-16 column `utf16` in `line` (clamped to the line end).
pub fn char_col(line: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in line.chars().enumerate() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    line.chars().count()
}

pub(crate) fn initialize_params(root: &Path) -> Value {
    json!({
        "processId": std::process::id(),
        "rootUri": path_to_uri(root),
        "workspaceFolders": [{"uri": path_to_uri(root), "name": root.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())}],
        "capabilities": {
            "textDocument": {
                "synchronization": {"didSave": false},
                "publishDiagnostics": {"relatedInformation": false},
                "definition": {"linkSupport": false}
            }
        }
    })
}

pub(crate) fn did_open(path: &Path, lang: &str, text: &str) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path), "languageId": lang, "version": 1, "text": text}})
}

pub(crate) fn did_change(path: &Path, version: i64, text: &str) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path), "version": version}, "contentChanges": [{"text": text}]})
}

pub(crate) fn position_params(path: &Path, line: usize, utf16: usize) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path)}, "position": {"line": line, "character": utf16}})
}

fn position(v: &Value) -> Option<(usize, usize)> {
    Some((
        usize::try_from(v.get("line")?.as_u64()?).ok()?,
        usize::try_from(v.get("character")?.as_u64()?).ok()?,
    ))
}

fn severity(n: Option<u64>) -> DiagSeverity {
    match n {
        Some(1) => DiagSeverity::Error,
        Some(2) => DiagSeverity::Warning,
        Some(3) => DiagSeverity::Info,
        Some(4) => DiagSeverity::Hint,
        _ => DiagSeverity::Warning,
    }
}

/// Parse a `publishDiagnostics` notification's params.
pub fn parse_diagnostics(params: &Value) -> Option<(PathBuf, Vec<Diag>)> {
    let path = uri_to_path(params.get("uri")?.as_str()?)?;
    let diags = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .filter_map(|d| {
            let r = d.get("range")?;
            let (line, start) = position(r.get("start")?)?;
            let (end_line, end) = position(r.get("end")?)?;
            Some(Diag {
                line,
                start,
                // Multi-line ranges are underlined to the end of their first line only.
                end: if end_line == line { end } else { usize::MAX },
                severity: severity(d.get("severity").and_then(Value::as_u64)),
                message: d
                    .get("message")?
                    .as_str()?
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned(),
            })
        })
        .take(MAX_DIAGS)
        .collect();
    Some((path, diags))
}

/// Parse the result of a definition request (a location, a list of them, or location links).
pub fn parse_locations(result: &Value) -> Vec<Location> {
    let one = |v: &Value| -> Option<Location> {
        let uri = v.get("uri").or_else(|| v.get("targetUri"))?.as_str()?;
        let range = v.get("range").or_else(|| v.get("targetSelectionRange"))?;
        let (line, col) = position(range.get("start")?)?;
        Some(Location {
            path: uri_to_path(uri)?,
            line,
            col,
        })
    };
    match result {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        Value::Object(_) => one(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// One completion offered by a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    /// Short extra text (a type or signature), may be empty.
    pub detail: String,
    /// Text to insert (snippet placeholders already reduced to plain text).
    pub insert: String,
}

/// Most completions kept from one answer.
const MAX_COMPLETIONS: usize = 80;
/// Most hover text kept (characters).
const MAX_HOVER: usize = 1200;

/// Reduce an LSP snippet to plain text: `$0`, `$1` vanish, `${1:name}` becomes `name`, `\$` is a dollar sign.
pub fn plain_snippet(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '$' if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() => {
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '{' => {
                // `${1:default}` keeps the default, `${1}` keeps nothing.
                let mut j = i + 2;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ':' {
                    j += 1;
                    let mut depth = 1;
                    while j < chars.len() {
                        match chars[j] {
                            '{' => {
                                depth += 1;
                                out.push('{');
                            }
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                                out.push('}');
                            }
                            c => out.push(c),
                        }
                        j += 1;
                    }
                } else {
                    while j < chars.len() && chars[j] != '}' {
                        j += 1;
                    }
                }
                i = j + 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Parse a completion answer (a list, or `{ items: [...] }`).
pub fn parse_completions(result: &Value) -> Vec<Completion> {
    let items = match result {
        Value::Array(a) => a.as_slice(),
        Value::Object(o) => o
            .get("items")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice),
        _ => &[],
    };
    items
        .iter()
        .filter_map(|it| {
            let label = it.get("label")?.as_str()?.trim().to_owned();
            if label.is_empty() {
                return None;
            }
            let raw = it
                .get("textEdit")
                .and_then(|e| e.get("newText"))
                .and_then(Value::as_str)
                .or_else(|| it.get("insertText").and_then(Value::as_str))
                .unwrap_or(&label);
            let snippet = it.get("insertTextFormat").and_then(Value::as_u64) == Some(2);
            let insert = if snippet {
                plain_snippet(raw)
            } else {
                raw.to_owned()
            };
            let detail = it
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_owned();
            Some(Completion {
                label,
                detail,
                insert,
            })
        })
        .take(MAX_COMPLETIONS)
        .collect()
}

/// Parse a hover answer into plain text lines: markdown fences and decoration are dropped.
pub fn parse_hover(result: &Value) -> Vec<String> {
    fn text_of(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Object(o) => o
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            Value::Array(a) => a.iter().map(text_of).collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        }
    }
    let raw = result.get("contents").map(text_of).unwrap_or_default();
    let mut out = Vec::new();
    let mut chars = 0usize;
    for line in raw.lines() {
        let l = line.trim_end();
        if l.trim_start().starts_with("```") || l.trim() == "---" {
            continue;
        }
        let l: String = l
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect();
        chars += l.chars().count();
        if chars > MAX_HOVER {
            break;
        }
        out.push(l.replace('\t', "    "));
    }
    while out.first().is_some_and(|l| l.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

/// Turn a message from the server into an app event, if it is one we use.
pub(crate) fn event_from(lang: &str, msg: &Value) -> Option<Event> {
    if msg.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics") {
        let (path, diags) = parse_diagnostics(msg.get("params")?)?;
        return Some(Event::Diagnostics {
            lang: lang.to_owned(),
            path: path.to_string_lossy().into_owned(),
            diags,
        });
    }
    // An answer to one of our requests (a server's own requests carry a `method` and are ignored).
    if msg.get("method").is_some() {
        return None;
    }
    let id = msg.get("id")?.as_i64()?;
    let result = msg.get("result").cloned().unwrap_or(Value::Null);
    Some(Event::Response {
        lang: lang.to_owned(),
        request: id,
        result,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn framing_round_trips_and_rejects_oversized_messages() {
        let msg = json!({"jsonrpc": "2.0", "id": 1, "result": null});
        let mut r = Cursor::new(frame(&msg));
        assert_eq!(read_message(&mut r).unwrap(), Some(msg));
        assert_eq!(read_message(&mut r).unwrap(), None, "end of stream");
        let huge = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE + 1);
        assert_eq!(read_message(&mut Cursor::new(huge)).unwrap(), None);
        assert_eq!(
            read_message(&mut Cursor::new("X: 1\r\n\r\n")).unwrap(),
            None,
            "no length"
        );
        let junk = "Content-Length: 3\r\n\r\n{{{";
        assert_eq!(
            read_message(&mut Cursor::new(junk)).unwrap(),
            Some(Value::Null),
            "bad json is skipped, not fatal"
        );
    }

    #[test]
    fn uris_round_trip_with_spaces_and_unicode() {
        let p = Path::new("/home/me/my proj/şifre.rs");
        let u = path_to_uri(p);
        assert_eq!(u, "file:///home/me/my%20proj/%C5%9Fifre.rs");
        assert_eq!(uri_to_path(&u).as_deref(), Some(p));
        assert_eq!(uri_to_path("http://x"), None);
        assert_eq!(uri_to_path("file:///a%zz"), None);
    }

    #[test]
    fn columns_convert_between_chars_and_utf16() {
        let line = "a😀b";
        assert_eq!(utf16_col(line, 2), 3);
        assert_eq!(char_col(line, 3), 2);
        assert_eq!(char_col(line, 1), 1);
        assert_eq!(char_col(line, 99), 3, "past the end is clamped");
    }

    #[test]
    fn diagnostics_are_parsed_defensively() {
        let params = json!({"uri": "file:///p/a.rs", "diagnostics": [
            {"range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 9}}, "severity": 1, "message": "oops\nmore"},
            {"range": {"start": {"line": 5, "character": 0}, "end": {"line": 7, "character": 1}}, "message": "multi"},
            {"range": "bad"},
            {"message": "no range"}
        ]});
        let (path, d) = parse_diagnostics(&params).unwrap();
        assert_eq!(path, Path::new("/p/a.rs"));
        assert_eq!(d.len(), 2);
        assert_eq!(
            (d[0].line, d[0].start, d[0].end, d[0].severity),
            (2, 4, 9, DiagSeverity::Error)
        );
        assert_eq!(d[0].message, "oops");
        assert_eq!(d[1].end, usize::MAX);
        assert_eq!(d[1].severity, DiagSeverity::Warning);
        assert!(parse_diagnostics(&json!({"uri": "http://x", "diagnostics": []})).is_none());
    }

    #[test]
    fn definition_answers_come_in_three_shapes() {
        let loc = json!({"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}}});
        assert_eq!(parse_locations(&loc).len(), 1);
        assert_eq!(parse_locations(&json!([loc.clone(), loc.clone()])).len(), 2);
        let link = json!([{"targetUri": "file:///b.rs", "targetSelectionRange": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 1}}}]);
        assert_eq!(parse_locations(&link)[0].line, 3);
        assert!(parse_locations(&Value::Null).is_empty());
    }

    #[test]
    fn events_are_picked_out_of_server_messages() {
        let n = json!({"method": "textDocument/publishDiagnostics", "params": {"uri": "file:///a.rs", "diagnostics": []}});
        assert!(matches!(
            event_from("rust", &n),
            Some(Event::Diagnostics { .. })
        ));
        let answer = json!({"id": 4, "result": [1]});
        assert!(matches!(
            event_from("rust", &answer),
            Some(Event::Response { request: 4, .. })
        ));
        let none = json!({"id": 5, "result": null});
        assert!(matches!(
            event_from("rust", &none),
            Some(Event::Response { request: 5, .. })
        ));
        let own = json!({"id": 6, "method": "window/workDoneProgress/create", "params": {}});
        assert_eq!(
            event_from("rust", &own),
            None,
            "a request from the server is not an answer"
        );
    }

    #[test]
    fn snippets_become_plain_text() {
        assert_eq!(
            plain_snippet("println!(\"${1:msg}\")$0"),
            "println!(\"msg\")"
        );
        assert_eq!(
            plain_snippet("fn ${1:name}(${2:a: u8}) {\n\t$0\n}"),
            "fn name(a: u8) {\n\t\n}"
        );
        assert_eq!(plain_snippet("cost \\$5 ${3}x"), "cost $5 x");
        assert_eq!(plain_snippet("${1:a{b}c}"), "a{b}c");
        assert_eq!(plain_snippet("plain"), "plain");
    }

    #[test]
    fn completions_come_as_a_list_or_an_object_with_items() {
        let list = json!([
            {"label": "push", "detail": "fn(&mut self, T)\nmore", "insertText": "push(${1:value})", "insertTextFormat": 2},
            {"label": "len", "textEdit": {"newText": "len()", "range": {}}},
            {"label": "plain"},
            {"label": ""},
            {"nolabel": 1}
        ]);
        let c = parse_completions(&list);
        assert_eq!(c.len(), 3);
        assert_eq!(
            (c[0].insert.as_str(), c[0].detail.as_str()),
            ("push(value)", "fn(&mut self, T)")
        );
        assert_eq!(c[1].insert, "len()");
        assert_eq!(c[2].insert, "plain");
        assert_eq!(
            parse_completions(&json!({"isIncomplete": false, "items": list})).len(),
            3
        );
        assert!(parse_completions(&Value::Null).is_empty());
        let many: Vec<Value> = (0..500)
            .map(|i| json!({"label": format!("c{i}")}))
            .collect();
        assert_eq!(
            parse_completions(&Value::Array(many)).len(),
            MAX_COMPLETIONS
        );
    }

    #[test]
    fn hover_text_is_reduced_to_plain_lines() {
        let h = json!({"contents": {"kind": "markdown", "value": "```rust\nfn main()\n```\n---\nEntry point.\n"}});
        assert_eq!(parse_hover(&h), ["fn main()", "Entry point."]);
        assert_eq!(
            parse_hover(&json!({"contents": "just text"})),
            ["just text"]
        );
        assert_eq!(
            parse_hover(&json!({"contents": [{"language": "rust", "value": "x: u8"}, "doc"]})),
            ["x: u8", "doc"]
        );
        assert!(parse_hover(&Value::Null).is_empty());
        let long = "a".repeat(5000);
        assert!(
            parse_hover(&json!({"contents": long})).is_empty(),
            "one huge line is dropped, not shown"
        );
    }
}
