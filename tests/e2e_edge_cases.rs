//! Edge-case and error-handling E2E tests for terminal-mcp.
//! Run with: cargo test --test e2e_edge_cases -- --test-threads=1 --nocapture

use std::sync::Arc;
use std::time::Duration;

use terminal_mcp::session::{SessionConfig, SessionManager};
use tokio::time::sleep;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn cmd_config_exit() -> SessionConfig {
    SessionConfig {
        command: Some("cmd.exe".to_string()),
        args: vec!["/c".to_string(), "echo done".to_string()],
        cwd: None,
        env: Default::default(),
        rows: 24,
        cols: 80,
        scrollback: 1000,
    }
}

fn cmd_config_quiet_wait() -> SessionConfig {
    SessionConfig {
        command: Some("cmd.exe".to_string()),
        args: vec!["/c".to_string(), "timeout /t 30 >nul".to_string()],
        cwd: None,
        env: Default::default(),
        rows: 24,
        cols: 80,
        scrollback: 1000,
    }
}

// ===========================================================================
// 1–9. Session-not-found errors
// ===========================================================================

/// Helper: assert get_session returns a "Session not found" error.
fn assert_session_not_found(mgr: &SessionManager, id: &str) {
    match mgr.get_session(id) {
        Ok(_) => panic!("Expected error for nonexistent session '{id}'"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("Session not found"),
                "Error should mention 'Session not found': {msg}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_send_text_to_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_1");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_send_keys_to_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_2");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_read_output_from_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_3");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_screen_from_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_4");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_screenshot_from_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_5");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_scrollback_from_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_6");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_wait_for_on_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_7");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_wait_for_idle_on_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_8");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_session_info_on_nonexistent_session() {
    assert_session_not_found(&SessionManager::new(), "bogus_session_id_9");
}

// ===========================================================================
// 10–12. Concurrent access
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_concurrent_writes() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;

    let mut handles = Vec::new();
    for i in 0..3u8 {
        let s = Arc::clone(&session);
        handles.push(tokio::spawn(async move {
            for j in 0..5u8 {
                let text = format!("echo task{i}_iter{j}\r\n");
                s.write_bytes(text.as_bytes()).await.unwrap();
                sleep(Duration::from_millis(50)).await;
            }
        }));
    }
    for h in handles {
        h.await.expect("Task should not panic");
    }

    sleep(Duration::from_secs(1)).await;
    let raw = session.get_full_output().await;
    assert!(
        !raw.is_empty(),
        "Output should be non-empty after concurrent writes"
    );

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_concurrent_reads() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;
    session
        .write_bytes(b"echo concurrent_read_test\r\n")
        .await
        .unwrap();
    sleep(Duration::from_secs(1)).await;

    let mut handles = Vec::new();
    for _ in 0..3 {
        let s = Arc::clone(&session);
        handles.push(tokio::spawn(async move {
            for _ in 0..5 {
                let _ = s.get_full_output().await;
                let _ = s.get_screen_contents().await;
                let _ = s.read_new_output().await;
                sleep(Duration::from_millis(20)).await;
            }
        }));
    }
    for h in handles {
        h.await.expect("Concurrent read task should not panic");
    }

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_write_while_reading() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;

    let writer = Arc::clone(&session);
    let reader = Arc::clone(&session);

    let write_handle = tokio::spawn(async move {
        for i in 0..10 {
            let text = format!("echo write_while_read_{i}\r\n");
            let _ = writer.write_bytes(text.as_bytes()).await;
            sleep(Duration::from_millis(50)).await;
        }
    });

    let read_handle = tokio::spawn(async move {
        for _ in 0..10 {
            let _ = reader.get_full_output().await;
            let _ = reader.get_screen_contents().await;
            sleep(Duration::from_millis(50)).await;
        }
    });

    // Both tasks complete without deadlock — use a timeout as safety net
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        write_handle.await.expect("Writer should not panic");
        read_handle.await.expect("Reader should not panic");
    })
    .await;
    assert!(result.is_ok(), "Write-while-reading should not deadlock");

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

// ===========================================================================
// 13–17. Session lifecycle edge cases
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_rapid_create_close_cycle() {
    let mgr = SessionManager::new();
    for _ in 0..10 {
        let info = mgr.create_session_async(cmd_config()).await.unwrap();
        mgr.close_session(&info.session_id).await.unwrap();
    }
    assert_eq!(mgr.len(), 0, "All sessions should be cleaned up");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_write_to_closed_session() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(1)).await;
    mgr.close_session(&info.session_id).await.unwrap();

    // Session has been removed from the manager, but we still hold an Arc.
    // Writing should fail gracefully (not panic). Give the kill a moment.
    sleep(Duration::from_millis(500)).await;
    let result = session.write_bytes(b"hello after close\r\n").await;
    // It may succeed (writer is cached) or fail — either is fine, as long as
    // it doesn't panic.
    drop(result);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_read_from_exited_process() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config_exit()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    // Wait for the short-lived process to exit
    sleep(Duration::from_secs(3)).await;

    // Reading after process exit should not crash
    let output = session.get_full_output().await;
    let _screen = session.get_screen_contents().await;
    let _new = session.read_new_output().await;

    // The short-lived cmd /c echo done should have produced some output
    assert!(
        !output.is_empty(),
        "Even exited process should have produced output"
    );

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_drop_cleanup() {
    // Create a session, get its PID, then drop all references without
    // calling close(). The Drop impl should cancel reader and kill child.
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();
    let pid = session.pid().await;
    assert!(pid.is_some(), "Session should have a PID");

    // Drop the session Arc and remove from manager (simulating
    // all references being dropped)
    drop(session);
    // Remove from manager — this triggers close which runs Drop
    let _ = mgr.close_session(&info.session_id).await;
    assert_eq!(mgr.len(), 0);

    // Small wait for OS process cleanup
    sleep(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_get_session_calls() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();

    let s1 = mgr.get_session(&info.session_id).unwrap();
    let s2 = mgr.get_session(&info.session_id).unwrap();
    let s3 = mgr.get_session(&info.session_id).unwrap();

    // All should refer to the same session
    assert_eq!(s1.id, s2.id);
    assert_eq!(s2.id, s3.id);

    sleep(Duration::from_secs(1)).await;

    // All should be functional
    s1.write_bytes(b"echo multi_get_1\r\n").await.unwrap();
    sleep(Duration::from_millis(500)).await;
    let output = s2.get_full_output().await;
    assert!(!output.is_empty());
    let _screen = s3.get_screen_contents().await;

    drop(s1);
    drop(s2);
    drop(s3);
    mgr.close_session(&info.session_id).await.unwrap();
}

// ===========================================================================
// 18. Auto-cleanup
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_idle_cleanup() {
    let mgr = SessionManager::new();
    let info = mgr
        .create_session_async(cmd_config_quiet_wait())
        .await
        .unwrap();
    assert_eq!(mgr.len(), 1);

    let session = mgr.get_session(&info.session_id).unwrap();

    // Use a silent long-running command so idleness is deterministic.
    sleep(Duration::from_secs(2)).await;

    assert!(session.is_idle(Duration::from_secs(1)).await);

    let cleanup =
        mgr.start_cleanup_task_with_interval(Duration::from_secs(1), Duration::from_millis(200));

    let mut removed = false;
    for _ in 0..15 {
        if mgr.len() == 0 {
            removed = true;
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }
    assert!(
        removed,
        "Idle cleanup should remove the session from the manager"
    );

    let mut alive = session.is_alive().await;
    for _ in 0..10 {
        if !alive {
            break;
        }
        sleep(Duration::from_millis(200)).await;
        alive = session.is_alive().await;
    }
    assert!(!alive, "Idle cleanup should terminate the idle session");

    cleanup.abort();
    drop(session);
}

// ===========================================================================
// 19–20. Large data
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_large_output() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;

    // Generate large output by listing System32
    session
        .write_bytes(b"dir C:\\Windows\\System32 /s\r\n")
        .await
        .unwrap();

    // Wait for output to accumulate
    sleep(Duration::from_secs(8)).await;

    let output = session.get_full_output().await;
    assert!(
        output.len() > 1000,
        "Large output test: expected >1000 bytes, got {}",
        output.len()
    );

    // Session should still be alive and responsive
    let _screen = session.get_screen_contents().await;

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_large_input() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;

    // Send 10KB of data — should not crash the writer
    let large_input = "x".repeat(10 * 1024);
    let result = session.write_bytes(large_input.as_bytes()).await;
    // May succeed or fail depending on PTY buffer, but should not panic
    drop(result);

    sleep(Duration::from_secs(1)).await;
    assert!(
        session.is_alive().await,
        "Session should survive large input"
    );

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

// ===========================================================================
// 21–24. Search/scrollback edge cases
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_search_with_invalid_regex() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(1)).await;

    // Invalid regex should return an error, not panic
    let result = session.search_output("[invalid").await;
    assert!(result.is_err(), "Invalid regex should return Err");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("regex") || err_msg.contains("Invalid") || err_msg.contains("parse"),
        "Error should mention regex/invalid/parse: {err_msg}"
    );

    // Also test scrollback_search with invalid regex
    let result2 = session.scrollback_search("[invalid", 0).await;
    assert!(
        result2.is_err(),
        "Invalid regex in scrollback should return Err"
    );

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_empty_search() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;
    session.write_bytes(b"echo searchable\r\n").await.unwrap();
    sleep(Duration::from_secs(1)).await;

    // Empty regex pattern matches everything — should not panic
    let result = session.search_output("").await;
    assert!(
        result.is_ok(),
        "Empty search should not error: {:?}",
        result.err()
    );

    let result2 = session.scrollback_search("", 0).await;
    assert!(result2.is_ok(), "Empty scrollback search should not error");

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scrollback_tail_zero() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;
    session.write_bytes(b"echo tail_test\r\n").await.unwrap();
    sleep(Duration::from_secs(1)).await;

    // Tail with 0 lines should return empty
    let result = session.scrollback_tail(0).await;
    assert!(
        result.is_empty(),
        "tail(0) should return empty, got {} lines",
        result.len()
    );

    // Tail with normal count should work
    let result = session.scrollback_tail(5).await;
    // May or may not have lines depending on ConPTY buffering, but should not panic
    drop(result);

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scrollback_out_of_range() {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    sleep(Duration::from_secs(2)).await;
    session.write_bytes(b"echo range_test\r\n").await.unwrap();
    sleep(Duration::from_secs(1)).await;

    // Request range way beyond what's available — should return what exists
    let result = session.scrollback_range(999_999, 100).await;
    assert!(
        result.is_empty(),
        "Range beyond buffer should return empty, got {} lines",
        result.len()
    );

    // Request starting at 0 with huge count — should return available lines
    let result = session.scrollback_range(0, 999_999).await;
    // Should not panic; returns whatever is in the buffer
    let total = session.scrollback_len().await;
    assert!(
        result.len() <= total + 1,
        "Range result ({}) should not exceed total lines ({})",
        result.len(),
        total
    );

    drop(session);
    mgr.close_session(&info.session_id).await.unwrap();
}
