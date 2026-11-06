//! Desktop notifications through the system's own tool. Best effort: a missing tool is not an error.

use std::process::{Command, Stdio};

/// Strip control characters and shorten, so text from a program cannot smuggle escapes into a notification.
fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// Show a desktop notification. Linux/BSD: `notify-send`; macOS: `osascript`; elsewhere nothing happens.
/// Returns whether a notification tool was started.
pub fn notify(title: &str, body: &str) -> bool {
    let (title, body) = (clean(title, 80), clean(body, 200));
    let spawned = if cfg!(target_os = "macos") {
        // The text is passed as arguments to a fixed script, never spliced into it.
        Command::new("osascript")
            .args([
                "-e",
                "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run",
                &title,
                &body,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    } else if cfg!(windows) {
        return false;
    } else {
        Command::new("notify-send")
            .args(["--app-name=MahzenTerm", "--", &title, &body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    match spawned {
        Ok(mut child) => {
            // Reap it without blocking the caller.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn notification_text_is_cleaned_and_bounded() {
        assert_eq!(clean("a\u{1b}[31mb\nc", 80), "a[31mbc");
        assert_eq!(clean(&"x".repeat(500), 200).len(), 200);
    }
}
