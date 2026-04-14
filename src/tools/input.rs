use anyhow::{anyhow, Result};
use serde_json::json;

use crate::keys::key_to_bytes;
use crate::session::Session;

/// Write text to a session's PTY stdin, optionally pressing Enter afterwards.
pub async fn handle_send_text(
    session: &Session,
    text: &str,
    press_enter: bool,
) -> Result<serde_json::Value> {
    let mut bytes = text.as_bytes().to_vec();
    if press_enter {
        bytes.push(0x0d); // \r
    }
    session.write_bytes(&bytes).await?;
    Ok(json!({ "sent": bytes.len(), "status": "ok" }))
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
