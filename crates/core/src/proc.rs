//! Run an external program with a time limit and bounded output (for read-only queries by modules).
//!
//! Arguments are a plain vector, never a shell string. Output beyond `max_bytes` is dropped; a program that runs
//! past `timeout` is killed.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a finished program left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub stdout: String,
    /// First part of standard error (at most 16 KiB).
    pub stderr: String,
    pub success: bool,
}

/// Why a program could not be run to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The program is not on `PATH`.
    NotFound,
    Spawn(String),
    Timeout,
}

/// Run `program args...` (optionally in `cwd`) and capture its output. Blocking: call only from a background task.
pub fn capture(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Captured, RunError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            RunError::NotFound
        } else {
            RunError::Spawn(e.to_string())
        }
    })?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.take(max_bytes as u64).read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.take(16 * 1024).read_to_end(&mut buf);
        }
        buf
    });
    let end = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(RunError::Spawn(e.to_string())),
        }
    };
    Ok(Captured {
        stdout: String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned(),
        success: status.success(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_status() {
        let r = capture(
            "sh",
            &["-c", "echo hi; echo oops >&2; exit 3"],
            None,
            Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert_eq!(r.stdout.trim(), "hi");
        assert_eq!(r.stderr.trim(), "oops");
        assert!(!r.success);
    }

    #[test]
    fn missing_programs_and_slow_programs_are_reported() {
        assert_eq!(
            capture(
                "definitely-not-a-program-xyz",
                &[],
                None,
                Duration::from_secs(1),
                10
            ),
            Err(RunError::NotFound)
        );
        assert_eq!(
            capture("sleep", &["5"], None, Duration::from_millis(100), 10),
            Err(RunError::Timeout)
        );
    }

    #[test]
    fn output_is_bounded() {
        let r = capture(
            "sh",
            &["-c", "yes | head -c 100000"],
            None,
            Duration::from_secs(5),
            100,
        )
        .unwrap();
        assert!(r.stdout.len() <= 100);
    }
}
