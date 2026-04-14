use anyhow::Result;
use serde_json::json;

use crate::server::CreateSessionParams;
use crate::session::{SessionConfig, SessionManager};

/// Spawn a new PTY session from the given parameters.
pub async fn handle_create_session(
    manager: &SessionManager,
    params: &CreateSessionParams,
) -> Result<serde_json::Value> {
    let config = SessionConfig {
        command: params.command.clone(),
        args: params.args.clone().unwrap_or_default(),
        cwd: params.cwd.clone(),
        env: params.env.clone().unwrap_or_default(),
        rows: params.rows.unwrap_or(24),
        cols: params.cols.unwrap_or(80),
        scrollback: params.scrollback.unwrap_or(1000) as usize,
    };
    let info = manager.create_session_async(config).await?;
    Ok(serde_json::to_value(info)?)
}

/// Close and remove a session by ID.
pub async fn handle_close_session(
    manager: &SessionManager,
    session_id: &str,
) -> Result<serde_json::Value> {
    manager.close_session(session_id).await?;
    Ok(json!({ "session_id": session_id, "status": "closed" }))
}

/// List metadata for every active session.
pub async fn handle_list_sessions(
    manager: &SessionManager,
) -> Result<serde_json::Value> {
    let sessions = manager.list_sessions().await;
    Ok(json!({ "sessions": sessions, "count": sessions.len() }))
}
