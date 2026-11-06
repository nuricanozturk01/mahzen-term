//! Docker at a glance: running containers as an info-card widget (card, tab or window).
//!
//! Drives the user's own `docker` CLI with read-only queries (`docker ps`), never a socket client, so contexts and
//! credentials keep working and nothing heavy is linked. Output is untrusted: it is size-bounded and parsed
//! defensively. Starting, stopping and removing containers is not offered here.

mod panel;
mod view;

use std::time::Duration;

use mahzenterm_core::command::CommandSpec;
use mahzenterm_core::module::{Capability, Module, ModuleManifest};
use mahzenterm_core::proc::{self, RunError};
use mahzenterm_ui_kit::{InfoCard, InfoRow, InfoTone};
use serde::Deserialize;

/// Command ids of this module.
pub mod id {
    pub const TOGGLE: &str = "docker.toggle";
    pub const PANEL: &str = "docker.panel";
}

const TIMEOUT: Duration = Duration::from_secs(6);
const MAX_OUTPUT: usize = 2 * 1024 * 1024;
/// Containers kept from one query.
const MAX_CONTAINERS: usize = 200;

/// `[modules.docker]` in the config.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Program to run (a bare name looked up on `PATH`, e.g. `podman`).
    pub program: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            program: "docker".into(),
        }
    }
}

impl Settings {
    /// The program name, falling back to `docker` when the setting is unusable.
    pub fn program(&self) -> &str {
        let p = self.program.as_str();
        if p.is_empty()
            || p.starts_with('-')
            || p.contains(['/', '\\'])
            || p.chars().any(char::is_whitespace)
        {
            "docker"
        } else {
            p
        }
    }
}

/// One container from `docker ps -a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// Container id as `docker ps` prints it (hexadecimal).
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `paused`, ...
    pub state: String,
    /// Human text such as `Up 3 hours (healthy)`.
    pub status: String,
}

impl Container {
    pub fn running(&self) -> bool {
        self.state == "running"
    }

    fn unhealthy(&self) -> bool {
        self.status.contains("unhealthy")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Raw {
    #[serde(default, rename = "ID")]
    id: String,
    #[serde(default)]
    names: String,
    #[serde(default)]
    image: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
}

/// Parse `docker ps -a --format '{{json .}}'` output (one JSON object per line). Bad lines are skipped.
pub fn parse_ps(text: &str) -> Vec<Container> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Raw>(l.trim()).ok())
        .take(MAX_CONTAINERS)
        .map(|r| Container {
            id: r.id,
            name: r
                .names
                .split(',')
                .next()
                .unwrap_or("")
                .trim_start_matches('/')
                .to_owned(),
            image: r.image,
            state: r.state,
            status: r.status,
        })
        .collect()
}

/// Run `docker ps -a` (blocking: call only inside a background task).
pub fn fetch(program: &str) -> Result<Vec<Container>, String> {
    let r = proc::capture(
        program,
        &["ps", "-a", "--format", "{{json .}}"],
        None,
        TIMEOUT,
        MAX_OUTPUT,
    )
    .map_err(|e| match e {
        RunError::NotFound => {
            format!("{program} not found: install Docker or set [modules.docker] program")
        }
        RunError::Spawn(m) => format!("cannot start {program}: {m}"),
        RunError::Timeout => format!("{program} did not answer in {}s", TIMEOUT.as_secs()),
    })?;
    if !r.success {
        let line = r.stderr.lines().next().unwrap_or("failed");
        return Err(if line.contains("daemon") || line.contains("connect") {
            "Docker daemon is not running".to_owned()
        } else {
            line.chars().take(80).collect()
        });
    }
    Ok(parse_ps(&r.stdout))
}

/// The card for a query result: running containers first, then stopped ones.
pub fn card(result: &Result<Vec<Container>, String>) -> InfoCard {
    let mut card = InfoCard {
        title: "Docker".into(),
        ..InfoCard::default()
    };
    let list = match result {
        Ok(l) => l,
        Err(e) => {
            card.badge = "offline".into();
            card.empty = e.clone();
            return card;
        }
    };
    let running = list.iter().filter(|c| c.running()).count();
    card.badge = format!("{running}/{} up", list.len());
    if list.is_empty() {
        card.empty = "no containers".into();
        return card;
    }
    let mut sorted: Vec<&Container> = list.iter().collect();
    sorted.sort_by_key(|c| (!c.running(), c.name.clone()));
    let room = mahzenterm_ui_kit::infocard::MAX_ROWS;
    for c in sorted
        .iter()
        .take(if sorted.len() > room { room - 1 } else { room })
    {
        let tone = if c.unhealthy() {
            InfoTone::Bad
        } else if c.running() {
            InfoTone::Good
        } else {
            InfoTone::Dim
        };
        let value = if c.running() {
            c.status.trim_start_matches("Up ").to_owned()
        } else {
            c.state.clone()
        };
        card.rows.push(InfoRow::new(c.name.clone(), value, tone));
    }
    if sorted.len() > room {
        card.rows.push(InfoRow::new(
            format!("+{} more", sorted.len() - (room - 1)),
            "",
            InfoTone::Dim,
        ));
    }
    card
}

pub use panel::{Action, Confirm, Key, Outcome, Panel};
pub use view::view;

/// A container id is safe to pass as an argument when it is hexadecimal (so it can never be an option).
pub fn safe_id(id: &str) -> bool {
    (6..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Run one container operation (`start`, `stop`, `restart`, `rm -f`). Blocking: call from a background task.
pub fn operate(program: &str, op: Op, id: &str) -> Result<String, String> {
    if !safe_id(id) {
        return Err("that container id cannot be used".into());
    }
    let args: &[&str] = match op {
        Op::Start => &["start"],
        Op::Stop => &["stop"],
        Op::Restart => &["restart"],
        Op::Remove => &["rm", "-f"],
    };
    let mut full: Vec<&str> = args.to_vec();
    full.push(id);
    let r =
        proc::capture(program, &full, None, Duration::from_secs(60), MAX_OUTPUT).map_err(|e| {
            match e {
                RunError::NotFound => format!("{program} not found"),
                RunError::Spawn(m) => format!("cannot start {program}: {m}"),
                RunError::Timeout => format!("{program} did not finish in time"),
            }
        })?;
    if r.success {
        Ok(match op {
            Op::Start => "started",
            Op::Stop => "stopped",
            Op::Restart => "restarted",
            Op::Remove => "removed",
        }
        .into())
    } else {
        Err(r
            .stderr
            .lines()
            .next()
            .unwrap_or("failed")
            .chars()
            .take(120)
            .collect())
    }
}

/// A container operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Start,
    Stop,
    Restart,
    Remove,
}

/// The module registration.
pub struct DockerModule;

impl Module for DockerModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "docker",
            name: "Docker",
            version: env!("CARGO_PKG_VERSION"),
            capabilities: vec![Capability::ProcessSpawn],
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(id::TOGGLE, "Widgets: Docker", "Widgets"),
            CommandSpec::new(id::PANEL, "Docker: Containers", "Docker").key("Ctrl+Alt+Y"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = r#"{"Names":"web","Image":"nginx:1","State":"running","Status":"Up 3 hours"}
{"Names":"db","Image":"postgres","State":"running","Status":"Up 2 days (unhealthy)"}
garbage line
{"Names":"old","Image":"x","State":"exited","Status":"Exited (0) 2 days ago"}"#;

    #[test]
    fn ps_lines_parse_and_garbage_is_skipped() {
        let l = parse_ps(PS);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].name, "web");
        assert!(l[0].running() && !l[2].running());
    }

    #[test]
    fn card_lists_running_first_and_flags_unhealthy() {
        let c = card(&Ok(parse_ps(PS)));
        assert_eq!(c.badge, "2/3 up");
        assert_eq!(c.rows[0].label, "db");
        assert_eq!(c.rows[0].tone, InfoTone::Bad);
        assert_eq!(c.rows[1].tone, InfoTone::Good);
        assert_eq!(c.rows[2].label, "old");
        assert_eq!(c.rows[2].tone, InfoTone::Dim);
    }

    #[test]
    fn errors_and_empty_lists_have_a_message() {
        assert_eq!(
            card(&Err("Docker daemon is not running".into())).empty,
            "Docker daemon is not running"
        );
        assert_eq!(card(&Ok(Vec::new())).empty, "no containers");
    }

    #[test]
    fn long_lists_are_summarised() {
        let many: Vec<Container> = (0..40)
            .map(|i| Container {
                id: "abcdef".into(),
                name: format!("c{i:02}"),
                image: "i".into(),
                state: "running".into(),
                status: "Up 1 hour".into(),
            })
            .collect();
        let c = card(&Ok(many));
        assert_eq!(c.rows.len(), mahzenterm_ui_kit::infocard::MAX_ROWS);
        assert!(c.rows.last().is_some_and(|r| r.label.starts_with('+')));
    }

    #[test]
    fn program_setting_cannot_be_a_path_or_option() {
        for bad in ["/bin/evil", "-x", "a b", ""] {
            assert_eq!(
                Settings {
                    program: bad.into()
                }
                .program(),
                "docker"
            );
        }
        assert_eq!(
            Settings {
                program: "podman".into()
            }
            .program(),
            "podman"
        );
    }

    #[test]
    fn module_is_valid() {
        mahzenterm_core::module::validate(&DockerModule).unwrap();
    }
}

#[cfg(test)]
mod ops_tests {
    use super::*;

    #[test]
    fn only_hex_ids_reach_the_command_line() {
        for ok in ["abcdef", "0123456789ab", &"f".repeat(64)] {
            assert!(safe_id(ok), "{ok}");
        }
        for bad in [
            "",
            "abc",
            "--force",
            "name with space",
            "g12345",
            &"a".repeat(65),
            "-rf123",
        ] {
            assert!(!safe_id(bad), "{bad}");
        }
        assert!(operate("docker", Op::Remove, "--all").is_err());
    }

    #[test]
    fn ids_are_read_from_ps_output() {
        let l = parse_ps(
            r#"{"ID":"1a2b3c4d5e6f","Names":"web","Image":"nginx","State":"running","Status":"Up"}"#,
        );
        assert_eq!(l[0].id, "1a2b3c4d5e6f");
    }
}
