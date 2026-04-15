//! In-memory scrollback buffer with regex search.
//!
//! Stores terminal output lines in a capped ring buffer and provides
//! tail, range, and regex search access.

use std::collections::VecDeque;
use std::time::Instant;

use anyhow::{Context, Result};
use regex::RegexBuilder;
use serde::Serialize;

const MAX_PENDING_BYTES: usize = 1_048_576; // 1 MB
const MAX_LINE_BYTES: usize = 1_048_576; // 1 MB

/// A single line stored in the scrollback buffer.
pub struct ScrollbackLine {
    pub text: String,
    pub timestamp: Instant,
}

/// A regex search hit with surrounding context lines.
#[derive(Debug, Clone, Serialize)]
pub struct SearchMatch {
    pub line_number: usize,
    pub text: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// In-memory ring buffer of terminal output lines.
pub struct ScrollbackBuffer {
    lines: VecDeque<ScrollbackLine>,
    max_lines: usize,
    /// Partial line not yet terminated by a newline.
    pending: String,
}

impl ScrollbackBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(max_lines.min(4096)),
            max_lines,
            pending: String::new(),
        }
    }

    /// Append raw output text, splitting by newlines.
    ///
    /// Incomplete trailing lines are buffered until the next newline arrives.
    pub fn append(&mut self, text: &str) {
        let now = Instant::now();
        let combined = if self.pending.is_empty() {
            text.to_string()
        } else {
            let mut s = std::mem::take(&mut self.pending);
            s.push_str(text);
            s
        };

        let mut parts = combined.split('\n').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                // Complete line (followed by another segment)
                self.push_line(part.to_string(), now);
            } else {
                // Last segment — may be incomplete
                self.pending = part.to_string();
            }
        }

        // Cap pending buffer to prevent unbounded growth
        if self.pending.len() > MAX_PENDING_BYTES {
            let flushed = std::mem::take(&mut self.pending);
            self.push_line(flushed, Instant::now());
        }
    }

    /// Get the last `n` lines (tail mode).
    pub fn tail(&self, n: usize) -> Vec<&str> {
        let start = self.lines.len().saturating_sub(n);
        self.lines
            .iter()
            .skip(start)
            .map(|l| l.text.as_str())
            .collect()
    }

    /// Get lines in range `[start, start+count)`.
    pub fn range(&self, start: usize, count: usize) -> Vec<&str> {
        self.lines
            .iter()
            .skip(start)
            .take(count)
            .map(|l| l.text.as_str())
            .collect()
    }

    /// Search for a regex pattern, returning matching lines with context.
    pub fn search(
        &self,
        pattern: &str,
        context_lines: usize,
    ) -> Result<Vec<SearchMatch>> {
        let re = RegexBuilder::new(pattern)
            .size_limit(1_000_000)
            .build()
            .context("Invalid regex pattern")?;
        let total = self.lines.len();
        let mut matches = Vec::new();

        for (i, line) in self.lines.iter().enumerate() {
            if re.is_match(&line.text) {
                let ctx_start = i.saturating_sub(context_lines);
                let ctx_end = (i + context_lines + 1).min(total);

                let context_before: Vec<String> = (ctx_start..i)
                    .map(|j| self.lines[j].text.clone())
                    .collect();
                let context_after: Vec<String> = ((i + 1)..ctx_end)
                    .map(|j| self.lines[j].text.clone())
                    .collect();

                matches.push(SearchMatch {
                    line_number: i,
                    text: line.text.clone(),
                    context_before,
                    context_after,
                });
            }
        }

        Ok(matches)
    }

    /// Total number of stored lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    // -- internal -----------------------------------------------------------

    fn push_line(&mut self, mut text: String, timestamp: Instant) {
        if text.len() > MAX_LINE_BYTES {
            let mut end = MAX_LINE_BYTES;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        if self.lines.len() >= self.max_lines {
            self.lines.pop_front();
        }
        self.lines.push_back(ScrollbackLine { text, timestamp });
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_splits_newlines() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("line1\nline2\nline3\n");
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.tail(3), vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn append_handles_partial_lines() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("hello ");
        assert_eq!(buf.len(), 0);
        buf.append("world\n");
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.tail(1), vec!["hello world"]);
    }

    #[test]
    fn tail_returns_last_n() {
        let mut buf = ScrollbackBuffer::new(100);
        for i in 0..10 {
            buf.append(&format!("line {i}\n"));
        }
        let last3 = buf.tail(3);
        assert_eq!(last3, vec!["line 7", "line 8", "line 9"]);
    }

    #[test]
    fn range_reads_slice() {
        let mut buf = ScrollbackBuffer::new(100);
        for i in 0..5 {
            buf.append(&format!("L{i}\n"));
        }
        assert_eq!(buf.range(1, 2), vec!["L1", "L2"]);
    }

    #[test]
    fn max_lines_evicts_oldest() {
        let mut buf = ScrollbackBuffer::new(3);
        for i in 0..5 {
            buf.append(&format!("{i}\n"));
        }
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.tail(3), vec!["2", "3", "4"]);
    }

    #[test]
    fn search_finds_matches_with_context() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("alpha\nbeta\ngamma\ndelta\nepsilon\n");
        let results = buf.search("gamma", 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_number, 2);
        assert_eq!(results[0].context_before, vec!["beta"]);
        assert_eq!(results[0].context_after, vec!["delta"]);
    }

    #[test]
    fn search_regex_works() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("error: file not found\nwarning: deprecated\nerror: timeout\n");
        let results = buf.search("^error:", 0).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_invalid_regex_returns_error() {
        let buf = ScrollbackBuffer::new(100);
        assert!(buf.search("[invalid", 0).is_err());
    }

    // ── Additional tests ──────────────────────────────────────

    #[test]
    fn empty_buffer() {
        let buf = ScrollbackBuffer::new(100);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert!(buf.tail(10).is_empty());
        assert!(buf.range(0, 10).is_empty());
    }

    #[test]
    fn single_line() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("only line\n");
        assert_eq!(buf.len(), 1);
        assert!(!buf.is_empty());
        assert_eq!(buf.tail(1), vec!["only line"]);
        assert_eq!(buf.tail(5), vec!["only line"]);
    }

    #[test]
    fn tail_more_than_available() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("a\nb\n");
        assert_eq!(buf.tail(100), vec!["a", "b"]);
    }

    #[test]
    fn range_beyond_end() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("x\ny\n");
        assert_eq!(buf.range(1, 100), vec!["y"]);
    }

    #[test]
    fn range_start_beyond_len() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("x\n");
        assert!(buf.range(10, 5).is_empty());
    }

    #[test]
    fn max_capacity_ring_buffer_eviction() {
        let mut buf = ScrollbackBuffer::new(5);
        for i in 0..20 {
            buf.append(&format!("line{i}\n"));
        }
        assert_eq!(buf.len(), 5);
        // Should contain the last 5 lines
        assert_eq!(buf.tail(5), vec!["line15", "line16", "line17", "line18", "line19"]);
    }

    #[test]
    fn max_capacity_exact() {
        let mut buf = ScrollbackBuffer::new(3);
        buf.append("a\nb\nc\n");
        assert_eq!(buf.len(), 3);
        buf.append("d\n");
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.tail(3), vec!["b", "c", "d"]);
    }

    #[test]
    fn search_no_matches() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("hello\nworld\n");
        let results = buf.search("nonexistent", 0).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_multiple_matches() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("error: one\nok\nerror: two\nok\nerror: three\n");
        let results = buf.search("error:", 0).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].line_number, 0);
        assert_eq!(results[1].line_number, 2);
        assert_eq!(results[2].line_number, 4);
    }

    #[test]
    fn search_context_at_boundaries() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("first\nsecond\nthird\n");
        // Search for "first" with context 2 - context_before should be empty
        let results = buf.search("first", 2).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].context_before.is_empty());
        assert_eq!(results[0].context_after, vec!["second", "third"]);

        // Search for "third" with context 2 - context_after should be empty
        let results = buf.search("third", 2).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].context_before, vec!["first", "second"]);
        assert!(results[0].context_after.is_empty());
    }

    #[test]
    fn append_incremental_partial() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("hel");
        buf.append("lo ");
        buf.append("wor");
        assert_eq!(buf.len(), 0); // no newline yet
        buf.append("ld\n");
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.tail(1), vec!["hello world"]);
    }

    #[test]
    fn append_empty_string() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("");
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn append_only_newlines() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.append("\n\n\n");
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.tail(3), vec!["", "", ""]);
    }

    #[test]
    fn range_basic() {
        let mut buf = ScrollbackBuffer::new(100);
        for i in 0..10 {
            buf.append(&format!("L{i}\n"));
        }
        assert_eq!(buf.range(0, 3), vec!["L0", "L1", "L2"]);
        assert_eq!(buf.range(7, 3), vec!["L7", "L8", "L9"]);
        assert_eq!(buf.range(3, 2), vec!["L3", "L4"]);
    }
}
