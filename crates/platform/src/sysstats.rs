//! System load sampling and shell one-liners for status-bar widgets.

use std::process::Command;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// One reading of machine load.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    /// Global CPU use, 0..=100.
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    /// One-minute load average (0 where the OS has none).
    pub load: f64,
    pub uptime_secs: u64,
}

impl Stats {
    /// Memory use, 0..=100.
    pub fn mem_percent(&self) -> f32 {
        if self.mem_total == 0 {
            0.0
        } else {
            (self.mem_used as f64 / self.mem_total as f64 * 100.0) as f32
        }
    }
}

/// Reads CPU, memory, load and uptime. CPU use needs two samples, so the first reading reports 0.
pub struct Sampler {
    sys: Option<System>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        let kind = RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing().with_usage())
            .with_memory(MemoryRefreshKind::nothing().with_ram());
        Self {
            sys: System::new_with_specifics(kind).ok(),
        }
    }

    /// Take a fresh reading; an unsupported platform yields zeros rather than failing.
    pub fn sample(&mut self) -> Stats {
        let Some(sys) = self.sys.as_mut() else {
            return Stats::default();
        };
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Stats {
            cpu: sys.global_cpu_usage().clamp(0.0, 100.0),
            mem_used: sys.used_memory(),
            mem_total: sys.total_memory(),
            load: System::load_average().map_or(0.0, |l| l.one),
            uptime_secs: System::uptime().unwrap_or(0),
        }
    }
}

/// A command that runs `line` through the platform shell (`sh -c` / `cmd /C`).
pub fn shell_command(line: &str) -> Command {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C");
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c");
        c
    };
    cmd.arg(line);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_reports_memory() {
        let s = Sampler::new().sample();
        assert!(s.mem_total > 0);
        assert!(s.mem_used <= s.mem_total);
        assert!((0.0..=100.0).contains(&s.mem_percent()));
    }

    #[cfg(unix)]
    #[test]
    fn shell_command_runs_a_line() {
        let out = shell_command("echo hi").output().expect("sh exists");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    }
}
