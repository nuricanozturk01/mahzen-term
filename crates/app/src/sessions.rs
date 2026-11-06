//! Background worker of the AI sessions card: reads Claude Code's session records and the process table while the
//! card is visible, and sleeps (no wakeups) while it is hidden.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use iced::futures::channel::mpsc::UnboundedSender;
use mahzenterm_ai_sessions::read_sessions;
use mahzenterm_platform::proc::ProcTable;

use crate::feed::Feed;

enum Ctl {
    Show(Duration),
    Hide,
    /// The Claude directory changed (config reload).
    Dir(PathBuf),
}

/// Handle to the worker thread; dropping it stops the thread.
pub struct Worker {
    tx: mpsc::Sender<Ctl>,
}

impl Worker {
    pub fn spawn(claude_dir: Option<PathBuf>, out: UnboundedSender<Feed>) -> Self {
        let (tx, rx) = mpsc::channel::<Ctl>();
        thread::Builder::new()
            .name("mahzenterm-sessions".into())
            .spawn(move || run(claude_dir, &rx, &out))
            .ok();
        Self { tx }
    }

    pub fn show(&self, period: Duration) {
        let _ = self.tx.send(Ctl::Show(period));
    }

    pub fn hide(&self) {
        let _ = self.tx.send(Ctl::Hide);
    }

    pub fn set_dir(&self, dir: PathBuf) {
        let _ = self.tx.send(Ctl::Dir(dir));
    }
}

fn run(mut dir: Option<PathBuf>, rx: &mpsc::Receiver<Ctl>, out: &UnboundedSender<Feed>) {
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
            Ok(Ctl::Dir(d)) => dir = Some(d),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let sessions = match &dir {
            Some(d) => {
                let table = ProcTable::snapshot();
                let mut v = read_sessions(d, &|pid| table.is_alive(pid));
                for s in &mut v {
                    s.lineage = table.lineage(s.pid);
                }
                v
            }
            None => Vec::new(),
        };
        if out.unbounded_send(Feed::Sessions(sessions)).is_err() {
            return;
        }
    }
}
