use anyhow::{anyhow, Result};
use serde_json::json;

use crate::keys::key_to_bytes;
use crate::session::Session;

/// Write text to a session's PTY stdin, optionally pressing Enter afterwards.
/// If `delay_between_ms` is set, characters are sent one at a time with the
/// specified delay between them (useful for timing-sensitive TUI input).
pub async fn handle_send_text(
    session: &Session,
    text: &str,
    press_enter: bool,
    delay_between_ms: Option<u64>,
) -> Result<serde_json::Value> {
    if let Some(delay_ms) = delay_between_ms {
        let delay = std::time::Duration::from_millis(delay_ms);
        for ch in text.chars() {
            let mut buf = [0u8; 4];
            let bytes = ch.encode_utf8(&mut buf);
            session.write_bytes(bytes.as_bytes()).await?;
            tokio::time::sleep(delay).await;
        }
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
pub async fn handle_send_keys(
    session: &Session,
    keys: &[String],
) -> Result<serde_json::Value> {
    let app_cursor = session.application_cursor().await;
    let mut total = 0usize;
    for key in keys {
        let bytes = key_to_bytes(key, app_cursor)
            .ok_or_else(|| anyhow!("Unknown key: {}", key))?;
        session.write_bytes(&bytes).await?;
        total += 1;
    }
    Ok(json!({ "sent": total, "status": "ok" }))
}
