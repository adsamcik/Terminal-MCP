use anyhow::{Result, anyhow};
use serde_json::json;

use crate::keys::key_to_bytes;
use crate::session::Session;

const MAX_SEND_TEXT_BYTES: usize = 1_048_576; // 1 MB
const MAX_KEYS_COUNT: usize = 1_000;
const DEFAULT_TYPED_CHARACTER_DELAY_MS: u64 = 5;
const DEFAULT_TYPED_DELAY_LIMIT_BYTES: usize = 4 * 1024;

/// Write text to a session's PTY stdin, optionally pressing Enter afterwards.
/// Small text-entry sends are paced character-by-character so raw-input TUIs
/// receive typed input instead of one pasted chunk. `delay_between_ms` can slow
/// the cadence further for timing-sensitive flows.
pub async fn handle_send_text(
    session: &Session,
    text: &str,
    press_enter: bool,
    delay_between_ms: Option<u64>,
) -> Result<serde_json::Value> {
    if text.len() > MAX_SEND_TEXT_BYTES {
        anyhow::bail!(
            "send_text input exceeds maximum size of {} bytes",
            MAX_SEND_TEXT_BYTES
        );
    }

    let default_typed_delay = delay_between_ms
        .is_none()
        .then_some(text.len())
        .filter(|len| *len > 0 && *len <= DEFAULT_TYPED_DELAY_LIMIT_BYTES)
        .map(|_| DEFAULT_TYPED_CHARACTER_DELAY_MS);

    let delay_between = delay_between_ms
        .or(default_typed_delay)
        .map(std::time::Duration::from_millis);

    if delay_between.is_some() {
        session.write_text(text, delay_between).await?;
    } else {
        session.write_bytes(text.as_bytes()).await?;
    }

    if press_enter {
        session.write_bytes(&[0x0d]).await?;
    }

    Ok(json!({
        "sent": text.len(),
        "status": "ok",
        "delay_between_ms": delay_between_ms,
    }))
}

/// Send a sequence of named keystrokes (e.g. "Ctrl+C", "Up", "Enter") to
/// a session, translating each to VT escape sequences.
pub async fn handle_send_keys(session: &Session, keys: &[String]) -> Result<serde_json::Value> {
    if keys.len() > MAX_KEYS_COUNT {
        anyhow::bail!(
            "send_keys exceeds maximum of {} keys per call",
            MAX_KEYS_COUNT
        );
    }

    let app_cursor = session.application_cursor().await;
    let mut total = 0usize;
    for key in keys {
        let bytes = key_to_bytes(key, app_cursor).ok_or_else(|| anyhow!("Unknown key: {}", key))?;
        session.write_bytes(&bytes).await?;
        total += 1;
    }
    Ok(json!({ "sent": total, "status": "ok" }))
}
