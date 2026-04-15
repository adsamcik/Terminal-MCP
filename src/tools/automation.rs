//! Automation tools — compound operations that send input and wait for output.
//!
//! - [`handle_send_and_wait`]: send text and wait for output to settle or match a pattern
//! - [`handle_wait_for`]: wait for a specific pattern to appear (or disappear) in output
//! - [`handle_wait_for_idle`]: wait until terminal output stops changing

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use regex::RegexBuilder;
use serde_json::json;

use crate::ansi::strip_ansi;
use crate::session::Session;

/// Maximum input payload size (1 MiB) to prevent resource exhaustion.
const MAX_INPUT_BYTES: usize = 1 << 20;
/// Maximum timeout for any wait operation (5 minutes).
const MAX_TIMEOUT_MS: u64 = 300_000;
/// Maximum rolling output window used for regex matching across delta chunks.
const MAX_MATCH_BUFFER_BYTES: usize = 1 << 20;
/// Poll interval used while waiting for output or screen changes.
const WAIT_POLL_MS: u64 = 50;
/// Idle threshold for command-style send_and_wait calls without an explicit pattern.
const SEND_AND_WAIT_IDLE_MS: u64 = 500;
/// Visible-screen settle window for interactive screen navigation.
const SEND_AND_WAIT_SCREEN_STABLE_MS: u64 = 100;

fn append_capped_tail(buf: &mut Vec<u8>, chunk: &[u8], max_bytes: usize) {
    if chunk.is_empty() {
        return;
    }

    if chunk.len() >= max_bytes {
        buf.clear();
        buf.extend_from_slice(&chunk[chunk.len() - max_bytes..]);
        return;
    }

    let overflow = buf.len().saturating_add(chunk.len()).saturating_sub(max_bytes);
    if overflow > 0 {
        buf.drain(..overflow);
    }
    buf.extend_from_slice(chunk);
}

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
    if input.len() > MAX_INPUT_BYTES {
        anyhow::bail!(
            "Input too large: {} bytes exceeds maximum of {} bytes",
            input.len(),
            MAX_INPUT_BYTES
        );
    }

    let timeout_ms = timeout_ms.min(MAX_TIMEOUT_MS);

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
        .map(|p| RegexBuilder::new(p).size_limit(1_000_000).build())
        .transpose()
        .context("Invalid wait_for regex")?;
    let use_screen_stable_shortcut =
        pattern.is_none() && !press_enter && matches!(output_mode, "screen" | "both");

    let mut accumulated_output = Vec::new();
    let mut dropped_bytes = 0usize;
    let mut baseline_screen = None;
    let mut last_screen = None;
    let mut screen_changed = false;
    let mut last_screen_change = Instant::now();

    if use_screen_stable_shortcut {
        let screen = session.get_screen_contents().await;
        baseline_screen = Some(screen.clone());
        last_screen = Some(screen);
    }

    loop {
        if Instant::now() >= deadline {
            break;
        }

        tokio::time::sleep(Duration::from_millis(WAIT_POLL_MS)).await;

        if let Some(ref pat) = pattern {
            // Accumulate delta output for pattern match (avoids cloning entire log)
            let new_output = session.read_new_output_chunk().await;
            dropped_bytes = dropped_bytes.saturating_add(new_output.dropped_bytes);
            append_capped_tail(&mut accumulated_output, &new_output.bytes, MAX_MATCH_BUFFER_BYTES);
            let output_text = String::from_utf8_lossy(&accumulated_output);
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
            if use_screen_stable_shortcut {
                let current_screen = session.get_screen_contents().await;
                if let Some(last) = &last_screen {
                    if &current_screen != last {
                        last_screen = Some(current_screen.clone());
                        last_screen_change = Instant::now();
                        if let Some(baseline) = &baseline_screen {
                            if &current_screen != baseline {
                                screen_changed = true;
                            }
                        }
                    } else if screen_changed
                        && last_screen_change.elapsed()
                            >= Duration::from_millis(SEND_AND_WAIT_SCREEN_STABLE_MS)
                    {
                        matched = true;
                        break;
                    }
                }
            }

            // No pattern — fall back to idle for command-style execution or
            // if the interactive screen never visibly changed.
            if session
                .is_idle(Duration::from_millis(SEND_AND_WAIT_IDLE_MS))
                .await
            {
                matched = true;
                break;
            }
        }
    }

    // Small settle delay to let final output arrive
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Grab any remaining output after the settle delay
    let remaining = session.read_new_output_chunk().await;
    dropped_bytes = dropped_bytes.saturating_add(remaining.dropped_bytes);
    append_capped_tail(&mut accumulated_output, &remaining.bytes, MAX_MATCH_BUFFER_BYTES);

    // 3. Collect output based on mode
    let delta_output = String::from_utf8_lossy(&accumulated_output).to_string();
    let screen_output = session.get_screen_contents().await;

    let response = match output_mode {
        "screen" => json!({
            "matched": matched,
            "match_text": match_text,
            "timed_out": !matched,
            "dropped_bytes": dropped_bytes,
            "screen": screen_output,
        }),
        "both" => json!({
            "matched": matched,
            "match_text": match_text,
            "timed_out": !matched,
            "dropped_bytes": dropped_bytes,
            "output": strip_ansi(&delta_output),
            "screen": screen_output,
        }),
        // "delta" (default)
        _ => json!({
            "matched": matched,
            "match_text": match_text,
            "timed_out": !matched,
            "dropped_bytes": dropped_bytes,
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
    let timeout_ms = timeout_ms.min(MAX_TIMEOUT_MS);

    // Validate: need at least one of pattern or line_count
    if pattern.is_none() && line_count.is_none() {
        anyhow::bail!("Either 'pattern' or 'line_count' must be provided");
    }

    // Pattern mode takes precedence
    if let Some(pat) = pattern {
        let re = RegexBuilder::new(pat)
            .size_limit(1_000_000)
            .build()
            .context("Invalid wait_for regex")?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut matched = false;
        let mut match_text: Option<String> = None;
        let mut accumulated_output = Vec::new();
        let mut dropped_bytes = 0usize;

        loop {
            if Instant::now() >= deadline {
                break;
            }

            let text = if on_screen {
                session.get_screen_contents().await
            } else {
                // Accumulate delta output (avoids cloning entire log)
                let new_output = session.read_new_output_chunk().await;
                dropped_bytes = dropped_bytes.saturating_add(new_output.dropped_bytes);
                append_capped_tail(&mut accumulated_output, &new_output.bytes, MAX_MATCH_BUFFER_BYTES);
                String::from_utf8_lossy(&accumulated_output).to_string()
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
            "dropped_bytes": dropped_bytes,
        }))
    } else {
        // line_count mode
        let target = line_count.unwrap();
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut lines_received: u32 = 0;
        let mut dropped_bytes = 0usize;

        loop {
            if Instant::now() >= deadline {
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;

            let new_output = session.read_new_output_chunk().await;
            dropped_bytes = dropped_bytes.saturating_add(new_output.dropped_bytes);
            let text = String::from_utf8_lossy(&new_output.bytes);
            lines_received = lines_received
                .saturating_add(text.chars().filter(|&c| c == '\n').count() as u32);
            if lines_received >= target {
                break;
            }
        }

        let matched = lines_received >= target;
        Ok(json!({
            "matched": matched,
            "lines_received": lines_received,
            "line_count": target,
            "timed_out": !matched,
            "dropped_bytes": dropped_bytes,
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
    let timeout_ms = timeout_ms.min(MAX_TIMEOUT_MS);
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
