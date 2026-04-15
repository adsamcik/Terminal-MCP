//! End-to-end tests for observation tools (read_output, get_screen,
//! screenshot, get_scrollback).
//!
//! Run with: cargo test --test e2e_observation -- --test-threads=1 --nocapture

use std::time::Duration;

use terminal_mcp::session::{SessionConfig, SessionManager};
use terminal_mcp::tools::observation::{self, GetScreenResponse};
use tokio::time::sleep;

// ── Helpers ────────────────────────────────────────────────────────

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

/// Create a session, wait for cmd.exe startup, and drain initial output.
async fn setup_session(
    mgr: &SessionManager,
) -> (String, std::sync::Arc<terminal_mcp::session::Session>) {
    let info = mgr.create_session_async(cmd_config()).await.unwrap();
    let session = mgr.get_session(&info.session_id).unwrap();
    // Wait for cmd.exe prompt to appear.
    sleep(Duration::from_secs(3)).await;
    // Drain initial output so delta reads start fresh.
    let _ = session.read_new_output().await;
    (info.session_id, session)
}

// ════════════════════════════════════════════════════════════════════
// read_output tests
// ════════════════════════════════════════════════════════════════════

/// 1. Delta mode: first read returns command output, second read returns
/// only new output.
#[tokio::test(flavor = "multi_thread")]
async fn read_output_delta_mode() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    // Send first command and read delta.
    session.write_bytes(b"echo DELTA_ONE\r").await.unwrap();
    sleep(Duration::from_secs(3)).await;
    let resp1 = observation::handle_read_output(&session, Some(5000), None)
        .await
        .unwrap();
    assert!(
        resp1.output.contains("DELTA_ONE"),
        "First delta should contain DELTA_ONE, got: {}",
        resp1.output
    );

    // Send second command and read again — should only contain new output.
    session.write_bytes(b"echo DELTA_TWO\r").await.unwrap();
    sleep(Duration::from_secs(3)).await;
    let resp2 = observation::handle_read_output(&session, Some(5000), None)
        .await
        .unwrap();
    assert!(
        resp2.output.contains("DELTA_TWO"),
        "Second delta should contain DELTA_TWO, got: {}",
        resp2.output
    );
    // The first marker should NOT reappear in the second delta.
    assert!(
        !resp2.output.contains("DELTA_ONE"),
        "Second delta should NOT contain DELTA_ONE, got: {}",
        resp2.output
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 2. Empty when idle: reading from an idle session should return empty.
#[tokio::test(flavor = "multi_thread")]
async fn read_output_empty_when_idle() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    // Wait a bit more so everything is idle.
    sleep(Duration::from_secs(1)).await;

    // read_new_output (low-level) should be empty.
    let raw = session.read_new_output().await;
    assert!(
        raw.is_empty(),
        "Idle session should have no new output, got {} bytes",
        raw.len()
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 3. Large output: dir C:\Windows\System32 produces lots of output.
#[tokio::test(flavor = "multi_thread")]
async fn read_output_large_output() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    session
        .write_bytes(b"dir C:\\Windows\\System32\r")
        .await
        .unwrap();
    sleep(Duration::from_secs(5)).await;

    let resp = observation::handle_read_output(&session, Some(5000), None)
        .await
        .unwrap();
    assert!(
        resp.bytes_read > 100,
        "Large output should be > 100 bytes, got {}",
        resp.bytes_read
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 4. ANSI stripping: handle_read_output should strip all escape sequences.
#[tokio::test(flavor = "multi_thread")]
async fn read_output_ansi_stripping() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    session.write_bytes(b"echo ANSI_CHECK\r").await.unwrap();
    sleep(Duration::from_secs(3)).await;

    let resp = observation::handle_read_output(&session, Some(5000), None)
        .await
        .unwrap();
    assert!(
        !resp.output.contains('\x1b'),
        "Output should have no ESC characters after ANSI stripping, got: {:?}",
        resp.output
    );
    assert!(
        resp.output.contains("ANSI_CHECK"),
        "Output should still contain the command text"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

// ════════════════════════════════════════════════════════════════════
// get_screen tests
// ════════════════════════════════════════════════════════════════════

/// Helper to call get_screen via with_vt.
async fn call_get_screen(
    session: &terminal_mcp::session::Session,
    include_cursor: bool,
    include_colors: bool,
    diff_mode: bool,
) -> GetScreenResponse {
    session
        .with_vt(|vt| observation::get_screen(vt, include_cursor, include_colors, None, diff_mode))
        .await
}

/// 5. Basic screen: verify rows/cols metadata.
#[tokio::test(flavor = "multi_thread")]
async fn get_screen_basic_metadata() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    let resp = call_get_screen(&session, false, false, false).await;
    assert_eq!(resp.rows, 24, "Should report 24 rows");
    assert_eq!(resp.cols, 80, "Should report 80 cols");

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 6. Screen with cursor: include_cursor reports cursor position.
#[tokio::test(flavor = "multi_thread")]
async fn get_screen_with_cursor() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    let resp = call_get_screen(&session, true, false, false).await;
    // Cursor position should be within bounds.
    assert!(resp.cursor.row < resp.rows, "Cursor row within bounds");
    assert!(resp.cursor.col < resp.cols, "Cursor col within bounds");
    // The screen text should contain the cursor marker.
    assert!(
        resp.screen.contains('▏'),
        "Screen with include_cursor should contain cursor marker ▏"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 7. Screen content not empty after command.
#[tokio::test(flavor = "multi_thread")]
async fn get_screen_content_not_empty() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    session
        .write_bytes(b"echo SCREEN_CONTENT_TEST\r")
        .await
        .unwrap();
    sleep(Duration::from_secs(3)).await;

    let resp = call_get_screen(&session, false, false, false).await;
    assert!(
        !resp.screen.is_empty(),
        "Screen should have content after command"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 8. Color spans: include_colors produces color_spans field.
#[tokio::test(flavor = "multi_thread")]
async fn get_screen_color_spans() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    session.write_bytes(b"echo colors\r").await.unwrap();
    sleep(Duration::from_secs(3)).await;

    let resp = call_get_screen(&session, false, true, false).await;
    // color_spans should be Some (even if empty for plain cmd output).
    assert!(
        resp.color_spans.is_some(),
        "color_spans should be present when include_colors=true"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 9. Glyph styles: include_colors produces palette-backed per-glyph style rows.
#[tokio::test(flavor = "multi_thread")]
async fn get_screen_glyph_styles() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    session.write_bytes(b"echo glyph styles\r").await.unwrap();
    sleep(Duration::from_secs(3)).await;

    let resp = call_get_screen(&session, false, true, false).await;
    let glyph_styles = resp
        .glyph_styles
        .expect("glyph_styles should be present when include_colors=true");

    assert!(
        !glyph_styles.palette.is_empty(),
        "glyph_styles should include at least the default palette entry"
    );

    let lines: Vec<&str> = resp.screen.lines().collect();
    assert_eq!(
        glyph_styles.rows.len(),
        lines.len(),
        "glyph style rows should align with screen lines"
    );

    for (style_row, line) in glyph_styles.rows.iter().zip(lines.iter()) {
        assert_eq!(
            style_row.len(),
            line.chars().count(),
            "style row should align with rendered glyph count"
        );
        assert!(
            style_row
                .iter()
                .flatten()
                .all(|index| (*index as usize) < glyph_styles.palette.len()),
            "style rows should only reference palette entries"
        );
    }

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 10. Diff mode: first call takes snapshot, second reports changes.
#[tokio::test(flavor = "multi_thread")]
async fn get_screen_diff_mode() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    // First diff call — establishes snapshot.
    let resp1 = call_get_screen(&session, false, false, true).await;
    assert!(
        resp1.changed_rows.is_some(),
        "Diff mode should always return changed_rows"
    );

    // Send a command to change the screen.
    session
        .write_bytes(b"echo DIFF_CHANGE_MARKER\r")
        .await
        .unwrap();
    sleep(Duration::from_secs(3)).await;

    // Second diff call — should detect changed rows.
    let resp2 = call_get_screen(&session, false, false, true).await;
    let changed = resp2.changed_rows.unwrap();
    assert!(
        !changed.is_empty(),
        "Screen should have changed rows after command"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

// ════════════════════════════════════════════════════════════════════
// screenshot tests
// ════════════════════════════════════════════════════════════════════

/// 11. Valid PNG: magic bytes.
#[tokio::test(flavor = "multi_thread")]
async fn screenshot_valid_png() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    let png = session
        .with_vt(|vt| observation::screenshot(vt, "dark", 14, 1.0))
        .await
        .unwrap();
    assert!(
        png.starts_with(&[0x89, b'P', b'N', b'G']),
        "Screenshot should start with PNG magic bytes"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 12. Non-zero size: PNG should be reasonably sized.
#[tokio::test(flavor = "multi_thread")]
async fn screenshot_nonzero_size() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    session.write_bytes(b"echo SCREENSHOT\r").await.unwrap();
    sleep(Duration::from_secs(2)).await;

    let png = session
        .with_vt(|vt| observation::screenshot(vt, "dark", 14, 1.0))
        .await
        .unwrap();
    assert!(
        png.len() > 1000,
        "PNG should be > 1000 bytes, got {}",
        png.len()
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 12. Dark theme screenshot.
#[tokio::test(flavor = "multi_thread")]
async fn screenshot_dark_theme() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    let png = session
        .with_vt(|vt| observation::screenshot(vt, "dark", 14, 1.0))
        .await
        .unwrap();
    assert!(
        png.starts_with(&[0x89, b'P', b'N', b'G']),
        "Dark theme should produce valid PNG"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 13. Light theme screenshot.
#[tokio::test(flavor = "multi_thread")]
async fn screenshot_light_theme() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    let png = session
        .with_vt(|vt| observation::screenshot(vt, "light", 14, 1.0))
        .await
        .unwrap();
    assert!(
        png.starts_with(&[0x89, b'P', b'N', b'G']),
        "Light theme should produce valid PNG"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 14. Scaled: scale=2.0 should produce a larger PNG than scale=1.0.
#[tokio::test(flavor = "multi_thread")]
async fn screenshot_scaled() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    session.write_bytes(b"echo SCALE_TEST\r").await.unwrap();
    sleep(Duration::from_secs(2)).await;

    let png_1x = session
        .with_vt(|vt| observation::screenshot(vt, "dark", 14, 1.0))
        .await
        .unwrap();
    let png_2x = session
        .with_vt(|vt| observation::screenshot(vt, "dark", 14, 2.0))
        .await
        .unwrap();

    assert!(
        png_2x.len() > png_1x.len(),
        "2x scale PNG ({} bytes) should be larger than 1x ({} bytes)",
        png_2x.len(),
        png_1x.len()
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 15. Custom font size: font_size=20 should produce valid PNG.
#[tokio::test(flavor = "multi_thread")]
async fn screenshot_custom_font_size() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    let png = session
        .with_vt(|vt| observation::screenshot(vt, "dark", 20, 1.0))
        .await
        .unwrap();
    assert!(
        png.starts_with(&[0x89, b'P', b'N', b'G']),
        "Custom font size should produce valid PNG"
    );
    assert!(
        png.len() > 1000,
        "Custom font PNG should be reasonably sized, got {}",
        png.len()
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

// ════════════════════════════════════════════════════════════════════
// get_scrollback tests
// ════════════════════════════════════════════════════════════════════

/// 16. Empty/minimal scrollback for a freshly started session.
#[tokio::test(flavor = "multi_thread")]
async fn scrollback_empty_initial() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    // handle_get_scrollback with tail should work even on a new session.
    let result = observation::handle_get_scrollback(&session, Some(-10), None, None)
        .await
        .unwrap();
    // Result should be a valid JSON object with "type": "range".
    assert_eq!(result["type"], "range");
    assert!(
        result["total_lines"].as_u64().is_some(),
        "Should report total_lines"
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 17. Tail mode: after commands, scrollback tail returns recent lines.
#[tokio::test(flavor = "multi_thread")]
async fn scrollback_tail_mode() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    // Send several commands.
    for i in 0..3 {
        session
            .write_bytes(format!("echo TAIL_LINE_{i}\r").as_bytes())
            .await
            .unwrap();
        sleep(Duration::from_secs(1)).await;
    }
    sleep(Duration::from_secs(2)).await;

    let result = observation::handle_get_scrollback(&session, Some(-50), None, None)
        .await
        .unwrap();
    let content = result["content"].as_str().unwrap_or("");
    assert!(
        result["returned_lines"].as_u64().unwrap_or(0) > 0,
        "Tail should return some lines"
    );
    // At least one of the tail markers should appear.
    let has_marker = content.contains("TAIL_LINE_0")
        || content.contains("TAIL_LINE_1")
        || content.contains("TAIL_LINE_2");
    assert!(
        has_marker,
        "Tail content should contain at least one TAIL_LINE marker, got: {}",
        &content[..content.len().min(500)]
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 18. Search: echo a unique marker and search scrollback for it.
#[tokio::test(flavor = "multi_thread")]
async fn scrollback_search() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    session
        .write_bytes(b"echo UNIQUE_SEARCH_MARKER_XJ9Q\r")
        .await
        .unwrap();
    sleep(Duration::from_secs(3)).await;

    let result =
        observation::handle_get_scrollback(&session, None, Some("UNIQUE_SEARCH_MARKER_XJ9Q"), None)
            .await
            .unwrap();
    assert_eq!(result["type"], "search");
    let match_count = result["match_count"].as_u64().unwrap_or(0);
    assert!(
        match_count > 0,
        "Search should find at least one match for the marker, got {}",
        match_count
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}

/// 19. Range query: retrieve a specific range from scrollback.
#[tokio::test(flavor = "multi_thread")]
async fn scrollback_range_query() {
    let mgr = SessionManager::new();
    let (sid, session) = setup_session(&mgr).await;

    // Send a few commands to populate scrollback.
    for i in 0..5 {
        session
            .write_bytes(format!("echo RANGE_{i}\r").as_bytes())
            .await
            .unwrap();
        sleep(Duration::from_millis(800)).await;
    }
    sleep(Duration::from_secs(2)).await;

    // Request the first 20 lines (positive n = range from start).
    let result = observation::handle_get_scrollback(&session, Some(20), None, None)
        .await
        .unwrap();
    assert_eq!(result["type"], "range");
    let returned = result["returned_lines"].as_u64().unwrap_or(0);
    assert!(
        returned > 0,
        "Range query should return at least some lines, got {}",
        returned
    );

    drop(session);
    mgr.close_session(&sid).await.unwrap();
}
