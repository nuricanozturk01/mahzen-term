//! Typed message bus: `Command` (one consumer) and `Event` (broadcast).
//!
//! Modules never call each other; they send `Command`s and publish/subscribe to `Event`s.
//! Both enums are `#[non_exhaustive]` and serde-serializable so they can later cross a process or WASM boundary.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::workspace::{Axis, PaneId, PaneKind, TabId};

/// Current wire schema version carried in every [`Envelope`].
pub const SCHEMA: u32 = 1;

/// Bounded queue sizes: back-pressure instead of unbounded memory growth.
const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 1024;

/// A request addressed to the core or a module. Exactly one receiver handles it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Command {
    OpenTab {
        kind: PaneKind,
    },
    CloseTab {
        tab: TabId,
    },
    ClosePane {
        pane: PaneId,
    },
    SplitPane {
        pane: PaneId,
        axis: Axis,
        kind: PaneKind,
    },
    FocusPane {
        pane: PaneId,
    },
    RunInPane {
        pane: PaneId,
        command: String,
    },
    ShowPanel {
        module: String,
        panel: String,
    },
    OpenFile {
        path: PathBuf,
        line: Option<u32>,
    },
    /// Run a registered command by id (palette, keybinding, module).
    Invoke {
        id: String,
        args: Vec<String>,
    },
}

/// A fact that happened. Any number of subscribers may observe it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Event {
    PaneSpawned { pane: PaneId },
    PaneExited { pane: PaneId, code: Option<i32> },
    CwdChanged { pane: PaneId, cwd: PathBuf },
    TitleChanged { pane: PaneId, title: String },
    FocusChanged { pane: PaneId },
    ConfigReloaded,
    ThemeChanged { name: String },
}

/// Versioned wrapper so receivers can reject or migrate messages from other schema versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub schema: u32,
    pub body: T,
}

impl<T> Envelope<T> {
    /// Wrap `body` with the current schema version.
    pub fn new(body: T) -> Self {
        Self {
            schema: SCHEMA,
            body,
        }
    }
}

/// Error returned when the receiving side is gone.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BusError {
    #[error("command receiver is closed; the application is shutting down")]
    Closed,
    #[error("command queue is full; the consumer is not keeping up")]
    Full,
}

/// Cloneable sending side of the bus.
#[derive(Debug, Clone)]
pub struct Bus {
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
}

/// Receiving side of the command channel, owned by the dispatcher.
#[derive(Debug)]
pub struct CommandRx(mpsc::Receiver<Command>);

impl CommandRx {
    /// Wait for the next command; `None` once every [`Bus`] handle is dropped.
    pub async fn recv(&mut self) -> Option<Command> {
        self.0.recv().await
    }

    /// Non-blocking receive for UI loops.
    pub fn try_recv(&mut self) -> Option<Command> {
        self.0.try_recv().ok()
    }
}

/// Event subscription. Slow subscribers skip missed events instead of failing.
#[derive(Debug)]
pub struct EventRx(broadcast::Receiver<Event>);

impl EventRx {
    /// Wait for the next event. If this subscriber fell behind, the missed events are
    /// dropped (logged) and the oldest still-available event is returned. `None` when the bus is gone.
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            match self.0.recv().await {
                Ok(ev) => return Some(ev),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "event subscriber lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Non-blocking receive; skips lag gaps the same way as [`EventRx::recv`].
    pub fn try_recv(&mut self) -> Option<Event> {
        loop {
            match self.0.try_recv() {
                Ok(ev) => return Some(ev),
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "event subscriber lagged");
                }
                Err(_) => return None,
            }
        }
    }
}

impl Bus {
    /// Create a bus and the single command receiver.
    pub fn new() -> (Self, CommandRx) {
        let (ctx, crx) = mpsc::channel(COMMAND_CAPACITY);
        let (etx, _) = broadcast::channel(EVENT_CAPACITY);
        (
            Self {
                commands: ctx,
                events: etx,
            },
            CommandRx(crx),
        )
    }

    /// Queue a command without blocking (safe from the UI thread).
    pub fn send(&self, cmd: Command) -> Result<(), BusError> {
        self.commands.try_send(cmd).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => BusError::Full,
            mpsc::error::TrySendError::Closed(_) => BusError::Closed,
        })
    }

    /// Publish an event to all current subscribers. Having no subscriber is not an error.
    pub fn publish(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    /// Subscribe to events published from now on.
    pub fn subscribe(&self) -> EventRx {
        EventRx(self.events.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_reaches_single_receiver() {
        let (bus, mut rx) = Bus::new();
        bus.send(Command::FocusPane { pane: PaneId(3) }).unwrap();
        assert_eq!(
            rx.recv().await,
            Some(Command::FocusPane { pane: PaneId(3) })
        );
    }

    #[tokio::test]
    async fn event_fans_out_to_all_subscribers() {
        let (bus, _rx) = Bus::new();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.publish(Event::ConfigReloaded);
        assert_eq!(a.recv().await, Some(Event::ConfigReloaded));
        assert_eq!(b.recv().await, Some(Event::ConfigReloaded));
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_fine() {
        let (bus, _rx) = Bus::new();
        bus.publish(Event::ConfigReloaded);
    }

    #[tokio::test]
    async fn slow_subscriber_skips_lag_and_continues() {
        let (bus, _rx) = Bus::new();
        let mut slow = bus.subscribe();
        for i in 0..(EVENT_CAPACITY as u64 + 10) {
            bus.publish(Event::FocusChanged { pane: PaneId(i) });
        }
        // Must not error: returns an event still in the ring buffer.
        assert!(matches!(
            slow.recv().await,
            Some(Event::FocusChanged { .. })
        ));
    }

    #[tokio::test]
    async fn full_queue_reports_error_instead_of_blocking() {
        let (bus, _rx) = Bus::new();
        for _ in 0..COMMAND_CAPACITY {
            bus.send(Command::ShowPanel {
                module: "m".into(),
                panel: "p".into(),
            })
            .unwrap();
        }
        assert_eq!(
            bus.send(Command::ShowPanel {
                module: "m".into(),
                panel: "p".into()
            }),
            Err(BusError::Full)
        );
    }

    #[tokio::test]
    async fn closed_receiver_reports_closed() {
        let (bus, rx) = Bus::new();
        drop(rx);
        assert_eq!(
            bus.send(Command::FocusPane { pane: PaneId(0) }),
            Err(BusError::Closed)
        );
    }

    #[test]
    fn envelope_carries_schema_and_round_trips() {
        let env = Envelope::new(Command::Invoke {
            id: "tab.new".into(),
            args: vec![],
        });
        assert_eq!(env.schema, SCHEMA);
        let s = toml::to_string(&env).unwrap();
        let back: Envelope<Command> = toml::from_str(&s).unwrap();
        assert_eq!(env, back);
    }
}
