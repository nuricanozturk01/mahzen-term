//! Background worker of the AI sessions card: reads Claude Code's session records and the process table while the
//! card is visible, and sleeps (no wakeups) while it is hidden.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use iced::futures::channel::mpsc::UnboundedSender;
use mahzenterm_ai_sessions::{codex, read_sessions};
use mahzenterm_platform::proc::ProcTable;

use crate::feed::Feed;

enum Ctl {
    Show(Duration),
    Hide,
    /// The Claude and Codex directories changed (config reload).
    Dirs(Option<PathBuf>, Option<PathBuf>),
}

/// Handle to the worker thread; dropping it stops the thread.
pub struct Worker {
    tx: mpsc::Sender<Ctl>,
}

impl Worker {
    pub fn spawn(
        claude_dir: Option<PathBuf>,
        codex_dir: Option<PathBuf>,
        out: UnboundedSender<Feed>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Ctl>();
        thread::Builder::new()
            .name("mahzenterm-sessions".into())
            .spawn(move || run(claude_dir, codex_dir, &rx, &out))
            .ok();
        Self { tx }
    }

    pub fn show(&self, period: Duration) {
        let _ = self.tx.send(Ctl::Show(period));
    }

    pub fn hide(&self) {
        let _ = self.tx.send(Ctl::Hide);
    }

    pub fn set_dirs(&self, claude: Option<PathBuf>, codex: Option<PathBuf>) {
        let _ = self.tx.send(Ctl::Dirs(claude, codex));
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

fn run(
    mut dir: Option<PathBuf>,
    mut codex_dir: Option<PathBuf>,
    rx: &mpsc::Receiver<Ctl>,
    out: &UnboundedSender<Feed>,
) {
    let mut period: Option<Duration> = None;
    loop {
        let msg = match period {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(p) => rx.recv_timeout(p),
        };
        match msg {
            Ok(Ctl::Hide) => {
                period = None;
                continue;
            }
            Ok(Ctl::Show(p)) => period = Some(p),
            Ok(Ctl::Dirs(c, x)) => {
                dir = c;
                codex_dir = x;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let table = ProcTable::snapshot();
        let mut sessions = match &dir {
            Some(d) => {
                let mut v = read_sessions(d, &|pid| table.is_alive(pid));
                for s in &mut v {
                    s.lineage = table.lineage(s.pid);
                }
                v
            }
            None => Vec::new(),
        };
        if let Some(cd) = &codex_dir {
            let now = now_ms();
            let procs: Vec<codex::Proc> = table
                .find_program("codex")
                .into_iter()
                .map(|pid| codex::Proc {
                    pid,
                    cwd: mahzenterm_platform::process_cwd(pid).map(|p| p.display().to_string()),
                    lineage: table.lineage(pid),
                })
                .collect();
            if !procs.is_empty() {
                sessions.extend(codex::sessions(&procs, &codex::scan(cd, now), now));
            }
        }
        if out.unbounded_send(Feed::Sessions(sessions)).is_err() {
            return;
        }
    }
}
