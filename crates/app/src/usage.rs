//! Background worker of the AI token card: scans the Claude Code / Codex logs while the card is visible.
//!
//! The thread blocks (no wakeups) while the card is hidden and scans every `period` while it is shown.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use chrono::{Local, TimeZone};
use iced::futures::channel::mpsc::UnboundedSender;
use mahzenterm_ai_usage::Scanner;

use crate::feed::Feed;

enum Ctl {
    /// Scan now and then every period.
    Show(Duration),
    Hide,
}

/// Handle to the worker thread; dropping it stops the thread.
pub struct Worker {
    tx: mpsc::Sender<Ctl>,
}

/// Unix time of local midnight today.
fn day_start(now: chrono::DateTime<Local>) -> i64 {
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|d| Local.from_local_datetime(&d).earliest())
        .map_or(now.timestamp() - 86_400, |d| d.timestamp())
}

impl Worker {
    pub fn spawn(scanner: Scanner, out: UnboundedSender<Feed>) -> Self {
        let (tx, rx) = mpsc::channel::<Ctl>();
        thread::Builder::new()
            .name("mahzenterm-usage".into())
            .spawn(move || run(scanner, &rx, &out))
            .ok();
        Self { tx }
    }

    pub fn show(&self, period: Duration) {
        let _ = self.tx.send(Ctl::Show(period));
    }

    pub fn hide(&self) {
        let _ = self.tx.send(Ctl::Hide);
    }
}

fn run(mut scanner: Scanner, rx: &mpsc::Receiver<Ctl>, out: &UnboundedSender<Feed>) {
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
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let now = Local::now();
        scanner.poll(now.timestamp());
        let snap = scanner.snapshot(now.timestamp(), day_start(now));
        if out.unbounded_send(Feed::Usage(snap)).is_err() {
            return;
        }
    }
}
