//! Language servers for editor panes (opt-in, `[modules.lsp.servers.<language>]`): diagnostics underline problems,
//! F12 jumps to a definition. A server starts the first time a file of its language opens and stops with the app.
//! Edits are sent as whole-text changes after a short pause in typing.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::futures::channel::mpsc;
use iced::Task;
use mahzenterm_editor::{CompletionItem, Diagnostic, Severity};
use mahzenterm_lsp as lsp;

use super::finder::project_root;
use super::{Message, PaneKey, Shell};

/// Pause after the last edit before the new text is sent.
const FLUSH_AFTER: Duration = Duration::from_millis(350);

pub(super) type EventRx = Arc<Mutex<Option<mpsc::UnboundedReceiver<lsp::Event>>>>;

/// What a pending request was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ask {
    Definition,
    Hover,
    Completion,
}

/// Per-file sync state.
#[derive(Debug, Default)]
pub(super) struct Doc {
    pub version: i64,
    /// Hash of the text the server last saw.
    pub sent: u64,
    pub flush_pending: bool,
}

fn hash_text(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

fn severity(s: lsp::DiagSeverity) -> Severity {
    match s {
        lsp::DiagSeverity::Error => Severity::Error,
        lsp::DiagSeverity::Warning => Severity::Warning,
        lsp::DiagSeverity::Info => Severity::Info,
        lsp::DiagSeverity::Hint => Severity::Hint,
    }
}

impl Shell {
    /// Tell a language server about a file that just opened (starting the server on first use).
    pub(super) fn lsp_attach(&mut self, key: PaneKey) {
        let Some(p) = self.editors.get(&key) else {
            return;
        };
        let lang = p.state.lang;
        let Some(path) = p.state.buffer.path().map(Path::to_path_buf) else {
            return;
        };
        let Some(server) = self.lsp_settings.servers.get(lang).cloned() else {
            return;
        };
        if !self.lsp_clients.contains_key(lang) {
            let root = project_root(path.parent().unwrap_or(Path::new(".")));
            match lsp::Client::start(lang, &server, &root, self.lsp_tx.clone()) {
                Ok(c) => {
                    self.lsp_clients.insert(lang.to_owned(), c);
                }
                Err(e) => {
                    if let Some(p) = self.editors.get_mut(&key) {
                        p.state.notice = Some(e);
                    }
                    return;
                }
            }
        }
        let Some(client) = self.lsp_clients.get(lang) else {
            return;
        };
        let text = self.editors[&key].state.buffer.text();
        client.did_open(&path, &text);
        self.lsp_docs.insert(
            key,
            Doc {
                version: 1,
                sent: hash_text(&text),
                flush_pending: false,
            },
        );
    }

    /// After a message: schedule a send for every synced editor whose text may have changed.
    pub(super) fn lsp_sync(&mut self) -> Task<Message> {
        if self.lsp_docs.is_empty() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        let keys: Vec<PaneKey> = self.lsp_docs.keys().copied().collect();
        for key in keys {
            let Some(p) = self.editors.get(&key) else {
                self.lsp_docs.remove(&key);
                continue;
            };
            let Some(doc) = self.lsp_docs.get_mut(&key) else {
                continue;
            };
            // The draw revision changes with every edit (and with cursor moves; the hash check filters those).
            let _ = p.state.revision;
            if doc.flush_pending {
                continue;
            }
            doc.flush_pending = true;
            tasks.push(Task::perform(
                async {
                    std::thread::sleep(FLUSH_AFTER);
                },
                move |()| Message::LspFlush(key),
            ));
        }
        Task::batch(tasks)
    }

    /// Send the current text when it differs from what the server has.
    pub(super) fn on_lsp_flush(&mut self, key: PaneKey) -> Task<Message> {
        let (Some(p), Some(doc)) = (self.editors.get(&key), self.lsp_docs.get_mut(&key)) else {
            return Task::none();
        };
        doc.flush_pending = false;
        let text = p.state.buffer.text();
        let hash = hash_text(&text);
        if hash != doc.sent {
            doc.sent = hash;
            doc.version += 1;
            if let (Some(path), Some(client)) =
                (p.state.buffer.path(), self.lsp_clients.get(p.state.lang))
            {
                client.did_change(path, doc.version, &text);
            }
        }
        Task::none()
    }

    /// F12 (definition), hover and completion: ask the server about the symbol at the cursor.
    pub(super) fn lsp_request(&mut self, key: PaneKey, ask: Ask) {
        let Some(p) = self.editors.get_mut(&key) else {
            return;
        };
        let lang = p.state.lang;
        let (Some(client), Some(path)) = (
            self.lsp_clients.get(lang),
            p.state.buffer.path().map(Path::to_path_buf),
        ) else {
            p.state.notice = Some(format!(
                "no language server for {lang}: set [modules.lsp.servers.{lang}] command"
            ));
            return;
        };
        let (line, col) = p.state.buffer.line_col(p.state.buffer.cursor());
        let text = p.state.buffer.line(line);
        // Make sure the server has the latest text before asking.
        let all = p.state.buffer.text();
        if let Some(doc) = self.lsp_docs.get_mut(&key) {
            let hash = hash_text(&all);
            if hash != doc.sent {
                doc.sent = hash;
                doc.version += 1;
                client.did_change(&path, doc.version, &all);
            }
        }
        let at = lsp::utf16_col(&text, col);
        let id = match ask {
            Ask::Definition => client.definition(&path, line, at),
            Ask::Hover => client.hover(&path, line, at),
            Ask::Completion => client.completion(&path, line, at),
        };
        self.lsp_pending.insert((lang.to_owned(), id), (key, ask));
        p.state.notice = Some("asking the language server...".into());
    }

    /// An answer arrived: show it where it was asked for (the file may have been closed meanwhile).
    fn on_lsp_response(&mut self, lang: String, request: i64, result: &serde_json::Value) {
        let Some((key, ask)) = self.lsp_pending.remove(&(lang, request)) else {
            return;
        };
        if let Some(p) = self.editors.get_mut(&key) {
            p.state.notice = None;
        }
        match ask {
            Ask::Definition => {
                let locations = lsp::parse_locations(result);
                match locations.first() {
                    Some(loc) => {
                        let text = std::fs::read_to_string(&loc.path)
                            .ok()
                            .and_then(|t| t.lines().nth(loc.line).map(str::to_owned))
                            .unwrap_or_default();
                        let col = lsp::char_col(&text, loc.col) + 1;
                        let path = loc.path.clone();
                        self.open_editor_file(&path, Some((loc.line + 1, col)));
                    }
                    None => {
                        if let Some(p) = self.editors.get_mut(&key) {
                            p.state.notice = Some("no definition found".into());
                        }
                    }
                }
            }
            Ask::Hover => {
                if let Some(p) = self.editors.get_mut(&key) {
                    p.state.show_hover(lsp::parse_hover(result));
                }
            }
            Ask::Completion => {
                if let Some(p) = self.editors.get_mut(&key) {
                    let items = lsp::parse_completions(result)
                        .into_iter()
                        .map(|c| CompletionItem {
                            label: c.label,
                            detail: c.detail,
                            insert: c.insert,
                        })
                        .collect();
                    p.state.show_completions(items);
                }
            }
        }
    }

    pub(super) fn on_lsp_event(&mut self, ev: lsp::Event) {
        match ev {
            lsp::Event::Diagnostics { path, diags, .. } => {
                let path = PathBuf::from(path);
                let keys: Vec<PaneKey> = self
                    .editors
                    .iter()
                    .filter(|(_, p)| p.state.buffer.path().is_some_and(|b| b == path))
                    .map(|(k, _)| *k)
                    .collect();
                for k in keys {
                    if let Some(p) = self.editors.get_mut(&k) {
                        p.state.diagnostics = diags
                            .iter()
                            .map(|d| {
                                let text = p.state.buffer.line(d.line);
                                Diagnostic {
                                    line: d.line,
                                    start_col: lsp::char_col(&text, d.start),
                                    end_col: if d.end == usize::MAX {
                                        text.chars().count()
                                    } else {
                                        lsp::char_col(&text, d.end)
                                    },
                                    severity: severity(d.severity),
                                    message: d.message.clone(),
                                }
                            })
                            .collect();
                        p.state.revision = p.state.revision.wrapping_add(1);
                    }
                }
            }
            lsp::Event::Response {
                lang,
                request,
                result,
            } => self.on_lsp_response(lang, request, &result),
            lsp::Event::Exited { lang } => {
                self.lsp_clients.remove(&lang);
                self.lsp_docs
                    .retain(|k, _| self.editors.get(k).is_none_or(|p| p.state.lang != lang));
                for p in self.editors.values_mut().filter(|p| p.state.lang == lang) {
                    p.state.notice = Some(format!("the {lang} language server stopped"));
                    p.state.diagnostics.clear();
                }
            }
        }
    }

    /// Stop every server and start fresh for the open files (palette: "Language Servers: Restart").
    pub(super) fn lsp_restart(&mut self) {
        self.lsp_clients.clear();
        self.lsp_docs.clear();
        self.lsp_pending.clear();
        let keys: Vec<PaneKey> = self.editors.keys().copied().collect();
        for k in keys {
            self.lsp_attach(k);
        }
    }
}

pub(super) fn event_stream(
    src: &super::LspSource,
) -> impl iced::futures::Stream<Item = Message> + use<> {
    use iced::futures::StreamExt;
    let rx = src.0.lock().ok().and_then(|mut g| g.take());
    iced::futures::stream::iter(rx).flatten().map(Message::Lsp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(s: &mut Shell, dir: &Path, name: &str, text: &str) -> (PaneKey, PathBuf) {
        let file = dir.join(name);
        std::fs::write(&file, text).unwrap();
        s.open_editor_file(&file, None);
        s.queue_editor_loads();
        let (key, p) = s.editor_queue.pop().unwrap();
        let loaded = std::sync::Arc::new(std::sync::Mutex::new(Some(
            mahzenterm_editor::Buffer::open(&p).map_err(|e| e.to_string()),
        )));
        s.on_editor_loaded(key, &loaded);
        (key, std::fs::canonicalize(file).unwrap())
    }

    #[test]
    fn diagnostics_underline_the_right_columns_even_after_wide_characters() {
        let d = tempfile::tempdir().unwrap();
        let mut s = super::super::tests::shell();
        let (key, path) = open(&mut s, d.path(), "a.rs", "let \u{1f600}x = 1;\n");
        // The server counts the emoji as two units: `x` is at UTF-16 column 7, character column 6.
        s.on_lsp_event(lsp::Event::Diagnostics {
            lang: "rust".into(),
            path: path.to_string_lossy().into_owned(),
            diags: vec![lsp::Diag {
                line: 0,
                start: 7,
                end: 8,
                severity: lsp::DiagSeverity::Error,
                message: "bad".into(),
            }],
        });
        let d = &s.editors[&key].state.diagnostics;
        assert_eq!(
            (d[0].start_col, d[0].end_col, d[0].severity),
            (6, 7, Severity::Error)
        );
        // Other files are untouched.
        s.on_lsp_event(lsp::Event::Diagnostics {
            lang: "rust".into(),
            path: "/nowhere.rs".into(),
            diags: Vec::new(),
        });
        assert_eq!(s.editors[&key].state.diagnostics.len(), 1);
    }

    #[test]
    fn a_definition_answer_opens_the_target_at_its_position() {
        let d = tempfile::tempdir().unwrap();
        let mut s = super::super::tests::shell();
        let (key, _) = open(&mut s, d.path(), "a.rs", "fn a() {}\n");
        let target = d.path().join("b.rs");
        std::fs::write(&target, "// one\nfn b() {}\n").unwrap();
        s.lsp_pending
            .insert(("rust".into(), 7), (key, Ask::Definition));
        let tabs = s.window.tabs.len();
        let loc = serde_json::json!({"uri": lsp::path_to_uri(&target), "range": {"start": {"line": 1, "character": 3}, "end": {"line": 1, "character": 4}}});
        s.on_lsp_event(lsp::Event::Response {
            lang: "rust".into(),
            request: 7,
            result: loc,
        });
        assert_eq!(s.window.tabs.len(), tabs + 1);
        assert!(s.lsp_pending.is_empty());
        // An answer nobody asked for is ignored.
        s.on_lsp_event(lsp::Event::Response {
            lang: "rust".into(),
            request: 99,
            result: serde_json::Value::Null,
        });
    }

    #[test]
    fn hover_and_completion_answers_reach_the_editor() {
        let d = tempfile::tempdir().unwrap();
        let mut s = super::super::tests::shell();
        let (key, _) = open(&mut s, d.path(), "a.rs", "let x = ve");
        s.editors.get_mut(&key).unwrap().state.goto(1, 11);
        s.lsp_pending.insert(("rust".into(), 1), (key, Ask::Hover));
        s.on_lsp_event(lsp::Event::Response { lang: "rust".into(), request: 1, result: serde_json::json!({"contents": {"kind": "markdown", "value": "```rust\nVec<T>\n```"}}) });
        assert_eq!(
            s.editors[&key].state.hover.as_deref(),
            Some(&["Vec<T>".to_owned()][..])
        );
        s.lsp_pending
            .insert(("rust".into(), 2), (key, Ask::Completion));
        s.on_lsp_event(lsp::Event::Response {
            lang: "rust".into(),
            request: 2,
            result: serde_json::json!([{"label": "vec!", "insertText": "vec![${1}]", "insertTextFormat": 2}, {"label": "other"}]),
        });
        let c = s.editors[&key].state.completion.as_ref().unwrap();
        assert_eq!(c.items().next().unwrap().insert, "vec![]");
        // The pane closed before the answer came: nothing breaks.
        let gone = (
            s.window.active_tab().id,
            mahzenterm_core::workspace::PaneId(9999),
        );
        s.lsp_pending.insert(("rust".into(), 3), (gone, Ask::Hover));
        s.on_lsp_event(lsp::Event::Response {
            lang: "rust".into(),
            request: 3,
            result: serde_json::Value::Null,
        });
    }

    #[test]
    fn without_a_configured_server_f12_explains_what_to_set() {
        let d = tempfile::tempdir().unwrap();
        let mut s = super::super::tests::shell();
        let (key, _) = open(&mut s, d.path(), "a.rs", "fn a() {}\n");
        s.lsp_request(key, Ask::Definition);
        assert!(s.editors[&key]
            .state
            .notice
            .as_deref()
            .unwrap()
            .contains("modules.lsp.servers.rust"));
    }

    #[test]
    fn a_crashed_server_is_forgotten_and_the_editor_says_so() {
        let d = tempfile::tempdir().unwrap();
        let mut s = super::super::tests::shell();
        let (key, _) = open(&mut s, d.path(), "a.rs", "x\n");
        s.editors
            .get_mut(&key)
            .unwrap()
            .state
            .diagnostics
            .push(Diagnostic {
                line: 0,
                start_col: 0,
                end_col: 1,
                severity: Severity::Error,
                message: "m".into(),
            });
        s.on_lsp_event(lsp::Event::Exited {
            lang: "rust".into(),
        });
        assert!(s.editors[&key].state.diagnostics.is_empty());
        assert!(s.editors[&key]
            .state
            .notice
            .as_deref()
            .unwrap()
            .contains("stopped"));
    }
}
