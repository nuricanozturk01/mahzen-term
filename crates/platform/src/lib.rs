//! OS abstraction: PTY, shell detection, directories, clipboard.

pub mod sysstats;
pub use sysstats::{shell_command, Sampler, Stats};

use thiserror::Error;

/// Error returned when no graphical session can be reached.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("no graphical display found: set DISPLAY or WAYLAND_DISPLAY (are you on SSH or a headless machine?)")]
pub struct NoDisplay;

/// Check that a graphical session is available, reading variables through `env`.
/// Only Linux/BSD need an explicit check; macOS and Windows always have a window server.
pub fn check_display_with(env: impl Fn(&str) -> Option<String>) -> Result<(), NoDisplay> {
    if cfg!(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    )) {
        let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
        if !(set("DISPLAY") || set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET")) {
            return Err(NoDisplay);
        }
    }
    Ok(())
}

/// Check the real process environment.
pub fn check_display() -> Result<(), NoDisplay> {
    check_display_with(|k| std::env::var(k).ok())
}

/// Pick the user's default shell. `env` reads variables, `exists` tests whether an
/// executable name or path is available.
pub fn default_shell_with(
    env: impl Fn(&str) -> Option<String>,
    exists: impl Fn(&str) -> bool,
) -> String {
    if cfg!(windows) {
        for cand in ["pwsh.exe", "powershell.exe"] {
            if exists(cand) {
                return cand.to_string();
            }
        }
        return env("COMSPEC").unwrap_or_else(|| "cmd.exe".to_string());
    }
    match env("SHELL") {
        Some(s) if !s.is_empty() => s,
        _ => "/bin/sh".to_string(),
    }
}

/// Default shell for the current machine.
pub fn default_shell() -> String {
    default_shell_with(
        |k| std::env::var(k).ok(),
        |name| {
            std::env::var_os("PATH")
                .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(name).is_file()))
        },
    )
}

/// The user's home directory, if known.
pub fn home_dir() -> Option<std::path::PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(Into::into)
}

/// Directory for user configuration (`<dir>/config.toml`, `<dir>/state.toml`), reading variables through `env`.
/// Linux/BSD: `$XDG_CONFIG_HOME/mahzenterm` or `~/.config/mahzenterm`; macOS: `~/Library/Application Support/mahzenterm`;
/// Windows: `%APPDATA%\mahzenterm`.
pub fn config_dir_with(env: impl Fn(&str) -> Option<String>) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let nonempty = |k: &str| env(k).filter(|v| !v.is_empty());
    let base = if cfg!(windows) {
        PathBuf::from(nonempty("APPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(nonempty("HOME")?).join("Library/Application Support")
    } else if let Some(x) = nonempty("XDG_CONFIG_HOME") {
        PathBuf::from(x)
    } else {
        PathBuf::from(nonempty("HOME")?).join(".config")
    };
    Some(base.join("mahzenterm"))
}

/// User configuration directory for the current machine.
pub fn config_dir() -> Option<std::path::PathBuf> {
    config_dir_with(|k| std::env::var(k).ok())
}

/// Current working directory of another process, when the OS lets us read it.
/// Linux reads `/proc/<pid>/cwd`; other platforms return `None` until implemented (callers fall back to the default).
pub fn process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Why a URL was not opened.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpenError {
    #[error("only http, https, ftp and mailto links can be opened")]
    Scheme,
    #[error("the link contains spaces or control characters")]
    Malformed,
    #[error("no program could open the link: {0}")]
    Launch(String),
}

/// Check that `url` is a single-line link with a safe scheme.
pub fn check_url(url: &str) -> Result<(), OpenError> {
    let lower = url.to_ascii_lowercase();
    if !["https://", "http://", "ftp://", "mailto:"]
        .iter()
        .any(|s| lower.starts_with(s))
    {
        return Err(OpenError::Scheme);
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) || url.len() > 2048 {
        return Err(OpenError::Malformed);
    }
    Ok(())
}

/// Open `url` in the user's default program. The link is validated and passed as one argument, never
/// through a shell.
pub fn open_url(url: &str) -> Result<(), OpenError> {
    check_url(url)?;
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            // Reap the launcher in the background so it never lingers as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
        .map_err(|e| OpenError::Launch(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_checked_before_opening() {
        assert!(check_url("https://example.com/a?b=c").is_ok());
        assert_eq!(check_url("file:///etc/passwd"), Err(OpenError::Scheme));
        assert_eq!(check_url("javascript:alert(1)"), Err(OpenError::Scheme));
        assert_eq!(check_url("https://a b"), Err(OpenError::Malformed));
        assert_eq!(check_url("https://a\n--flag"), Err(OpenError::Malformed));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_config_dir_prefers_xdg_then_home() {
        let xdg = |k: &str| (k == "XDG_CONFIG_HOME").then(|| "/x".to_string());
        assert_eq!(config_dir_with(xdg), Some("/x/mahzenterm".into()));
        let home = |k: &str| (k == "HOME").then(|| "/h".to_string());
        assert_eq!(config_dir_with(home), Some("/h/.config/mahzenterm".into()));
        assert_eq!(config_dir_with(|_| None), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn own_cwd_is_readable_via_proc() {
        let ours = std::env::current_dir().unwrap();
        assert_eq!(process_cwd(std::process::id()), Some(ours));
        assert_eq!(process_cwd(u32::MAX), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_without_display_vars_fails() {
        assert_eq!(check_display_with(|_| None), Err(NoDisplay));
        assert_eq!(check_display_with(|_| Some(String::new())), Err(NoDisplay));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_with_x11_or_wayland_passes() {
        assert!(check_display_with(|k| (k == "DISPLAY").then(|| ":0".into())).is_ok());
        assert!(
            check_display_with(|k| (k == "WAYLAND_DISPLAY").then(|| "wayland-0".into())).is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_shell_uses_env_or_falls_back() {
        assert_eq!(
            default_shell_with(|_| Some("/bin/zsh".into()), |_| false),
            "/bin/zsh"
        );
        assert_eq!(default_shell_with(|_| None, |_| false), "/bin/sh");
        assert_eq!(
            default_shell_with(|_| Some(String::new()), |_| false),
            "/bin/sh"
        );
    }
}
