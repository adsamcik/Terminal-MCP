use anyhow::Result;
use serde_json::json;

use crate::server::CreateSessionParams;
use crate::session::{SessionConfig, SessionManager};

/// Maximum terminal rows for a new session.
pub const MAX_ROWS: u16 = 300;
/// Maximum terminal columns for a new session.
pub const MAX_COLS: u16 = 500;
/// Maximum scrollback lines retained for a new session.
pub const MAX_SCROLLBACK_LINES: usize = 10_000;

/// Spawn a new PTY session from the given parameters.
pub async fn handle_create_session(
    manager: &SessionManager,
    params: &CreateSessionParams,
    owner_key: Option<String>,
) -> Result<serde_json::Value> {
    let config = SessionConfig {
        command: params.command.clone(),
        args: params.args.clone().unwrap_or_default(),
        cwd: params.cwd.clone(),
        env: params.env.clone().unwrap_or_default(),
        rows: params.rows.unwrap_or(24).clamp(1, MAX_ROWS),
        cols: params.cols.unwrap_or(80).clamp(1, MAX_COLS),
        scrollback: (params.scrollback.unwrap_or(1000) as usize).min(MAX_SCROLLBACK_LINES),
    };
    let info = manager
        .create_session_async_for_owner(config, owner_key)
        .await?;
    Ok(serde_json::to_value(info)?)
}

/// Close and remove a session by ID.
pub async fn handle_close_session(
    manager: &SessionManager,
    session_id: &str,
    owner_key: Option<&str>,
) -> Result<serde_json::Value> {
    manager.get_session_visible(session_id, owner_key)?;
    manager.close_session(session_id).await?;
    Ok(json!({ "session_id": session_id, "status": "closed" }))
}

/// List metadata for every active session.
pub async fn handle_list_sessions(
    manager: &SessionManager,
    owner_key: Option<&str>,
) -> Result<serde_json::Value> {
    let sessions = manager.list_sessions_visible(owner_key).await;
    Ok(json!({ "sessions": sessions, "count": sessions.len() }))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_clamped_to_max() {
        assert_eq!(u16::MAX.clamp(1, MAX_ROWS), MAX_ROWS);
    }

    #[test]
    fn cols_clamped_to_max() {
        assert_eq!(u16::MAX.clamp(1, MAX_COLS), MAX_COLS);
    }

    #[test]
    fn scrollback_clamped_to_max() {
        assert_eq!(usize::MAX.min(MAX_SCROLLBACK_LINES), MAX_SCROLLBACK_LINES);
    }

    #[test]
    fn rows_below_min_clamped_to_one() {
        assert_eq!(0u16.clamp(1, MAX_ROWS), 1);
    }

    #[test]
    fn default_rows_cols_within_limits() {
        let default_rows: u16 = 24;
        let default_cols: u16 = 80;
        assert!(default_rows <= MAX_ROWS);
        assert!(default_cols <= MAX_COLS);
    }
}
