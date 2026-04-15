//! Introspection tools: terminal mode reporting and session capability manifests.

use serde::Serialize;

use crate::session::{SessionInfo, SessionStatus};
use crate::terminal::{MouseMode, VtParser};

// ── Terminal modes ─────────────────────────────────────────────────

/// Snapshot of the terminal's current DEC private‑mode flags.
#[derive(Debug, Serialize)]
pub struct TerminalModes {
    /// Whether the alternate screen buffer is active.
    ///
    /// **Windows/ConPTY caveat:** always `false` because ConPTY handles
    /// DECSET 1049 internally and does not forward the escape sequence.
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub cursor_visible: bool,
    /// One of `"none"`, `"click"`, `"drag"`, `"any"`, `"sgr"`.
    pub mouse_mode: String,
}

impl TerminalModes {
    /// Build from a `VtParser` reference.
    pub fn from_parser(vt: &VtParser) -> Self {
        let mouse_mode = match vt.mouse_mode() {
            MouseMode::None => "none",
            MouseMode::Press => "click",
            MouseMode::PressRelease => "click",
            MouseMode::ButtonMotion => "drag",
            MouseMode::AnyMotion => "any",
        };

        Self {
            alternate_screen: vt.is_alternate_screen(),
            application_cursor: vt.application_cursor(),
            bracketed_paste: vt.bracketed_paste(),
            cursor_visible: vt.cursor_visible(),
            mouse_mode: mouse_mode.to_string(),
        }
    }
}

// ── Capabilities manifest ──────────────────────────────────────────

/// Declares what input the server is able to translate for the agent.
#[derive(Debug, Serialize)]
pub struct SessionCapabilities {
    /// All named keys the server supports (e.g. `"Up"`, `"F5"`, `"Escape"`).
    pub supported_keys: Vec<String>,
    /// Modifier prefixes (e.g. `"Ctrl+"`, `"Alt+"`).
    pub modifier_combos: Vec<String>,
    /// Higher‑level protocol features the server can inject.
    pub special_sequences: Vec<String>,
}

/// Full catalogue of named keys and modifiers the server can encode.
pub fn default_capabilities() -> SessionCapabilities {
    SessionCapabilities {
        supported_keys: vec![
            // Arrow keys
            "Up", "Down", "Left", "Right",
            // Navigation
            "Home", "End", "PageUp", "PageDown", "Insert", "Delete",
            // Whitespace / editing
            "Enter", "Tab", "Escape", "Backspace", "Space",
            // Function keys
            "F1", "F2", "F3", "F4", "F5", "F6",
            "F7", "F8", "F9", "F10", "F11", "F12",
            // Ctrl+letter (A–Z)
            "Ctrl+A", "Ctrl+B", "Ctrl+C", "Ctrl+D", "Ctrl+E", "Ctrl+F",
            "Ctrl+G", "Ctrl+H", "Ctrl+I", "Ctrl+J", "Ctrl+K", "Ctrl+L",
            "Ctrl+M", "Ctrl+N", "Ctrl+O", "Ctrl+P", "Ctrl+Q", "Ctrl+R",
            "Ctrl+S", "Ctrl+T", "Ctrl+U", "Ctrl+V", "Ctrl+W", "Ctrl+X",
            "Ctrl+Y", "Ctrl+Z",
            // Ctrl+punctuation
            "Ctrl+[", "Ctrl+\\", "Ctrl+]", "Ctrl+^", "Ctrl+_",
            // Alt+letter (common readline/shell bindings)
            "Alt+A", "Alt+B", "Alt+C", "Alt+D", "Alt+E", "Alt+F",
            "Alt+G", "Alt+H", "Alt+I", "Alt+J", "Alt+K", "Alt+L",
            "Alt+M", "Alt+N", "Alt+O", "Alt+P", "Alt+Q", "Alt+R",
            "Alt+S", "Alt+T", "Alt+U", "Alt+V", "Alt+W", "Alt+X",
            "Alt+Y", "Alt+Z",
            "Alt+.", "Alt+Enter",
            // Shift combos
            "Shift+Tab",
            "Shift+Up", "Shift+Down", "Shift+Left", "Shift+Right",
            "Shift+Home", "Shift+End",
            "Shift+F1", "Shift+F2", "Shift+F3", "Shift+F4",
            "Shift+F5", "Shift+F6", "Shift+F7", "Shift+F8",
            "Shift+F9", "Shift+F10", "Shift+F11", "Shift+F12",
            // Ctrl+arrow / Ctrl+nav
            "Ctrl+Up", "Ctrl+Down", "Ctrl+Left", "Ctrl+Right",
            "Ctrl+Home", "Ctrl+End",
            // Ctrl+Shift combos
            "Ctrl+Shift+Up", "Ctrl+Shift+Down",
            "Ctrl+Shift+Left", "Ctrl+Shift+Right",
        ]
        .into_iter()
        .map(String::from)
        .collect(),

        modifier_combos: vec![
            "Ctrl+", "Alt+", "Shift+", "Ctrl+Shift+", "Ctrl+Alt+",
        ]
        .into_iter()
        .map(String::from)
        .collect(),

        special_sequences: vec![
            "BracketedPaste",
            "FocusEvents",
            "MouseSGR",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    }
}

// ── Session info response ──────────────────────────────────────────

/// Complete introspection payload returned by `get_session_info`.
#[derive(Debug, Serialize)]
pub struct SessionInfoResponse {
    pub session_id: String,
    pub pid: Option<u32>,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub rows: u16,
    pub cols: u16,
    /// `"running"`, `"idle"`, or `"exited"`.
    pub status: String,
    /// ISO‑8601 timestamp of session creation.
    pub created_at: String,
    /// Milliseconds since the last output activity.
    pub idle_duration_ms: u64,
    pub modes: TerminalModes,
    pub capabilities: SessionCapabilities,
    /// `"detecting"`, `"active"`, `"injected"`, or `"unavailable"`.
    pub shell_integration: String,
}

/// Build a [`SessionInfoResponse`] from session metadata and VT parser state.
///
/// * `info`              – basic session record (id, pid, command, size, status,
///                         created_at).
/// * `vt`                – reference to the session's VT parser.
/// * `args`              – original command‑line arguments.
/// * `cwd`               – working directory, if known.
/// * `idle_duration_ms`  – time since last PTY output.
/// * `shell_integration` – current shell‑integration state string.
pub fn build_session_info(
    info: &SessionInfo,
    vt: &VtParser,
    args: &[String],
    cwd: Option<&str>,
    idle_duration_ms: u64,
    shell_integration: &str,
) -> SessionInfoResponse {
    let status = match &info.status {
        SessionStatus::Running => "running".to_string(),
        SessionStatus::Idle => "idle".to_string(),
        SessionStatus::Exited { code } => match code {
            Some(c) => format!("exited({})", c),
            None => "exited".to_string(),
        },
    };

    SessionInfoResponse {
        session_id: info.session_id.clone(),
        pid: info.pid,
        command: info.command.clone(),
        args: args.to_vec(),
        cwd: cwd.map(String::from),
        rows: info.rows,
        cols: info.cols,
        status,
        created_at: info.created_at.clone(),
        idle_duration_ms,
        modes: TerminalModes::from_parser(vt),
        capabilities: default_capabilities(),
        shell_integration: shell_integration.to_string(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_are_non_empty() {
        let caps = default_capabilities();
        assert!(!caps.supported_keys.is_empty());
        assert!(!caps.modifier_combos.is_empty());
        assert!(!caps.special_sequences.is_empty());
    }

    #[test]
    fn capabilities_contain_essential_keys() {
        let caps = default_capabilities();
        for key in &["Up", "Down", "Enter", "Escape", "Tab", "F1", "Ctrl+C"] {
            assert!(
                caps.supported_keys.contains(&key.to_string()),
                "missing key: {key}"
            );
        }
    }

    #[test]
    fn terminal_modes_from_fresh_parser() {
        let vt = VtParser::new(24, 80, 0);
        let modes = TerminalModes::from_parser(&vt);
        assert!(!modes.alternate_screen);
        assert!(!modes.application_cursor);
        assert!(!modes.bracketed_paste);
        assert!(modes.cursor_visible);
        assert_eq!(modes.mouse_mode, "none");
    }

    #[test]
    fn build_session_info_smoke() {
        let info = SessionInfo {
            session_id: "test-1".into(),
            pid: Some(1234),
            command: "bash".into(),
            rows: 24,
            cols: 80,
            status: SessionStatus::Running,
            created_at: "2025-01-01T00:00:00Z".into(),
        };
        let vt = VtParser::new(24, 80, 0);
        let resp = build_session_info(&info, &vt, &[], None, 0, "unavailable");
        assert_eq!(resp.session_id, "test-1");
        assert_eq!(resp.status, "running");
        assert_eq!(resp.shell_integration, "unavailable");
        assert!(!resp.capabilities.supported_keys.is_empty());
    }

    #[test]
    fn session_info_exited_status_with_code() {
        let info = SessionInfo {
            session_id: "test-2".into(),
            pid: None,
            command: "ls".into(),
            rows: 24,
            cols: 80,
            status: SessionStatus::Exited { code: Some(0) },
            created_at: "2025-01-01T00:00:00Z".into(),
        };
        let vt = VtParser::new(24, 80, 0);
        let resp = build_session_info(&info, &vt, &["-la".into()], Some("/home"), 500, "active");
        assert_eq!(resp.status, "exited(0)");
        assert_eq!(resp.args, vec!["la".to_string()].iter().map(|_| "-la".to_string()).collect::<Vec<_>>());
        assert_eq!(resp.cwd, Some("/home".into()));
    }

    #[test]
    fn serialization_roundtrip() {
        let vt = VtParser::new(24, 80, 0);
        let modes = TerminalModes::from_parser(&vt);
        let json = serde_json::to_string(&modes).unwrap();
        assert!(json.contains("\"alternate_screen\":false"));
        assert!(json.contains("\"mouse_mode\":\"none\""));
    }

    // ── Additional tests ──────────────────────────────────────

    #[test]
    fn capabilities_contain_all_ctrl_keys() {
        let caps = default_capabilities();
        for ch in 'A'..='Z' {
            let key = format!("Ctrl+{ch}");
            assert!(
                caps.supported_keys.contains(&key),
                "missing: {key}"
            );
        }
    }

    #[test]
    fn capabilities_contain_all_function_keys() {
        let caps = default_capabilities();
        for i in 1..=12 {
            let key = format!("F{i}");
            assert!(
                caps.supported_keys.contains(&key),
                "missing: {key}"
            );
        }
    }

    #[test]
    fn capabilities_contain_shift_tab() {
        let caps = default_capabilities();
        assert!(caps.supported_keys.contains(&"Shift+Tab".to_string()));
    }

    #[test]
    fn capabilities_contain_arrow_keys() {
        let caps = default_capabilities();
        for key in &["Up", "Down", "Left", "Right"] {
            assert!(caps.supported_keys.contains(&key.to_string()));
        }
    }

    #[test]
    fn capabilities_contain_alt_keys() {
        let caps = default_capabilities();
        assert!(caps.supported_keys.contains(&"Alt+A".to_string()));
        assert!(caps.supported_keys.contains(&"Alt+Enter".to_string()));
    }

    #[test]
    fn capabilities_modifier_combos() {
        let caps = default_capabilities();
        assert!(caps.modifier_combos.contains(&"Ctrl+".to_string()));
        assert!(caps.modifier_combos.contains(&"Alt+".to_string()));
        assert!(caps.modifier_combos.contains(&"Shift+".to_string()));
        assert!(caps.modifier_combos.contains(&"Ctrl+Shift+".to_string()));
        assert!(caps.modifier_combos.contains(&"Ctrl+Alt+".to_string()));
    }

    #[test]
    fn capabilities_special_sequences() {
        let caps = default_capabilities();
        assert!(caps.special_sequences.contains(&"BracketedPaste".to_string()));
        assert!(caps.special_sequences.contains(&"MouseSGR".to_string()));
    }

    #[test]
    fn terminal_modes_alternate_screen() {
        let mut vt = VtParser::new(24, 80, 0);
        vt.process(b"\x1b[?1049h");
        let modes = TerminalModes::from_parser(&vt);
        assert!(modes.alternate_screen);
    }

    #[test]
    fn terminal_modes_application_cursor() {
        let mut vt = VtParser::new(24, 80, 0);
        vt.process(b"\x1b[?1h");
        let modes = TerminalModes::from_parser(&vt);
        assert!(modes.application_cursor);
    }

    #[test]
    fn terminal_modes_bracketed_paste() {
        let mut vt = VtParser::new(24, 80, 0);
        vt.process(b"\x1b[?2004h");
        let modes = TerminalModes::from_parser(&vt);
        assert!(modes.bracketed_paste);
    }

    #[test]
    fn terminal_modes_cursor_hidden() {
        let mut vt = VtParser::new(24, 80, 0);
        vt.process(b"\x1b[?25l");
        let modes = TerminalModes::from_parser(&vt);
        assert!(!modes.cursor_visible);
    }

    #[test]
    fn terminal_modes_serializes_to_json() {
        let vt = VtParser::new(24, 80, 0);
        let modes = TerminalModes::from_parser(&vt);
        let json = serde_json::to_string(&modes).unwrap();
        assert!(json.contains("\"cursor_visible\":true"));
        assert!(json.contains("\"bracketed_paste\":false"));
    }

    #[test]
    fn session_info_idle_status() {
        let info = SessionInfo {
            session_id: "test-idle".into(),
            pid: Some(5678),
            command: "sh".into(),
            rows: 24,
            cols: 80,
            status: SessionStatus::Idle,
            created_at: "2025-01-01T00:00:00Z".into(),
        };
        let vt = VtParser::new(24, 80, 0);
        let resp = build_session_info(&info, &vt, &[], None, 1000, "active");
        assert_eq!(resp.status, "idle");
    }

    #[test]
    fn session_info_exited_no_code() {
        let info = SessionInfo {
            session_id: "test-exit".into(),
            pid: None,
            command: "echo".into(),
            rows: 24,
            cols: 80,
            status: SessionStatus::Exited { code: None },
            created_at: "2025-01-01T00:00:00Z".into(),
        };
        let vt = VtParser::new(24, 80, 0);
        let resp = build_session_info(&info, &vt, &[], None, 0, "unavailable");
        assert_eq!(resp.status, "exited");
    }

    #[test]
    fn session_info_with_args_and_cwd() {
        let info = SessionInfo {
            session_id: "test-args".into(),
            pid: Some(999),
            command: "python".into(),
            rows: 30,
            cols: 120,
            status: SessionStatus::Running,
            created_at: "2025-06-01T12:00:00Z".into(),
        };
        let vt = VtParser::new(30, 120, 0);
        let args = vec!["-m".into(), "pytest".into()];
        let resp = build_session_info(&info, &vt, &args, Some("/work"), 250, "injected");
        assert_eq!(resp.args, vec!["-m", "pytest"]);
        assert_eq!(resp.cwd, Some("/work".into()));
        assert_eq!(resp.rows, 30);
        assert_eq!(resp.cols, 120);
        assert_eq!(resp.idle_duration_ms, 250);
        assert_eq!(resp.shell_integration, "injected");
    }
}
