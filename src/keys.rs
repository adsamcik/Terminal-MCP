const MAX_KEY_DEPTH: usize = 3;

/// Maps named key strings to VT escape sequences.
/// e.g., "Up" → "\x1b[A", "Ctrl+C" → "\x03"
///
/// Key names are case-insensitive ("ctrl+c" == "Ctrl+C").
/// When `application_cursor` is true, arrow keys use SS3 (`\x1bO`) instead of CSI (`\x1b[`).
pub fn key_to_bytes(key: &str, application_cursor: bool) -> Option<Vec<u8>> {
    key_to_bytes_inner(key, application_cursor, 0)
}

fn key_to_bytes_inner(key: &str, application_cursor: bool, depth: usize) -> Option<Vec<u8>> {
    if depth > MAX_KEY_DEPTH {
        return None;
    }
    // Normalize to consistent casing for matching
    let normalized = normalize_key(key);
    let k = normalized.as_str();

    match k {
        // -- Basic keys --
        "Enter" => Some(vec![0x0d]),
        "Tab" => Some(vec![0x09]),
        "Escape" => Some(vec![0x1b]),
        "Backspace" => Some(vec![0x7f]),
        "Space" => Some(vec![0x20]),

        // -- Arrow keys (mode-dependent) --
        "Up" => Some(if application_cursor {
            b"\x1bOA".to_vec()
        } else {
            b"\x1b[A".to_vec()
        }),
        "Down" => Some(if application_cursor {
            b"\x1bOB".to_vec()
        } else {
            b"\x1b[B".to_vec()
        }),
        "Right" => Some(if application_cursor {
            b"\x1bOC".to_vec()
        } else {
            b"\x1b[C".to_vec()
        }),
        "Left" => Some(if application_cursor {
            b"\x1bOD".to_vec()
        } else {
            b"\x1b[D".to_vec()
        }),

        // -- Navigation --
        "Home" => Some(b"\x1b[H".to_vec()),
        "End" => Some(b"\x1b[F".to_vec()),
        "PageUp" => Some(b"\x1b[5~".to_vec()),
        "PageDown" => Some(b"\x1b[6~".to_vec()),
        "Insert" => Some(b"\x1b[2~".to_vec()),
        "Delete" => Some(b"\x1b[3~".to_vec()),

        // -- Function keys F1–F12 --
        // F1–F4 use SS3 prefix; F5–F12 use CSI with ~ suffix
        "F1" => Some(b"\x1bOP".to_vec()),
        "F2" => Some(b"\x1bOQ".to_vec()),
        "F3" => Some(b"\x1bOR".to_vec()),
        "F4" => Some(b"\x1bOS".to_vec()),
        "F5" => Some(b"\x1b[15~".to_vec()),
        "F6" => Some(b"\x1b[17~".to_vec()),
        "F7" => Some(b"\x1b[18~".to_vec()),
        "F8" => Some(b"\x1b[19~".to_vec()),
        "F9" => Some(b"\x1b[20~".to_vec()),
        "F10" => Some(b"\x1b[21~".to_vec()),
        "F11" => Some(b"\x1b[23~".to_vec()),
        "F12" => Some(b"\x1b[24~".to_vec()),

        // -- Shift+Tab (backtab) --
        "Shift+Tab" => Some(b"\x1b[Z".to_vec()),

        // -- Ctrl+<key> --
        _ if k.starts_with("Ctrl+") => {
            let rest = &k[5..];
            let ch = rest.chars().next()?;
            match ch.to_ascii_uppercase() {
                c @ 'A'..='Z' => Some(vec![c as u8 - b'A' + 1]),
                _ => None,
            }
        }

        // -- Alt+<key> (sends ESC prefix before the key bytes) --
        _ if k.starts_with("Alt+") => {
            let rest = &k[4..];
            let bytes = key_to_bytes_inner(rest, application_cursor, depth + 1)?;
            let mut result = vec![0x1b];
            result.extend(bytes);
            Some(result)
        }

        // Single character — send as-is (supports "q", "a", "1", etc.)
        _ if k.chars().count() == 1 => {
            let mut buf = [0u8; 4];
            let s = k.chars().next().unwrap().encode_utf8(&mut buf);
            Some(s.as_bytes().to_vec())
        }
        _ => None,
    }
}

/// Normalize a key name to title-case for matching.
/// "ctrl+c" → "Ctrl+c", "ENTER" → "Enter", "shift+tab" → "Shift+Tab", "f1" → "F1"
fn normalize_key(key: &str) -> String {
    normalize_key_inner(key, 0)
}

fn normalize_key_inner(key: &str, depth: usize) -> String {
    if depth > MAX_KEY_DEPTH {
        return key.to_string();
    }
    let trimmed = key.trim();
    // Single character keys — preserve case
    if trimmed.chars().count() == 1 {
        return trimmed.to_string();
    }
    let lower = trimmed.to_lowercase();
    match lower.as_str() {
        "enter" => "Enter".to_string(),
        "tab" => "Tab".to_string(),
        "escape" => "Escape".to_string(),
        "backspace" => "Backspace".to_string(),
        "space" => "Space".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        "insert" => "Insert".to_string(),
        "delete" => "Delete".to_string(),
        "f1" => "F1".to_string(),
        "f2" => "F2".to_string(),
        "f3" => "F3".to_string(),
        "f4" => "F4".to_string(),
        "f5" => "F5".to_string(),
        "f6" => "F6".to_string(),
        "f7" => "F7".to_string(),
        "f8" => "F8".to_string(),
        "f9" => "F9".to_string(),
        "f10" => "F10".to_string(),
        "f11" => "F11".to_string(),
        "f12" => "F12".to_string(),
        _ if lower.starts_with("shift+") => {
            let rest = &key[6..];
            format!("Shift+{}", normalize_key_inner(rest, depth + 1))
        }
        _ if lower.starts_with("ctrl+") => {
            let rest = &key[5..];
            format!("Ctrl+{rest}")
        }
        _ if lower.starts_with("alt+") => {
            let rest = &key[4..];
            format!("Alt+{}", normalize_key_inner(rest, depth + 1))
        }
        _ => key.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_keys() {
        assert_eq!(key_to_bytes("Enter", false), Some(vec![0x0d]));
        assert_eq!(key_to_bytes("Tab", false), Some(vec![0x09]));
        assert_eq!(key_to_bytes("Escape", false), Some(vec![0x1b]));
        assert_eq!(key_to_bytes("Backspace", false), Some(vec![0x7f]));
        assert_eq!(key_to_bytes("Space", false), Some(vec![0x20]));
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(key_to_bytes("enter", false), key_to_bytes("Enter", false));
        assert_eq!(key_to_bytes("CTRL+C", false), key_to_bytes("Ctrl+C", false));
        assert_eq!(key_to_bytes("ctrl+c", false), key_to_bytes("Ctrl+C", false));
        assert_eq!(key_to_bytes("f1", false), key_to_bytes("F1", false));
    }

    #[test]
    fn arrow_keys_normal_mode() {
        assert_eq!(key_to_bytes("Up", false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_to_bytes("Down", false), Some(b"\x1b[B".to_vec()));
        assert_eq!(key_to_bytes("Right", false), Some(b"\x1b[C".to_vec()));
        assert_eq!(key_to_bytes("Left", false), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn arrow_keys_application_mode() {
        assert_eq!(key_to_bytes("Up", true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_to_bytes("Down", true), Some(b"\x1bOB".to_vec()));
        assert_eq!(key_to_bytes("Right", true), Some(b"\x1bOC".to_vec()));
        assert_eq!(key_to_bytes("Left", true), Some(b"\x1bOD".to_vec()));
    }

    #[test]
    fn ctrl_keys() {
        assert_eq!(key_to_bytes("Ctrl+C", false), Some(vec![0x03]));
        assert_eq!(key_to_bytes("Ctrl+A", false), Some(vec![0x01]));
        assert_eq!(key_to_bytes("Ctrl+Z", false), Some(vec![0x1a]));
    }

    #[test]
    fn function_keys() {
        assert_eq!(key_to_bytes("F1", false), Some(b"\x1bOP".to_vec()));
        assert_eq!(key_to_bytes("F4", false), Some(b"\x1bOS".to_vec()));
        assert_eq!(key_to_bytes("F5", false), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key_to_bytes("F12", false), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn alt_key() {
        // Alt+A = ESC + 'A' (but 'A' is a plain char, not in our mapping → None)
        // Alt+Enter = ESC + CR
        let alt_enter = key_to_bytes("Alt+Enter", false);
        assert_eq!(alt_enter, Some(vec![0x1b, 0x0d]));
    }

    #[test]
    fn shift_tab() {
        assert_eq!(key_to_bytes("Shift+Tab", false), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn unknown_key() {
        assert_eq!(key_to_bytes("FooBar", false), None);
    }

    // ── Additional tests ──────────────────────────────────────────

    #[test]
    fn all_function_keys_f1_to_f12() {
        let expected: Vec<(&str, &[u8])> = vec![
            ("F1", b"\x1bOP"),
            ("F2", b"\x1bOQ"),
            ("F3", b"\x1bOR"),
            ("F4", b"\x1bOS"),
            ("F5", b"\x1b[15~"),
            ("F6", b"\x1b[17~"),
            ("F7", b"\x1b[18~"),
            ("F8", b"\x1b[19~"),
            ("F9", b"\x1b[20~"),
            ("F10", b"\x1b[21~"),
            ("F11", b"\x1b[23~"),
            ("F12", b"\x1b[24~"),
        ];
        for (name, bytes) in expected {
            assert_eq!(
                key_to_bytes(name, false),
                Some(bytes.to_vec()),
                "Failed for {name}"
            );
        }
    }

    #[test]
    fn all_ctrl_a_through_z() {
        for (i, ch) in ('A'..='Z').enumerate() {
            let key = format!("Ctrl+{ch}");
            let expected = vec![(i as u8) + 1];
            assert_eq!(
                key_to_bytes(&key, false),
                Some(expected),
                "Failed for {key}"
            );
        }
    }

    #[test]
    fn ctrl_key_case_insensitive_letter() {
        // Ctrl+a should produce same as Ctrl+A
        assert_eq!(key_to_bytes("Ctrl+a", false), Some(vec![0x01]));
        assert_eq!(key_to_bytes("Ctrl+z", false), Some(vec![0x1a]));
    }

    #[test]
    fn ctrl_nonalpha_returns_none() {
        assert_eq!(key_to_bytes("Ctrl+1", false), None);
        assert_eq!(key_to_bytes("Ctrl+!", false), None);
    }

    #[test]
    fn alt_key_combos() {
        // Alt+F = ESC + 'F' → but 'F' is not in mapping (plain char), so None
        // Alt+Enter = ESC + CR
        assert_eq!(key_to_bytes("Alt+Enter", false), Some(vec![0x1b, 0x0d]));
        // Alt+Tab = ESC + Tab byte
        assert_eq!(key_to_bytes("Alt+Tab", false), Some(vec![0x1b, 0x09]));
        // Alt+Escape = ESC + ESC
        assert_eq!(key_to_bytes("Alt+Escape", false), Some(vec![0x1b, 0x1b]));
        // Alt+Up (normal mode) = ESC + CSI A
        assert_eq!(
            key_to_bytes("Alt+Up", false),
            Some(vec![0x1b, 0x1b, b'[', b'A'])
        );
        // Alt+Up (app cursor) = ESC + SS3 A
        assert_eq!(
            key_to_bytes("Alt+Up", true),
            Some(vec![0x1b, 0x1b, b'O', b'A'])
        );
    }

    #[test]
    fn shift_tab_case_insensitive() {
        assert_eq!(key_to_bytes("shift+tab", false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(key_to_bytes("SHIFT+TAB", false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(key_to_bytes("Shift+Tab", false), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn case_insensitive_thorough() {
        // Various casing combos all yield the same result
        let cases = [
            ("enter", "ENTER", "Enter"),
            ("escape", "ESCAPE", "Escape"),
            ("backspace", "BACKSPACE", "Backspace"),
            ("space", "SPACE", "Space"),
            ("tab", "TAB", "Tab"),
            ("up", "UP", "Up"),
            ("pageup", "PAGEUP", "PageUp"),
            ("home", "HOME", "Home"),
        ];
        for (lower, upper, title) in cases {
            let a = key_to_bytes(lower, false);
            let b = key_to_bytes(upper, false);
            let c = key_to_bytes(title, false);
            assert_eq!(a, b, "Mismatch for {lower} vs {upper}");
            assert_eq!(b, c, "Mismatch for {upper} vs {title}");
        }
    }

    #[test]
    fn navigation_keys() {
        assert_eq!(key_to_bytes("Home", false), Some(b"\x1b[H".to_vec()));
        assert_eq!(key_to_bytes("End", false), Some(b"\x1b[F".to_vec()));
        assert_eq!(key_to_bytes("PageUp", false), Some(b"\x1b[5~".to_vec()));
        assert_eq!(key_to_bytes("PageDown", false), Some(b"\x1b[6~".to_vec()));
        assert_eq!(key_to_bytes("Insert", false), Some(b"\x1b[2~".to_vec()));
        assert_eq!(key_to_bytes("Delete", false), Some(b"\x1b[3~".to_vec()));
    }

    #[test]
    fn application_cursor_does_not_affect_non_arrow_keys() {
        // Function keys, navigation, etc. should be the same regardless of mode
        assert_eq!(key_to_bytes("F1", true), key_to_bytes("F1", false));
        assert_eq!(key_to_bytes("Home", true), key_to_bytes("Home", false));
        assert_eq!(key_to_bytes("Enter", true), key_to_bytes("Enter", false));
        assert_eq!(key_to_bytes("Ctrl+C", true), key_to_bytes("Ctrl+C", false));
    }

    #[test]
    fn unknown_keys_return_none() {
        assert_eq!(key_to_bytes("", false), None);
        assert_eq!(key_to_bytes("FooBar", false), None);
        assert_eq!(key_to_bytes("SuperKey", false), None);
        assert_eq!(key_to_bytes("F13", false), None);
        assert_eq!(key_to_bytes("F0", false), None);
    }

    #[test]
    fn normalize_key_preserves_unknown() {
        // Unknown keys pass through as-is
        assert_eq!(normalize_key("xyz"), "xyz");
    }

    // ── Single-character plain key tests ──────────────────────────

    #[test]
    fn single_char_plain_letter() {
        assert_eq!(key_to_bytes("q", false), Some(vec![b'q']));
        assert_eq!(key_to_bytes("Q", false), Some(vec![b'Q']));
    }

    #[test]
    fn single_char_digit() {
        assert_eq!(key_to_bytes("1", false), Some(vec![b'1']));
    }

    #[test]
    fn single_char_special() {
        assert_eq!(key_to_bytes(".", false), Some(vec![b'.']));
        assert_eq!(key_to_bytes("/", false), Some(vec![b'/']));
    }
}
