//! Automation tools — compound operations that send input and wait for output.
//!
//! - [`handle_send_and_wait`]: send text and wait for output to settle or match a pattern
//! - [`handle_wait_for`]: wait for a specific pattern to appear (or disappear) in output
//! - [`handle_wait_for_idle`]: wait until terminal output stops changing

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::json;

use crate::session::Session;

/// Send input to a terminal session and wait for expected output.
///
/// 1. Sends `input` as raw bytes (with optional CR for Enter).
/// 2. Waits for `wait_for` regex match **or** idle timeout if no pattern given.
/// 3. Collects output in the requested `output_mode`.
pub async fn handle_send_and_wait(
    session: &Session,
    input: &str,
    press_enter: bool,
    wait_for: Option<&str>,
    timeout_ms: u64,
    output_mode: &str,
) -> Result<serde_json::Value> {
    // 1. Send input
    let mut bytes = input.as_bytes().to_vec();
    if press_enter {
        bytes.push(0x0d); // CR
    }
    session.write_bytes(&bytes).await?;

    // 2. Wait for pattern or idle
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut matched = false;
    let mut match_text: Option<String> = None;
    let pattern = wait_for
        .map(|p| Regex::new(p))
        .transpose()
        .context("Invalid wait_for regex")?;

    loop {
        if Instant::now() >= deadline {
            break;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;

        if let Some(ref pat) = pattern {
            // Check stream output for pattern match
            let output_bytes = session.get_full_output().await;
            let output_text = String::from_utf8_lossy(&output_bytes);
            if let Some(m) = pat.find(&output_text) {
                matched = true;
                match_text = Some(m.as_str().to_string());
                break;
            }
            // Also check the visible screen
            let screen = session.get_screen_contents().await;
            if let Some(m) = pat.find(&screen) {
                matched = true;
                match_text = Some(m.as_str().to_string());
                break;
            }
        } else {
            // No pattern — wait for idle (500ms of no output)
            if session.is_idle(Duration::from_millis(500)).await {
                matched = true;
                break;
            }
        }
    }

    // Small settle delay to let final output arrive
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 3. Collect output based on mode
    let delta_output = {
        let raw = session.read_new_output().await;
        String::from_utf8_lossy(&raw).to_string()
    };
    let screen_output = session.get_screen_contents().await;

    let response = match output_mode {
        "screen" => json!({
            "matched": matched,
            "match_text": match_text,
            "timed_out": !matched,
            "screen": screen_output,
        }),
        "both" => json!({
            "matched": matched,
            "match_text": match_text,
            "timed_out": !matched,
            "output": strip_ansi(&delta_output),
            "screen": screen_output,
        }),
        // "delta" (default)
        _ => json!({
            "matched": matched,
            "match_text": match_text,
            "timed_out": !matched,
            "output": strip_ansi(&delta_output),
        }),
    };

    Ok(response)
}

/// Wait for a pattern to appear (or disappear if `invert`) in terminal output,
/// or wait for a specified number of new output lines.
///
/// Searches the raw output stream or visible screen buffer depending on `on_screen`.
/// If `line_count` is provided instead of a pattern, counts newlines in new output.
pub async fn handle_wait_for(
    session: &Session,
    pattern: Option<&str>,
    line_count: Option<u32>,
    timeout_ms: u64,
    on_screen: bool,
    invert: bool,
) -> Result<serde_json::Value> {
    // Validate: need at least one of pattern or line_count
    if pattern.is_none() && line_count.is_none() {
        anyhow::bail!("Either 'pattern' or 'line_count' must be provided");
    }

    // Pattern mode takes precedence
    if let Some(pat) = pattern {
        let re = Regex::new(pat).context("Invalid wait_for regex")?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut matched = false;
        let mut match_text: Option<String> = None;

        loop {
            if Instant::now() >= deadline {
                break;
            }

            let text = if on_screen {
                session.get_screen_contents().await
            } else {
                let raw = session.get_full_output().await;
                String::from_utf8_lossy(&raw).to_string()
            };

            let found = re.find(&text);

            if invert {
                if found.is_none() {
                    matched = true;
                    break;
                }
            } else {
                if let Some(m) = found {
                    matched = true;
                    match_text = Some(m.as_str().to_string());
                    break;
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(json!({
            "matched": matched,
            "match_text": match_text,
            "timed_out": !matched,
            "pattern": pat,
            "on_screen": on_screen,
            "invert": invert,
        }))
    } else {
        // line_count mode
        let target = line_count.unwrap();
        let baseline_len = session.get_full_output().await.len();
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut lines_received: u32 = 0;

        loop {
            if Instant::now() >= deadline {
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;

            let full_output = session.get_full_output().await;
            if full_output.len() > baseline_len {
                let new_bytes = &full_output[baseline_len..];
                let new_text = String::from_utf8_lossy(new_bytes);
                lines_received = new_text.chars().filter(|&c| c == '\n').count() as u32;
                if lines_received >= target {
                    break;
                }
            }
        }

        let matched = lines_received >= target;
        Ok(json!({
            "matched": matched,
            "lines_received": lines_received,
            "line_count": target,
            "timed_out": !matched,
        }))
    }
}

/// Wait until the terminal has been idle (no new output) for `stable_ms`.
///
/// When `screen_stable` is true, compares screen snapshots instead of checking
/// PTY output timestamps — more reliable for TUI apps with spinners/animations.
pub async fn handle_wait_for_idle(
    session: &Session,
    stable_ms: u64,
    timeout_ms: u64,
    screen_stable: bool,
) -> Result<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let stable_duration = Duration::from_millis(stable_ms);
    let mut idle = false;

    if screen_stable {
        let mut last_screen = session.get_screen_contents().await;
        let mut last_change = Instant::now();

        loop {
            if Instant::now() >= deadline {
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;

            let current_screen = session.get_screen_contents().await;
            if current_screen != last_screen {
                last_screen = current_screen;
                last_change = Instant::now();
            } else if last_change.elapsed() >= stable_duration {
                idle = true;
                break;
            }
        }

        Ok(json!({
            "idle": idle,
            "timed_out": !idle,
            "stable_ms": stable_ms,
            "mode": "screen_stable",
        }))
    } else {
        loop {
            if Instant::now() >= deadline {
                break;
            }

            if session.is_idle(stable_duration).await {
                idle = true;
                break;
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Ok(json!({
            "idle": idle,
            "timed_out": !idle,
            "stable_ms": stable_ms,
        }))
    }
}

/// Wait for the child process to exit and return its exit code.
pub async fn handle_wait_for_exit(
    session: &Session,
    timeout_ms: u64,
) -> Result<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    loop {
        if !session.is_alive().await {
            let exit_code = session.exit_code().await;
            return Ok(json!({
                "exited": true,
                "exit_code": exit_code,
                "timed_out": false,
            }));
        }

        if Instant::now() >= deadline {
            return Ok(json!({
                "exited": false,
                "exit_code": null,
                "timed_out": true,
            }));
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Strip ANSI escape sequences from text for cleaner output.
fn strip_ansi(text: &str) -> String {
    // Match CSI sequences, OSC sequences, and simple ESC sequences
    let re = Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][A-B012]|\x1b[=>Ncmo78]")
        .expect("static regex");
    re.replace_all(text, "").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi() {
        let input = "\x1b[1;31mERROR\x1b[0m: something failed";
        assert_eq!(strip_ansi(input), "ERROR: something failed");
    }

    #[test]
    fn strip_ansi_removes_osc() {
        let input = "\x1b]2;Window Title\x07Hello";
        assert_eq!(strip_ansi(input), "Hello");
    }

    #[test]
    fn strip_ansi_preserves_plain_text() {
        let input = "plain text without escapes";
        assert_eq!(strip_ansi(input), input);
    }
}
