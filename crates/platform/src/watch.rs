//! Folder watching (inotify / FSEvents / ReadDirectoryChangesW through `notify`).
//!
//! The watcher costs nothing while files are quiet. Bursts are coalesced: after one notification no further one is
//! sent until the receiver calls [`Gate::rearm`], so a build writing thousands of files wakes the UI once per refresh.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Shared flag that limits notifications to one at a time.
#[derive(Debug, Clone, Default)]
pub struct Gate(Arc<AtomicBool>);

impl Gate {
    /// Allow the next notification (call when the receiver starts handling the previous one).
    pub fn rearm(&self) {
        self.0.store(false, Ordering::Release);
    }

    /// True when this call closed the gate (the caller should notify); false when one is already pending.
    fn close(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }
}

/// Watches a set of folders (not recursively) and calls `on_change` for content changes.
pub struct DirWatcher {
    inner: RecommendedWatcher,
    watched: HashSet<PathBuf>,
    gate: Gate,
}

/// Whether an event changes what a folder listing would show.
fn relevant(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(notify::event::ModifyKind::Name(_))
    )
}

impl DirWatcher {
    /// Create a watcher. `on_change` runs on the watcher's thread, at most once until the gate is re-armed.
    pub fn new(on_change: impl Fn() + Send + 'static) -> Result<(Self, Gate), notify::Error> {
        let gate = Gate::default();
        let g = gate.clone();
        let inner = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if res.is_ok_and(|e| relevant(&e.kind)) && g.close() {
                on_change();
            }
        })?;
        Ok((
            Self {
                inner,
                watched: HashSet::new(),
                gate: gate.clone(),
            },
            gate,
        ))
    }

    /// Make the watched set equal to `dirs` (folders that vanished are dropped silently).
    pub fn set_dirs(&mut self, dirs: &[PathBuf]) {
        let want: HashSet<PathBuf> = dirs.iter().cloned().collect();
        for gone in self.watched.difference(&want).cloned().collect::<Vec<_>>() {
            let _ = self.inner.unwatch(&gone);
            self.watched.remove(&gone);
        }
        for new in want.difference(&self.watched.clone()) {
            if self
                .inner
                .watch(Path::new(new), RecursiveMode::NonRecursive)
                .is_ok()
            {
                self.watched.insert(new.clone());
            }
        }
    }

    pub fn watched(&self) -> &HashSet<PathBuf> {
        &self.watched
    }

    /// Re-arm the notification gate (see [`Gate::rearm`]).
    pub fn rearm(&self) {
        self.gate.rearm();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn creating_files_notifies_once_until_rearmed() {
        let d = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let (mut w, gate) = DirWatcher::new(move || {
            let _ = tx.send(());
        })
        .expect("watcher");
        w.set_dirs(&[d.path().to_path_buf()]);
        assert!(w.watched().contains(d.path()));
        std::fs::write(d.path().join("a"), "1").unwrap();
        rx.recv_timeout(Duration::from_secs(5))
            .expect("first notification");
        for i in 0..20 {
            std::fs::write(d.path().join(format!("f{i}")), "x").unwrap();
        }
        assert!(
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "coalesced while the gate is closed"
        );
        gate.rearm();
        std::fs::write(d.path().join("b"), "2").unwrap();
        rx.recv_timeout(Duration::from_secs(5))
            .expect("notified again after rearm");
        // Dropping a folder from the set stops watching it.
        w.set_dirs(&[]);
        assert!(w.watched().is_empty());
    }
}
