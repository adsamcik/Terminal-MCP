//! Error detection — recognises compiler errors, stack traces, and failure
//! indicators in terminal output.
//!
//! Uses a `RegexSet` for efficient multi-pattern matching and produces
//! [`ErrorMatch`] records with line numbers and severity.

// Retained: full error-detection surface kept for upcoming `detect_errors`
// tool plumbing and downstream library consumers.
#![allow(dead_code)]

use regex::RegexSet;
use serde::Serialize;

// ── Public types ───────────────────────────────────────────────────

/// A single error match found in terminal output.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorMatch {
    /// 0-based line number in the searched text.
    pub line_number: usize,
    /// The full line containing the match.
    pub line: String,
    /// Index of the pattern that matched (maps to [`PATTERN_DEFS`]).
    pub pattern_index: usize,
    /// Human-readable name of the matched pattern.
    pub pattern_name: String,
}

// ── Pattern catalogue ──────────────────────────────────────────────

/// Pattern catalogue: each entry is `(human_label, regex_pattern)`.
const PATTERN_DEFS: &[(&str, &str)] = &[
    // gcc / clang: "file.c:10:5: error: ..."
    ("gcc/clang error", r"(?m)^[^\s:]+:\d+:\d+:\s+error:"),
    // gcc / clang warning
    ("gcc/clang warning", r"(?m)^[^\s:]+:\d+:\d+:\s+warning:"),
    // rustc: "error[E0308]: ..."
    ("rustc error", r"(?m)^error(\[E\d+\])?:"),
    // rustc warning
    ("rustc warning", r"(?m)^warning(\[.+\])?:"),
    // TypeScript: "src/file.ts(10,5): error TS..."
    (
        "typescript error",
        r"(?m)[^\s]+\.tsx?\(\d+,\d+\):\s+error\s+TS\d+:",
    ),
    // .NET / MSBuild: "file.cs(10,5): error CS..."
    (
        "dotnet error",
        r"(?m)[^\s]+\(\d+,\d+\):\s+error\s+[A-Z]+\d+:",
    ),
    // .NET build failed
    ("dotnet build failed", r"(?mi)Build\s+FAILED"),
    // npm ERR!
    ("npm error", r"(?m)^npm\s+ERR!"),
    // Python traceback header
    (
        "python traceback",
        r"(?m)^Traceback \(most recent call last\):",
    ),
    // Python error line (e.g. "ValueError: ...")
    ("python error line", r"(?m)^[A-Za-z]*Error:\s"),
    // Java exception: "Exception in thread ..."
    ("java exception", r"(?m)Exception in thread\b"),
    // Java stack trace frame
    (
        "java stack frame",
        r"(?m)^\s+at\s+[a-zA-Z0-9.$_]+\(.*\.java:\d+\)",
    ),
    // Go error
    ("go error", r"(?m)^\.?/[^\s:]+\.go:\d+:\d+:"),
    // Generic "error:" (case-insensitive)
    ("generic error:", r"(?mi)^\s*error\s*:"),
    // Generic "FAILED"
    ("generic FAILED", r"(?m)\bFAILED\b"),
    // Generic "fatal:" (git, etc.)
    ("generic fatal:", r"(?mi)^\s*fatal\s*:"),
    // Generic "FATAL"
    ("generic FATAL", r"(?m)\bFATAL\b"),
    // Panic (Rust, Go, etc.)
    (
        "generic panic",
        r"(?m)^thread\s+'[^']+'\s+panicked\s+at|^panic:",
    ),
    // Segfault / signal
    (
        "segfault/signal",
        r"(?mi)Segmentation fault|SIGSEGV|SIGABRT|SIGBUS",
    ),
    // Permission denied
    ("permission denied", r"(?mi)Permission denied"),
    // Command not found
    (
        "command not found",
        r"(?mi)command not found|is not recognized",
    ),
    // No such file or directory
    ("no such file", r"(?mi)No such file or directory"),
    // Exit code
    (
        "exit code nonzero",
        r"(?mi)exit(?:ed)?\s+(?:with\s+)?(?:status|code)\s+[1-9]\d*",
    ),
];

// ── ErrorDetector ──────────────────────────────────────────────────

/// Detects errors in terminal output (compiler errors, stack traces, etc.)
pub struct ErrorDetector {
    patterns: RegexSet,
}

impl ErrorDetector {
    /// Build a new detector with the built-in pattern catalogue.
    pub fn new() -> Self {
        let patterns: Vec<&str> = PATTERN_DEFS.iter().map(|(_, p)| *p).collect();
        let set = RegexSet::new(&patterns).expect("error patterns should compile");
        Self { patterns: set }
    }

    /// Scan `text` and return all matched error patterns with line numbers.
    pub fn detect_errors(&self, text: &str) -> Vec<ErrorMatch> {
        let mut matches = Vec::new();
        for (line_number, line) in text.lines().enumerate() {
            let set_matches = self.patterns.matches(line);
            if set_matches.matched_any() {
                for idx in set_matches.iter() {
                    matches.push(ErrorMatch {
                        line_number,
                        line: line.to_string(),
                        pattern_index: idx,
                        pattern_name: PATTERN_DEFS
                            .get(idx)
                            .map(|(name, _)| *name)
                            .unwrap_or("unknown")
                            .to_string(),
                    });
                }
            }
        }
        matches
    }

    /// Whether `text` contains any recognised error pattern.
    pub fn has_errors(&self, text: &str) -> bool {
        for line in text.lines() {
            if self.patterns.is_match(line) {
                return true;
            }
        }
        false
    }

    /// Compute a heuristic error severity score (0–100).
    ///
    /// Factors in the number of matched patterns, unique pattern categories,
    /// and a nonzero exit code.
    pub fn error_score(&self, text: &str, exit_code: Option<i32>) -> u32 {
        let errors = self.detect_errors(text);
        if errors.is_empty() && exit_code.unwrap_or(0) == 0 {
            return 0;
        }

        let mut score: u32 = 0;

        // Each unique pattern match adds weight
        let mut seen_patterns = std::collections::HashSet::new();
        for e in &errors {
            seen_patterns.insert(e.pattern_index);
        }

        // Base score from match count (diminishing returns)
        score += (errors.len() as u32).min(20) * 3;

        // Bonus for pattern diversity
        score += (seen_patterns.len() as u32) * 5;

        // Nonzero exit code adds significant weight
        if let Some(code) = exit_code {
            if code != 0 {
                score += 20;
            }
        }

        score.min(100)
    }
}

impl Default for ErrorDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> ErrorDetector {
        ErrorDetector::new()
    }

    #[test]
    fn detect_gcc_error() {
        let text = "main.c:10:5: error: expected ';' before '}' token";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(!matches.is_empty());
        assert!(matches.iter().any(|m| m.pattern_name == "gcc/clang error"));
    }

    #[test]
    fn detect_rustc_error() {
        let text = "error[E0308]: mismatched types\n  --> src/main.rs:5:10";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "rustc error"));
    }

    #[test]
    fn detect_typescript_error() {
        let text = "src/app.ts(42,10): error TS2345: Argument of type 'string' is not assignable";
        let d = detector();
        assert!(d.has_errors(text));
    }

    #[test]
    fn detect_python_traceback() {
        let text = "Traceback (most recent call last):\n  File \"test.py\", line 1\nValueError: invalid literal";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.len() >= 2);
    }

    #[test]
    fn detect_java_exception() {
        let text = "Exception in thread \"main\" java.lang.NullPointerException\n\tat com.example.App.run(App.java:42)";
        let d = detector();
        assert!(d.has_errors(text));
    }

    #[test]
    fn detect_npm_error() {
        let text = "npm ERR! code ENOENT\nnpm ERR! path /app/package.json";
        let d = detector();
        assert!(d.has_errors(text));
    }

    #[test]
    fn detect_dotnet_error() {
        let text = "Program.cs(15,9): error CS1002: ; expected";
        let d = detector();
        assert!(d.has_errors(text));
    }

    #[test]
    fn detect_generic_failed() {
        let text = "test result: FAILED. 2 passed; 1 failed";
        let d = detector();
        assert!(d.has_errors(text));
    }

    #[test]
    fn no_false_positive_on_clean_output() {
        let text = "Compiling terminal-mcp v0.1.0\n    Finished dev [unoptimized + debuginfo]\n";
        let d = detector();
        assert!(!d.has_errors(text));
    }

    #[test]
    fn error_score_zero_for_clean() {
        let d = detector();
        assert_eq!(d.error_score("All tests passed!", Some(0)), 0);
    }

    #[test]
    fn error_score_high_for_errors() {
        let text = "error[E0308]: mismatched types\nerror: aborting due to previous error";
        let d = detector();
        let score = d.error_score(text, Some(1));
        assert!(score > 30, "score={score}");
    }

    #[test]
    fn error_score_nonzero_exit_alone() {
        let d = detector();
        let score = d.error_score("some output", Some(1));
        assert!(score >= 20);
    }

    #[test]
    fn pattern_defs_non_empty() {
        assert!(
            !PATTERN_DEFS.is_empty(),
            "PATTERN_DEFS must contain at least one entry"
        );
    }

    // ── Additional tests ──────────────────────────────────────

    #[test]
    fn detect_gcc_warning() {
        let text = "main.c:10:5: warning: unused variable 'x'";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(
            matches
                .iter()
                .any(|m| m.pattern_name == "gcc/clang warning")
        );
    }

    #[test]
    fn detect_rustc_warning() {
        let text = "warning[unused_imports]: unused import";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "rustc warning"));
    }

    #[test]
    fn detect_rustc_error_without_code() {
        let text = "error: aborting due to previous error";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "rustc error"));
    }

    #[test]
    fn detect_dotnet_build_failed() {
        let text = "Build FAILED.\n\n0 Warning(s)\n1 Error(s)";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(
            matches
                .iter()
                .any(|m| m.pattern_name == "dotnet build failed")
        );
    }

    #[test]
    fn detect_python_error_line() {
        let text = "ValueError: invalid literal for int()";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(
            matches
                .iter()
                .any(|m| m.pattern_name == "python error line")
        );
    }

    #[test]
    fn detect_java_stack_frame() {
        let text = "\tat com.example.MyClass.method(MyClass.java:42)";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "java stack frame"));
    }

    #[test]
    fn detect_go_error() {
        let text = "./main.go:15:2: undefined: fmt";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "go error"));
    }

    #[test]
    fn detect_generic_fatal() {
        let text = "fatal: not a git repository";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "generic fatal:"));
    }

    #[test]
    fn detect_panic() {
        let text = "thread 'main' panicked at 'index out of bounds'";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "generic panic"));
    }

    #[test]
    fn detect_segfault() {
        let text = "Segmentation fault (core dumped)";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "segfault/signal"));
    }

    #[test]
    fn detect_permission_denied() {
        let text = "bash: /usr/sbin/iptables: Permission denied";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(
            matches
                .iter()
                .any(|m| m.pattern_name == "permission denied")
        );
    }

    #[test]
    fn detect_command_not_found() {
        let text = "zsh: command not found: foobar";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(
            matches
                .iter()
                .any(|m| m.pattern_name == "command not found")
        );
    }

    #[test]
    fn detect_no_such_file() {
        let text = "cat: myfile.txt: No such file or directory";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(matches.iter().any(|m| m.pattern_name == "no such file"));
    }

    #[test]
    fn detect_exit_code() {
        let text = "process exited with code 1";
        let d = detector();
        assert!(d.has_errors(text));
        let matches = d.detect_errors(text);
        assert!(
            matches
                .iter()
                .any(|m| m.pattern_name == "exit code nonzero")
        );
    }

    #[test]
    fn detect_exit_code_status_variant() {
        let text = "exit status 127";
        let d = detector();
        assert!(d.has_errors(text));
    }

    #[test]
    fn no_false_positive_exit_code_zero() {
        let text = "exit code 0";
        let d = detector();
        // "exit code 0" should NOT match (pattern requires [1-9])
        let matches = d.detect_errors(text);
        assert!(
            !matches
                .iter()
                .any(|m| m.pattern_name == "exit code nonzero"),
            "exit code 0 should not match"
        );
    }

    #[test]
    fn error_score_zero_for_no_errors_and_zero_exit() {
        let d = detector();
        assert_eq!(d.error_score("All tests passed!", Some(0)), 0);
        assert_eq!(d.error_score("Build succeeded", None), 0);
    }

    #[test]
    fn error_score_nonzero_exit_code_contributes() {
        let d = detector();
        let score = d.error_score("some output", Some(1));
        assert!(
            score >= 20,
            "nonzero exit should contribute at least 20, got {score}"
        );
    }

    #[test]
    fn error_score_capped_at_100() {
        let d = detector();
        // Lots of errors
        let text = (0..50)
            .map(|i| format!("error: problem {i}\nFATAL: issue {i}\nFAILED test {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let score = d.error_score(&text, Some(1));
        assert!(score <= 100, "score should be capped at 100, got {score}");
    }

    #[test]
    fn error_score_multiple_pattern_diversity() {
        let d = detector();
        let text = "error[E0308]: mismatch\nnpm ERR! failed\nTraceback (most recent call last):";
        let score = d.error_score(text, Some(0));
        assert!(
            score > 20,
            "diverse patterns should produce high score, got {score}"
        );
    }

    #[test]
    fn detect_errors_line_numbers_correct() {
        let d = detector();
        let text = "line 0 ok\nerror: line 1 bad\nline 2 ok\nerror: line 3 bad";
        let matches = d.detect_errors(text);
        let line_nums: Vec<usize> = matches.iter().map(|m| m.line_number).collect();
        assert!(line_nums.contains(&1));
        assert!(line_nums.contains(&3));
    }

    #[test]
    fn has_errors_returns_false_for_clean() {
        let d = detector();
        assert!(!d.has_errors("Compiling crate v1.0\nFinished dev target"));
        assert!(!d.has_errors(""));
    }

    #[test]
    fn default_impl() {
        let d = ErrorDetector::default();
        assert!(!d.has_errors("clean"));
    }
}
