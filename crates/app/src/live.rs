//! Bridge from the core's config watcher (tokio) to the Iced UI.
//!
//! The watcher runs on its own thread with a tiny single-threaded runtime so the UI thread never waits on file I/O.
//! Each reload arrives as `(config, optional problem text)`; the UI keeps the last valid config on problems.

use std::sync::Arc;

use iced::futures::channel::mpsc;
use mahzenterm_core::bus::Bus;
use mahzenterm_core::config::{watch_config, Config, ConfigError, Loader};

/// A reload result sent to the UI.
pub type Reload = (Arc<Config>, Option<String>);

/// One problem with the directory part of its file path dropped (`config.toml:2:14: ...`): the status line is narrow.
fn short(err: &ConfigError) -> String {
    let full = err.to_string();
    let path = match err {
        ConfigError::Read { path, .. }
        | ConfigError::Syntax { path, .. }
        | ConfigError::SchemaTooNew { path, .. } => Some(path),
        ConfigError::Invalid { path, .. } => path.as_ref(),
    };
    match path.and_then(|p| {
        p.file_name()
            .map(|n| (p.display().to_string(), n.to_string_lossy().into_owned()))
    }) {
        Some((whole, name)) => full.replacen(&whole, &name, 1),
        None => full,
    }
}

/// First problem rendered for the status line (what happened, where; the error text already says what to fix).
pub fn describe(issues: &[ConfigError]) -> Option<String> {
    issues.first().map(|first| {
        if issues.len() > 1 {
            format!("{} (+{} more)", short(first), issues.len() - 1)
        } else {
            short(first)
        }
    })
}

/// Start watching. The returned receiver yields one item per reload; it ends if the watcher cannot start.
pub fn spawn(loader: Loader) -> mpsc::UnboundedReceiver<Reload> {
    let (tx, rx) = mpsc::unbounded::<Reload>();
    let spawned = std::thread::Builder::new()
        .name("mahzenterm-config".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "config watcher runtime failed; live reload disabled");
                    return;
                }
            };
            runtime.block_on(async move {
                let (bus, _commands) = Bus::new();
                let mut live = match watch_config(loader, bus) {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::warn!(error = %e, "cannot watch config files; live reload disabled");
                        return;
                    }
                };
                // The UI already loaded the initial config itself.
                live.config.mark_unchanged();
                while live.config.changed().await.is_ok() {
                    let config = live.config.borrow_and_update().clone();
                    let problem = describe(&live.issues.borrow_and_update());
                    if tx.unbounded_send((config, problem)).is_err() {
                        break;
                    }
                }
            });
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "cannot start config watcher thread");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn describe_summarises_problems() {
        assert_eq!(describe(&[]), None);
        let e = |m: &str| ConfigError::Invalid {
            path: Some(PathBuf::from("c.toml")),
            field: "terminal.font_size".into(),
            message: m.into(),
        };
        let one = describe(&[e("must be between 6 and 72")]).unwrap();
        assert!(one.contains("terminal.font_size") && !one.contains("more"));
        let deep = ConfigError::Syntax {
            path: PathBuf::from("/very/long/dir/mahzenterm/config.toml"),
            line: 2,
            col: 14,
            message: "invalid value".into(),
        };
        assert_eq!(
            describe(&[deep]).unwrap(),
            "config.toml:2:14: invalid value"
        );
        let two = describe(&[e("a"), e("b")]).unwrap();
        assert!(two.ends_with("(+1 more)"));
    }

    #[test]
    fn edits_reach_the_ui_and_broken_edits_report_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[terminal]\nfont_size = 14.0\n").unwrap();
        let mut rx = spawn(Loader::new(path.clone(), None));
        std::thread::sleep(Duration::from_millis(400));
        std::fs::write(&path, "[terminal]\nfont_size = 19.0\n").unwrap();
        let (cfg, problem) = recv(&mut rx);
        assert_eq!(cfg.terminal.font_size, 19.0);
        assert_eq!(problem, None);

        std::fs::write(&path, "[terminal]\nfont_size = nope\n").unwrap();
        let (cfg, problem) = recv(&mut rx);
        assert_eq!(cfg.terminal.font_size, 19.0, "last valid value stays");
        assert!(problem.unwrap().contains("config.toml:2"));
    }

    fn recv(rx: &mut mpsc::UnboundedReceiver<Reload>) -> Reload {
        let end = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(item) = rx.try_recv() {
                return item;
            }
            assert!(std::time::Instant::now() < end, "no reload arrived");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
