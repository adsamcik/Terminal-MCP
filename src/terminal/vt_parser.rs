//! Wrapper around `vt100::Parser` for terminal state tracking.
//!
//! Provides screen content access, terminal mode queries, color span
//! extraction, screen diffing, region reading, and scrollback access.

use serde::{Deserialize, Serialize};

// ── Public types ───────────────────────────────────────────────────

/// Terminal color representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Color {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl From<vt100::Color> for Color {
    fn from(c: vt100::Color) -> Self {
        match c {
            vt100::Color::Default => Color::Default,
            vt100::Color::Idx(i) => Color::Indexed(i),
            vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        }
    }
}

/// A contiguous span of cells sharing the same formatting attributes.
#[derive(Debug, Clone, Serialize)]
pub struct ColorSpan {
    pub row: u16,
    pub col: u16,
    pub len: u16,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// Mouse protocol mode (mirrors `vt100::MouseProtocolMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseMode {
    None,
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

impl From<vt100::MouseProtocolMode> for MouseMode {
    fn from(m: vt100::MouseProtocolMode) -> Self {
        match m {
            vt100::MouseProtocolMode::None => MouseMode::None,
            vt100::MouseProtocolMode::Press => MouseMode::Press,
            vt100::MouseProtocolMode::PressRelease => MouseMode::PressRelease,
            vt100::MouseProtocolMode::ButtonMotion => MouseMode::ButtonMotion,
            vt100::MouseProtocolMode::AnyMotion => MouseMode::AnyMotion,
        }
    }
}

/// Information about a single terminal cell.
#[derive(Debug, Clone)]
pub struct CellInfo {
    pub contents: String,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub is_wide: bool,
    pub is_wide_continuation: bool,
}

// ── Callbacks (title capture) ──────────────────────────────────────

/// Captures OSC window-title sequences from the byte stream.
#[derive(Default)]
struct TerminalCallbacks {
    title: Option<String>,
}

impl vt100::Callbacks for TerminalCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).into_owned());
    }
}

// ── VtParser ───────────────────────────────────────────────────────

/// Wrapper around `vt100::Parser` for terminal state tracking.
pub struct VtParser {
    parser: vt100::Parser<TerminalCallbacks>,
    /// Snapshot of the screen saved by [`take_snapshot`] for diffing.
    previous_screen: Option<vt100::Screen>,
}

impl VtParser {
    /// Create a new parser with the given dimensions and scrollback capacity.
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                rows,
                cols,
                scrollback,
                TerminalCallbacks::default(),
            ),
            previous_screen: None,
        }
    }

    /// Feed raw PTY output bytes through the parser.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Resize the virtual screen to new dimensions.
    pub fn set_size(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }

    /// Direct access to the underlying `vt100::Screen`.
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    // ── Screen content access ──────────────────────────────────────

    /// Plain text of the visible terminal.
    ///
    /// Trailing whitespace is trimmed per line; trailing blank rows are
    /// omitted.
    pub fn screen_contents(&self) -> String {
        trim_screen_text(&self.parser.screen().contents())
    }

    /// Screen text with the cursor position marked by `▏`.
    ///
    /// The marker is inserted *before* the character at the cursor column.
    /// Trailing blank rows (other than the cursor row) are omitted.
    pub fn screen_contents_with_cursor(&self) -> String {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let cursor_visible = !screen.hide_cursor();

        let mut lines: Vec<String> = Vec::with_capacity(rows as usize);

        for row in 0..rows {
            let mut line = String::new();
            for col in 0..cols {
                if cursor_visible && row == cursor_row && col == cursor_col {
                    line.push('▏');
                }
                if let Some(cell) = screen.cell(row, col) {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let text = cell.contents();
                    if text.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&text);
                    }
                }
            }
            // Cursor at or past end of row
            if cursor_visible && row == cursor_row && cursor_col >= cols {
                line.push('▏');
            }
            lines.push(line.trim_end().to_string());
        }

        // Remove trailing blank lines, but keep the cursor row
        while lines.len() > 1
            && lines.last().is_some_and(|l| l.is_empty())
            && (lines.len() - 1) as u16 != cursor_row
        {
            lines.pop();
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }

        lines.join("\n")
    }

    /// Information about a single cell at `(row, col)`.
    pub fn screen_cell(&self, row: u16, col: u16) -> Option<CellInfo> {
        self.parser.screen().cell(row, col).map(|cell| CellInfo {
            contents: cell.contents().to_string(),
            fg: cell.fgcolor().into(),
            bg: cell.bgcolor().into(),
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
            is_wide: cell.is_wide(),
            is_wide_continuation: cell.is_wide_continuation(),
        })
    }

    /// Current cursor position as `(row, col)`.
    pub fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// Window title set via OSC escape sequences (e.g. `\e]2;…\a`).
    pub fn terminal_title(&self) -> Option<String> {
        self.parser.callbacks().title.clone()
    }

    // ── Terminal mode queries ──────────────────────────────────────

    /// Whether the alternate screen buffer is active (e.g. vim, less).
    ///
    /// **Windows/ConPTY caveat:** On Windows, ConPTY handles DECSET 1049
    /// (alternate screen buffer) internally and does *not* pass the escape
    /// sequence through to the output stream.  Because this method relies on
    /// the `vt100` parser seeing the raw sequence, it will always return
    /// `false` under ConPTY even when a program like vim or less has
    /// activated the alternate buffer.  The method works correctly when raw
    /// VT sequences are fed directly (unit tests, Unix PTY backends).
    pub fn is_alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// Whether application cursor key mode is enabled (DECCKM).
    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    /// Whether bracketed paste mode is enabled.
    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    /// Whether the cursor is visible (inverse of `hide_cursor`).
    pub fn cursor_visible(&self) -> bool {
        !self.parser.screen().hide_cursor()
    }

    /// Current xterm mouse protocol mode.
    pub fn mouse_mode(&self) -> MouseMode {
        self.parser.screen().mouse_protocol_mode().into()
    }

    // ── Color span extraction ──────────────────────────────────────

    /// Extract spans of consecutive cells sharing the same non-default
    /// attributes.  Cells with all-default styling are skipped.
    pub fn color_spans(&self) -> Vec<ColorSpan> {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let mut spans = Vec::new();

        for row in 0..rows {
            let mut col = 0u16;
            while col < cols {
                let cell = match screen.cell(row, col) {
                    Some(c) => c,
                    None => {
                        col += 1;
                        continue;
                    }
                };

                if cell.is_wide_continuation() {
                    col += 1;
                    continue;
                }

                let fg = cell.fgcolor();
                let bg = cell.bgcolor();
                let bold = cell.bold();
                let italic = cell.italic();
                let underline = cell.underline();
                let inverse = cell.inverse();

                // Skip cells with all-default attributes
                if is_default_attrs(fg, bg, bold, italic, underline, inverse) {
                    col += 1;
                    continue;
                }

                let start_col = col;
                col += 1;

                // Extend span while attributes match
                while col < cols {
                    let next = match screen.cell(row, col) {
                        Some(c) => c,
                        None => break,
                    };

                    if next.is_wide_continuation() {
                        col += 1;
                        continue;
                    }

                    if next.fgcolor() == fg
                        && next.bgcolor() == bg
                        && next.bold() == bold
                        && next.italic() == italic
                        && next.underline() == underline
                        && next.inverse() == inverse
                    {
                        col += 1;
                    } else {
                        break;
                    }
                }

                spans.push(ColorSpan {
                    row,
                    col: start_col,
                    len: col - start_col,
                    fg: Some(fg.into()),
                    bg: Some(bg.into()),
                    bold,
                    italic,
                    underline,
                    inverse,
                });
            }
        }

        spans
    }

    /// Inverse-video spans only — typically used by TUI apps for selection
    /// highlights.
    pub fn highlights(&self) -> Vec<ColorSpan> {
        self.color_spans()
            .into_iter()
            .filter(|span| span.inverse)
            .collect()
    }

    // ── Screen diffing ─────────────────────────────────────────────

    /// Terminal byte stream that transforms `previous` into the current
    /// screen (via `vt100::Screen::contents_diff`).
    pub fn screen_diff(&self, previous: &vt100::Screen) -> Vec<u8> {
        self.parser.screen().contents_diff(previous)
    }

    /// Save the current screen as a snapshot for later comparison.
    pub fn take_snapshot(&mut self) {
        self.previous_screen = Some(self.parser.screen().clone());
    }

    /// Reference to the previously saved snapshot, if any.
    pub fn previous_snapshot(&self) -> Option<&vt100::Screen> {
        self.previous_screen.as_ref()
    }

    /// Row indices that changed between the stored snapshot and the current
    /// screen (0-based).  Returns an empty vec if no snapshot exists.
    pub fn changed_rows(&self) -> Vec<u16> {
        let Some(prev) = &self.previous_screen else {
            return Vec::new();
        };

        let screen = self.parser.screen();
        let (rows, cols) = screen.size();

        (0..rows)
            .filter(|&row| {
                for col in 0..cols {
                    let curr = screen.cell(row, col);
                    let prev_cell = prev.cell(row, col);
                    match (curr, prev_cell) {
                        (Some(a), Some(b)) => {
                            if a != b {
                                return true;
                            }
                        }
                        (None, None) => {}
                        _ => return true,
                    }
                }
                false
            })
            .collect()
    }

    /// Row indices and their content that changed between the stored snapshot
    /// and the current screen. Each entry includes the row index, current text,
    /// and previous text. Returns an empty vec if no snapshot exists.
    pub fn changed_rows_with_content(&self) -> Vec<(u16, String, String)> {
        let Some(prev) = &self.previous_screen else {
            return Vec::new();
        };

        let screen = self.parser.screen();
        let (rows, cols) = screen.size();

        (0..rows)
            .filter_map(|row| {
                let mut changed = false;
                for col in 0..cols {
                    let curr = screen.cell(row, col);
                    let prev_cell = prev.cell(row, col);
                    match (curr, prev_cell) {
                        (Some(a), Some(b)) => {
                            if a != b {
                                changed = true;
                                break;
                            }
                        }
                        (None, None) => {}
                        _ => {
                            changed = true;
                            break;
                        }
                    }
                }
                if !changed {
                    return None;
                }

                let mut current_text = String::new();
                for col in 0..cols {
                    if let Some(cell) = screen.cell(row, col) {
                        if cell.is_wide_continuation() {
                            continue;
                        }
                        let text = cell.contents();
                        if text.is_empty() {
                            current_text.push(' ');
                        } else {
                            current_text.push_str(&text);
                        }
                    }
                }

                let mut previous_text = String::new();
                for col in 0..cols {
                    if let Some(cell) = prev.cell(row, col) {
                        if cell.is_wide_continuation() {
                            continue;
                        }
                        let text = cell.contents();
                        if text.is_empty() {
                            previous_text.push(' ');
                        } else {
                            previous_text.push_str(&text);
                        }
                    }
                }

                Some((
                    row,
                    current_text.trim_end().to_string(),
                    previous_text.trim_end().to_string(),
                ))
            })
            .collect()
    }

    // ── Region reading ─────────────────────────────────────────────

    /// Text content of a sub-rectangle (inclusive bounds).
    ///
    /// Trailing whitespace is trimmed per line; trailing blank rows are
    /// omitted.
    pub fn read_region(&self, top: u16, left: u16, bottom: u16, right: u16) -> String {
        let screen = self.parser.screen();
        let mut lines = Vec::new();

        for row in top..=bottom {
            let mut line = String::new();
            for col in left..=right {
                if let Some(cell) = screen.cell(row, col) {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let text = cell.contents();
                    if text.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&text);
                    }
                }
            }
            lines.push(line.trim_end().to_string());
        }

        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }

        lines.join("\n")
    }

    // ── Scrollback access ──────────────────────────────────────────

    /// Number of scrollback lines currently stored.
    ///
    /// Determined by probing the maximum scrollback position.
    pub fn scrollback_len(&mut self) -> usize {
        let saved = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max_pos = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(saved);
        max_pos
    }

    /// Read scrollback text.
    ///
    /// * `start` — 0-indexed from the **oldest** scrollback line.
    /// * `count` — maximum number of lines to return.
    ///
    /// Trailing whitespace is trimmed per line.
    pub fn scrollback_contents(&mut self, start: usize, count: usize) -> String {
        let total = self.scrollback_len();
        if total == 0 || count == 0 || start >= total {
            return String::new();
        }

        let saved_pos = self.parser.screen().scrollback();
        let (screen_rows, cols) = self.parser.screen().size();
        let screen_rows_usize = screen_rows as usize;
        let effective_count = count.min(total - start);

        let mut all_lines: Vec<String> = Vec::with_capacity(effective_count);
        let mut read_pos = start;

        while read_pos < start + effective_count {
            // Position viewport so row 0 = scrollback line at `read_pos`.
            //
            // scrollback offset = total - read_pos  →  row 0 is the line at
            // `read_pos` from the oldest.
            let scrollback_offset = total - read_pos;
            self.parser.screen_mut().set_scrollback(scrollback_offset);

            // Only the first `min(scrollback_offset, screen_rows)` rows of
            // the viewport are scrollback lines.
            let scrollback_visible = scrollback_offset.min(screen_rows_usize);
            let needed = start + effective_count - read_pos;
            let to_read = scrollback_visible.min(needed);

            for row_idx in 0..to_read {
                let mut line = String::new();
                for col in 0..cols {
                    if let Some(cell) =
                        self.parser.screen().cell(row_idx as u16, col)
                    {
                        if cell.is_wide_continuation() {
                            continue;
                        }
                        let text = cell.contents();
                        if text.is_empty() {
                            line.push(' ');
                        } else {
                            line.push_str(&text);
                        }
                    }
                }
                all_lines.push(line.trim_end().to_string());
            }

            read_pos += to_read;
            if to_read == 0 {
                break;
            }
        }

        self.parser.screen_mut().set_scrollback(saved_pos);
        all_lines.join("\n")
    }
}

// ── Helpers ────────────────────────────────────────────────────────

fn is_default_attrs(
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
) -> bool {
    fg == vt100::Color::Default
        && bg == vt100::Color::Default
        && !bold
        && !italic
        && !underline
        && !inverse
}

/// Trim trailing whitespace per line and drop trailing blank rows.
fn trim_screen_text(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make(rows: u16, cols: u16) -> VtParser {
        VtParser::new(rows, cols, 0)
    }

    // ── screen_contents ────────────────────────────────────────

    #[test]
    fn screen_contents_plain_text() {
        let mut vt = make(5, 20);
        vt.process(b"Hello, world!");
        assert_eq!(vt.screen_contents(), "Hello, world!");
    }

    #[test]
    fn screen_contents_empty() {
        let vt = make(5, 20);
        assert_eq!(vt.screen_contents(), "");
    }

    #[test]
    fn screen_contents_trims_trailing_whitespace() {
        let mut vt = make(5, 20);
        vt.process(b"abc   ");
        // Trailing spaces on a line should be trimmed
        assert_eq!(vt.screen_contents(), "abc");
    }

    #[test]
    fn screen_contents_multiple_lines() {
        let mut vt = make(10, 20);
        vt.process(b"line1\r\nline2\r\nline3");
        assert_eq!(vt.screen_contents(), "line1\nline2\nline3");
    }

    // ── screen_contents_with_cursor ────────────────────────────

    #[test]
    fn cursor_marker_at_start() {
        let vt = make(5, 20);
        let text = vt.screen_contents_with_cursor();
        assert!(text.starts_with('▏'), "text: {text}");
    }

    #[test]
    fn cursor_marker_after_text() {
        let mut vt = make(5, 20);
        vt.process(b"abc");
        let text = vt.screen_contents_with_cursor();
        assert!(text.contains("abc▏"), "text: {text}");
    }

    #[test]
    fn cursor_marker_after_newline() {
        let mut vt = make(5, 20);
        vt.process(b"abc\r\n");
        let text = vt.screen_contents_with_cursor();
        // Cursor should be on row 1, col 0
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() >= 2);
        assert_eq!(lines[0], "abc");
        assert!(lines[1].starts_with('▏'), "line1: {}", lines[1]);
    }

    #[test]
    fn hidden_cursor_omits_marker() {
        let mut vt = make(5, 20);
        vt.process(b"abc");
        vt.process(b"\x1b[?25l");
        let text = vt.screen_contents_with_cursor();
        assert!(!text.contains('▏'), "text: {text}");
    }

    // ── ANSI sequences and screen state ───────────────────────

    #[test]
    fn process_cursor_movement() {
        let mut vt = make(5, 20);
        // Move cursor to row 2, col 5 (1-based in ANSI: 3,6)
        vt.process(b"\x1b[3;6H");
        assert_eq!(vt.cursor_position(), (2, 5));
    }

    #[test]
    fn process_clear_screen() {
        let mut vt = make(5, 20);
        vt.process(b"some text");
        vt.process(b"\x1b[2J\x1b[H");
        assert_eq!(vt.screen_contents(), "");
        assert_eq!(vt.cursor_position(), (0, 0));
    }

    // ── is_alternate_screen ───────────────────────────────────

    #[test]
    fn alternate_screen_off_by_default() {
        let vt = make(24, 80);
        assert!(!vt.is_alternate_screen());
    }

    #[test]
    fn alternate_screen_enable_disable() {
        let mut vt = make(24, 80);
        // Enable alternate screen (DECSET 1049)
        vt.process(b"\x1b[?1049h");
        assert!(vt.is_alternate_screen());
        // Disable alternate screen (DECRST 1049)
        vt.process(b"\x1b[?1049l");
        assert!(!vt.is_alternate_screen());
    }

    // ── application_cursor ────────────────────────────────────

    #[test]
    fn application_cursor_default_off() {
        let vt = make(24, 80);
        assert!(!vt.application_cursor());
    }

    #[test]
    fn application_cursor_toggle() {
        let mut vt = make(24, 80);
        // DECCKM on
        vt.process(b"\x1b[?1h");
        assert!(vt.application_cursor());
        // DECCKM off
        vt.process(b"\x1b[?1l");
        assert!(!vt.application_cursor());
    }

    // ── bracketed_paste ───────────────────────────────────────

    #[test]
    fn bracketed_paste_toggle() {
        let mut vt = make(24, 80);
        assert!(!vt.bracketed_paste());
        vt.process(b"\x1b[?2004h");
        assert!(vt.bracketed_paste());
        vt.process(b"\x1b[?2004l");
        assert!(!vt.bracketed_paste());
    }

    // ── cursor_visible ────────────────────────────────────────

    #[test]
    fn cursor_visible_toggle() {
        let mut vt = make(24, 80);
        assert!(vt.cursor_visible());
        // Hide cursor (DECTCEM reset)
        vt.process(b"\x1b[?25l");
        assert!(!vt.cursor_visible());
        // Show cursor
        vt.process(b"\x1b[?25h");
        assert!(vt.cursor_visible());
    }

    // ── color_spans ───────────────────────────────────────────

    #[test]
    fn color_spans_empty_for_plain_text() {
        let mut vt = make(5, 20);
        vt.process(b"plain text");
        let spans = vt.color_spans();
        assert!(spans.is_empty(), "plain text should produce no color spans");
    }

    #[test]
    fn color_spans_for_bold() {
        let mut vt = make(5, 40);
        vt.process(b"\x1b[1mBOLD\x1b[0m");
        let spans = vt.color_spans();
        assert!(!spans.is_empty());
        let bold_span = spans.iter().find(|s| s.bold).expect("should have bold span");
        assert_eq!(bold_span.row, 0);
        assert_eq!(bold_span.col, 0);
        assert_eq!(bold_span.len, 4);
    }

    #[test]
    fn color_spans_for_fg_color() {
        let mut vt = make(5, 40);
        // Red foreground
        vt.process(b"\x1b[31mRED\x1b[0m");
        let spans = vt.color_spans();
        assert!(!spans.is_empty());
        let red_span = &spans[0];
        assert_eq!(red_span.len, 3);
        assert!(matches!(red_span.fg, Some(Color::Indexed(1))));
    }

    #[test]
    fn color_spans_for_inverse() {
        let mut vt = make(5, 40);
        vt.process(b"\x1b[7mINVERSE\x1b[0m");
        let spans = vt.color_spans();
        assert!(!spans.is_empty());
        assert!(spans[0].inverse);
    }

    // ── highlights ────────────────────────────────────────────

    #[test]
    fn highlights_returns_only_inverse() {
        let mut vt = make(5, 40);
        // Bold (not inverse) then inverse text
        vt.process(b"\x1b[1mBOLD\x1b[0m \x1b[7mSEL\x1b[0m");
        let hi = vt.highlights();
        assert_eq!(hi.len(), 1);
        assert!(hi[0].inverse);
        assert_eq!(hi[0].len, 3); // "SEL"
    }

    #[test]
    fn highlights_empty_for_no_inverse() {
        let mut vt = make(5, 40);
        vt.process(b"\x1b[1mBOLD\x1b[0m");
        assert!(vt.highlights().is_empty());
    }

    // ── read_region ───────────────────────────────────────────

    #[test]
    fn read_region_basic() {
        let mut vt = make(5, 10);
        vt.process(b"0123456789\r\nabcdefghij\r\nKLMNOPQRST");
        let region = vt.read_region(0, 0, 0, 4);
        assert_eq!(region, "01234");
    }

    #[test]
    fn read_region_sub_rectangle() {
        let mut vt = make(5, 10);
        vt.process(b"0123456789\r\nabcdefghij\r\nKLMNOPQRST");
        let region = vt.read_region(1, 2, 2, 5);
        let lines: Vec<&str> = region.lines().collect();
        assert_eq!(lines[0], "cdef");
        assert_eq!(lines[1], "MNOP");
    }

    #[test]
    fn read_region_single_cell() {
        let mut vt = make(5, 10);
        vt.process(b"ABCDE");
        let region = vt.read_region(0, 2, 0, 2);
        assert_eq!(region, "C");
    }

    // ── cursor_position ───────────────────────────────────────

    #[test]
    fn cursor_position_default() {
        let vt = make(24, 80);
        assert_eq!(vt.cursor_position(), (0, 0));
    }

    #[test]
    fn cursor_position_after_text() {
        let mut vt = make(24, 80);
        vt.process(b"hello");
        assert_eq!(vt.cursor_position(), (0, 5));
    }

    #[test]
    fn cursor_position_after_newlines() {
        let mut vt = make(24, 80);
        vt.process(b"a\r\nb\r\nc");
        assert_eq!(vt.cursor_position(), (2, 1));
    }

    // ── terminal_title ────────────────────────────────────────

    #[test]
    fn terminal_title_none_by_default() {
        let vt = make(24, 80);
        assert_eq!(vt.terminal_title(), None);
    }

    #[test]
    fn terminal_title_set_via_osc() {
        let mut vt = make(24, 80);
        // OSC 2 ; title ST
        vt.process(b"\x1b]2;My Terminal\x1b\\");
        assert_eq!(vt.terminal_title(), Some("My Terminal".to_string()));
    }

    // ── mouse_mode ────────────────────────────────────────────

    #[test]
    fn mouse_mode_default_none() {
        let vt = make(24, 80);
        assert_eq!(vt.mouse_mode(), MouseMode::None);
    }

    // ── screen_cell ───────────────────────────────────────────

    #[test]
    fn screen_cell_returns_content() {
        let mut vt = make(5, 20);
        vt.process(b"X");
        let cell = vt.screen_cell(0, 0).expect("cell should exist");
        assert_eq!(cell.contents, "X");
        assert!(!cell.bold);
        assert!(!cell.inverse);
    }

    #[test]
    fn screen_cell_bold_attribute() {
        let mut vt = make(5, 20);
        vt.process(b"\x1b[1mB\x1b[0m");
        let cell = vt.screen_cell(0, 0).expect("cell should exist");
        assert_eq!(cell.contents, "B");
        assert!(cell.bold);
    }

    #[test]
    fn screen_cell_out_of_bounds() {
        let vt = make(5, 20);
        assert!(vt.screen_cell(100, 100).is_none());
    }

    // ── snapshot / changed_rows ────────────────────────────────

    #[test]
    fn changed_rows_empty_without_snapshot() {
        let vt = make(5, 20);
        assert!(vt.changed_rows().is_empty());
    }

    #[test]
    fn changed_rows_detects_change() {
        let mut vt = make(5, 20);
        vt.process(b"initial");
        vt.take_snapshot();
        vt.process(b"\x1b[2;1Hchanged");
        let changed = vt.changed_rows();
        assert!(changed.contains(&1), "row 1 should be changed");
    }

    #[test]
    fn changed_rows_no_change() {
        let mut vt = make(5, 20);
        vt.process(b"static");
        vt.take_snapshot();
        // No further changes
        let changed = vt.changed_rows();
        assert!(changed.is_empty());
    }

    #[test]
    fn changed_rows_with_content_returns_text() {
        let mut vt = make(5, 20);
        vt.process(b"hello");
        vt.take_snapshot();
        vt.process(b"\x1b[1;1H"); // move to start
        vt.process(b"world");
        let changes = vt.changed_rows_with_content();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, 0); // row 0
        assert_eq!(changes[0].1, "world"); // current (trimmed)
        assert_eq!(changes[0].2, "hello"); // previous (trimmed)
    }

    // ── trim_screen_text helper ───────────────────────────────

    #[test]
    fn trim_screen_text_removes_trailing_blanks() {
        assert_eq!(trim_screen_text("abc  \n\n\n"), "abc");
    }

    #[test]
    fn trim_screen_text_preserves_inner_blanks() {
        assert_eq!(trim_screen_text("a\n\nb"), "a\n\nb");
    }

    #[test]
    fn trim_screen_text_empty() {
        assert_eq!(trim_screen_text(""), "");
    }

    // ── Color conversion ──────────────────────────────────────

    #[test]
    fn color_from_vt100_default() {
        let c: Color = vt100::Color::Default.into();
        assert_eq!(c, Color::Default);
    }

    #[test]
    fn color_from_vt100_indexed() {
        let c: Color = vt100::Color::Idx(5).into();
        assert_eq!(c, Color::Indexed(5));
    }

    #[test]
    fn color_from_vt100_rgb() {
        let c: Color = vt100::Color::Rgb(10, 20, 30).into();
        assert_eq!(c, Color::Rgb(10, 20, 30));
    }

    // ── MouseMode conversion ──────────────────────────────────

    #[test]
    fn mouse_mode_conversions() {
        assert_eq!(MouseMode::from(vt100::MouseProtocolMode::None), MouseMode::None);
        assert_eq!(MouseMode::from(vt100::MouseProtocolMode::Press), MouseMode::Press);
        assert_eq!(MouseMode::from(vt100::MouseProtocolMode::PressRelease), MouseMode::PressRelease);
        assert_eq!(MouseMode::from(vt100::MouseProtocolMode::ButtonMotion), MouseMode::ButtonMotion);
        assert_eq!(MouseMode::from(vt100::MouseProtocolMode::AnyMotion), MouseMode::AnyMotion);
    }
}
