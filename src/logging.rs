/// Structured tracing helpers for session and tool instrumentation.
///
/// These spans are available for use in tool handlers and session lifecycle
/// but are not yet wired into the codebase.

use tracing::Span;

#[allow(dead_code)]
/// Create a tracing span for a session lifecycle.
pub fn session_span(session_id: &str) -> Span {
    tracing::info_span!("session", id = session_id)
}

#[allow(dead_code)]
/// Create a tracing span for a tool call.
pub fn tool_span(tool_name: &str, session_id: Option<&str>) -> Span {
    if let Some(sid) = session_id {
        tracing::info_span!("tool", name = tool_name, session = sid)
    } else {
        tracing::info_span!("tool", name = tool_name)
    }
}

// Key log points (use at call sites):
//
// Session lifecycle:
//   tracing::info!(session_id, "session created");
//   tracing::info!(session_id, "session closed");
//
// Tool execution:
//   tracing::info!(tool = name, session_id, "tool called");
//
// PTY errors:
//   tracing::error!(session_id, error = %e, "pty read error");
//   tracing::error!(session_id, error = %e, "pty write error");
//
// Shell integration:
//   tracing::info!(session_id, "shell integration detected");
//
// Idle timeout:
//   tracing::warn!(session_id, "idle timeout triggered");
