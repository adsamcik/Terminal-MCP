use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::Parameters,
    },
    model::{
        CallToolResult, Content, Implementation, InitializeResult,
        ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
    service::RequestContext,
    transport::io::stdio,
};
use serde::Deserialize;

use crate::session::SessionManager;
use crate::tools::automation;
use crate::tools::{introspection, observation};

// ---------------------------------------------------------------------------
// Tool parameter structs — schemars derives the JSON Schema automatically
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Create a new interactive terminal session with a PTY")]
pub struct CreateSessionParams {
    #[schemars(description = "Command to run. Defaults to the user's default shell.")]
    pub command: Option<String>,
    #[schemars(description = "Command arguments")]
    pub args: Option<Vec<String>>,
    #[schemars(description = "Working directory for the session")]
    pub cwd: Option<String>,
    #[schemars(description = "Additional environment variables")]
    pub env: Option<HashMap<String, String>>,
    #[schemars(description = "Terminal height in rows (default: 24)")]
    pub rows: Option<u16>,
    #[schemars(description = "Terminal width in columns (default: 80)")]
    pub cols: Option<u16>,
    #[schemars(description = "Number of scrollback lines to retain (default: 1000)")]
    pub scrollback: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Close a terminal session by ID")]
pub struct CloseSessionParams {
    #[schemars(description = "Session identifier to close")]
    pub session_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "List all active terminal sessions")]
pub struct ListSessionsParams {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Type text into a terminal session")]
pub struct SendTextParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(description = "Text to type. Sent as raw UTF-8 characters.")]
    pub text: String,
    #[schemars(description = "If true, press Enter after typing the text (default: false)")]
    pub press_enter: Option<bool>,
    #[schemars(description = "Delay in milliseconds between each character. Useful for testing timing-sensitive input like double-tap sequences. If omitted, all characters are sent at once.")]
    pub delay_between_ms: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(
    description = "Send named keystrokes to a terminal session (e.g. Ctrl+C, Up, Tab)"
)]
pub struct SendKeysParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(
        description = "Sequence of named keys. Examples: [\"Ctrl+C\"], [\"Up\", \"Up\", \"Enter\"]"
    )]
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(
    description = "Send input and wait for expected output. The primary tool for command execution."
)]
pub struct SendAndWaitParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(description = "Text to send (typically a command to execute)")]
    pub input: String,
    #[schemars(description = "Press Enter after sending input (default: true)")]
    pub press_enter: Option<bool>,
    #[schemars(
        description = "Pattern (string or regex) to wait for. If omitted, waits for idle."
    )]
    pub wait_for: Option<String>,
    #[schemars(description = "Maximum milliseconds to wait (default: 30000)")]
    pub timeout_ms: Option<u64>,
    #[schemars(description = "What to return: 'delta', 'screen', or 'both' (default: 'delta')")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Read new output from a terminal session since the last read")]
pub struct ReadOutputParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(description = "Max milliseconds to wait for new output (default: 5000)")]
    pub timeout_ms: Option<u64>,
    #[schemars(description = "Maximum bytes to return (default: 16384)")]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Get the current terminal screen contents as a text grid")]
pub struct GetScreenParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(
        description = "If true, include color/attribute annotations per span (default: false)"
    )]
    pub include_colors: Option<bool>,
    #[schemars(description = "If true, mark the cursor position in the output (default: true)")]
    pub include_cursor: Option<bool>,
    #[schemars(
        description = "If true, include a list of changed row indices since last call (default: false)"
    )]
    pub diff_mode: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Capture a PNG screenshot of the terminal screen")]
pub struct ScreenshotParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(description = "Color theme for rendering (default: 'dark')")]
    pub theme: Option<String>,
    #[schemars(description = "Font size in pixels (default: 14)")]
    pub font_size: Option<u32>,
    #[schemars(description = "Render scale factor, e.g. 2.0 for retina (default: 1.0)")]
    pub scale: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Read scrollback buffer content that has scrolled above the visible screen")]
pub struct GetScrollbackParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(
        description = "Number of lines to return. Negative = from bottom. (default: 100)"
    )]
    pub lines: Option<i32>,
    #[schemars(description = "Optional text or regex to search for in scrollback")]
    pub search: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Wait for a pattern to appear in terminal output, or for a target number of new lines")]
pub struct WaitForParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(description = "Text or regex pattern to wait for. Optional if line_count is provided")]
    pub pattern: Option<String>,
    #[schemars(
        description = "Wait until this many new lines of output appear. Alternative to pattern matching."
    )]
    pub line_count: Option<u32>,
    #[schemars(description = "Maximum milliseconds to wait (default: 30000)")]
    pub timeout_ms: Option<u64>,
    #[schemars(
        description = "If true, match against the full screen buffer instead of streaming output (default: false)"
    )]
    pub on_screen: Option<bool>,
    #[schemars(description = "If true, wait for the pattern to DISAPPEAR (default: false)")]
    pub invert: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Wait for the terminal to become idle based on output silence or screen stability")]
pub struct WaitForIdleParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(
        description = "Consider idle after this many milliseconds of no output (default: 1000)"
    )]
    pub stable_ms: Option<u64>,
    #[schemars(description = "Maximum milliseconds to wait overall (default: 30000)")]
    pub timeout_ms: Option<u64>,
    #[schemars(
        description = "If true, wait for the screen content to stop changing instead of waiting for output to stop. More reliable for TUI apps with spinners or animations. (default: false)"
    )]
    pub screen_stable: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Wait for the child process to exit and return its exit code")]
pub struct WaitForExitParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(description = "Maximum milliseconds to wait (default: 30000)")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Get detailed session metadata and capabilities")]
pub struct GetSessionInfoParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Regex search in session scrollback history")]
pub struct SearchOutputParams {
    #[schemars(description = "Target session identifier")]
    pub session_id: String,
    #[schemars(description = "Regex pattern to search for")]
    pub pattern: String,
    #[schemars(description = "Maximum number of matches to return (default: 50)")]
    pub max_results: Option<usize>,
    #[schemars(description = "Lines of context around each match (default: 2)")]
    pub context_lines: Option<usize>,
}

// ---------------------------------------------------------------------------
// Server struct
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TerminalMcpServer {
    #[allow(dead_code)]
    session_manager: Arc<SessionManager>,
    tool_router: ToolRouter<Self>,
}

// ---------------------------------------------------------------------------
// Tool implementations (stubs)
// ---------------------------------------------------------------------------

/// Extractimage dimensions from PNG IHDR chunk.
/// PNG format: 8-byte signature, then IHDR chunk with width (4 bytes BE) and height (4 bytes BE).
fn png_dimensions(data: &[u8]) -> (u32, u32) {
    if data.len() < 24 {
        return (0, 0);
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    (width, height)
}

#[tool_router]
impl TerminalMcpServer {
    pub fn new() -> Self {
        Self {
            session_manager: Arc::new(SessionManager::new()),
            tool_router: Self::tool_router(),
        }
    }

    // -- Tier 1: Essential ------------------------------------------------

    /// Create a new interactive terminal session with a PTY.
    /// Spawns a shell or specified command.
    #[tool(description = "Create a new interactive terminal session with a PTY. Spawns a shell or specified command.")]
    async fn create_session(
        &self,
        params: Parameters<CreateSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        match crate::tools::lifecycle::handle_create_session(&self.session_manager, &params.0).await {
            Ok(value) => {
                let json = serde_json::to_string_pretty(&value)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!("create_session error: {e:#}"))])),
        }
    }

    /// Close a terminal session by ID, terminating the PTY process.
    #[tool(description = "Close a terminal session by ID, terminating the PTY process.")]
    async fn close_session(
        &self,
        params: Parameters<CloseSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        match crate::tools::lifecycle::handle_close_session(&self.session_manager, &params.0.session_id).await {
            Ok(value) => {
                let json = serde_json::to_string_pretty(&value)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!("close_session error: {e:#}"))])),
        }
    }

    /// List all active terminal sessions with their status.
    #[tool(description = "List all active terminal sessions with their status.")]
    async fn list_sessions(
        &self,
        _params: Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        match crate::tools::lifecycle::handle_list_sessions(&self.session_manager).await {
            Ok(value) => {
                let json = serde_json::to_string_pretty(&value)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!("list_sessions error: {e:#}"))])),
        }
    }

    /// Type text into a terminal session. Characters are sent as-is.
    /// For control keys or navigation, use send_keys instead.
    #[tool(description = "Type text into a terminal session. Characters are sent as-is. For control keys, navigation, or function keys, use send_keys instead. Optionally press Enter after the text.")]
    async fn send_text(
        &self,
        params: Parameters<SendTextParams>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.session_manager.get_session(&params.0.session_id).map_err(|e| {
            McpError::invalid_params(format!("Session not found: {e}"), None)
        })?;
        let press_enter = params.0.press_enter.unwrap_or(false);
        match crate::tools::input::handle_send_text(&session, &params.0.text, press_enter, params.0.delay_between_ms).await {
            Ok(value) => {
                let json = serde_json::to_string_pretty(&value)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!("send_text error: {e:#}"))])),
        }
    }

    /// Send named keystrokes to a terminal session.
    /// Use for control keys (Ctrl+C), navigation (Up/Down/Tab), and function keys.
    #[tool(description = "Send named keystrokes to a terminal session. Use for control keys (Ctrl+C), navigation (Up/Down/Tab), and function keys (F1). For typing text, use send_text instead.")]
    async fn send_keys(
        &self,
        params: Parameters<SendKeysParams>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.session_manager.get_session(&params.0.session_id).map_err(|e| {
            McpError::invalid_params(format!("Session not found: {e}"), None)
        })?;
        match crate::tools::input::handle_send_keys(&session, &params.0.keys).await {
            Ok(value) => {
                let json = serde_json::to_string_pretty(&value)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!("send_keys error: {e:#}"))])),
        }
    }

    /// Send input to a terminal and wait for expected output.
    /// Combines send_text + wait_for + read_output into one efficient call.
    #[tool(description = "Send input to a terminal session and wait for expected output. This is the primary tool for command execution — type a command, wait for it to complete, and get the output. Combines send_text + wait_for + read_output into one efficient call.")]
    async fn send_and_wait(
        &self,
        params: Parameters<SendAndWaitParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let session = self
            .session_manager
            .get_session(&p.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let result = automation::handle_send_and_wait(
            &session,
            &p.input,
            p.press_enter.unwrap_or(true),
            p.wait_for.as_deref(),
            p.timeout_ms.unwrap_or(30_000),
            p.output_mode.as_deref().unwrap_or("delta"),
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    /// Read new output from a terminal session since the last read.
    /// Best for following command output, logs, and streaming results.
    #[tool(description = "Read new output from a terminal session since the last read. Returns raw text output (ANSI codes stripped). Best for following command output, logs, and streaming results. For TUI/full-screen apps, use get_screen instead.")]
    async fn read_output(
        &self,
        params: Parameters<ReadOutputParams>,
    ) -> Result<CallToolResult, McpError> {
        let session = self
            .session_manager
            .get_session(&params.0.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let resp = crate::tools::observation::handle_read_output(
            &session,
            params.0.timeout_ms,
            params.0.max_bytes,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let json = serde_json::to_string_pretty(&resp)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get the current terminal screen contents as a text grid.
    /// Best for TUI apps, editors, debuggers, and any full-screen application.
    #[tool(description = "Get the current terminal screen contents as a text grid. Returns the full visible buffer (e.g., 80x24). Best for TUI apps, editors, debuggers, and any full-screen application. For streaming command output, use read_output instead.")]
    async fn get_screen(
        &self,
        params: Parameters<GetScreenParams>,
    ) -> Result<CallToolResult, McpError> {
        let session = self
            .session_manager
            .get_session(&params.0.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let include_cursor = params.0.include_cursor.unwrap_or(true);
        let include_colors = params.0.include_colors.unwrap_or(false);
        let diff_mode = params.0.diff_mode.unwrap_or(false);

        let resp = session
            .with_vt(|vt| {
                observation::get_screen(vt, include_cursor, include_colors, None, diff_mode)
            })
            .await;

        let json = serde_json::to_string_pretty(&resp)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // -- Tier 2: Important ------------------------------------------------

    /// Capture a PNG screenshot of the terminal screen.
    /// Renders with a monospace font preserving colors, bold, italic, underline.
    #[tool(description = "Capture a PNG screenshot of the terminal screen. Renders with a monospace font preserving colors, bold, italic, underline. Returns an MCP image content block.")]
    async fn screenshot(
        &self,
        params: Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let session = self
            .session_manager
            .get_session(&p.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let theme = p.theme.as_deref().unwrap_or("dark");
        let font_size = p.font_size.unwrap_or(14);
        let scale = p.scale.unwrap_or(1.0) as f32;

        // Preflight: reject oversized render parameters before acquiring the VT lock.
        crate::screenshot::preflight_screenshot(
            session.config.rows,
            session.config.cols,
            font_size,
            scale,
        )
        .map_err(|e| McpError::invalid_params(format!("Screenshot preflight failed: {e}"), None))?;

        let (png_bytes, terminal_rows, terminal_cols) = session
            .with_vt(|vt| {
                let screen = vt.screen();
                let (rows, cols) = screen.size();
                let png = crate::screenshot::render_screenshot(screen, theme, font_size, scale);
                (png, rows, cols)
            })
            .await;

        let png_bytes = png_bytes
            .map_err(|e| McpError::internal_error(format!("Screenshot render failed: {e}"), None))?;

        let (image_width, image_height) = png_dimensions(&png_bytes);

        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

        Ok(CallToolResult::success(vec![
            Content::text(format!(
                "Screenshot: {}x{} pixels, {}x{} terminal",
                image_width, image_height, terminal_cols, terminal_rows
            )),
            Content::image(b64, "image/png"),
        ]))
    }

    /// Read scrollback buffer (content that has scrolled above the visible screen).
    #[tool(description = "Read scrollback buffer (content that has scrolled above the visible screen). Useful for retrieving earlier command output or error messages.")]
    async fn get_scrollback(
        &self,
        params: Parameters<GetScrollbackParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let session = self
            .session_manager
            .get_session(&p.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let result = observation::handle_get_scrollback(
            &session,
            p.lines.map(|l| l as i64),
            p.search.as_deref(),
            None,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    /// Wait for a pattern or a target line count in terminal output. Does not send any input.
    #[tool(description = "Wait for a pattern to appear in terminal output, or for a target number of new output lines. Does not send any input. Use for monitoring long-running processes, waiting for prompts, or detecting errors.")]
    async fn wait_for(
        &self,
        params: Parameters<WaitForParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let session = self
            .session_manager
            .get_session(&p.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let result = automation::handle_wait_for(
            &session,
            p.pattern.as_deref(),
            p.line_count,
            p.timeout_ms.unwrap_or(30_000),
            p.on_screen.unwrap_or(false),
            p.invert.unwrap_or(false),
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    /// Wait for the terminal to become idle based on output silence or screen stability.
    #[tool(description = "Wait for the terminal to become idle (no new output for a specified duration). Optionally watch for the visible screen to stop changing instead, which is more reliable for some TUI apps. Use when you don't know the exact completion pattern.")]
    async fn wait_for_idle(
        &self,
        params: Parameters<WaitForIdleParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let session = self
            .session_manager
            .get_session(&p.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let result = automation::handle_wait_for_idle(
            &session,
            p.stable_ms.unwrap_or(1_000),
            p.timeout_ms.unwrap_or(30_000),
            p.screen_stable.unwrap_or(false),
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    /// Wait for the child process to exit and return its exit code.
    #[tool(description = "Wait for the child process in a terminal session to exit. Returns the exit code. Use when you need to verify a process completed successfully.")]
    async fn wait_for_exit(
        &self,
        params: Parameters<WaitForExitParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let session = self
            .session_manager
            .get_session(&p.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let result = automation::handle_wait_for_exit(
            &session,
            p.timeout_ms.unwrap_or(30_000),
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    /// Get detailed session metadata and capabilities.
    #[tool(description = "Get detailed session metadata including PID, command, terminal size, status, and capabilities.")]
    async fn get_session_info(
        &self,
        params: Parameters<GetSessionInfoParams>,
    ) -> Result<CallToolResult, McpError> {
        let session = self
            .session_manager
            .get_session(&params.0.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let info = session.info().await;
        let idle_ms = session.idle_duration_ms().await;
        let args = session.config.args.clone();
        let cwd = session.config.cwd.as_deref();
        let shell_integration_status = session.shell_integration_status_str().await;

        let resp = session
            .with_vt(|vt| {
                introspection::build_session_info(
                    &info,
                    vt,
                    &args,
                    cwd,
                    idle_ms,
                    &shell_integration_status,
                )
            })
            .await;

        let json = serde_json::to_string_pretty(&resp)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // -- Tier 3 -----------------------------------------------------------

    /// Search scrollback history using a regex pattern.
    #[tool(description = "Search scrollback history using a regex pattern. Returns matching lines with surrounding context.")]
    async fn search_output(
        &self,
        params: Parameters<SearchOutputParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = &params.0;
        let session = self
            .session_manager
            .get_session(&p.session_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let context = p.context_lines.unwrap_or(2);
        let matches = session
            .scrollback_search(&p.pattern, context)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let max = p.max_results.unwrap_or(50);
        let limited: Vec<_> = matches.into_iter().take(max).collect();

        let json = serde_json::to_string_pretty(&limited)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler trait implementation
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for TerminalMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut server_impl = Implementation::default();
        server_impl.name = "terminal-mcp".to_string();
        server_impl.version = env!("CARGO_PKG_VERSION").to_string();

        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(server_impl)
        .with_instructions(
            "MCP server for interactive terminal session management. \
             Create PTY sessions, send input, observe output, and automate \
             CLI interactions. Use create_session to start, send_and_wait for \
             command execution, and get_screen for TUI apps."
                .to_string(),
        )
    }

    async fn initialize(
        &self,
        _request: rmcp::model::InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        tracing::info!("MCP client connected, initializing terminal-mcp");
        Ok(self.get_info())
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run() -> Result<()> {
    tracing::info!("MCP server starting on stdio");

    let server = TerminalMcpServer::new();
    // Start idle-session cleanup: reap sessions idle for more than 1 hour.
    server
        .session_manager
        .start_cleanup_task(std::time::Duration::from_secs(3600));
    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("MCP serve error: {:?}", e);
    })?;

    tracing::info!("terminal-mcp server running, waiting for requests");
    service.waiting().await?;

    tracing::info!("terminal-mcp server shutting down");
    Ok(())
}
