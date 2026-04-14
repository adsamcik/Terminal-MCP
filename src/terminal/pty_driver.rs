use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;
use tracing;

/// Configuration for spawning a PTY session.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    /// Command to execute. If empty, the default shell is used.
    pub command: String,
    /// Arguments for the command.
    pub args: Vec<String>,
    /// Working directory. If `None`, inherits the current directory.
    pub cwd: Option<String>,
    /// Additional environment variables merged into the child's environment.
    pub env: HashMap<String, String>,
    /// Terminal height in rows.
    pub rows: u16,
    /// Terminal width in columns.
    pub cols: u16,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            rows: 24,
            cols: 80,
        }
    }
}

/// Cross-platform PTY driver wrapping `portable-pty`.
///
/// Bridges the synchronous `portable-pty` I/O to async Tokio via channels
/// and `spawn_blocking` tasks.
pub struct PtyDriver {
    /// Synchronous writer handle wrapped for thread-safe access.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Handle to the master side of the PTY (for resize operations).
    master: Box<dyn MasterPty + Send>,
    /// The child process spawned inside the PTY.
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    /// A cloned killer for signalling the child independently.
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    /// Cached process ID of the child (if available).
    process_id: Option<u32>,
}

/// Receives output bytes from the PTY reader background task.
/// Separated from PtyDriver so it can be awaited without holding the PTY lock.
pub struct PtyReader {
    rx: mpsc::Receiver<Vec<u8>>,
}

impl PtyReader {
    /// Read the next chunk of output from the PTY.
    /// Returns `None` when the PTY closes (EOF / child exited).
    pub async fn read(&mut self) -> Option<Vec<u8>> {
        self.rx.recv().await
    }
}

impl PtyDriver {
    /// Spawn a new PTY session according to the given configuration.
    ///
    /// This sets up the PTY pair, spawns the child process, and starts
    /// an async background task that reads output from the PTY master
    /// and forwards it through an `mpsc` channel.
    pub fn spawn(config: &PtyConfig) -> Result<(Self, PtyReader)> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows: config.rows,
            cols: config.cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system
            .openpty(size)
            .context("Failed to open PTY pair")?;

        // Destructure the pair to get separate ownership of master and slave.
        let master = pair.master;
        let slave = pair.slave;

        // Build the command
        let command = if config.command.is_empty() {
            detect_default_shell()
        } else {
            config.command.clone()
        };

        let mut cmd = CommandBuilder::new(&command);
        for arg in &config.args {
            cmd.arg(arg);
        }
        if let Some(ref cwd) = config.cwd {
            cmd.cwd(cwd);
        }
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let child = slave
            .spawn_command(cmd)
            .with_context(|| format!("Failed to spawn command: {command}"))?;

        // CRITICAL: Drop the slave handle immediately after spawning the child.
        // On Windows ConPTY, keeping the slave pipe handles open in the parent
        // process prevents output from flowing through the master's read pipe.
        drop(slave);

        let process_id = child.process_id();
        let killer = child.clone_killer();

        // Obtain reader and writer handles from the master PTY
        let reader = master
            .try_clone_reader()
            .context("Failed to obtain PTY reader")?;
        let mut writer = master
            .take_writer()
            .context("Failed to obtain PTY writer")?;

        // On Windows, portable-pty creates the ConPTY with the
        // PSUEDOCONSOLE_INHERIT_CURSOR flag. This causes ConPTY to send a
        // Device Status Report request (ESC[6n, 4 bytes) through the output
        // pipe and then *block* until it receives a cursor position response
        // (ESC[row;colR) on the input pipe. Without this handshake, child
        // process output never flows through the master's read pipe.
        #[cfg(windows)]
        {
            let _ = writer.write_all(b"\x1b[?1;0c\x1b[1;1R");
            let _ = writer.flush();
        }

        // Channel for forwarding PTY output to the async world
        let (reader_tx, reader_rx) = mpsc::channel::<Vec<u8>>(64);

        // Background task: read from the PTY master in a blocking thread
        tokio::task::spawn_blocking(move || {
            pty_reader_task(reader, reader_tx);
        });

        tracing::info!(pid = process_id, command = %command, "PTY session spawned");

        let driver = Self {
            writer: Arc::new(Mutex::new(writer)),
            master,
            child: Arc::new(Mutex::new(child)),
            killer: Arc::new(Mutex::new(killer)),
            process_id,
        };
        let reader = PtyReader { rx: reader_rx };

        Ok((driver, reader))
    }

    /// Write data to the PTY (sends input to the child process).
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let writer = Arc::clone(&self.writer);
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut w = writer
                .lock()
                .map_err(|e| anyhow::anyhow!("Writer lock poisoned: {e}"))?;
            w.write_all(&data).context("Failed to write to PTY")?;
            w.flush().context("Failed to flush PTY writer")
        })
        .await
        .context("Writer task panicked")?
    }

    /// Get a clone of the writer handle for use outside the PtyDriver lock.
    pub fn writer_handle(&self) -> Arc<Mutex<Box<dyn Write + Send>>> {
        Arc::clone(&self.writer)
    }

    /// Resize the PTY to the given dimensions.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")
    }

    /// Return the process ID of the child, if available.
    pub fn pid(&self) -> Option<u32> {
        self.process_id
    }

    /// Kill the child process (best-effort, for use in Drop).
    pub fn kill(&self) {
        if let Ok(mut k) = self.killer.lock() {
            let _ = k.kill();
        }
    }

    /// Check whether the child process is still running.
    pub fn is_alive(&self) -> bool {
        if let Ok(mut child) = self.child.lock() {
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }

    /// Close the PTY session.
    ///
    /// Kills the child process (if still running), drops the writer to send
    /// EOF, and waits for the child to exit. Returns the exit status.
    pub async fn close(self) -> Result<Option<portable_pty::ExitStatus>> {
        // Drop the writer to signal EOF to the child
        drop(self.writer);

        let child = self.child;
        let killer = self.killer;

        let status = tokio::task::spawn_blocking(move || -> Result<Option<portable_pty::ExitStatus>> {
            let mut child_guard = child
                .lock()
                .map_err(|e| anyhow::anyhow!("Child lock poisoned: {e}"))?;

            // Check if already exited
            match child_guard.try_wait() {
                Ok(Some(status)) => return Ok(Some(status)),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "Error checking child status");
                }
            }

            // Kill the child
            if let Ok(mut k) = killer.lock() {
                if let Err(e) = k.kill() {
                    tracing::warn!(error = %e, "Error killing child process");
                }
            }

            // Wait for exit
            match child_guard.wait() {
                Ok(status) => Ok(Some(status)),
                Err(e) => {
                    tracing::warn!(error = %e, "Error waiting for child exit");
                    Ok(None)
                }
            }
        })
        .await
        .context("Close task panicked")??;

        Ok(status)
    }
}

/// Background task that continuously reads from the PTY master and sends
/// chunks through the channel. Exits when the PTY closes (EOF) or the
/// receiver is dropped.
fn pty_reader_task(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                tracing::debug!("PTY reader reached EOF");
                break;
            }
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    tracing::debug!("PTY reader channel closed");
                    break;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "PTY read error (likely closed)");
                break;
            }
        }
    }
}

/// Detect the default shell for the current platform.
fn detect_default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}
