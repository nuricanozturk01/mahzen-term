//! Log summariser and pretty printer: coloured levels, JSON-lines logs shown as `time LEVEL message key=value`,
//! a count per level at the top and an optional minimum level. Works on plain text logs too.

use serde_json::Value;

use crate::ansi::{paint, Style, DIM, PLAIN};
use crate::Options;

/// Most lines read.
const MAX_LINES: usize = 200_000;

/// Severity, from most to least verbose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "trace" | "trc" | "verbose" => Some(Self::Trace),
            "debug" | "dbg" => Some(Self::Debug),
            "info" | "inf" | "notice" | "information" => Some(Self::Info),
            "warn" | "warning" | "wrn" => Some(Self::Warn),
            "error" | "err" | "fatal" | "panic" | "critical" | "crit" | "severe" => {
                Some(Self::Error)
            }
            _ => None,
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Trace | Self::Debug => DIM,
            Self::Info => Style::fg(2),
            Self::Warn => Style::fg(3).bold(),
            Self::Error => Style::fg(1).bold(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO ",
            Self::Warn => "WARN ",
            Self::Error => "ERROR",
        }
    }
}

/// One line understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub time: String,
    pub level: Option<Level>,
    pub message: String,
    /// Extra `key=value` fields of a JSON line.
    pub fields: Vec<(String, String)>,
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Understand one line: a JSON object (common field names for time, level and message) or plain text with a level word.
pub fn parse_line(line: &str) -> Entry {
    let trimmed = line.trim_start();
    if trimmed.starts_with('{') {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
            let pick = |keys: &[&str]| keys.iter().find_map(|k| map.get(*k)).map(text_of);
            let time = pick(&["time", "timestamp", "ts", "@timestamp", "t"]).unwrap_or_default();
            let level = pick(&["level", "lvl", "severity", "levelname", "log.level"])
                .and_then(|l| Level::parse(&l));
            let message = pick(&["msg", "message", "event", "text"]).unwrap_or_default();
            let used = [
                "time",
                "timestamp",
                "ts",
                "@timestamp",
                "t",
                "level",
                "lvl",
                "severity",
                "levelname",
                "log.level",
                "msg",
                "message",
                "event",
                "text",
            ];
            let fields = map
                .iter()
                .filter(|(k, _)| !used.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), text_of(v)))
                .collect();
            return Entry {
                time,
                level,
                message,
                fields,
            };
        }
    }
    // Plain text: the first word that is a level name (optionally in brackets) sets the level.
    let level = line
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
        .take(6)
        .find_map(Level::parse);
    Entry {
        time: String::new(),
        level,
        message: line.to_owned(),
        fields: Vec::new(),
    }
}

/// Render a log. `min` hides lines below a level; lines without a level are kept next to the line they follow
/// (stack traces) only while the previous shown line was shown.
pub fn render(text: &str, min: Option<Level>, opts: &Options) -> String {
    let mut counts = [0usize; 5];
    let mut other = 0usize;
    let mut body = String::new();
    let mut show_continuation = true;
    let mut total = 0usize;
    for line in text.lines().take(MAX_LINES) {
        total += 1;
        let e = parse_line(line);
        match e.level {
            Some(l) => counts[l as usize] += 1,
            None => other += 1,
        }
        let visible = match (e.level, min) {
            (Some(l), Some(m)) => l >= m,
            (None, Some(_)) => show_continuation,
            _ => true,
        };
        if e.level.is_some() {
            show_continuation = visible;
        }
        if !visible {
            continue;
        }
        if e.fields.is_empty() && e.time.is_empty() && e.level.is_none() {
            body.push_str(&paint(&e.message, PLAIN, opts.color));
        } else {
            if !e.time.is_empty() {
                body.push_str(&paint(&e.time, DIM, opts.color));
                body.push(' ');
            }
            if let Some(l) = e.level {
                body.push_str(&paint(l.label(), l.style(), opts.color));
                body.push(' ');
            }
            body.push_str(&paint(
                &e.message,
                e.level
                    .map_or(PLAIN, |l| if l >= Level::Warn { l.style() } else { PLAIN }),
                opts.color,
            ));
            for (k, v) in &e.fields {
                body.push(' ');
                body.push_str(&paint(&format!("{k}="), DIM, opts.color));
                body.push_str(v);
            }
        }
        body.push('\n');
    }
    let mut head = format!("{total} lines");
    for l in [
        Level::Error,
        Level::Warn,
        Level::Info,
        Level::Debug,
        Level::Trace,
    ] {
        let n = counts[l as usize];
        if n > 0 {
            head.push_str(&format!(
                " \u{b7} {} {}",
                n,
                l.label().trim().to_ascii_lowercase()
            ));
        }
    }
    if other > 0 {
        head.push_str(&format!(" \u{b7} {other} other"));
    }
    format!("{}\n{body}", paint(&head, DIM, opts.color))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Options {
        Options {
            width: 80,
            color: false,
            line_numbers: false,
        }
    }

    #[test]
    fn json_lines_are_flattened() {
        let e = parse_line(
            r#"{"time":"2026-11-06T10:00:00Z","level":"error","msg":"disk full","path":"/var","code":28}"#,
        );
        assert_eq!(
            (e.time.as_str(), e.level, e.message.as_str()),
            ("2026-11-06T10:00:00Z", Some(Level::Error), "disk full")
        );
        assert!(
            e.fields.contains(&("path".into(), "/var".into()))
                && e.fields.contains(&("code".into(), "28".into()))
        );
        let e = parse_line(r#"{"@timestamp":"t","severity":"WARNING","message":"slow"}"#);
        assert_eq!((e.level, e.message.as_str()), (Some(Level::Warn), "slow"));
        // A broken or non-object JSON line is plain text.
        assert_eq!(parse_line("{not json").level, None);
        assert_eq!(parse_line("[1,2]").message, "[1,2]");
    }

    #[test]
    fn plain_text_levels_are_found_in_the_first_words() {
        assert_eq!(
            parse_line("2026-11-06 12:00:00 ERROR cannot connect").level,
            Some(Level::Error)
        );
        assert_eq!(parse_line("[WARN] disk at 90%").level, Some(Level::Warn));
        assert_eq!(parse_line("INFO server started").level, Some(Level::Info));
        assert_eq!(
            parse_line("  at com.example.Foo.bar(Foo.java:12)").level,
            None
        );
        assert_eq!(
            parse_line("we saw nothing odd until much later when an error finally appeared").level,
            None
        );
    }

    #[test]
    fn summary_counts_and_minimum_level() {
        let log = "INFO start\nWARN careful\nERROR boom\n  at stack frame\nDEBUG noise\n";
        let all = render(log, None, &plain());
        assert!(
            all.starts_with(
                "5 lines \u{b7} 1 error \u{b7} 1 warn \u{b7} 1 info \u{b7} 1 debug \u{b7} 1 other"
            ),
            "{all}"
        );
        let warn = render(log, Some(Level::Warn), &plain());
        assert!(warn.contains("careful") && warn.contains("boom"));
        assert!(!warn.contains("start") && !warn.contains("noise"), "{warn}");
        assert!(
            warn.contains("at stack frame"),
            "a stack trace stays with its error: {warn}"
        );
        let err_only = render(
            "ERROR a\nWARN b\n  trace of b\n",
            Some(Level::Error),
            &plain(),
        );
        assert!(
            !err_only.contains("trace of b"),
            "and goes with a hidden warning: {err_only}"
        );
    }

    #[test]
    fn colours_mark_errors() {
        let c = render(
            "ERROR boom\n",
            None,
            &Options {
                color: true,
                ..plain()
            },
        );
        assert!(c.contains("\x1b[1;31m"), "{c:?}");
    }
}
