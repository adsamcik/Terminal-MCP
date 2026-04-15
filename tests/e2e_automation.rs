//! Comprehensive E2E tests for automation tools.
//! Run with: cargo test --test e2e_automation -- --test-threads=1 --nocapture

use std::sync::Arc;
use std::time::Duration;

use terminal_mcp::error_detection::ErrorDetector;
use terminal_mcp::session::{Session, SessionConfig, SessionManager};
use terminal_mcp::shell_integration::ShellIntegration;
use terminal_mcp::tools::automation::{handle_send_and_wait, handle_wait_for, handle_wait_for_idle};
use tokio::time::sleep;

// ── Helpers ───────────────────────────────────────────────────────

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

fn powershell_screen_stable_config() -> SessionConfig {
    SessionConfig {
        command: Some("powershell.exe".to_string()),
        args: vec![
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            concat!(
                "$ErrorActionPreference='Stop'; ",
                "[Console]::Write('READY 0'); ",
                "while ($true) { ",
                "  $key = [Console]::ReadKey($true); ",
                "  if ($key.KeyChar -eq 'j') { ",
                "    [Console]::Write(\"`rREADY 1\"); ",
                "    1..30 | ForEach-Object { ",
                "      [Console]::Write(([string][char]27) + '[6n'); ",
                "      Start-Sleep -Milliseconds 50 ",
                "    } ",
                "  } elseif ($key.KeyChar -eq 'q') { ",
                "    break ",
                "  } ",
                "}"
            )
            .to_string(),
        ],
        cwd: None,
        env: Default::default(),
        rows: 24,
        cols: 80,
        scrollback: 1000,
    }
}

/// Create a session, wait for it to settle, and return (manager, session).
async fn create_settled_session() -> (SessionManager, Arc<Session>, String) {
    let mgr = SessionManager::new();
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();
    // Wait for cmd.exe prompt to appear
    sleep(Duration::from_secs(3)).await;
    // Drain startup output so delta reads start fresh
    let _ = session.read_new_output().await;
    (mgr, session, info.session_id)
}

/// Cleanup helper
async fn cleanup(mgr: &SessionManager, session: Arc<Session>, id: &str) {
    drop(session);
    let _ = mgr.close_session(id).await;
}

async fn create_screen_stable_session() -> (SessionManager, Arc<Session>, String) {
    let mgr = SessionManager::new();
    let info = mgr
        .create_session_async(powershell_screen_stable_config())
        .await
        .unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();

    for _ in 0..30 {
        if session.get_screen_contents().await.contains("READY 0") {
            let _ = session.read_new_output().await;
            return (mgr, session, info.session_id);
        }
        sleep(Duration::from_millis(100)).await;
    }

    panic!("PowerShell screen-stable test session did not initialize");
}

// ═══════════════════════════════════════════════════════════════════
// send_and_wait tests
// ═══════════════════════════════════════════════════════════════════

/// 1. Basic command execution: send "echo hello" and wait for idle.
#[tokio::test(flavor = "multi_thread")]
async fn send_and_wait_basic_echo() {
    let (mgr, session, id) = create_settled_session().await;

    let result = handle_send_and_wait(
        &session, "echo hello", true, // press_enter
        None,  // no pattern — wait for idle
        5000,  // timeout_ms
        "delta",
    )
    .await
    .unwrap();

    assert_eq!(result["matched"], true, "Should match (idle): {result}");
    assert_eq!(result["timed_out"], false);

    // Verify the output contains "hello" — check both the result output and raw output
    let output = result["output"].as_str().unwrap_or("");
    let full_raw = session.get_full_output().await;
    let full = String::from_utf8_lossy(&full_raw);
    let screen = session.get_screen_contents().await;
    let found = output.contains("hello") || full.contains("hello") || screen.contains("hello");
    assert!(found, "Expected 'hello' in output.\nresult_output: {output}\nscreen: {screen}");

    cleanup(&mgr, session, &id).await;
}

/// 2. Wait for pattern: send "echo marker_42" and wait for "marker_42".
#[tokio::test(flavor = "multi_thread")]
async fn send_and_wait_pattern_match() {
    let (mgr, session, id) = create_settled_session().await;

    let result = handle_send_and_wait(
        &session,
        "echo marker_42",
        true,
        Some("marker_42"),
        5000,
        "delta",
    )
    .await
    .unwrap();

    assert_eq!(result["matched"], true, "Pattern should match: {result}");
    assert_eq!(result["timed_out"], false);

    // match_text should contain the matched pattern
    let match_text = result["match_text"].as_str().unwrap_or("");
    assert!(
        match_text.contains("marker_42"),
        "match_text should contain marker_42, got: {match_text}"
    );

    cleanup(&mgr, session, &id).await;
}

/// 3. Timeout: wait for pattern that won't appear.
#[tokio::test(flavor = "multi_thread")]
async fn send_and_wait_timeout() {
    let (mgr, session, id) = create_settled_session().await;

    let result = handle_send_and_wait(
        &session,
        "echo something",
        true,
        Some("this_pattern_will_never_appear_xyz987"),
        2000, // short timeout
        "delta",
    )
    .await
    .unwrap();

    assert_eq!(result["timed_out"], true, "Should time out: {result}");
    assert_eq!(result["matched"], false);

    cleanup(&mgr, session, &id).await;
}

/// 4. Output mode delta: verify output field has command result.
#[tokio::test(flavor = "multi_thread")]
async fn send_and_wait_output_mode_delta() {
    let (mgr, session, id) = create_settled_session().await;

    let result = handle_send_and_wait(
        &session,
        "echo delta_test_99",
        true,
        Some("delta_test_99"),
        5000,
        "delta",
    )
    .await
    .unwrap();

    assert!(result["matched"].as_bool().unwrap());
    // delta mode should have "output" field
    assert!(
        result.get("output").is_some(),
        "Delta mode should have 'output' field: {result}"
    );
    // delta mode should NOT have "screen" field
    assert!(
        result.get("screen").is_none(),
        "Delta mode should not have 'screen' field: {result}"
    );

    cleanup(&mgr, session, &id).await;
}

/// 5. Output mode screen: verify screen field has terminal grid.
#[tokio::test(flavor = "multi_thread")]
async fn send_and_wait_output_mode_screen() {
    let (mgr, session, id) = create_settled_session().await;

    let result = handle_send_and_wait(
        &session,
        "echo screen_test_77",
        true,
        Some("screen_test_77"),
        5000,
        "screen",
    )
    .await
    .unwrap();

    assert!(result["matched"].as_bool().unwrap());
    // screen mode should have "screen" field
    let screen = result["screen"].as_str().unwrap_or("");
    assert!(!screen.is_empty(), "Screen mode should have non-empty 'screen' field");
    // screen mode should NOT have "output" field
    assert!(
        result.get("output").is_none(),
        "Screen mode should not have 'output' field: {result}"
    );

    cleanup(&mgr, session, &id).await;
}

/// 6. Output mode both: verify both output and screen present.
#[tokio::test(flavor = "multi_thread")]
async fn send_and_wait_output_mode_both() {
    let (mgr, session, id) = create_settled_session().await;

    let result = handle_send_and_wait(
        &session,
        "echo both_test_55",
        true,
        Some("both_test_55"),
        5000,
        "both",
    )
    .await
    .unwrap();

    assert!(result["matched"].as_bool().unwrap());
    // both mode should have "output" and "screen"
    assert!(
        result.get("output").is_some(),
        "'both' mode should have 'output' field: {result}"
    );
    assert!(
        result.get("screen").is_some(),
        "'both' mode should have 'screen' field: {result}"
    );
    let screen = result["screen"].as_str().unwrap_or("");
    assert!(!screen.is_empty(), "Screen should be non-empty in 'both' mode");

    cleanup(&mgr, session, &id).await;
}

/// 7. Press enter false: send text without executing.
#[tokio::test(flavor = "multi_thread")]
async fn send_and_wait_press_enter_false() {
    let (mgr, session, id) = create_settled_session().await;

    // Send text WITHOUT pressing enter — the text should be typed but not executed
    let result = handle_send_and_wait(
        &session,
        "echo no_enter_test_88",
        false, // press_enter = false
        None,  // wait for idle
        3000,
        "both",
    )
    .await
    .unwrap();

    // Should eventually go idle (since no command was executed)
    // The text should appear on screen (typed into the prompt) but NOT as echoed output
    let screen = result["screen"].as_str().unwrap_or("");
    let full_raw = session.get_full_output().await;
    let full = String::from_utf8_lossy(&full_raw);

    // The typed text should appear somewhere (either screen or output stream)
    let text_visible = screen.contains("no_enter_test_88") || full.contains("no_enter_test_88");
    assert!(
        text_visible,
        "Typed text should be visible on screen.\nscreen: {screen}"
    );

    // The text should NOT have been executed — so there should be no second
    // occurrence of the marker as command output. We verify by checking
    // the screen doesn't show it on multiple lines (which would indicate execution).
    // On cmd.exe, "echo X" when executed produces the marker on a new line.
    // Since we didn't press enter, we should see it only once (on the input line).
    let occurrences = screen.matches("no_enter_test_88").count();
    assert!(
        occurrences <= 1,
        "Text should appear at most once (typed, not executed). Found {occurrences} times."
    );

    cleanup(&mgr, session, &id).await;
}

/// 8. Screen-mode navigation should return once the visible screen settles,
/// even if the app keeps emitting non-visible control output.
#[tokio::test(flavor = "multi_thread")]
async fn send_and_wait_screen_mode_returns_before_idle_for_tui_navigation() {
    let (mgr, session, id) = create_screen_stable_session().await;

    let result = handle_send_and_wait(
        &session,
        "j",
        false,
        None,
        700,
        "screen",
    )
    .await
    .unwrap();

    assert_eq!(
        result["timed_out"], false,
        "Visible screen updates should not wait for invisible background output: {result}"
    );
    assert_eq!(result["matched"], true, "Expected send_and_wait to complete: {result}");
    assert!(
        result["screen"].as_str().unwrap_or("").contains("READY 1"),
        "Expected updated screen content after navigation: {result}"
    );

    cleanup(&mgr, session, &id).await;
}

// ═══════════════════════════════════════════════════════════════════
// wait_for tests
// ═══════════════════════════════════════════════════════════════════

/// 9. Pattern appears: send a command, then wait_for the output pattern.
#[tokio::test(flavor = "multi_thread")]
async fn wait_for_pattern_appears() {
    let (mgr, session, id) = create_settled_session().await;

    // Send a command that will produce output
    session
        .write_bytes(b"echo waitfor_marker_123\r")
        .await
        .unwrap();

    // Now wait for the pattern to appear
    let result = handle_wait_for(
        &session,
        Some("waitfor_marker_123"),
        None,
        5000, // timeout
        false, // on_screen = false (check raw output)
        false, // invert = false
    )
    .await
    .unwrap();

    assert_eq!(result["matched"], true, "Pattern should be matched: {result}");
    assert_eq!(result["timed_out"], false);
    let mt = result["match_text"].as_str().unwrap_or("");
    assert!(mt.contains("waitfor_marker_123"));

    cleanup(&mgr, session, &id).await;
}

/// 10. Pattern timeout: wait for pattern that won't appear.
#[tokio::test(flavor = "multi_thread")]
async fn wait_for_pattern_timeout() {
    let (mgr, session, id) = create_settled_session().await;

    let result = handle_wait_for(
        &session,
        Some("impossible_pattern_never_appears_zzz"),
        None,
        2000, // short timeout
        false,
        false,
    )
    .await
    .unwrap();

    assert_eq!(result["timed_out"], true, "Should time out: {result}");
    assert_eq!(result["matched"], false);

    cleanup(&mgr, session, &id).await;
}

/// 11. Invert mode: wait for a pattern to NOT be present.
#[tokio::test(flavor = "multi_thread")]
async fn wait_for_invert_mode() {
    let (mgr, session, id) = create_settled_session().await;

    // Wait for a pattern that was never present — with invert=true,
    // this should immediately succeed since the pattern isn't there.
    let result = handle_wait_for(
        &session,
        Some("pattern_that_does_not_exist_xyz"),
        None,
        3000,
        false,
        true, // invert = true
    )
    .await
    .unwrap();

    assert_eq!(
        result["matched"], true,
        "Invert mode should match when pattern is absent: {result}"
    );
    assert_eq!(result["timed_out"], false);
    assert_eq!(result["invert"], true);

    cleanup(&mgr, session, &id).await;
}

// ═══════════════════════════════════════════════════════════════════
// wait_for_idle tests
// ═══════════════════════════════════════════════════════════════════

/// 12. Idle after command: send a quick command, then wait_for_idle.
#[tokio::test(flavor = "multi_thread")]
async fn wait_for_idle_after_command() {
    let (mgr, session, id) = create_settled_session().await;

    // Send a fast command
    session.write_bytes(b"echo idle_test\r").await.unwrap();
    // Give it a moment to produce output
    sleep(Duration::from_millis(500)).await;

    let result = handle_wait_for_idle(
        &session,
        500,  // stable_ms — 500ms of no output
        5000, // timeout_ms
        false,
    )
    .await
    .unwrap();

    assert_eq!(result["idle"], true, "Should detect idle: {result}");
    assert_eq!(result["timed_out"], false);

    cleanup(&mgr, session, &id).await;
}

/// 13. Idle with custom threshold: both 500ms and 2000ms should eventually succeed.
#[tokio::test(flavor = "multi_thread")]
async fn wait_for_idle_custom_threshold() {
    let (mgr, session, id) = create_settled_session().await;

    // With stable_ms=500
    let result_500 = handle_wait_for_idle(&session, 500, 5000, false)
        .await
        .unwrap();
    assert_eq!(
        result_500["idle"], true,
        "500ms threshold should succeed: {result_500}"
    );

    // With stable_ms=2000
    let result_2000 = handle_wait_for_idle(&session, 2000, 8000, false)
        .await
        .unwrap();
    assert_eq!(
        result_2000["idle"], true,
        "2000ms threshold should succeed: {result_2000}"
    );

    // Verify the stable_ms is echoed back
    assert_eq!(result_500["stable_ms"], 500);
    assert_eq!(result_2000["stable_ms"], 2000);

    cleanup(&mgr, session, &id).await;
}

/// 14. Idle timeout: very short timeout while session has recent output.
#[tokio::test(flavor = "multi_thread")]
async fn wait_for_idle_timeout() {
    let (mgr, session, id) = create_settled_session().await;

    // Send a command to produce output RIGHT NOW
    session.write_bytes(b"echo activity\r").await.unwrap();

    // Immediately try to detect idle with a very short timeout and long stable window.
    // The stable_ms is longer than the timeout, so it should always time out.
    let result = handle_wait_for_idle(
        &session,
        5000, // stable_ms — need 5s of quiet
        100,  // timeout_ms — but only wait 100ms total
        false,
    )
    .await
    .unwrap();

    assert_eq!(result["timed_out"], true, "Should time out: {result}");
    assert_eq!(result["idle"], false);

    cleanup(&mgr, session, &id).await;
}

// ═══════════════════════════════════════════════════════════════════
// Shell integration tests (unit-level, no PTY needed)
// ═══════════════════════════════════════════════════════════════════

/// 15. Prompt regex matching: test ShellIntegration prompt patterns.
#[tokio::test(flavor = "multi_thread")]
async fn shell_integration_prompt_patterns() {
    // Test known prompt strings against the internal prompt patterns.
    // We can't call is_at_prompt without a vt100::Screen, but we can
    // test the OSC processing and phase transitions.

    // Test OSC 133 lifecycle
    let mut si = ShellIntegration::new();

    // Prompt start
    si.process_osc("133;A");
    assert_eq!(
        *si.phase(),
        terminal_mcp::shell_integration::ShellPhase::PromptActive
    );

    // Input ready
    si.process_osc("133;B");
    assert_eq!(
        *si.phase(),
        terminal_mcp::shell_integration::ShellPhase::InputReady
    );

    // Command execution
    si.process_osc("133;C");
    assert_eq!(
        *si.phase(),
        terminal_mcp::shell_integration::ShellPhase::Executing
    );

    // Command finished with exit code
    si.process_osc("133;D;0");
    assert_eq!(
        *si.phase(),
        terminal_mcp::shell_integration::ShellPhase::PromptActive
    );
    assert_eq!(si.last_exit_code(), Some(0));

    // VS Code variant (OSC 633)
    let mut si2 = ShellIntegration::new();
    si2.process_osc("633;A");
    assert_eq!(
        *si2.phase(),
        terminal_mcp::shell_integration::ShellPhase::PromptActive
    );
    assert_eq!(
        si2.status(),
        terminal_mcp::shell_integration::IntegrationStatus::ExternalActive
    );

    // CWD notification
    si2.process_osc("7;file://hostname/home/user");
    assert_eq!(si2.cwd(), Some("/home/user"));
}

// ═══════════════════════════════════════════════════════════════════
// Error detection tests (unit-level, no PTY needed)
// ═══════════════════════════════════════════════════════════════════

/// 16. Error pattern detection: known error patterns are detected.
#[tokio::test(flavor = "multi_thread")]
async fn error_detection_known_patterns() {
    let detector = ErrorDetector::new();

    // Rust error
    let text = "error[E0308]: mismatched types\n  --> src/main.rs:5:10";
    assert!(detector.has_errors(text));
    let matches = detector.detect_errors(text);
    assert!(matches.iter().any(|m| m.pattern_name == "rustc error"));

    // GCC error
    let text = "main.c:10:5: error: expected ';' before '}' token";
    assert!(detector.has_errors(text));
    let matches = detector.detect_errors(text);
    assert!(matches.iter().any(|m| m.pattern_name == "gcc/clang error"));

    // Python traceback
    let text = "Traceback (most recent call last):\n  File \"test.py\", line 1\nValueError: bad";
    assert!(detector.has_errors(text));
    let matches = detector.detect_errors(text);
    assert!(matches.iter().any(|m| m.pattern_name == "python traceback"));

    // npm error
    let text = "npm ERR! code ENOENT\nnpm ERR! path /app/package.json";
    assert!(detector.has_errors(text));

    // Java exception
    let text = "Exception in thread \"main\" java.lang.NullPointerException";
    assert!(detector.has_errors(text));

    // Command not found
    let text = "bash: foobar: command not found";
    assert!(detector.has_errors(text));

    // Permission denied
    let text = "bash: /usr/sbin/iptables: Permission denied";
    assert!(detector.has_errors(text));

    // Segfault
    let text = "Segmentation fault (core dumped)";
    assert!(detector.has_errors(text));

    // Exit code
    let text = "process exited with code 1";
    assert!(detector.has_errors(text));

    // Panic
    let text = "thread 'main' panicked at 'index out of bounds'";
    assert!(detector.has_errors(text));
}

/// 16. Clean output: no false positives.
#[tokio::test(flavor = "multi_thread")]
async fn error_detection_clean_output() {
    let detector = ErrorDetector::new();

    // Normal build output
    assert!(!detector.has_errors("Compiling terminal-mcp v0.1.0"));
    assert!(!detector.has_errors("Finished dev [unoptimized + debuginfo]"));
    assert!(!detector.has_errors("All tests passed!"));
    assert!(!detector.has_errors("Build succeeded"));
    assert!(!detector.has_errors(""));
    assert!(!detector.has_errors("Running 42 tests\ntest result: ok. 42 passed"));

    // exit code 0 should not trigger
    let matches = detector.detect_errors("exit code 0");
    assert!(
        !matches.iter().any(|m| m.pattern_name == "exit code nonzero"),
        "exit code 0 should not match"
    );
}

/// 17. Error score: test scoring with various combinations.
#[tokio::test(flavor = "multi_thread")]
async fn error_detection_scoring() {
    let detector = ErrorDetector::new();

    // Clean output with zero exit code → score 0
    assert_eq!(detector.error_score("All tests passed!", Some(0)), 0);
    assert_eq!(detector.error_score("Build succeeded", None), 0);

    // Nonzero exit code alone → at least 20
    let score = detector.error_score("some output", Some(1));
    assert!(score >= 20, "Nonzero exit should give ≥20, got {score}");

    // Error patterns + nonzero exit → higher score
    let text = "error[E0308]: mismatched types\nerror: aborting due to previous error";
    let score_with_exit = detector.error_score(text, Some(1));
    let score_no_exit = detector.error_score(text, Some(0));
    assert!(
        score_with_exit > score_no_exit,
        "Exit code should increase score: {score_with_exit} vs {score_no_exit}"
    );
    assert!(score_with_exit > 30, "Combined score should be >30, got {score_with_exit}");

    // Diverse patterns → high score
    let text = "error[E0308]: mismatch\nnpm ERR! failed\nTraceback (most recent call last):";
    let score = detector.error_score(text, Some(0));
    assert!(score > 20, "Diverse patterns should produce high score, got {score}");

    // Score capped at 100
    let text = (0..50)
        .map(|i| format!("error: problem {i}\nFATAL: issue {i}\nFAILED test {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let score = detector.error_score(&text, Some(1));
    assert!(score <= 100, "Score should be capped at 100, got {score}");

    // Line numbers are correct
    let text = "line 0 ok\nerror: line 1 bad\nline 2 ok\nerror: line 3 bad";
    let matches = detector.detect_errors(text);
    let lines: Vec<usize> = matches.iter().map(|m| m.line_number).collect();
    assert!(lines.contains(&1), "Should detect error on line 1");
    assert!(lines.contains(&3), "Should detect error on line 3");
}
