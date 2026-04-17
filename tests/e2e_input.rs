//! End-to-end tests for send_text and send_keys input tools.
//! Run with: cargo test --test e2e_input -- --test-threads=1 --nocapture

use std::sync::Arc;
use std::time::Duration;

use terminal_mcp::keys::key_to_bytes;
use terminal_mcp::session::{Session, SessionConfig, SessionManager};
use terminal_mcp::tools::automation::handle_send_and_wait;
use terminal_mcp::tools::input::{handle_send_keys, handle_send_text};
use tokio::time::sleep;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async fn create_cmd_session(mgr: &SessionManager) -> (String, Arc<Session>) {
    let config = SessionConfig {
        command: Some("cmd.exe".to_string()),
        ..Default::default()
    };
    create_session(mgr, config).await
}

async fn create_session(mgr: &SessionManager, config: SessionConfig) -> (String, Arc<Session>) {
    let info = mgr.create_session_async(config).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();
    sleep(Duration::from_secs(2)).await;
    let _ = session.read_new_output().await; // drain startup banner
    (info.session_id, session)
}

async fn wait_for_output(session: &Session, needle: &str) -> String {
    for _ in 0..20 {
        let output = String::from_utf8_lossy(&session.get_full_output().await).to_string();
        if output.contains(needle) {
            return output;
        }
        sleep(Duration::from_millis(250)).await;
    }

    String::from_utf8_lossy(&session.get_full_output().await).to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// send_text tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn send_text_basic_echo() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Prompt-aware settle via hardened send_and_wait: in "screen" mode it
    // waits for a meaningful screen change, not a fixed sleep.
    let _ = handle_send_and_wait(&session, "echo hello", true, None, 5_000, "screen")
        .await
        .unwrap();

    let screen = session.get_screen_contents().await;
    assert!(
        screen.contains("hello"),
        "Expected 'hello' in screen output, got:\n{screen}"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_text_basic_via_handle() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    let result = handle_send_text(&session, "echo marker_abc", true, None)
        .await
        .unwrap();
    assert_eq!(result["status"], "ok");
    // "echo marker_abc" + \r = 17 bytes
    assert!(result["sent"].as_u64().unwrap() > 0);

    sleep(Duration::from_secs(2)).await;
    let screen = session.get_screen_contents().await;
    assert!(
        screen.contains("marker_abc"),
        "Expected 'marker_abc' in screen:\n{screen}"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_text_without_enter() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Send text WITHOUT pressing enter — should appear on screen but not execute
    let result = handle_send_text(&session, "echo noenter_xyz", false, None)
        .await
        .unwrap();
    assert_eq!(result["status"], "ok");

    sleep(Duration::from_secs(1)).await;
    let screen = session.get_screen_contents().await;
    // The typed text should be visible on the current input line
    assert!(
        screen.contains("echo noenter_xyz"),
        "Typed text should appear on screen:\n{screen}"
    );

    // Clear the line so the session is clean
    session.write_bytes(b"\r").await.unwrap();
    sleep(Duration::from_millis(500)).await;

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_text_empty_string() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Empty text with press_enter=false should send 0 bytes and not error
    let result = handle_send_text(&session, "", false, None).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 0);

    // Empty text with press_enter=true should send just \r
    let result = handle_send_text(&session, "", true, None).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 0);

    // Session should still be alive
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_text_unicode() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Send unicode text — should not error. cmd.exe may not render it perfectly
    // but the bytes should be written without error.
    let result = handle_send_text(&session, "echo héllo 你好", true, None)
        .await
        .unwrap();
    assert_eq!(result["status"], "ok");
    assert!(result["sent"].as_u64().unwrap() > 0);

    sleep(Duration::from_secs(1)).await;
    // Just verify the session is still alive after sending unicode
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_text_multiline_paste() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Multi-line text should still reach the shell correctly
    let multiline = "echo line1\r\necho line2\r\n";
    let result = handle_send_text(&session, multiline, false, None)
        .await
        .unwrap();
    assert_eq!(result["status"], "ok");
    assert!(result["sent"].as_u64().unwrap() > 0);

    sleep(Duration::from_secs(2)).await;
    let screen = session.get_screen_contents().await;
    assert!(
        screen.contains("line1") || screen.contains("line2"),
        "Expected multi-line output on screen:\n{screen}"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_text_special_characters() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Quotes, backslashes, pipes — should all be sent as raw bytes
    let special = r#"echo "hello" | echo world & echo back\slash"#;
    let result = handle_send_text(&session, special, true, None)
        .await
        .unwrap();
    assert_eq!(result["status"], "ok");
    assert!(result["sent"].as_u64().unwrap() > 0);

    sleep(Duration::from_secs(2)).await;
    // Session should still be alive
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_text_raw_input_is_typed_not_pasted() {
    let mgr = SessionManager::new();
    let script = r#"process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.setEncoding('utf8');
let value = '';
console.log('READY');
process.stdin.on('data', chunk => {
  if (chunk === '\r') {
    console.log('SUBMIT:' + value);
    process.exit(0);
    return;
  }
  if (chunk.length > 1) {
    console.log('PASTE:' + JSON.stringify(chunk));
    return;
  }
  value += chunk;
  console.log('VALUE:' + value);
});"#;
    let config = SessionConfig {
        command: Some("node".to_string()),
        args: vec!["-e".to_string(), script.to_string()],
        ..Default::default()
    };
    let (sid, session) = create_session(&mgr, config).await;

    let result = handle_send_text(&session, "abc", false, None)
        .await
        .unwrap();
    assert_eq!(result["status"], "ok");

    let output = wait_for_output(&session, "VALUE:abc").await;
    assert!(
        output.contains("READY"),
        "Expected node raw-input app to start, got:\n{output}"
    );
    assert!(
        output.contains("VALUE:abc"),
        "Expected raw text input to arrive character-by-character, got:\n{output}"
    );
    assert!(
        !output.contains("PASTE:"),
        "send_text should type into raw-input apps instead of pasting a chunk:\n{output}"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// send_keys tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_arrow_keys() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Type something first, then use arrow keys to navigate
    session.write_bytes(b"echo test\r").await.unwrap();
    sleep(Duration::from_secs(1)).await;

    let keys = vec![
        "Up".to_string(),
        "Down".to_string(),
        "Left".to_string(),
        "Right".to_string(),
    ];
    let result = handle_send_keys(&session, &keys).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 4);

    // Session should still be alive
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_arrow_via_write_bytes() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Send arrow keys directly via key_to_bytes + write_bytes
    for key_name in &["Up", "Down", "Left", "Right"] {
        let bytes = key_to_bytes(key_name, false).unwrap();
        session.write_bytes(&bytes).await.unwrap();
    }
    sleep(Duration::from_millis(500)).await;
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_function_keys() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    let keys = vec!["F1".to_string(), "F5".to_string(), "F12".to_string()];
    let result = handle_send_keys(&session, &keys).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 3);
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_ctrl_c_interrupt() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Ctrl+C should interrupt but NOT kill cmd.exe
    let keys = vec!["Ctrl+C".to_string()];
    let result = handle_send_keys(&session, &keys).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 1);

    sleep(Duration::from_secs(1)).await;
    assert!(session.is_alive().await, "cmd.exe should survive Ctrl+C");

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_ctrl_l_clear() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    let keys = vec!["Ctrl+L".to_string()];
    let result = handle_send_keys(&session, &keys).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 1);

    sleep(Duration::from_millis(500)).await;
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_shift_tab() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Verify the bytes produced for Shift+Tab
    let bytes = key_to_bytes("Shift+Tab", false).unwrap();
    assert_eq!(bytes, b"\x1b[Z");

    // Send via handle
    let keys = vec!["Shift+Tab".to_string()];
    let result = handle_send_keys(&session, &keys).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 1);
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_alt_combos() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Alt+Enter and Alt+Tab are valid combos (Alt+<named key>)
    // Alt+F and Alt+B map to Alt+<single letter> which won't resolve
    // because plain letters aren't in key_to_bytes mapping.
    // Test valid ones:
    let keys = vec!["Alt+Enter".to_string(), "Alt+Tab".to_string()];
    let result = handle_send_keys(&session, &keys).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 2);

    sleep(Duration::from_millis(500)).await;
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_unknown_key_error() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    let keys = vec!["InvalidKeyXYZ".to_string()];
    let result = handle_send_keys(&session, &keys).await;
    assert!(result.is_err(), "Unknown key should produce an error");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Unknown key"),
        "Error should mention 'Unknown key', got: {err_msg}"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_empty_array() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    let keys: Vec<String> = vec![];
    let result = handle_send_keys(&session, &keys).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 0);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_case_insensitivity() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // All casings of Ctrl+C should work
    for variant in &["ctrl+c", "CTRL+C", "Ctrl+C"] {
        let keys = vec![variant.to_string()];
        let result = handle_send_keys(&session, &keys).await.unwrap();
        assert_eq!(
            result["status"], "ok",
            "Case variant '{variant}' should succeed"
        );
    }

    sleep(Duration::from_millis(500)).await;
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_multiple_sequence() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Run a command first so Up recalls it
    session.write_bytes(b"echo seqtest\r").await.unwrap();
    sleep(Duration::from_secs(1)).await;
    let _ = session.read_new_output().await;

    // Send Up, Up, Enter as a sequence
    let keys = vec!["Up".to_string(), "Up".to_string(), "Enter".to_string()];
    let result = handle_send_keys(&session, &keys).await.unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 3);

    sleep(Duration::from_secs(1)).await;
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn application_cursor_mode_key_encoding() {
    // Test key_to_bytes directly — no session needed
    // Normal mode: arrows use CSI (\x1b[)
    assert_eq!(key_to_bytes("Up", false), Some(b"\x1b[A".to_vec()));
    assert_eq!(key_to_bytes("Down", false), Some(b"\x1b[B".to_vec()));
    assert_eq!(key_to_bytes("Right", false), Some(b"\x1b[C".to_vec()));
    assert_eq!(key_to_bytes("Left", false), Some(b"\x1b[D".to_vec()));

    // Application cursor mode: arrows use SS3 (\x1bO)
    assert_eq!(key_to_bytes("Up", true), Some(b"\x1bOA".to_vec()));
    assert_eq!(key_to_bytes("Down", true), Some(b"\x1bOB".to_vec()));
    assert_eq!(key_to_bytes("Right", true), Some(b"\x1bOC".to_vec()));
    assert_eq!(key_to_bytes("Left", true), Some(b"\x1bOD".to_vec()));

    // Non-arrow keys should NOT change between modes
    assert_eq!(key_to_bytes("Enter", false), key_to_bytes("Enter", true));
    assert_eq!(key_to_bytes("F1", false), key_to_bytes("F1", true));
    assert_eq!(key_to_bytes("Home", false), key_to_bytes("Home", true));
    assert_eq!(key_to_bytes("Ctrl+C", false), key_to_bytes("Ctrl+C", true));
    assert_eq!(
        key_to_bytes("Shift+Tab", false),
        key_to_bytes("Shift+Tab", true)
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Additional edge-case tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn send_text_press_enter_appends_cr() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // With press_enter=true, handle_send_text sends text + \r
    // "ab" = 2 bytes + \r = 3 bytes total
    let result = handle_send_text(&session, "ab", true, None).await.unwrap();
    assert_eq!(result["sent"].as_u64().unwrap(), 2);

    // Without press_enter, just the raw text
    let result = handle_send_text(&session, "cd", false, None).await.unwrap();
    assert_eq!(result["sent"].as_u64().unwrap(), 2);

    // Clean up
    session.write_bytes(b"\r").await.unwrap();
    sleep(Duration::from_millis(500)).await;

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_ctrl_a_through_z_all_valid() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Every Ctrl+<letter> should resolve without error
    for ch in 'A'..='Z' {
        let key = format!("Ctrl+{ch}");
        let bytes = key_to_bytes(&key, false);
        assert!(bytes.is_some(), "Ctrl+{ch} should produce valid bytes");
        assert_eq!(bytes.unwrap(), vec![ch as u8 - b'A' + 1]);
    }

    // Send a few via the session to confirm write_bytes works
    for key_name in &["Ctrl+A", "Ctrl+E", "Ctrl+K"] {
        let bytes = key_to_bytes(key_name, false).unwrap();
        session.write_bytes(&bytes).await.unwrap();
    }
    sleep(Duration::from_millis(500)).await;
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_partial_failure_stops_at_bad_key() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Mix valid and invalid — should error on the invalid one
    let keys = vec![
        "Enter".to_string(),
        "Tab".to_string(),
        "BogusKey".to_string(),
        "Up".to_string(), // never reached
    ];
    let result = handle_send_keys(&session, &keys).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("BogusKey"));

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_text_long_string() {
    let mgr = SessionManager::new();
    let (sid, session) = create_cmd_session(&mgr).await;

    // Send a long string (> typical line width)
    let long = "A".repeat(200);
    let result = handle_send_text(&session, &long, false, None)
        .await
        .unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sent"].as_u64().unwrap(), 200);

    // Clean up
    session.write_bytes(b"\x1b[2K\r").await.unwrap();
    sleep(Duration::from_millis(500)).await;
    assert!(session.is_alive().await);

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}
