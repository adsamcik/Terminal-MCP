use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::scrollback::ScrollbackBuffer;
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

// -- Session ----------------------------------------------------------------

/// A live terminal session binding a PTY driver, VT parser, and output log.
pub struct Session {
    pub id: SessionId,
    pub config: SessionConfig,
    pub(crate) pty: Arc<Mutex<PtyDriver>>,
    /// Cached writer handle — avoids locking the PTY mutex for writes
    /// (which would deadlock with the reader task).
    writer: Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>>,
    vt: Arc<Mutex<VtParser>>,
    /// Raw output log (full history, VT sequences included).
    output_log: Arc<Mutex<Vec<u8>>>,
    /// Delta tracking position for `read_new_output`.
    read_position: Arc<Mutex<usize>>,
    /// In-memory scrollback buffer with timestamps and search.
    scrollback_buf: Arc<Mutex<ScrollbackBuffer>>,
    created_at: Instant,
    /// Updated every time new PTY output arrives.
    last_activity: Arc<Mutex<Instant>>,
    /// Cancels the background reader task on close/drop.
    pub(crate) cancel: CancellationToken,
}

impl Session {
    /// Spawn a new session from the given configuration.
    ///
    /// Creates the PTY + VT parser and starts a background tokio task that
    /// continuously reads PTY output, feeds it through the VT parser, and
    /// appends it to the output log.
    pub fn new(id: SessionId, config: SessionConfig) -> Result<Self> {
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

        let now = Instant::now();
        let cancel = CancellationToken::new();

        let session = Self {
            id,
            config,
            pty: Arc::new(Mutex::new(pty)),
            writer,
            vt: Arc::new(Mutex::new(vt)),
            output_log: Arc::new(Mutex::new(Vec::new())),
            read_position: Arc::new(Mutex::new(0)),
            scrollback_buf: Arc::new(Mutex::new(ScrollbackBuffer::new(
                pty_config.rows as usize * 100,
            ))),
            created_at: now,
            last_activity: Arc::new(Mutex::new(now)),
            cancel: cancel.clone(),
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
                            log.extend_from_slice(&data);
                        }
                        {
                            let text = String::from_utf8_lossy(&data);
                            let mut sb = scrollback_buf.lock().await;
                            sb.append(&text);
                        }
                        {
                            let mut ts = last_activity.lock().await;
                            *ts = Instant::now();
                        }
                    }
                    None => {
                        tracing::debug!("PTY reader reached EOF");
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

    // -- Output -------------------------------------------------------------

    /// Return output received since the last call (delta mode).
    pub async fn read_new_output(&self) -> Vec<u8> {
        let log = self.output_log.lock().await;
        let mut pos = self.read_position.lock().await;
        let start = *pos;
        *pos = log.len();
        log[start..].to_vec()
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

    /// The entire raw output log since session creation.
    pub async fn get_full_output(&self) -> Vec<u8> {
        let log = self.output_log.lock().await;
        log.clone()
    }

    /// Regex search across the output log (interpreted as UTF-8 lossy).
    pub async fn search_output(&self, pattern: &str) -> Result<Vec<SearchMatch>> {
        let re = Regex::new(pattern).context("Invalid regex pattern")?;
        let log = self.output_log.lock().await;
        let text = String::from_utf8_lossy(&log);

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

    /// Build a `SessionInfo` snapshot for the current state.
    pub async fn info(&self) -> SessionInfo {
        let alive = self.is_alive().await;
        let pid = self.pid().await;
        let idle = self.is_idle(Duration::from_secs(2)).await;

        let status = if alive {
            if idle {
                SessionStatus::Idle
            } else {
                SessionStatus::Running
            }
        } else {
            SessionStatus::Exited { code: None }
        };

        let command = self
            .config
            .command
            .clone()
            .unwrap_or_else(|| "(default shell)".to_string());

        let elapsed = self.created_at.elapsed();
        let created_at = format!("{}s ago", elapsed.as_secs());

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