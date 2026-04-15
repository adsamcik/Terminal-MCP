use std::sync::LazyLock;

use regex::Regex;

/// Lazily compiled regex covering all common ANSI escape sequences.
static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\x1b\[[0-9;?]*[ -/]*[@-~]", // CSI sequences
        r"|\x1b\].*?(?:\x1b\\|\x07)",  // OSC sequences (terminated by ST or BEL)
        r"|\x1b[()][A-Z0-9]",          // charset selection (e.g. ESC ( B)
        r"|\x1b[A-Z@-_]",             // two-byte sequences (e.g. ESC M)
    ))
    .expect("ANSI regex is valid")
});

/// Strip ANSI escape sequences from raw terminal output.
pub(crate) fn strip_ansi(input: &str) -> String {
    ANSI_RE.replace_all(input, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        let input = "\x1b[31mred text\x1b[0m";
        assert_eq!(strip_ansi(input), "red text");
    }

    #[test]
    fn strip_ansi_removes_osc_sequences() {
        let input = "\x1b]2;Window Title\x07normal text";
        assert_eq!(strip_ansi(input), "normal text");
    }

    #[test]
    fn strip_ansi_removes_osc_with_st_terminator() {
        let input = "\x1b]0;title\x1b\\text here";
        assert_eq!(strip_ansi(input), "text here");
    }

    #[test]
    fn strip_ansi_preserves_plain_text() {
        let input = "Hello, world! No escapes here.";
        assert_eq!(strip_ansi(input), input);
    }

    #[test]
    fn strip_ansi_removes_charset_selection() {
        let input = "\x1b(Bnormal";
        assert_eq!(strip_ansi(input), "normal");
    }

    #[test]
    fn strip_ansi_removes_two_byte_sequences() {
        let input = "\x1bMtext";
        assert_eq!(strip_ansi(input), "text");
    }

    #[test]
    fn strip_ansi_complex_mixed() {
        let input = "\x1b[1;32mgreen bold\x1b[0m \x1b]2;title\x07\x1b[31mred\x1b[0m end";
        assert_eq!(strip_ansi(input), "green bold red end");
    }

    #[test]
    fn strip_ansi_empty_input() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_ansi_cursor_movement() {
        let input = "\x1b[2J\x1b[Hclear screen";
        assert_eq!(strip_ansi(input), "clear screen");
    }

    #[test]
    fn strip_ansi_removes_csi() {
        let input = "\x1b[1;31mERROR\x1b[0m: something failed";
        assert_eq!(strip_ansi(input), "ERROR: something failed");
    }
}
