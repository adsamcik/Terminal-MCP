//! Shell integration — 3-layer prompt detection via OSC sequences, regex
//! heuristics, and cursor stability analysis.
//!
//! Supports OSC 133 (FinalTerm / iTerm2) and OSC 633 (VS Code terminal
//! integration) for definite prompt detection. Falls back to regex patterns
//! and cursor position stability when shell integration is not active.

use std::time::{Duration, Instant};

use regex::RegexSet;

// ── Public types ───────────────────────────────────────────────────

/// Phase of the shell's command lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellPhase {
    Unknown,
    PromptActive,
    InputReady,
    Executing,
}

/// Integration availability status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationStatus {
    /// Have not yet determined whether integration is available.
    Detecting,
    /// An external shell integration (e.g. from VS Code) is active.
    ExternalActive,
    /// We injected our own integration script.
    Injected,
    /// No integration available; rely on heuristics.
    Unavailable,
}

/// Result of prompt detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptStatus {
    /// OSC 133 confirmed the shell is at a prompt.
    Definite { exit_code: Option<i32> },
    /// Regex + cursor stability suggest a prompt.
    Probable,
    /// Cannot determine.
    Unknown,
}

/// Known shell types for injection scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellType {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Cmd,
    Unknown,
}

// ── Internal state ─────────────────────────────────────────────────

struct ShellState {
    phase: ShellPhase,
    last_exit_code: Option<i32>,
    cwd: Option<String>,
}

impl ShellState {
    fn new() -> Self {
        Self {
            phase: ShellPhase::Unknown,
            last_exit_code: None,
            cwd: None,
        }
    }
}

// ── ShellIntegration ───────────────────────────────────────────────

/// Tracks shell integration state via OSC sequences, prompt regex patterns,
/// and cursor stability for 3-layer prompt detection.
pub struct ShellIntegration {
    osc_state: ShellState,
    prompt_patterns: RegexSet,
    last_cursor: (u16, u16),
    last_cursor_change: Instant,
    cursor_stable_threshold: Duration,
    integration_status: IntegrationStatus,
}

impl ShellIntegration {
    /// Create a new `ShellIntegration` with default prompt patterns.
    pub fn new() -> Self {
        Self {
            osc_state: ShellState::new(),
            prompt_patterns: build_prompt_patterns(),
            last_cursor: (0, 0),
            last_cursor_change: Instant::now(),
            cursor_stable_threshold: Duration::from_millis(500),
            integration_status: IntegrationStatus::Detecting,
        }
    }

    /// Process an OSC sequence payload (called when the VT parser encounters
    /// an OSC escape).
    ///
    /// Recognised sequences:
    /// - OSC 133;A — prompt start
    /// - OSC 133;B — prompt end / input start
    /// - OSC 133;C — command start (executing)
    /// - OSC 133;D;{exit_code} — command finished
    /// - OSC 633;* — VS Code variants of the above
    /// - OSC 7;file://{host}/{cwd} — current working directory
    pub fn process_osc(&mut self, params: &str) {
        // Split on ';' to get the OSC number and sub-params
        let parts: Vec<&str> = params.splitn(3, ';').collect();
        if parts.is_empty() {
            return;
        }

        match parts[0] {
            "133" | "633" => {
                // Mark integration as externally active
                if self.integration_status == IntegrationStatus::Detecting {
                    self.integration_status = IntegrationStatus::ExternalActive;
                }
                if parts.len() >= 2 {
                    self.process_ftcs(parts[1], parts.get(2).copied());
                }
            }
            "7" => {
                // CWD notification: OSC 7;file://host/path
                if parts.len() >= 2 {
                    let uri = parts[1..].join(";");
                    if let Some(path) = uri.strip_prefix("file://") {
                        // Skip hostname (everything up to the second '/')
                        if let Some(idx) = path.find('/') {
                            self.osc_state.cwd = Some(path[idx..].to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Process FinalTerm Command Sequences (FTCS) sub-commands.
    fn process_ftcs(&mut self, command: &str, arg: Option<&str>) {
        match command {
            "A" => {
                // Prompt start
                self.osc_state.phase = ShellPhase::PromptActive;
            }
            "B" => {
                // Prompt end / input ready
                self.osc_state.phase = ShellPhase::InputReady;
            }
            "C" => {
                // Command execution start
                self.osc_state.phase = ShellPhase::Executing;
            }
            "D" => {
                // Command finished — parse exit code
                let exit_code = arg.and_then(|s| s.trim().parse::<i32>().ok());
                self.osc_state.last_exit_code = exit_code;
                self.osc_state.phase = ShellPhase::PromptActive;
            }
            _ => {}
        }
    }

    /// Check if the shell appears to be at a prompt using 3-layer detection:
    ///
    /// 1. **OSC 133/633** — definite if integration is active.
    /// 2. **Regex heuristic** — match last non-empty screen line against
    ///    common prompt patterns.
    /// 3. **Cursor stability** — cursor hasn't moved for the threshold
    ///    duration, suggesting the shell is waiting for input.
    pub fn is_at_prompt(&mut self, screen: &vt100::Screen) -> PromptStatus {
        // Layer 1: OSC 133 / 633
        if self.integration_status == IntegrationStatus::ExternalActive
            || self.integration_status == IntegrationStatus::Injected
        {
            match self.osc_state.phase {
                ShellPhase::PromptActive | ShellPhase::InputReady => {
                    return PromptStatus::Definite {
                        exit_code: self.osc_state.last_exit_code,
                    };
                }
                ShellPhase::Executing => return PromptStatus::Unknown,
                ShellPhase::Unknown => {} // fall through to heuristics
            }
        }

        // Update cursor tracking
        let cursor_pos = screen.cursor_position();
        if cursor_pos != self.last_cursor {
            self.last_cursor = cursor_pos;
            self.last_cursor_change = Instant::now();
        }

        // Layer 2: Regex heuristic on the last non-empty screen line
        let last_line = self.last_nonempty_line(screen);
        let regex_match = if let Some(ref line) = last_line {
            self.prompt_patterns.is_match(line)
        } else {
            false
        };

        // Layer 3: Cursor stability
        let cursor_stable = self.last_cursor_change.elapsed() >= self.cursor_stable_threshold;

        if regex_match && cursor_stable {
            PromptStatus::Probable
        } else {
            PromptStatus::Unknown
        }
    }

    /// Get a shell integration injection script for the given shell type.
    ///
    /// Returns `None` for unsupported shells.
    pub fn injection_script(shell_type: ShellType) -> Option<String> {
        match shell_type {
            ShellType::Bash => Some(BASH_INTEGRATION.to_string()),
            ShellType::Zsh => Some(ZSH_INTEGRATION.to_string()),
            ShellType::Fish => Some(FISH_INTEGRATION.to_string()),
            ShellType::PowerShell => Some(POWERSHELL_INTEGRATION.to_string()),
            ShellType::Cmd | ShellType::Unknown => None,
        }
    }

    /// Last exit code reported by the shell (via OSC 133;D).
    pub fn last_exit_code(&self) -> Option<i32> {
        self.osc_state.last_exit_code
    }

    /// Current shell phase.
    pub fn phase(&self) -> &ShellPhase {
        &self.osc_state.phase
    }

    /// Current working directory (if reported via OSC 7).
    pub fn cwd(&self) -> Option<&str> {
        self.osc_state.cwd.as_deref()
    }

    /// Current integration status.
    pub fn status(&self) -> IntegrationStatus {
        self.integration_status
    }

    /// Mark integration as unavailable (e.g. after a detection timeout).
    pub fn mark_unavailable(&mut self) {
        if self.integration_status == IntegrationStatus::Detecting {
            self.integration_status = IntegrationStatus::Unavailable;
        }
    }

    /// Extract the last non-empty line from the screen.
    fn last_nonempty_line(&self, screen: &vt100::Screen) -> Option<String> {
        let (rows, cols) = screen.size();
        for row in (0..rows).rev() {
            let mut line = String::new();
            for col in 0..cols {
                if let Some(cell) = screen.cell(row, col) {
                    let text = cell.contents();
                    if text.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&text);
                    }
                }
            }
            let trimmed = line.trim_end().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
        None
    }
}

impl Default for ShellIntegration {
    fn default() -> Self {
        Self::new()
    }
}

// ── Prompt regex patterns ──────────────────────────────────────────

/// Build a `RegexSet` matching common shell prompt patterns.
fn build_prompt_patterns() -> RegexSet {
    RegexSet::new([
        // bash/zsh: "user@host:path$ " or "user@host:path# "
        r"[a-zA-Z0-9._-]+@[a-zA-Z0-9._-]+:[^\$#]*[\$#]\s*$",
        // Simple dollar/hash prompt at end of line
        r"[\$#]\s*$",
        // PowerShell: "PS C:\path> " or "PS /path> "
        r"PS\s+[A-Za-z]?:?[/\\][^>]*>\s*$",
        // cmd.exe: "C:\path>"
        r"^[A-Za-z]:\\[^>]*>\s*$",
        // fish: "user@host ~> " or "> "
        r"[a-zA-Z0-9._-]+@[a-zA-Z0-9._-]+\s+[~][^>]*>\s*$",
        // Generic angle bracket prompt
        r">\s*$",
        // Python virtualenv prefix: "(venv) user@host:path$ "
        r"\([^)]+\)\s+[a-zA-Z0-9._-]+@[a-zA-Z0-9._-]+:[^\$#]*[\$#]\s*$",
        // nix-shell prompt
        r"\[nix-shell[^\]]*\][^\$#]*[\$#]\s*$",
        // Numbered prompts (ipython, gdb, etc.): "In [1]: " or "(gdb) "
        // Python continuation ">>> " or "... " (must be at line start)
        r"(?:In\s*\[\d+\]|^>>>|^\.\.\.|(?:\(gdb\)))\s*:?\s*$",
    ])
    .expect("prompt patterns should compile")
}

// ── Injection scripts ──────────────────────────────────────────────

const BASH_INTEGRATION: &str = r#"
# terminal-mcp shell integration (bash)
__tmcp_prompt_start() { printf '\e]133;A\a'; }
__tmcp_prompt_end()   { printf '\e]133;B\a'; }
__tmcp_preexec()      { printf '\e]133;C\a'; }
__tmcp_precmd() {
    local ec=$?
    printf '\e]133;D;%d\a' "$ec"
    printf '\e]7;file://%s%s\a' "$(hostname)" "$(pwd)"
    __tmcp_prompt_start
}
PROMPT_COMMAND="__tmcp_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
trap '__tmcp_preexec' DEBUG
"#;

const ZSH_INTEGRATION: &str = r#"
# terminal-mcp shell integration (zsh)
__tmcp_prompt_start() { printf '\e]133;A\a' }
__tmcp_prompt_end()   { printf '\e]133;B\a' }
precmd()  {
    local ec=$?
    printf '\e]133;D;%d\a' "$ec"
    printf '\e]7;file://%s%s\a' "$(hostname)" "$(pwd)"
    __tmcp_prompt_start
}
preexec() { printf '\e]133;C\a' }
"#;

const FISH_INTEGRATION: &str = r#"
# terminal-mcp shell integration (fish)
function __tmcp_prompt --on-event fish_prompt
    printf '\e]133;D;%d\a' $status
    printf '\e]7;file://%s%s\a' (hostname) (pwd)
    printf '\e]133;A\a'
end
function __tmcp_preexec --on-event fish_preexec
    printf '\e]133;C\a'
end
"#;

const POWERSHELL_INTEGRATION: &str = r#"
# terminal-mcp shell integration (PowerShell)
function global:prompt {
    $ec = if ($?) { 0 } else { 1 }
    [Console]::Write("`e]133;D;$ec`a")
    [Console]::Write("`e]7;file://$($env:COMPUTERNAME)$((Get-Location).Path)`a")
    [Console]::Write("`e]133;A`a")
    $p = "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
    [Console]::Write("`e]133;B`a")
    return $p
}
"#;

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults() {
        let si = ShellIntegration::new();
        assert_eq!(*si.phase(), ShellPhase::Unknown);
        assert_eq!(si.last_exit_code(), None);
        assert_eq!(si.status(), IntegrationStatus::Detecting);
    }

    #[test]
    fn osc_133_lifecycle() {
        let mut si = ShellIntegration::new();

        // Prompt start
        si.process_osc("133;A");
        assert_eq!(*si.phase(), ShellPhase::PromptActive);
        assert_eq!(si.status(), IntegrationStatus::ExternalActive);

        // Input ready
        si.process_osc("133;B");
        assert_eq!(*si.phase(), ShellPhase::InputReady);

        // Command execution
        si.process_osc("133;C");
        assert_eq!(*si.phase(), ShellPhase::Executing);

        // Command finished with exit code
        si.process_osc("133;D;0");
        assert_eq!(*si.phase(), ShellPhase::PromptActive);
        assert_eq!(si.last_exit_code(), Some(0));
    }

    #[test]
    fn osc_633_vscode_variant() {
        let mut si = ShellIntegration::new();
        si.process_osc("633;A");
        assert_eq!(*si.phase(), ShellPhase::PromptActive);
        assert_eq!(si.status(), IntegrationStatus::ExternalActive);
    }

    #[test]
    fn osc_7_cwd() {
        let mut si = ShellIntegration::new();
        si.process_osc("7;file://myhost/home/user/project");
        assert_eq!(si.cwd(), Some("/home/user/project"));
    }

    #[test]
    fn nonzero_exit_code() {
        let mut si = ShellIntegration::new();
        si.process_osc("133;D;127");
        assert_eq!(si.last_exit_code(), Some(127));
    }

    #[test]
    fn prompt_patterns_match_bash() {
        let patterns = build_prompt_patterns();
        assert!(patterns.is_match("user@host:/home/user$ "));
        assert!(patterns.is_match("root@server:~# "));
    }

    #[test]
    fn prompt_patterns_match_powershell() {
        let patterns = build_prompt_patterns();
        assert!(patterns.is_match("PS C:\\Users\\user> "));
        assert!(patterns.is_match("PS /home/user> "));
    }

    #[test]
    fn prompt_patterns_match_cmd() {
        let patterns = build_prompt_patterns();
        assert!(patterns.is_match("C:\\Users\\user>"));
    }

    #[test]
    fn prompt_patterns_no_false_positive() {
        let patterns = build_prompt_patterns();
        assert!(!patterns.is_match("compiling main.rs..."));
        assert!(!patterns.is_match("Hello, world!"));
    }

    #[test]
    fn injection_scripts_available() {
        assert!(ShellIntegration::injection_script(ShellType::Bash).is_some());
        assert!(ShellIntegration::injection_script(ShellType::Zsh).is_some());
        assert!(ShellIntegration::injection_script(ShellType::Fish).is_some());
        assert!(ShellIntegration::injection_script(ShellType::PowerShell).is_some());
        assert!(ShellIntegration::injection_script(ShellType::Cmd).is_none());
        assert!(ShellIntegration::injection_script(ShellType::Unknown).is_none());
    }

    #[test]
    fn mark_unavailable() {
        let mut si = ShellIntegration::new();
        assert_eq!(si.status(), IntegrationStatus::Detecting);
        si.mark_unavailable();
        assert_eq!(si.status(), IntegrationStatus::Unavailable);
    }

    #[test]
    fn definite_prompt_with_osc() {
        let mut si = ShellIntegration::new();
        si.process_osc("133;A");

        let parser = vt100::Parser::new(24, 80, 0);
        let screen = parser.screen();
        let status = si.is_at_prompt(screen);
        assert_eq!(status, PromptStatus::Definite { exit_code: None });
    }

    // ── Additional tests ──────────────────────────────────────

    #[test]
    fn osc_133_d_with_nonzero_exit() {
        let mut si = ShellIntegration::new();
        si.process_osc("133;D;1");
        assert_eq!(si.last_exit_code(), Some(1));
        assert_eq!(*si.phase(), ShellPhase::PromptActive);
    }

    #[test]
    fn osc_133_d_without_exit_code() {
        let mut si = ShellIntegration::new();
        si.process_osc("133;D");
        // D without an exit code arg
        assert_eq!(si.last_exit_code(), None);
        assert_eq!(*si.phase(), ShellPhase::PromptActive);
    }

    #[test]
    fn osc_133_d_with_negative_exit() {
        let mut si = ShellIntegration::new();
        si.process_osc("133;D;-1");
        assert_eq!(si.last_exit_code(), Some(-1));
    }

    #[test]
    fn osc_133_d_with_invalid_exit_code() {
        let mut si = ShellIntegration::new();
        si.process_osc("133;D;abc");
        assert_eq!(si.last_exit_code(), None);
    }

    #[test]
    fn osc_633_full_lifecycle() {
        let mut si = ShellIntegration::new();
        si.process_osc("633;A");
        assert_eq!(*si.phase(), ShellPhase::PromptActive);
        si.process_osc("633;B");
        assert_eq!(*si.phase(), ShellPhase::InputReady);
        si.process_osc("633;C");
        assert_eq!(*si.phase(), ShellPhase::Executing);
        si.process_osc("633;D;0");
        assert_eq!(*si.phase(), ShellPhase::PromptActive);
        assert_eq!(si.last_exit_code(), Some(0));
    }

    #[test]
    fn osc_7_cwd_no_path() {
        let mut si = ShellIntegration::new();
        // No file:// prefix
        si.process_osc("7;not_a_url");
        assert_eq!(si.cwd(), None);
    }

    #[test]
    fn osc_7_cwd_windows_path() {
        let mut si = ShellIntegration::new();
        si.process_osc("7;file://DESKTOP/C:/Users/test");
        assert_eq!(si.cwd(), Some("/C:/Users/test"));
    }

    #[test]
    fn osc_unknown_number_ignored() {
        let mut si = ShellIntegration::new();
        si.process_osc("999;some data");
        assert_eq!(*si.phase(), ShellPhase::Unknown);
        assert_eq!(si.status(), IntegrationStatus::Detecting);
    }

    #[test]
    fn osc_empty_string() {
        let mut si = ShellIntegration::new();
        si.process_osc("");
        assert_eq!(*si.phase(), ShellPhase::Unknown);
    }

    #[test]
    fn mark_unavailable_only_from_detecting() {
        let mut si = ShellIntegration::new();
        // First mark as external active
        si.process_osc("133;A");
        assert_eq!(si.status(), IntegrationStatus::ExternalActive);
        // mark_unavailable should not downgrade from ExternalActive
        si.mark_unavailable();
        assert_eq!(si.status(), IntegrationStatus::ExternalActive);
    }

    #[test]
    fn executing_phase_returns_unknown_prompt() {
        let mut si = ShellIntegration::new();
        si.process_osc("133;C"); // Executing
        let parser = vt100::Parser::new(24, 80, 0);
        let screen = parser.screen();
        let status = si.is_at_prompt(screen);
        assert_eq!(status, PromptStatus::Unknown);
    }

    #[test]
    fn input_ready_returns_definite() {
        let mut si = ShellIntegration::new();
        si.process_osc("133;B"); // InputReady
        let parser = vt100::Parser::new(24, 80, 0);
        let screen = parser.screen();
        let status = si.is_at_prompt(screen);
        assert_eq!(status, PromptStatus::Definite { exit_code: None });
    }

    #[test]
    fn prompt_patterns_match_fish() {
        let patterns = build_prompt_patterns();
        assert!(patterns.is_match("user@host ~> "));
    }

    #[test]
    fn prompt_patterns_match_generic_angle_bracket() {
        let patterns = build_prompt_patterns();
        assert!(patterns.is_match("> "));
    }

    #[test]
    fn prompt_patterns_match_virtualenv() {
        let patterns = build_prompt_patterns();
        assert!(patterns.is_match("(venv) user@host:/home$ "));
    }

    #[test]
    fn prompt_patterns_match_nix_shell() {
        let patterns = build_prompt_patterns();
        assert!(patterns.is_match("[nix-shell:~/project]$ "));
    }

    #[test]
    fn prompt_patterns_match_python_repl() {
        let patterns = build_prompt_patterns();
        assert!(patterns.is_match(">>> "));
    }

    #[test]
    fn injection_script_bash_contains_osc_133() {
        let script = ShellIntegration::injection_script(ShellType::Bash).unwrap();
        assert!(script.contains("133;A"));
        assert!(script.contains("133;B"));
        assert!(script.contains("133;C"));
        assert!(script.contains("133;D"));
    }

    #[test]
    fn injection_script_zsh_contains_osc_133() {
        let script = ShellIntegration::injection_script(ShellType::Zsh).unwrap();
        assert!(script.contains("133;A"));
        assert!(script.contains("133;D"));
    }

    #[test]
    fn injection_script_powershell_contains_osc_133() {
        let script = ShellIntegration::injection_script(ShellType::PowerShell).unwrap();
        assert!(script.contains("133;A"));
        assert!(script.contains("133;D"));
        assert!(script.contains("133;B"));
    }

    #[test]
    fn injection_script_fish_contains_osc() {
        let script = ShellIntegration::injection_script(ShellType::Fish).unwrap();
        assert!(script.contains("133;D"));
        assert!(script.contains("133;A"));
        assert!(script.contains("133;C"));
    }

    #[test]
    fn default_impl_matches_new() {
        let si = ShellIntegration::default();
        assert_eq!(*si.phase(), ShellPhase::Unknown);
        assert_eq!(si.status(), IntegrationStatus::Detecting);
    }
}
