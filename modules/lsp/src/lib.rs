//! A small Language Server Protocol client: diagnostics and go-to-definition for the built-in editor.
//!
//! Servers are **opt-in**: nothing is started unless the user names a command for a language in
//! `[modules.lsp.servers]`. The server is the user's own program, started with plain arguments (no shell), talking
//! JSON-RPC over stdio. Everything a server sends is untrusted: message sizes are capped and unknown or malformed
//! messages are ignored. Positions are converted between editor columns (characters) and LSP's UTF-16 units.

pub mod proto;

use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures_channel::mpsc::UnboundedSender;
use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use serde::Deserialize;
use serde_json::{json, Value};

pub use proto::{
    char_col, parse_completions, parse_hover, parse_locations, path_to_uri, plain_snippet,
    uri_to_path, utf16_col, Completion, Diag, DiagSeverity, Location,
};

/// Command ids of this module.
pub mod id {
    pub const RESTART: &str = "lsp.restart";
}

/// `[modules.lsp]` in the config.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Language name (as the editor shows it, e.g. `rust`) to the server command line.
    pub servers: BTreeMap<String, Server>,
}

/// One configured server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl Server {
    /// A command is usable when it is a bare program name or an absolute path, without control characters.
    pub fn usable(&self) -> bool {
        let c = self.command.as_str();
        !c.is_empty() && !c.starts_with('-') && !c.chars().any(char::is_control)
    }
}

/// What a running server tells the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Diagnostics {
        lang: String,
        path: String,
        diags: Vec<Diag>,
    },
    /// The answer to a request (`definition`, `hover` or `completion`); the caller knows which one it asked.
    Response {
        lang: String,
        request: i64,
        result: Value,
    },
    /// The server ended (crash or exit).
    Exited { lang: String },
}

/// A running language server.
pub struct Client {
    lang: String,
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: AtomicI64,
}

impl Client {
    /// Start `server` for `lang` with `root` as the workspace folder. Events arrive on `events`.
    pub fn start(
        lang: &str,
        server: &Server,
        root: &Path,
        events: UnboundedSender<Event>,
    ) -> Result<Self, String> {
        if !server.usable() {
            return Err(format!("the {lang} server command cannot be used"));
        }
        let mut child = Command::new(&server.command)
            .args(&server.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", server.command))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("server pipes are not available".into());
        };
        let stdin = Arc::new(Mutex::new(stdin));
        let reader_lang = lang.to_owned();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            while let Ok(Some(body)) = proto::read_message(&mut r) {
                if let Some(ev) = proto::event_from(&reader_lang, &body) {
                    if events.unbounded_send(ev).is_err() {
                        return;
                    }
                }
            }
            let _ = events.unbounded_send(Event::Exited { lang: reader_lang });
        });
        let client = Self {
            lang: lang.to_owned(),
            child,
            stdin,
            next_id: AtomicI64::new(1),
        };
        client.request("initialize", proto::initialize_params(root));
        client.notify("initialized", json!({}));
        Ok(client)
    }

    pub fn lang(&self) -> &str {
        &self.lang
    }

    fn send(&self, msg: &Value) {
        let framed = proto::frame(msg);
        if let Ok(mut s) = self.stdin.lock() {
            let _ = s.write_all(&framed).and_then(|()| s.flush());
        }
    }

    fn request(&self, method: &str, params: Value) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn did_open(&self, path: &Path, text: &str) {
        self.notify(
            "textDocument/didOpen",
            proto::did_open(path, &self.lang, text),
        );
    }

    /// Send the whole new text (full sync keeps this client simple).
    pub fn did_change(&self, path: &Path, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            proto::did_change(path, version, text),
        );
    }

    pub fn did_close(&self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": path_to_uri(path)}}),
        );
    }

    /// Ask what is known about the symbol at a position. Returns the request id echoed in the event.
    pub fn hover(&self, path: &Path, line: usize, utf16: usize) -> i64 {
        self.request(
            "textDocument/hover",
            proto::position_params(path, line, utf16),
        )
    }

    /// Ask for completions at a position. Returns the request id echoed in the event.
    pub fn completion(&self, path: &Path, line: usize, utf16: usize) -> i64 {
        self.request(
            "textDocument/completion",
            proto::position_params(path, line, utf16),
        )
    }

    /// Ask for the definition at a 0-based line and UTF-16 column. Returns the request id echoed in the event.
    pub fn definition(&self, path: &Path, line: usize, utf16: usize) -> i64 {
        self.request(
            "textDocument/definition",
            proto::position_params(path, line, utf16),
        )
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        // Give a well-behaved server a moment, then make sure it is gone.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The module registration.
pub struct LspModule;

impl Module for LspModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "lsp",
            name: "Language Servers",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::ProcessSpawn],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![CommandSpec::new(
            id::RESTART,
            "Language Servers: Restart",
            "Editor",
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_strict_and_commands_are_checked() {
        let s: Settings = toml_like(r#"{"servers":{"rust":{"command":"rust-analyzer"}}}"#);
        assert_eq!(s.servers["rust"].command, "rust-analyzer");
        assert!(serde_json::from_str::<Settings>(r#"{"bogus":1}"#).is_err());
        assert!(!Server {
            command: "-x".into(),
            args: vec![]
        }
        .usable());
        assert!(!Server {
            command: String::new(),
            args: vec![]
        }
        .usable());
        assert!(Server {
            command: "/usr/bin/clangd".into(),
            args: vec![]
        }
        .usable());
    }

    fn toml_like(json: &str) -> Settings {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn module_is_valid() {
        mahzenterm_core::module::validate(&LspModule).unwrap();
    }

    const FAKE: &str = r#"
import sys, json
def send(o):
    b = json.dumps(o).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b); sys.stdout.buffer.flush()
def read():
    n = 0
    while True:
        l = sys.stdin.buffer.readline()
        if not l: return None
        l = l.strip()
        if not l: break
        if l.lower().startswith(b"content-length:"): n = int(l.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(n))
while True:
    m = read()
    if m is None: break
    if m.get("method") == "initialize" or "id" in m and m.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": {}}})
    elif m.get("method") == "textDocument/didOpen":
        uri = m["params"]["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": [
            {"range": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 3}}, "severity": 1, "message": "fake error"}]}})
    elif m.get("method") == "textDocument/definition":
        uri = m["params"]["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"uri": uri, "range": {"start": {"line": 4, "character": 2}, "end": {"line": 4, "character": 3}}}})
    elif m.get("method") == "exit":
        break
"#;

    #[test]
    fn a_fake_server_gives_diagnostics_and_definitions() {
        let Ok(py) = Command::new("python3").arg("--version").output() else {
            return;
        };
        if !py.status.success() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mz-lsp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake.py");
        std::fs::write(&script, FAKE).unwrap();
        let (tx, mut rx) = futures_channel::mpsc::unbounded();
        let server = Server {
            command: "python3".into(),
            args: vec![script.display().to_string()],
        };
        let client = Client::start("rust", &server, &dir, tx).unwrap();
        let file = dir.join("a.rs");
        client.did_open(&file, "fn x() {}\n");
        let wait = |rx: &mut futures_channel::mpsc::UnboundedReceiver<Event>| {
            for _ in 0..200 {
                if let Ok(ev) = rx.try_recv() {
                    return Some(ev);
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            None
        };
        // The answer to `initialize` comes first; it is not what this test is about.
        let mut first = wait(&mut rx);
        while matches!(first, Some(Event::Response { request: 1, .. })) {
            first = wait(&mut rx);
        }
        let Some(Event::Diagnostics { diags, .. }) = first else {
            panic!("no diagnostics")
        };
        assert_eq!(diags[0].message, "fake error");
        let id = client.definition(&file, 0, 1);
        let Some(Event::Response {
            request, result, ..
        }) = wait(&mut rx)
        else {
            panic!("no answer")
        };
        assert_eq!(request, id);
        let locations = parse_locations(&result);
        assert_eq!((locations[0].line, locations[0].col), (4, 2));
        drop(client);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_server_is_reported() {
        let (tx, _rx) = futures_channel::mpsc::unbounded();
        let r = Client::start(
            "rust",
            &Server {
                command: "definitely-not-a-server".into(),
                args: vec![],
            },
            Path::new("."),
            tx,
        );
        assert!(r.err().unwrap().starts_with("cannot start"));
    }
}
