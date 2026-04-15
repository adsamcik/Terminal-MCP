use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::scrollback::ScrollbackBuffer;
use crate::shell_integration::{IntegrationStatus, ShellIntegration};
use crate::terminal::{ColorSpan, PtyConfig, PtyDriver, PtyReader, VtParser};

// -- Public types -----------------------------------------------------------

pub type SessionId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    pub rows: u16,
    pub cols: u16,
    #[serde(default = "default_scrollback")]
    pub scrollback: usize,
}

fn default_scrollback() -> usize {
    1000
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            command: None,
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            rows: 24,
            cols: 80,
            scrollback: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionStatus {
    Running,
    Idle,
    Exited { code: Option<i32> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub pid: Option<u32>,
    pub command: String,
    pub rows: u16,
    pub cols: u16,
    pub status: SessionStatus,
    pub created_at: String,
}

/// A single regex search match in the output log.
#[derive(Debug, Clone, Serialize)]
pub struct SearchMatch {
    pub byte_offset: usize,
    pub line_number: usize,
    pub line: String,
    pub match_text: String,
}

/// Maximum raw PTY output retained per session.
pub const MAX_OUTPUT_LOG_BYTES: usize = 50 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub(crate) struct OutputDelta {
    pub bytes: Vec<u8>,
    pub dropped_bytes: usize,
}

#[derive(Debug)]
struct RetainedOutput {
    data: VecDeque<u8>,
    start_offset: u64,
    max_bytes: usize,
}

impl RetainedOutput {
    fn new(max_bytes: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(max_bytes.min(4096)),
            start_offset: 0,
            max_bytes,
        }
    }

    fn append(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }

        if chunk.len() >= self.max_bytes {
            let dropped_existing = self.data.len() as u64;
            let dropped_from_chunk = (chunk.len() - self.max_bytes) as u64;
            self.start_offset = self
                .start_offset
                .saturating_add(dropped_existing)
                .saturating_add(dropped_from_chunk);
            self.data.clear();
            self.data
                .extend(chunk[chunk.len() - self.max_bytes..].iter().copied());
            return;
        }

        let overflow = self
            .data
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(self.max_bytes);
        if overflow > 0 {
            for _ in 0..overflow {
                let _ = self.data.pop_front();
            }
            self.start_offset = self.start_offset.saturating_add(overflow as u64);
        }

        self.data.extend(chunk.iter().copied());
    }

    fn read_from(&self, cursor: &mut u64) -> OutputDelta {
        let end_offset = self.start_offset + self.data.len() as u64;
        let mut dropped_bytes = 0;

        if *cursor < self.start_offset {
            dropped_bytes = (self.start_offset - *cursor) as usize;
            *cursor = self.start_offset;
        }

        if *cursor > end_offset {
            *cursor = end_offset;
            return OutputDelta {
                bytes: Vec::new(),
                dropped_bytes,
            };
        }

        let relative_start = (*cursor - self.start_offset) as usize;
        let bytes = self.data.iter().skip(relative_start).copied().collect();
        *cursor = end_offset;

        OutputDelta {
            bytes,
            dropped_bytes,
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        self.data.iter().copied().collect()
    }
}

// -- Session ----------------------------------------------------------------

/// A live terminal session binding a PTY driver, VT parser, and output log.
pub struct Session {
    pub id: SessionId,
    pub config: SessionConfig,
    owner_key: Option<String>,
    pub(crate) pty: Arc<Mutex<PtyDriver>>,
    /// Cached writer handle — avoids locking the PTY mutex for writes
    /// (which would deadlock with the reader task).
    writer: Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>>,
    vt: Arc<Mutex<VtParser>>,
    /// Raw PTY output retained in a capped buffer (VT sequences included).
    output_log: Arc<Mutex<RetainedOutput>>,
    /// Delta tracking position for `read_new_output`.
    read_position: Arc<Mutex<u64>>,
    /// In-memory scrollback buffer with timestamps and search.
    scrollback_buf: Arc<Mutex<ScrollbackBuffer>>,
    /// Wall-clock timestamp of session creation (for ISO 8601 reporting).
    created_at: SystemTime,
    /// Updated every time new PTY output arrives.
    last_activity: Arc<Mutex<Instant>>,
    /// Cached exit code, set when the reader task detects EOF.
    exit_code: Arc<Mutex<Option<i32>>>,
    /// Cancels the background reader task on close/drop.
    pub(crate) cancel: CancellationToken,
    /// Shell integration state, updated as OSC sequences arrive from the PTY.
    shell_integration: Arc<Mutex<ShellIntegration>>,
}

/// Scan raw PTY bytes for complete OSC sequences and return their payloads.
///
/// OSC format: `ESC ] <payload> (BEL | ESC \)`
/// Only sequences that are fully contained within `data` are returned.
fn extract_osc_payloads(data: &[u8]) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut i = 0;
    while i + 1 < data.len() {
        if data[i] == 0x1b && data[i + 1] == b']' {
            let start = i + 2;
            let mut j = start;
            loop {
                if j >= data.len() {
                    return payloads;
                }
                if data[j] == 0x07 {
                    if let Ok(s) = std::str::from_utf8(&data[start..j]) {
                        payloads.push(s.to_string());
                    }
                    i = j + 1;
                    break;
                } else if j + 1 < data.len() && data[j] == 0x1b && data[j + 1] == b'\\' {
                    if let Ok(s) = std::str::from_utf8(&data[start..j]) {
                        payloads.push(s.to_string());
                    }
                    i = j + 2;
                    break;
                } else {
                    j += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    payloads
}

impl Session {
    /// Spawn a new session from the given configuration.
    ///
    /// Creates the PTY + VT parser and starts a background tokio task that
    /// continuously reads PTY output, feeds it through the VT parser, and
    /// appends it to the output log.
    pub fn new(id: SessionId, config: SessionConfig, owner_key: Option<String>) -> Result<Self> {
        let pty_config = PtyConfig {
            command: config.command.clone().unwrap_or_default(),
            args: config.args.clone(),
            cwd: config.cwd.clone(),
            env: config.env.clone(),
            rows: config.rows,
            cols: config.cols,
        };

        let (pty, pty_reader) = PtyDriver::spawn(&pty_config).context("Failed to spawn PTY")?;
        let writer = pty.writer_handle();
        let vt = VtParser::new(config.rows, config.cols, config.scrollback);
        let scrollback_limit = config.scrollback;

        let now_instant = Instant::now();
        let created_at = SystemTime::now();
        let cancel = CancellationToken::new();

        let session = Self {
            id,
            config,
            owner_key,
            pty: Arc::new(Mutex::new(pty)),
            writer,
            vt: Arc::new(Mutex::new(vt)),
            output_log: Arc::new(Mutex::new(RetainedOutput::new(MAX_OUTPUT_LOG_BYTES))),
            read_position: Arc::new(Mutex::new(0)),
            scrollback_buf: Arc::new(Mutex::new(ScrollbackBuffer::new(scrollback_limit))),
            created_at,
            last_activity: Arc::new(Mutex::new(now_instant)),
            exit_code: Arc::new(Mutex::new(None)),
            cancel: cancel.clone(),
            shell_integration: Arc::new(Mutex::new(ShellIntegration::new())),
        };

        // Start the background reader task
        session.spawn_reader(cancel, pty_reader);

        Ok(session)
    }

    /// Spawn the background task that reads from the PTY and feeds the VT
    /// parser + output log.
    fn spawn_reader(&self, cancel: CancellationToken, mut pty_reader: PtyReader) {
        let vt = Arc::clone(&self.vt);
        let output_log = Arc::clone(&self.output_log);
        let last_activity = Arc::clone(&self.last_activity);
        let scrollback_buf = Arc::clone(&self.scrollback_buf);
        let pty_for_exit = Arc::clone(&self.pty);
        let exit_code_store = Arc::clone(&self.exit_code);
        let shell_integration = Arc::clone(&self.shell_integration);

        tokio::spawn(async move {
            loop {
                let chunk = tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::debug!("PTY reader cancelled");
                        return;
                    }
                    chunk = pty_reader.read() => chunk,
                };

                match chunk {
                    Some(data) => {
                        {
                            let mut vt_guard = vt.lock().await;
                            vt_guard.process(&data);
                        }
                        {
                            let mut log = output_log.lock().await;
                            log.append(&data);
                        }
                        {
                            let text = String::from_utf8_lossy(&data);
                            let mut sb = scrollback_buf.lock().await;
                            sb.append(&text);
                        }
                        {
                            let payloads = extract_osc_payloads(&data);
                            if !payloads.is_empty() {
                                let mut si = shell_integration.lock().await;
                                for payload in payloads {
                                    si.process_osc(&payload);
                                }
                            }
                        }
                        {
                            let mut ts = last_activity.lock().await;
                            *ts = Instant::now();
                        }
                    }
                    None => {
                        tracing::debug!("PTY reader reached EOF");
                        // Try to capture exit code before returning
                        if let Ok(pty_guard) = pty_for_exit.try_lock() {
                            if let Some(code) = pty_guard.try_exit_code() {
                                let mut ec = exit_code_store.lock().await;
                                *ec = Some(code);
                                tracing::debug!(exit_code = code, "Captured exit code at EOF");
                            }
                        }
                        return;
                    }
                }
            }
        });
    }

    // -- Input --------------------------------------------------------------

    /// Write raw bytes to the PTY (input to the child process).
    pub async fn write_bytes(&self, data: &[u8]) -> Result<()> {
        let writer = Arc::clone(&self.writer);
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut w = writer
                .lock()
                .map_err(|e| anyhow::anyhow!("Writer lock poisoned: {e}"))?;
            use std::io::Write;
            w.write_all(&data).context("Failed to write to PTY")?;
            w.flush().context("Failed to flush PTY writer")
        })
        .await
        .context("Writer task panicked")?
    }

    /// Type UTF-8 text into the PTY one character at a time.
    ///
    /// Raw-input TUIs often distinguish between typed characters and pasted
    /// chunks, so `send_text` must preserve per-character delivery semantics.
    pub async fn write_text(&self, text: &str, delay_between: Option<Duration>) -> Result<()> {
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            let mut buf = [0u8; 4];
            let bytes = ch.encode_utf8(&mut buf);
            self.write_bytes(bytes.as_bytes()).await?;

            if chars.peek().is_some() {
                if let Some(delay) = delay_between {
                    tokio::time::sleep(delay).await;
                }
            }
        }

        Ok(())
    }

    // -- Output -------------------------------------------------------------

    /// Return output received since the last call (delta mode).
    pub(crate) async fn read_new_output_chunk(&self) -> OutputDelta {
        let log = self.output_log.lock().await;
        let mut pos = self.read_position.lock().await;
        log.read_from(&mut *pos)
    }

    /// Return output received since the last call (delta mode).
    pub async fn read_new_output(&self) -> Vec<u8> {
        self.read_new_output_chunk().await.bytes
    }

    /// Plain-text content of the visible terminal screen.
    pub async fn get_screen_contents(&self) -> String {
        let vt = self.vt.lock().await;
        vt.screen_contents()
    }

    /// Screen text with cursor position marked.
    pub async fn get_screen_contents_with_cursor(&self) -> String {
        let vt = self.vt.lock().await;
        vt.screen_contents_with_cursor()
    }

    /// Color/attribute spans for the current screen.
    pub async fn get_screen_with_colors(&self) -> Vec<ColorSpan> {
        let vt = self.vt.lock().await;
        vt.color_spans()
    }

    /// The entire retained raw output log.
    pub async fn get_full_output(&self) -> Vec<u8> {
        let log = self.output_log.lock().await;
        log.snapshot()
    }

    /// Regex search across the retained output log (interpreted as UTF-8 lossy).
    pub async fn search_output(&self, pattern: &str) -> Result<Vec<SearchMatch>> {
        let re = RegexBuilder::new(pattern)
            .size_limit(1_000_000)
            .build()
            .context("Invalid regex pattern")?;
        let log = self.output_log.lock().await;
        let snapshot = log.snapshot();
        let text = String::from_utf8_lossy(&snapshot);

        let mut matches = Vec::new();
        for (line_number, line) in text.lines().enumerate() {
            if let Some(m) = re.find(line) {
                matches.push(SearchMatch {
                    byte_offset: m.start(),
                    line_number,
                    line: line.to_string(),
                    match_text: m.as_str().to_string(),
                });
            }
        }
        Ok(matches)
    }

    // -- Scrollback ---------------------------------------------------------

    /// Get the last N lines from the scrollback buffer.
    pub async fn scrollback_tail(&self, n: usize) -> Vec<String> {
        let sb = self.scrollback_buf.lock().await;
        sb.tail(n).into_iter().map(String::from).collect()
    }

    /// Get a range of lines from the scrollback buffer.
    pub async fn scrollback_range(&self, start: usize, count: usize) -> Vec<String> {
        let sb = self.scrollback_buf.lock().await;
        sb.range(start, count).into_iter().map(String::from).collect()
    }

    /// Search the scrollback buffer with a regex pattern.
    pub async fn scrollback_search(
        &self,
        pattern: &str,
        context_lines: usize,
    ) -> Result<Vec<crate::scrollback::SearchMatch>> {
        let sb = self.scrollback_buf.lock().await;
        sb.search(pattern, context_lines)
    }

    /// Total number of lines in the scrollback buffer.
    pub async fn scrollback_len(&self) -> usize {
        let sb = self.scrollback_buf.lock().await;
        sb.len()
    }

    // -- State queries ------------------------------------------------------

    /// Whether the session has been idle for at least `threshold`.
    pub async fn is_idle(&self, threshold: Duration) -> bool {
        let ts = self.last_activity.lock().await;
        ts.elapsed() >= threshold
    }

    /// Milliseconds since the last PTY output activity.
    pub async fn idle_duration_ms(&self) -> u64 {
        let ts = self.last_activity.lock().await;
        ts.elapsed().as_millis() as u64
    }

    /// Current cursor position as `(row, col)`.
    pub async fn cursor_position(&self) -> (u16, u16) {
        let vt = self.vt.lock().await;
        vt.cursor_position()
    }

    /// Whether the alternate screen buffer is active (e.g. vim, less).
    ///
    /// See [`VtParser::is_alternate_screen`] for a known Windows/ConPTY
    /// caveat: ConPTY handles DECSET 1049 internally and does not relay the
    /// escape sequence, so this will always return `false` on Windows.
    pub async fn is_alternate_screen(&self) -> bool {
        let vt = self.vt.lock().await;
        vt.is_alternate_screen()
    }

    /// Whether application cursor key mode is enabled (DECCKM).
    pub async fn application_cursor(&self) -> bool {
        let vt = self.vt.lock().await;
        vt.application_cursor()
    }

    /// Process ID of the child running in the PTY.
    pub async fn pid(&self) -> Option<u32> {
        let pty = self.pty.lock().await;
        pty.pid()
    }

    /// Whether the child process is still running.
    pub async fn is_alive(&self) -> bool {
        let pty = self.pty.lock().await;
        pty.is_alive()
    }

    /// The cached exit code of the child process, if it has exited.
    pub async fn exit_code(&self) -> Option<i32> {
        // First check cache
        let cached = self.exit_code.lock().await;
        if cached.is_some() {
            return *cached;
        }
        drop(cached);

        // If not cached yet but process is dead, try to get it now
        if !self.is_alive().await {
            let pty = self.pty.lock().await;
            // Re-check cache — reader task may have populated it while we
            // were waiting for the pty lock.
            let mut ec = self.exit_code.lock().await;
            if ec.is_some() {
                return *ec;
            }
            if let Some(code) = pty.try_exit_code() {
                *ec = Some(code);
                return Some(code);
            }
        }
        None
    }

    /// Return only the exit code that was captured naturally by the reader
    /// task at EOF. Returns `None` if the exit code is unknown, even if the
    /// process is no longer alive.
    pub async fn cached_exit_code(&self) -> Option<i32> {
        *self.exit_code.lock().await
    }

    /// Current shell integration status as a human-readable string:
    /// `"detecting"`, `"active"`, `"injected"`, or `"unavailable"`.
    pub async fn shell_integration_status_str(&self) -> String {
        let si = self.shell_integration.lock().await;
        match si.status() {
            IntegrationStatus::Detecting => "detecting",
            IntegrationStatus::ExternalActive => "active",
            IntegrationStatus::Injected => "injected",
            IntegrationStatus::Unavailable => "unavailable",
        }
        .to_string()
    }

    /// Whether this session is visible to the given owner key.
    pub fn is_visible_to(&self, owner_key: Option<&str>) -> bool {
        match owner_key {
            Some(owner_key) => self.owner_key.as_deref() == Some(owner_key),
            None => true,
        }
    }

    /// Build a `SessionInfo` snapshot for the current state.
    pub async fn info(&self) -> SessionInfo {
        let alive = self.is_alive().await;
        let pid = self.pid().await;
        let idle = self.is_idle(Duration::from_secs(2)).await;

        let exit_code = self.exit_code().await;
        let status = if alive {
            if idle {
                SessionStatus::Idle
            } else {
                SessionStatus::Running
            }
        } else {
            SessionStatus::Exited { code: exit_code }
        };

        let command = self
            .config
            .command
            .clone()
            .unwrap_or_else(|| "(default shell)".to_string());

        let created_at: DateTime<Utc> = self.created_at.into();
        let created_at = created_at.to_rfc3339();

        SessionInfo {
            session_id: self.id.clone(),
            pid,
            command,
            rows: self.config.rows,
            cols: self.config.cols,
            status,
            created_at,
        }
    }

    /// Access the VT parser under lock (e.g. for scrollback, screenshots).
    pub async fn with_vt<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut VtParser) -> R,
    {
        let mut vt = self.vt.lock().await;
        f(&mut vt)
    }

    // -- Lifecycle ----------------------------------------------------------

    /// Resize the PTY and VT parser to new dimensions.
    pub async fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let pty = self.pty.lock().await;
        pty.resize(rows, cols)?;

        let mut vt = self.vt.lock().await;
        vt.set_size(rows, cols);

        Ok(())
    }

    /// Cancel the reader task and kill the child process.
    /// Can be called without consuming self — used by Drop and close_session fallback.
    pub fn shutdown(&self) {
        self.cancel.cancel();
        // Kill the child process so the blocking reader thread unblocks
        let pty = self.pty.blocking_lock();
        pty.kill();
    }

    /// Close the session: cancel the reader, close the PTY, return exit status.
    pub async fn close(self) -> Result<Option<portable_pty::ExitStatus>> {
        self.cancel.cancel();

        // Kill the child so the blocking reader thread returns
        {
            let pty = self.pty.lock().await;
            pty.kill();
        }

        // Wait briefly for the reader task to finish
        tokio::time::sleep(Duration::from_millis(100)).await;

        // We can't move out of self.pty because Session implements Drop,
        // so just ensure the child is killed and let Drop handle cleanup.
        Ok(None)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.cancel();
        // Best-effort kill: try_lock to avoid blocking in drop
        if let Ok(pty) = self.pty.try_lock() {
            pty.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RetainedOutput, MAX_OUTPUT_LOG_BYTES};

    #[test]
    fn retained_output_reads_new_bytes_without_loss() {
        let mut buffer = RetainedOutput::new(16);
        let mut cursor = 0;

        buffer.append(b"hello");
        let first = buffer.read_from(&mut cursor);
        assert_eq!(first.bytes, b"hello");
        assert_eq!(first.dropped_bytes, 0);

        buffer.append(b" world");
        let second = buffer.read_from(&mut cursor);
        assert_eq!(second.bytes, b" world");
        assert_eq!(second.dropped_bytes, 0);
    }

    #[test]
    fn retained_output_reports_dropped_unread_bytes() {
        let mut buffer = RetainedOutput::new(5);
        let mut cursor = 0;

        buffer.append(b"abcdef");
        let delta = buffer.read_from(&mut cursor);

        assert_eq!(delta.bytes, b"bcdef");
        assert_eq!(delta.dropped_bytes, 1);
    }

    #[test]
    fn retained_output_keeps_only_recent_snapshot() {
        let mut buffer = RetainedOutput::new(4);
        buffer.append(b"ab");
        buffer.append(b"cdef");

        assert_eq!(buffer.snapshot(), b"cdef");
    }

    #[test]
    fn output_log_budget_matches_hardening_plan() {
        assert_eq!(MAX_OUTPUT_LOG_BYTES, 50 * 1024 * 1024);
    }
}
