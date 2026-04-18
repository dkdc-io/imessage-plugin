//! Send outbound messages via `osascript` → Messages.app.
//!
//! Injection-safe by construction: the AppleScript body is a fixed string
//! that takes `text` and `chat_guid` as numbered argv items via `on run argv`.
//! `osascript -` reads the script from stdin, so the user-controlled values
//! never hit the shell or `osascript`'s argument parser as code.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SEND_SCRIPT: &str = "on run argv
  tell application \"Messages\" to send (item 1 of argv) to chat id (item 2 of argv)
end run";

const OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(8);
const OSASCRIPT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub fn send_text(chat_guid: &str, text: &str) -> anyhow::Result<()> {
    let child = Command::new("osascript")
        .args(["-", text, chat_guid])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    write_and_wait(child, SEND_SCRIPT, OSASCRIPT_TIMEOUT)
}

fn write_and_wait(
    mut child: std::process::Child,
    script: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes())?;
    }

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut stderr_buf = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut stderr_buf);
                }
                if !status.success() {
                    anyhow::bail!(
                        "osascript failed ({}): {}",
                        status.code().unwrap_or(-1),
                        stderr_buf.trim()
                    );
                }
                return Ok(());
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!(
                        "osascript exceeded {}s timeout (Messages.app may be wedged)",
                        timeout.as_secs()
                    );
                }
                std::thread::sleep(OSASCRIPT_POLL_INTERVAL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_wait_kills_hung_child() {
        let child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sleep");
        let start = Instant::now();
        let result = write_and_wait(child, "ignored", Duration::from_millis(300));
        let elapsed = start.elapsed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timeout"));
        assert!(elapsed < Duration::from_secs(2));
    }

    #[test]
    fn write_and_wait_returns_on_fast_exit() {
        let child = Command::new("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn true");
        let result = write_and_wait(child, "ignored", Duration::from_secs(2));
        assert!(result.is_ok());
    }

    #[test]
    fn write_and_wait_surfaces_nonzero_exit() {
        let child = Command::new("false")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn false");
        let result = write_and_wait(child, "ignored", Duration::from_secs(2));
        let err = result.expect_err("expected error");
        assert!(err.to_string().contains("osascript failed"));
    }
}
