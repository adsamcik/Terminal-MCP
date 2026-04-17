//! End-to-end lifecycle tests for terminal-mcp session management.
//! Run with: cargo test --test e2e_lifecycle -- --test-threads=1 --nocapture

use std::collections::HashMap;
use std::time::Duration;

use terminal_mcp::session::{SessionConfig, SessionManager, SessionStatus};
use tokio::time::sleep;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_config() -> SessionConfig {
    SessionConfig::default()
}

fn cmd_config() -> SessionConfig {
    SessionConfig {
        command: Some("cmd.exe".to_string()),
        args: vec![],
        cwd: None,
        env: Default::default(),
        rows: 24,
        cols: 80,
        scrollback: 1000,
    }
}

/// Close all sessions in a manager (best-effort cleanup).
async fn cleanup(mgr: &SessionManager) {
    let sessions = mgr.list_sessions().await;
    for s in sessions {
        let _ = mgr.close_session(&s.session_id).await;
    }
}

// ---------------------------------------------------------------------------
// 1. create_session — default shell
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_session_default_shell() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(default_config()).await.unwrap();

    assert!(!info.session_id.is_empty(), "session_id must not be empty");
    assert!(
        info.pid.is_some(),
        "pid should be present for a running session"
    );
    assert_eq!(info.rows, 24);
    assert_eq!(info.cols, 80);

    cleanup(&mgr).await;
}

// ---------------------------------------------------------------------------
// 2. create_session — custom command
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_session_custom_command() {
    let mgr = SessionManager::new();
    let config = SessionConfig {
        command: Some("cmd.exe".to_string()),
        args: vec!["/c".to_string(), "echo test".to_string()],
        ..default_config()
    };
    let info = mgr.create_session_async(config).await.unwrap();

    assert!(!info.session_id.is_empty());
    assert!(info.pid.is_some());

    // Wait for short-lived command to produce output
    let session = mgr.get_session(&info.session_id).unwrap();
    sleep(Duration::from_secs(3)).await;

    let raw = session.get_full_output().await;
    assert!(
        !raw.is_empty(),
        "Custom command should produce some PTY output"
    );

    drop(session);
    cleanup(&mgr).await;
}

// ---------------------------------------------------------------------------
// 3. create_session — custom cwd
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_session_custom_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = tmp.path().to_string_lossy().to_string();

    let mgr = SessionManager::new();
    let config = SessionConfig {
        command: Some("cmd.exe".to_string()),
        cwd: Some(tmp_path.clone()),
        ..default_config()
    };
    let info = mgr.create_session_async(config).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;

    // Write `cd` and wait for output — cd on Windows prints the current dir.
    session.write_bytes(b"cd\r").await.unwrap();
    sleep(Duration::from_secs(2)).await;

    let raw = session.get_full_output().await;
    let raw_text = String::from_utf8_lossy(&raw);
    let screen = session.get_screen_contents().await;
    let found = raw_text.contains(&tmp_path) || screen.contains(&tmp_path);

    // Tempdir paths may use short-name aliases on Windows (e.g. C:\Users\USERNA~1\...).
    // Also check a case-insensitive prefix of the temp path.
    let canon_lower = tmp_path.to_lowercase();
    let raw_lower = raw_text.to_lowercase();
    let screen_lower = screen.to_lowercase();
    let found_ci = raw_lower.contains(&canon_lower) || screen_lower.contains(&canon_lower);

    assert!(
        found || found_ci,
        "Expected temp dir path '{}' in output or screen.\nraw({} bytes): ...{}\nscreen: {}",
        tmp_path,
        raw.len(),
        &raw_text[raw_text.len().saturating_sub(300)..],
        &screen[..screen.len().min(500)]
    );

    drop(session);
    cleanup(&mgr).await;
}

// ---------------------------------------------------------------------------
// 4. create_session — custom env
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_session_custom_env() {
    let mgr = SessionManager::new();
    let mut env = HashMap::new();
    env.insert("MY_TEST_VAR".to_string(), "test123".to_string());

    let config = SessionConfig {
        command: Some("cmd.exe".to_string()),
        env,
        ..default_config()
    };
    let info = mgr.create_session_async(config).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;

    session.write_bytes(b"echo %MY_TEST_VAR%\r").await.unwrap();
    sleep(Duration::from_secs(2)).await;

    let raw = session.get_full_output().await;
    let raw_text = String::from_utf8_lossy(&raw);
    let screen = session.get_screen_contents().await;
    let found = raw_text.contains("test123") || screen.contains("test123");

    assert!(
        found,
        "Expected 'test123' in output.\nraw: ...{}\nscreen: {}",
        &raw_text[raw_text.len().saturating_sub(300)..],
        &screen[..screen.len().min(500)]
    );

    drop(session);
    cleanup(&mgr).await;
}

// ---------------------------------------------------------------------------
// 5. create_session — invalid command
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_session_invalid_command() {
    let mgr = SessionManager::new();
    let config = SessionConfig {
        command: Some("nonexistent_binary_xyz".to_string()),
        ..default_config()
    };
    let result = mgr.create_session_async(config).await;

    assert!(
        result.is_err(),
        "Creating a session with a nonexistent command should fail"
    );

    // Manager should remain empty — no leaked session.
    assert_eq!(mgr.len(), 0);
}

// ---------------------------------------------------------------------------
// 6. close_session — normal close
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn close_session_normal() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    assert_eq!(mgr.len(), 1);

    mgr.close_session(&info.session_id).await.unwrap();
    assert_eq!(mgr.len(), 0);

    // Verify it no longer appears in list
    let list = mgr.list_sessions().await;
    assert!(list.is_empty());
}

// ---------------------------------------------------------------------------
// 7. close_session — double close
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn close_session_double_close() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();

    mgr.close_session(&info.session_id).await.unwrap();

    let result = mgr.close_session(&info.session_id).await;
    assert!(result.is_err(), "Double close should return an error");

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.to_lowercase().contains("not found"),
        "Error should mention 'not found', got: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// 8. close_session — nonexistent
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn close_session_nonexistent() {
    let mgr = SessionManager::new();
    let result = mgr.close_session("bogus_session_id_12345").await;

    assert!(
        result.is_err(),
        "Closing a nonexistent session should return an error"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.to_lowercase().contains("not found"),
        "Error should mention 'not found', got: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// 9. list_sessions — empty
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn list_sessions_empty() {
    let mgr = SessionManager::new();
    let list = mgr.list_sessions().await;
    assert!(
        list.is_empty(),
        "List should be empty when no sessions exist"
    );
}

// ---------------------------------------------------------------------------
// 10. list_sessions — multiple
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn list_sessions_multiple() {
    let mgr = SessionManager::new();

    let s1 = mgr.create_session_async(cmd_config()).await.unwrap();
    let s2 = mgr.create_session_async(cmd_config()).await.unwrap();
    let s3 = mgr.create_session_async(cmd_config()).await.unwrap();

    let list = mgr.list_sessions().await;
    assert_eq!(list.len(), 3, "Should have 3 sessions");

    // Verify all 3 session IDs appear
    let ids: Vec<&str> = list.iter().map(|s| s.session_id.as_str()).collect();
    assert!(ids.contains(&s1.session_id.as_str()));
    assert!(ids.contains(&s2.session_id.as_str()));
    assert!(ids.contains(&s3.session_id.as_str()));

    // Verify metadata is correct
    for info in &list {
        assert_eq!(info.rows, 24);
        assert_eq!(info.cols, 80);
        assert!(info.pid.is_some());
        assert_eq!(info.command, "cmd.exe");
    }

    cleanup(&mgr).await;
}

// ---------------------------------------------------------------------------
// 11. list_sessions — after close
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn list_sessions_after_close() {
    let mgr = SessionManager::new();

    let s1 = mgr.create_session_async(cmd_config()).await.unwrap();
    let s2 = mgr.create_session_async(cmd_config()).await.unwrap();
    assert_eq!(mgr.len(), 2);

    // Close the first session
    mgr.close_session(&s1.session_id).await.unwrap();

    let list = mgr.list_sessions().await;
    assert_eq!(list.len(), 1, "Should have 1 session after closing one");
    assert_eq!(list[0].session_id, s2.session_id);

    cleanup(&mgr).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_sessions_visible_filters_by_owner() {
    let mgr = SessionManager::new();
    let owner_a = Some("owner-a".to_string());
    let owner_b = Some("owner-b".to_string());

    let s1 = mgr
        .create_session_async_for_owner(default_config(), owner_a.clone())
        .await
        .unwrap();
    let s2 = mgr
        .create_session_async_for_owner(default_config(), owner_a)
        .await
        .unwrap();
    let s3 = mgr
        .create_session_async_for_owner(default_config(), owner_b)
        .await
        .unwrap();

    let visible_to_a = mgr.list_sessions_visible(Some("owner-a")).await;
    let visible_ids: Vec<&str> = visible_to_a.iter().map(|s| s.session_id.as_str()).collect();

    assert_eq!(visible_to_a.len(), 2);
    assert!(visible_ids.contains(&s1.session_id.as_str()));
    assert!(visible_ids.contains(&s2.session_id.as_str()));
    assert!(!visible_ids.contains(&s3.session_id.as_str()));

    cleanup(&mgr).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn get_session_visible_rejects_wrong_owner() {
    let mgr = SessionManager::new();
    let info = mgr
        .create_session_async_for_owner(default_config(), Some("owner-a".to_string()))
        .await
        .unwrap();

    let visible = mgr.get_session_visible(&info.session_id, Some("owner-a"));
    assert!(visible.is_ok(), "owner should see its own session");

    let hidden = mgr.get_session_visible(&info.session_id, Some("owner-b"));
    assert!(hidden.is_err(), "other owners must not see the session");

    cleanup(&mgr).await;
}

// ---------------------------------------------------------------------------
// 12. session_info — metadata
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn session_info_metadata() {
    let mgr = SessionManager::new();
    let created = mgr.create_session_async(cmd_config()).await.unwrap();

    let session = mgr.get_session(&created.session_id).unwrap();
    // Wait briefly so the session is considered idle
    sleep(Duration::from_secs(3)).await;

    let info = session.info().await;

    assert_eq!(info.session_id, created.session_id);
    assert!(info.pid.is_some(), "Running session should have a PID");
    assert_eq!(info.command, "cmd.exe");
    assert_eq!(info.rows, 24);
    assert_eq!(info.cols, 80);

    // Status should be Running or Idle (both are valid for a live session)
    match &info.status {
        SessionStatus::Running | SessionStatus::Idle => {}
        other => panic!("Expected Running or Idle status, got: {:?}", other),
    }

    drop(session);
    cleanup(&mgr).await;
}

// ---------------------------------------------------------------------------
// 13. session_info — nonexistent
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn session_info_nonexistent() {
    let mgr = SessionManager::new();
    let result = mgr.get_session("bogus_session_id_99999");

    assert!(
        result.is_err(),
        "Getting a nonexistent session should return an error"
    );

    let err_msg = format!("{}", result.err().unwrap());
    assert!(
        err_msg.to_lowercase().contains("not found"),
        "Error should mention 'not found', got: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// 14. rapid create/close — stress test
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn rapid_create_close() {
    let mgr = SessionManager::new();

    for i in 0..10 {
        let info = mgr
            .create_session_async(cmd_config())
            .await
            .unwrap_or_else(|e| panic!("Failed to create session {i}: {e}"));
        mgr.close_session(&info.session_id)
            .await
            .unwrap_or_else(|e| panic!("Failed to close session {i}: {e}"));
    }

    assert_eq!(
        mgr.len(),
        0,
        "All sessions should be cleaned up after rapid create/close"
    );

    // Also verify list is empty
    let list = mgr.list_sessions().await;
    assert!(
        list.is_empty(),
        "List should be empty after all sessions closed"
    );
}
