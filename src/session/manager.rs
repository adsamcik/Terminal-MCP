use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use dashmap::DashMap;
use uuid::Uuid;

use super::{Session, SessionConfig, SessionId, SessionInfo};

/// Hard cap on concurrent sessions to prevent resource exhaustion.
pub const MAX_SESSIONS: usize = 50;

/// Manages the set of active terminal sessions.
///
/// All public methods are `&self` — interior mutability is provided by
/// `DashMap` and per-session `tokio::sync::Mutex` fields.
pub struct SessionManager {
    sessions: Arc<DashMap<SessionId, Arc<Session>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
        }
    }

    /// Create a new terminal session, returning its metadata.
    pub fn create_session(&self, config: SessionConfig) -> Result<SessionInfo> {
        self.create_session_for_owner(config, None)
    }

    /// Create a new terminal session associated with the given owner key.
    pub fn create_session_for_owner(
        &self,
        config: SessionConfig,
        owner_key: Option<String>,
    ) -> Result<SessionInfo> {
        if self.sessions.len() >= MAX_SESSIONS {
            bail!("Maximum session limit ({MAX_SESSIONS}) reached");
        }
        let id = generate_id();
        let session =
            Session::new(id.clone(), config, owner_key).context("Failed to create session")?;
        let info = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(session.info())
        });
        self.sessions.insert(id, Arc::new(session));
        Ok(info)
    }

    /// Create a new terminal session (async version).
    pub async fn create_session_async(&self, config: SessionConfig) -> Result<SessionInfo> {
        self.create_session_async_for_owner(config, None).await
    }

    /// Create a new terminal session (async version) associated with the given owner key.
    pub async fn create_session_async_for_owner(
        &self,
        config: SessionConfig,
        owner_key: Option<String>,
    ) -> Result<SessionInfo> {
        if self.sessions.len() >= MAX_SESSIONS {
            bail!("Maximum session limit ({MAX_SESSIONS}) reached");
        }
        let id = generate_id();
        let session =
            Session::new(id.clone(), config, owner_key).context("Failed to create session")?;
        let info = session.info().await;
        self.sessions.insert(id, Arc::new(session));
        Ok(info)
    }

    /// Close and remove a session by ID.
    pub async fn close_session(&self, id: &str) -> Result<()> {
        let (_, session) = self
            .sessions
            .remove(id)
            .context(format!("Session not found: {id}"))?;

        // Try to unwrap the Arc so we can call close(self).
        // If other references exist, cancel + kill anyway so background tasks stop.
        match Arc::try_unwrap(session) {
            Ok(session) => {
                let _ = session.close().await;
            }
            Err(arc) => {
                tracing::warn!(session_id = id, "Session Arc has extra refs, forcing shutdown");
                arc.cancel.cancel();
                let pty = arc.pty.lock().await;
                pty.kill();
            }
        }
        Ok(())
    }

    /// List metadata for every active session.
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        self.list_sessions_visible(None).await
    }

    /// List metadata for sessions visible to the given owner key.
    pub async fn list_sessions_visible(&self, owner_key: Option<&str>) -> Vec<SessionInfo> {
        let mut infos = Vec::with_capacity(self.sessions.len());
        // Collect Arcs first to avoid holding DashMap shard locks across awaits.
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .iter()
            .filter(|entry| entry.value().is_visible_to(owner_key))
            .map(|r| Arc::clone(r.value()))
            .collect();
        for session in sessions {
            infos.push(session.info().await);
        }
        infos
    }

    /// Get a reference-counted handle to a session.
    pub fn get_session(&self, id: &str) -> Result<Arc<Session>> {
        self.get_session_visible(id, None)
    }

    /// Get a reference-counted handle to a session if it is visible to the owner key.
    pub fn get_session_visible(&self, id: &str, owner_key: Option<&str>) -> Result<Arc<Session>> {
        let session = self
            .sessions
            .get(id)
            .map(|r| Arc::clone(r.value()))
            .context(format!("Session not found: {id}"))?;

        if !session.is_visible_to(owner_key) {
            bail!("Session not found: {id}");
        }

        Ok(session)
    }

    /// Execute a closure with a visible session reference, returning its result.
    pub fn with_session_visible<F, R>(&self, id: &str, owner_key: Option<&str>, f: F) -> Result<R>
    where
        F: FnOnce(&Session) -> R,
    {
        let session = self.get_session_visible(id, owner_key)?;
        Ok(f(session.as_ref()))
    }

    /// Execute a closure with a session reference, returning its result.
    pub fn with_session<F, R>(&self, id: &str, f: F) -> Result<R>
    where
        F: FnOnce(&Session) -> R,
    {
        self.with_session_visible(id, None, f)
    }

    /// Number of active sessions.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether there are no active sessions.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Start a background task that periodically closes idle sessions.
    ///
    /// Sessions that have been idle (no PTY output) longer than
    /// `idle_timeout` are automatically closed.  The task runs every 30 s.
    pub fn start_cleanup_task(&self, idle_timeout: Duration) -> tokio::task::JoinHandle<()> {
        self.start_cleanup_task_with_interval(idle_timeout, Duration::from_secs(30))
    }

    /// Start a background task that periodically closes idle sessions with a custom interval.
    pub fn start_cleanup_task_with_interval(
        &self,
        idle_timeout: Duration,
        poll_interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(poll_interval).await;
                let snapshots: Vec<(SessionId, Arc<Session>)> = sessions
                    .iter()
                    .map(|entry| (entry.key().clone(), Arc::clone(entry.value())))
                    .collect();
                let mut to_remove = Vec::new();

                for (id, session) in snapshots {
                    if session.is_idle(idle_timeout).await {
                        to_remove.push(id);
                    }
                }

                for id in to_remove {
                    tracing::info!(session_id = %id, "Auto-closing idle session");
                    if let Some((_, session)) = sessions.remove(&id) {
                        match Arc::try_unwrap(session) {
                            Ok(s) => {
                                let _ = s.close().await;
                            }
                            Err(arc) => {
                                arc.cancel.cancel();
                                {
                                    let pty = arc.pty.lock().await;
                                    pty.kill();
                                }
                                tracing::warn!(
                                    session_id = %id,
                                    "Session had active references during cleanup; forcing shutdown"
                                );
                            }
                        }
                    }
                }
            }
        })
    }
}

/// Generate a full 32-character hex ID from a UUID v4.
fn generate_id() -> String {
    Uuid::new_v4().simple().to_string()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_sessions_is_fifty() {
        assert_eq!(MAX_SESSIONS, 50);
    }
}
