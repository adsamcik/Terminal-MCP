//! Observation tools — read terminal state without modifying it.
//!
//! - [`get_screen`]: visible terminal grid with optional color/cursor/diff enrichments
//! - [`handle_read_output`]: delta output since last read with ANSI stripping
//! - [`screenshot`]: PNG render of screen via fontdue + tiny-skia
//! - [`handle_get_scrollback`]: scrollback buffer content with search

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::ansi::strip_ansi;
use crate::session::Session;
use crate::terminal::{CellInfo, Color, ColorSpan, VtParser};

const MAX_TIMEOUT_MS: u64 = 300_000; // 5 minutes

// ── Serde helpers ──────────────────────────────────────────────────

fn is_false(v: &bool) -> bool {
    !v
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

// ── Public response types ──────────────────────────────────────────

/// Cursor position reported to the MCP client.
#[derive(Debug, Clone, Serialize)]
pub struct CursorPosition {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
}

/// A color span serialized for JSON transport.
///
/// Colors are rendered as human-readable strings:
/// - Named ANSI: `"red"`, `"bright_blue"`, …
/// - Indexed: `"idx:123"`
/// - True-color: `"#rrggbb"`
/// - `None` means default terminal color.
#[derive(Debug, Clone, Serialize)]
pub struct SerializedColorSpan {
    pub row: u16,
    pub col: u16,
    pub len: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub inverse: bool,
}

/// A compact, reusable style entry for per-glyph screen annotations.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SerializedGlyphStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub inverse: bool,
}

/// Per-glyph style indices aligned to the `screen` text grid.
///
/// Each row entry points into `palette`. When `include_cursor=true` and the
/// VT cursor is visible, the synthetic `▏` cursor marker is encoded as `null`
/// in `rows`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GlyphStyleGrid {
    pub palette: Vec<SerializedGlyphStyle>,
    pub rows: Vec<Vec<Option<u16>>>,
}

/// Full response payload for the `get_screen` tool.
#[derive(Debug, Clone, Serialize)]
pub struct GetScreenResponse {
    pub screen: String,
    pub rows: u16,
    pub cols: u16,
    pub cursor: CursorPosition,
    /// Whether the alternate screen buffer is active.
    ///
    /// **Windows/ConPTY caveat:** ConPTY handles DECSET 1049 internally and
    /// does not forward the escape sequence, so this will always be `false`
    /// on Windows even when vim, less, etc. have activated the alternate
    /// buffer.  See `alternate_screen_note` for a runtime hint.
    pub is_alternate_screen: bool,
    /// If non-`None`, a short diagnostic note about `is_alternate_screen`
    /// reliability on the current platform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alternate_screen_note: Option<String>,
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_spans: Option<Vec<SerializedColorSpan>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlights: Option<Vec<SerializedColorSpan>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glyph_styles: Option<GlyphStyleGrid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_rows: Option<Vec<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_content: Option<Vec<ChangedRow>>,
}

/// A row that changed between the current and previous screen snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedRow {
    pub row: u16,
    pub current: String,
    pub previous: String,
}

/// Optional sub-rectangle to read instead of the full screen.
#[derive(Debug, Clone, Copy)]
pub struct ScreenRegion {
    pub top: u16,
    pub left: u16,
    pub bottom: u16,
    pub right: u16,
}

// ── Color serialization ────────────────────────────────────────────

/// Standard ANSI color names (indices 0–7).
const ANSI_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// Bright ANSI color names (indices 8–15).
const BRIGHT_NAMES: [&str; 8] = [
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

fn serialize_color(color: &Color) -> Option<String> {
    match color {
        Color::Default => None,
        Color::Indexed(i) => {
            let i = *i;
            if i < 8 {
                Some(ANSI_NAMES[i as usize].to_string())
            } else if i < 16 {
                Some(BRIGHT_NAMES[(i - 8) as usize].to_string())
            } else {
                Some(format!("idx:{i}"))
            }
        }
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

fn serialize_span(span: &ColorSpan) -> SerializedColorSpan {
    SerializedColorSpan {
        row: span.row,
        col: span.col,
        len: span.len,
        fg: span.fg.as_ref().and_then(serialize_color),
        bg: span.bg.as_ref().and_then(serialize_color),
        bold: span.bold,
        italic: span.italic,
        underline: span.underline,
        inverse: span.inverse,
    }
}

fn serialize_style(
    fg: &Color,
    bg: &Color,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
) -> SerializedGlyphStyle {
    SerializedGlyphStyle {
        fg: serialize_color(fg),
        bg: serialize_color(bg),
        bold,
        italic,
        underline,
        inverse,
    }
}

fn serialize_cell_style(cell: &CellInfo) -> SerializedGlyphStyle {
    serialize_style(
        &cell.fg,
        &cell.bg,
        cell.bold,
        cell.italic,
        cell.underline,
        cell.inverse,
    )
}

fn palette_index(
    palette: &mut Vec<SerializedGlyphStyle>,
    style: SerializedGlyphStyle,
) -> Option<u16> {
    if let Some(index) = palette.iter().position(|candidate| candidate == &style) {
        return Some(index as u16);
    }

    let index = palette.len();
    if index > u16::MAX as usize {
        return None;
    }

    palette.push(style);
    Some(index as u16)
}

fn build_glyph_styles(
    vt: &VtParser,
    include_cursor: bool,
    region: Option<ScreenRegion>,
) -> GlyphStyleGrid {
    struct GlyphRowEntry {
        style_index: Option<u16>,
        is_trimmable_whitespace: bool,
    }

    let screen = vt.screen();
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = vt.cursor_position();

    let bounds = region.unwrap_or(ScreenRegion {
        top: 0,
        left: 0,
        bottom: rows.saturating_sub(1),
        right: cols.saturating_sub(1),
    });
    let include_cursor = include_cursor && region.is_none() && vt.cursor_visible();

    let mut palette = vec![SerializedGlyphStyle::default()];
    let mut style_rows = Vec::new();

    for row in bounds.top..=bounds.bottom {
        let mut row_entries = Vec::new();

        for col in bounds.left..=bounds.right {
            if include_cursor && row == cursor_row && col == cursor_col {
                row_entries.push(GlyphRowEntry {
                    style_index: None,
                    is_trimmable_whitespace: false,
                });
            }

            let Some(cell) = vt.screen_cell(row, col) else {
                continue;
            };

            if cell.is_wide_continuation {
                continue;
            }

            let rendered = if cell.contents.is_empty() {
                " ".to_string()
            } else {
                cell.contents.clone()
            };
            let style_index = palette_index(&mut palette, serialize_cell_style(&cell)).unwrap_or(0);

            row_entries.push(GlyphRowEntry {
                style_index: Some(style_index),
                is_trimmable_whitespace: rendered.chars().all(char::is_whitespace),
            });
        }

        if include_cursor && row == cursor_row && cursor_col >= cols {
            row_entries.push(GlyphRowEntry {
                style_index: None,
                is_trimmable_whitespace: false,
            });
        }

        while row_entries
            .last()
            .is_some_and(|entry| entry.is_trimmable_whitespace)
        {
            row_entries.pop();
        }

        style_rows.push(
            row_entries
                .into_iter()
                .map(|entry| entry.style_index)
                .collect(),
        );
    }

    while style_rows
        .last()
        .is_some_and(|row: &Vec<Option<u16>>| row.is_empty())
    {
        style_rows.pop();
    }

    GlyphStyleGrid {
        palette,
        rows: style_rows,
    }
}

// ── get_screen ─────────────────────────────────────────────────────

/// On Windows, ConPTY handles DECSET 1049 internally and does not pass
/// the escape sequence through to the output stream. Return a short
/// diagnostic note so callers know the field may be unreliable.
fn conpty_alternate_screen_note() -> Option<String> {
    #[cfg(windows)]
    {
        Some(
            "On Windows (ConPTY), alternate screen detection is unreliable \
             because ConPTY intercepts DECSET 1049 internally."
                .to_string(),
        )
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Build a [`GetScreenResponse`] from the current parser state.
///
/// * `include_cursor` — insert a `▏` marker at the visible cursor position.
/// * `include_colors` — attach color-span, highlight, and per-glyph style arrays.
/// * `region` — if `Some`, read only a sub-rectangle of the screen.
/// * `diff_mode` — if `true`, include only changed row indices and take a
///   snapshot for the next diff comparison.
pub fn get_screen(
    vt: &mut VtParser,
    include_cursor: bool,
    include_colors: bool,
    region: Option<ScreenRegion>,
    diff_mode: bool,
) -> GetScreenResponse {
    let screen_ref = vt.screen();
    let (rows, cols) = screen_ref.size();
    let (cursor_row, cursor_col) = vt.cursor_position();
    let include_cursor = include_cursor && region.is_none() && vt.cursor_visible();

    // Screen text
    let screen_text = match region {
        Some(r) => vt.read_region(r.top, r.left, r.bottom, r.right),
        None if include_cursor => vt.screen_contents_with_cursor(),
        None => vt.screen_contents(),
    };

    // Color spans / highlights
    let (color_spans, highlights, glyph_styles) = if include_colors {
        let spans: Vec<SerializedColorSpan> = vt.color_spans().iter().map(serialize_span).collect();
        let hi: Vec<SerializedColorSpan> = vt.highlights().iter().map(serialize_span).collect();
        let glyph_styles = build_glyph_styles(vt, include_cursor, region);
        (
            Some(spans),
            if hi.is_empty() { None } else { Some(hi) },
            Some(glyph_styles),
        )
    } else {
        (None, None, None)
    };

    // Changed rows (diff mode)
    let (changed_rows, changed_content) = if diff_mode {
        let content = vt.changed_rows_with_content();
        let row_indices: Vec<u16> = content.iter().map(|(row, _, _)| *row).collect();
        let changed: Vec<ChangedRow> = content
            .into_iter()
            .map(|(row, current, previous)| ChangedRow {
                row,
                current,
                previous,
            })
            .collect();
        vt.take_snapshot();
        (
            Some(row_indices),
            if changed.is_empty() {
                None
            } else {
                Some(changed)
            },
        )
    } else {
        (None, None)
    };

    GetScreenResponse {
        screen: screen_text,
        rows,
        cols,
        cursor: CursorPosition {
            row: cursor_row,
            col: cursor_col,
            visible: vt.cursor_visible(),
        },
        is_alternate_screen: vt.is_alternate_screen(),
        alternate_screen_note: conpty_alternate_screen_note(),
        title: vt.terminal_title(),
        color_spans,
        highlights,
        glyph_styles,
        changed_rows,
        changed_content,
    }
}

// ── read_output ────────────────────────────────────────────────────

/// Response payload for the `read_output` tool.
#[derive(Debug, Clone, Serialize)]
pub struct ReadOutputResponse {
    pub output: String,
    pub bytes_read: usize,
    pub has_more: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub dropped_bytes: usize,
    pub is_idle: bool,
    pub idle_duration_ms: u64,
    pub cursor: CursorPosition,
    pub exit_code: Option<i32>,
}

/// Read new output from a session since the last read (delta mode).
///
/// If no output is immediately available, polls every 100 ms up to
/// `timeout_ms` (default 5 000 ms).  Returns early as soon as data
/// appears.  ANSI escape sequences are stripped from the returned text.
pub async fn handle_read_output(
    session: &Session,
    timeout_ms: Option<u64>,
    max_bytes: Option<usize>,
) -> Result<ReadOutputResponse> {
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(5000).min(MAX_TIMEOUT_MS));
    let max = max_bytes.unwrap_or(16384).clamp(1, 10_485_760);
    let poll_interval = Duration::from_millis(100);

    // Wait for output, polling periodically up to the timeout.
    let raw = {
        let initial = session.read_new_output_chunk().await;
        if !initial.bytes.is_empty() || initial.dropped_bytes > 0 {
            initial
        } else {
            let deadline = tokio::time::Instant::now() + timeout;
            let mut data = session.read_new_output_chunk().await;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(poll_interval).await;
                data = session.read_new_output_chunk().await;
                if !data.bytes.is_empty() || data.dropped_bytes > 0 {
                    break;
                }
            }
            data
        }
    };

    // Interpret as (lossy) UTF-8 and strip ANSI escapes.
    let text = String::from_utf8_lossy(&raw.bytes);
    let clean = strip_ansi(&text);

    // Truncate to max_bytes on a char boundary.
    let (output, has_more) = if clean.len() > max {
        let mut end = max;
        while end > 0 && !clean.is_char_boundary(end) {
            end -= 1;
        }
        (clean[..end].to_string(), true)
    } else {
        (clean, false)
    };

    let bytes_read = output.len();

    // Idle detection (consider idle after 500 ms of no output).
    let idle_threshold = Duration::from_millis(500);
    let is_idle = session.is_idle(idle_threshold).await;
    let idle_duration_ms = session.idle_duration_ms().await;

    // Cursor position.
    let (row, col) = session.cursor_position().await;
    let visible = session.cursor_visible().await;
    let cursor = CursorPosition { row, col, visible };

    let exit_code = session.cached_exit_code().await;

    Ok(ReadOutputResponse {
        output,
        bytes_read,
        has_more,
        dropped_bytes: raw.dropped_bytes,
        is_idle,
        idle_duration_ms,
        cursor,
        exit_code,
    })
}

// ── Stub tools ─────────────────────────────────────────────────────

/// Render a PNG screenshot of the terminal via fontdue + tiny-skia.
pub fn screenshot(vt: &VtParser, theme: &str, font_size: u32, scale: f32) -> Result<Vec<u8>> {
    crate::screenshot::render_screenshot(vt.screen(), theme, font_size, scale)
}

/// Placeholder for `get_scrollback` — returns scrollback buffer lines.
pub async fn handle_get_scrollback(
    session: &crate::session::Session,
    lines: Option<i64>,
    search: Option<&str>,
    context: Option<usize>,
) -> anyhow::Result<serde_json::Value> {
    if let Some(pattern) = search {
        let ctx = context.unwrap_or(2);
        let matches = session.scrollback_search(pattern, ctx).await?;
        return Ok(serde_json::json!({
            "type": "search",
            "pattern": pattern,
            "total_lines": session.scrollback_len().await,
            "match_count": matches.len(),
            "matches": matches,
        }));
    }

    let n = lines.unwrap_or(-100).clamp(-50_000, 50_000);
    let total = session.scrollback_len().await;

    let result_lines = if n < 0 {
        let count = (-n) as usize;
        session.scrollback_tail(count).await
    } else {
        let count = n as usize;
        session.scrollback_range(0, count).await
    };

    Ok(serde_json::json!({
        "type": "range",
        "total_lines": total,
        "returned_lines": result_lines.len(),
        "content": result_lines.join("\n"),
    }))
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_parser(rows: u16, cols: u16) -> VtParser {
        VtParser::new(rows, cols, 0)
    }

    #[test]
    fn empty_screen_returns_empty_text() {
        let mut vt = make_parser(24, 80);
        let resp = get_screen(&mut vt, false, false, None, false);
        assert!(resp.screen.is_empty());
        assert_eq!(resp.rows, 24);
        assert_eq!(resp.cols, 80);
        assert!(!resp.is_alternate_screen);
    }

    #[test]
    fn screen_with_text_trims_trailing() {
        let mut vt = make_parser(5, 20);
        vt.process(b"hello world");
        let resp = get_screen(&mut vt, false, false, None, false);
        assert_eq!(resp.screen, "hello world");
    }

    #[test]
    fn cursor_marker_appears() {
        let mut vt = make_parser(5, 20);
        vt.process(b"abc");
        let resp = get_screen(&mut vt, true, false, None, false);
        // Cursor should be right after "abc" → "abc▏"
        assert!(resp.screen.contains('▏'), "screen: {}", resp.screen);
    }

    #[test]
    fn region_reads_sub_rectangle() {
        let mut vt = make_parser(5, 10);
        vt.process(b"0123456789\r\nabcdefghij\r\nKLMNOPQRST");
        let region = ScreenRegion {
            top: 1,
            left: 2,
            bottom: 2,
            right: 5,
        };
        let resp = get_screen(&mut vt, false, false, Some(region), false);
        let lines: Vec<&str> = resp.screen.lines().collect();
        assert_eq!(lines[0], "cdef");
        assert_eq!(lines[1], "MNOP");
    }

    #[test]
    fn color_spans_serialization() {
        assert_eq!(serialize_color(&Color::Default), None);
        assert_eq!(serialize_color(&Color::Indexed(1)), Some("red".to_string()));
        assert_eq!(
            serialize_color(&Color::Indexed(9)),
            Some("bright_red".to_string())
        );
        assert_eq!(
            serialize_color(&Color::Indexed(200)),
            Some("idx:200".to_string())
        );
        assert_eq!(
            serialize_color(&Color::Rgb(255, 128, 0)),
            Some("#ff8000".to_string())
        );
    }

    #[test]
    fn bold_text_produces_color_spans() {
        let mut vt = make_parser(5, 20);
        // ESC[1m = bold on, ESC[0m = reset
        vt.process(b"\x1b[1mBOLD\x1b[0m normal");
        let resp = get_screen(&mut vt, false, true, None, false);
        let spans = resp.color_spans.expect("should have color_spans");
        assert!(!spans.is_empty(), "bold text should produce spans");
        assert!(spans.iter().any(|s| s.bold), "at least one span is bold");
    }

    #[test]
    fn response_serializes_to_json() {
        let mut vt = make_parser(3, 10);
        let resp = get_screen(&mut vt, false, false, None, false);
        let json = serde_json::to_string(&resp).expect("serialization should work");
        assert!(json.contains("\"rows\":3"));
        assert!(json.contains("\"cols\":10"));
        // color_spans omitted when None
        assert!(!json.contains("color_spans"));
    }

    #[test]
    fn diff_mode_returns_changed_rows() {
        let mut vt = make_parser(5, 10);
        // First call with diff_mode: takes snapshot
        let resp1 = get_screen(&mut vt, false, false, None, true);
        assert!(resp1.changed_rows.is_some());

        // Process some text
        vt.process(b"hello");

        // Second call: should report changed rows
        let resp2 = get_screen(&mut vt, false, false, None, true);
        let changed = resp2.changed_rows.unwrap();
        assert!(!changed.is_empty(), "row 0 should have changed");
        assert!(changed.contains(&0));
    }

    #[test]
    fn diff_mode_includes_changed_content() {
        let mut vt = make_parser(5, 20);
        vt.process(b"original");
        // First call with diff_mode takes snapshot
        let resp1 = get_screen(&mut vt, false, false, None, true);
        assert!(
            resp1.changed_content.is_none()
                || resp1
                    .changed_content
                    .as_ref()
                    .map_or(true, |c| c.is_empty())
        );

        // Change content
        vt.process(b"\x1b[1;1Hmodified");
        let resp2 = get_screen(&mut vt, false, false, None, true);
        assert!(resp2.changed_content.is_some());
        let content = resp2.changed_content.unwrap();
        assert!(!content.is_empty());
        assert_eq!(content[0].row, 0);
        assert!(content[0].current.starts_with("modified"));
        assert!(content[0].previous.starts_with("original"));
    }

    #[test]
    fn color_serialization_all_ansi_colors() {
        for i in 0u8..8 {
            let color = Color::Indexed(i);
            let s = serialize_color(&color).unwrap();
            assert_eq!(s, ANSI_NAMES[i as usize]);
        }
    }

    #[test]
    fn color_serialization_all_bright_colors() {
        for i in 8u8..16 {
            let color = Color::Indexed(i);
            let s = serialize_color(&color).unwrap();
            assert_eq!(s, BRIGHT_NAMES[(i - 8) as usize]);
        }
    }

    #[test]
    fn color_serialization_indexed_above_16() {
        assert_eq!(serialize_color(&Color::Indexed(16)), Some("idx:16".into()));
        assert_eq!(
            serialize_color(&Color::Indexed(255)),
            Some("idx:255".into())
        );
    }

    #[test]
    fn color_serialization_rgb() {
        assert_eq!(
            serialize_color(&Color::Rgb(0, 0, 0)),
            Some("#000000".into())
        );
        assert_eq!(
            serialize_color(&Color::Rgb(255, 255, 255)),
            Some("#ffffff".into())
        );
        assert_eq!(
            serialize_color(&Color::Rgb(171, 205, 239)),
            Some("#abcdef".into())
        );
    }

    #[test]
    fn serialize_span_omits_defaults() {
        let span = ColorSpan {
            row: 0,
            col: 0,
            len: 5,
            fg: Some(Color::Default),
            bg: Some(Color::Default),
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        };
        let s = serialize_span(&span);
        assert!(s.fg.is_none());
        assert!(s.bg.is_none());
        // Check JSON serialization skips false bools
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("bold"));
        assert!(!json.contains("italic"));
    }

    #[test]
    fn serialize_span_includes_non_defaults() {
        let span = ColorSpan {
            row: 1,
            col: 2,
            len: 3,
            fg: Some(Color::Indexed(1)),
            bg: Some(Color::Rgb(10, 20, 30)),
            bold: true,
            italic: false,
            underline: true,
            inverse: false,
        };
        let s = serialize_span(&span);
        assert_eq!(s.fg, Some("red".into()));
        assert_eq!(s.bg, Some("#0a141e".into()));
        assert!(s.bold);
        assert!(s.underline);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"bold\":true"));
        assert!(json.contains("\"underline\":true"));
    }

    #[test]
    fn get_screen_with_colors() {
        let mut vt = make_parser(5, 20);
        vt.process(b"\x1b[31mRED\x1b[0m");
        let resp = get_screen(&mut vt, false, true, None, false);
        assert!(resp.color_spans.is_some());
        let spans = resp.color_spans.unwrap();
        assert!(!spans.is_empty());
    }

    #[test]
    fn get_screen_without_colors() {
        let mut vt = make_parser(5, 20);
        vt.process(b"\x1b[31mRED\x1b[0m");
        let resp = get_screen(&mut vt, false, false, None, false);
        assert!(resp.color_spans.is_none());
        assert!(resp.highlights.is_none());
        assert!(resp.glyph_styles.is_none());
    }

    #[test]
    fn get_screen_with_region() {
        let mut vt = make_parser(5, 10);
        vt.process(b"ABCDEFGHIJ\r\nklmnopqrst");
        let region = ScreenRegion {
            top: 0,
            left: 0,
            bottom: 0,
            right: 2,
        };
        let resp = get_screen(&mut vt, false, false, Some(region), false);
        assert_eq!(resp.screen, "ABC");
    }

    #[test]
    fn get_screen_alternate_screen_flag() {
        let mut vt = make_parser(5, 20);
        assert!(!get_screen(&mut vt, false, false, None, false).is_alternate_screen);
        vt.process(b"\x1b[?1049h");
        assert!(get_screen(&mut vt, false, false, None, false).is_alternate_screen);
    }

    #[test]
    fn get_screen_title() {
        let mut vt = make_parser(5, 20);
        assert!(
            get_screen(&mut vt, false, false, None, false)
                .title
                .is_none()
        );
        vt.process(b"\x1b]2;Test Title\x1b\\");
        assert_eq!(
            get_screen(&mut vt, false, false, None, false).title,
            Some("Test Title".into())
        );
    }

    #[test]
    fn get_screen_highlights_inverse_text() {
        let mut vt = make_parser(5, 40);
        vt.process(b"\x1b[7mSELECTED\x1b[0m");
        let resp = get_screen(&mut vt, false, true, None, false);
        assert!(resp.highlights.is_some());
        let hi = resp.highlights.unwrap();
        assert!(!hi.is_empty());
        assert!(hi[0].inverse);
    }

    #[test]
    fn get_screen_no_highlights_for_non_inverse() {
        let mut vt = make_parser(5, 40);
        vt.process(b"\x1b[1mBOLD ONLY\x1b[0m");
        let resp = get_screen(&mut vt, false, true, None, false);
        // highlights should be None when no inverse spans exist
        assert!(resp.highlights.is_none());
    }

    #[test]
    fn get_screen_glyph_styles_align_with_screen_text() {
        let mut vt = make_parser(5, 20);
        vt.process(b"\x1b[31mRED\x1b[0m X");
        let resp = get_screen(&mut vt, false, true, None, false);
        let glyph_styles = resp.glyph_styles.expect("should have glyph styles");

        assert_eq!(resp.screen, "RED X");
        assert_eq!(glyph_styles.rows.len(), 1);
        assert_eq!(glyph_styles.rows[0].len(), 5);

        let red_index = glyph_styles.rows[0][0].expect("first glyph should have a style");
        assert_eq!(glyph_styles.rows[0][1], Some(red_index));
        assert_eq!(glyph_styles.rows[0][2], Some(red_index));
        assert_eq!(
            glyph_styles.palette[red_index as usize].fg,
            Some("red".to_string())
        );
        assert_eq!(glyph_styles.rows[0][3], Some(0));
        assert_eq!(glyph_styles.rows[0][4], Some(0));
    }

    #[test]
    fn get_screen_glyph_styles_mark_cursor_with_null() {
        let mut vt = make_parser(5, 20);
        vt.process(b"abc");
        let resp = get_screen(&mut vt, true, true, None, false);
        let glyph_styles = resp.glyph_styles.expect("should have glyph styles");

        assert_eq!(resp.screen, "abc▏");
        assert_eq!(
            glyph_styles.rows,
            vec![vec![Some(0), Some(0), Some(0), None]]
        );
    }
}
