//! WSL (Windows Subsystem for Linux) detection and session helpers.
//!
//! WSL sessions are regular ConPTY sessions that spawn `wsl.exe` with
//! the appropriate arguments. This module provides detection utilities
//! and a helper to build a [`PtyConfig`] targeting a WSL distribution.

use std::collections::HashMap;

use crate::terminal::PtyConfig;

/// Check if WSL is available on this Windows system.
pub fn is_wsl_available() -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("wsl")
            .arg("--status")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// List available WSL distributions.
///
/// Runs `wsl --list --quiet` and returns the distribution names.
/// Returns an empty `Vec` on non-Windows platforms or when WSL is not installed.
pub fn list_wsl_distributions() -> Vec<String> {
    #[cfg(windows)]
    {
        let output = match std::process::Command::new("wsl")
            .args(["--list", "--quiet"])
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };

        // `wsl --list` outputs UTF-16LE on Windows. Try UTF-16 first, then UTF-8.
        let text = decode_wsl_output(&output.stdout);

        text.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Decode WSL command output which may be UTF-16LE (Windows) or UTF-8.
#[cfg(windows)]
fn decode_wsl_output(bytes: &[u8]) -> String {
    // UTF-16LE BOM: 0xFF 0xFE, or just pairs of bytes with frequent nulls
    if bytes.len() >= 2 && bytes.len() % 2 == 0 {
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();

        // Strip BOM if present
        let start = if u16s.first() == Some(&0xFEFF) { 1 } else { 0 };

        if let Ok(s) = String::from_utf16(&u16s[start..]) {
            return s;
        }
    }

    String::from_utf8_lossy(bytes).into_owned()
}

/// Create a [`PtyConfig`] for launching a shell inside WSL.
///
/// # Arguments
/// * `distro` – Target distribution name (e.g. `"Ubuntu"`). Uses the default
///   distribution when `None`.
/// * `command` – Command to run inside WSL. Launches the default login shell
///   when `None`.
/// * `cwd` – Working directory inside the WSL filesystem (Linux path such as
///   `"/home/user"`). WSL handles this via `--cd`.
pub fn wsl_config(
    distro: Option<&str>,
    command: Option<&str>,
    cwd: Option<&str>,
) -> PtyConfig {
    let mut args = Vec::new();

    if let Some(d) = distro {
        args.extend_from_slice(&["-d".to_string(), d.to_string()]);
    }

    if let Some(dir) = cwd {
        args.extend_from_slice(&["--cd".to_string(), dir.to_string()]);
    }

    if let Some(cmd) = command {
        args.push("--".to_string());
        args.push(cmd.to_string());
    }

    PtyConfig {
        command: "wsl".to_string(),
        args,
        cwd: None, // WSL handles its own cwd via --cd
        env: HashMap::new(),
        rows: 24,
        cols: 80,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_config_default() {
        let cfg = wsl_config(None, None, None);
        assert_eq!(cfg.command, "wsl");
        assert!(cfg.args.is_empty());
        assert!(cfg.cwd.is_none());
    }

    #[test]
    fn wsl_config_with_distro() {
        let cfg = wsl_config(Some("Ubuntu"), None, None);
        assert_eq!(cfg.args, vec!["-d", "Ubuntu"]);
    }

    #[test]
    fn wsl_config_with_all_options() {
        let cfg = wsl_config(Some("Debian"), Some("bash"), Some("/home/user"));
        assert_eq!(
            cfg.args,
            vec!["-d", "Debian", "--cd", "/home/user", "--", "bash"]
        );
        assert!(cfg.cwd.is_none());
    }

    #[test]
    fn wsl_config_command_without_distro() {
        let cfg = wsl_config(None, Some("zsh"), None);
        assert_eq!(cfg.args, vec!["--", "zsh"]);
    }

    #[test]
    fn wsl_config_cwd_without_distro() {
        let cfg = wsl_config(None, None, Some("/tmp"));
        assert_eq!(cfg.args, vec!["--cd", "/tmp"]);
    }

    // ── Additional tests ──────────────────────────────────────

    #[test]
    fn wsl_config_default_dimensions() {
        let cfg = wsl_config(None, None, None);
        assert_eq!(cfg.rows, 24);
        assert_eq!(cfg.cols, 80);
    }

    #[test]
    fn wsl_config_env_empty() {
        let cfg = wsl_config(Some("Ubuntu"), Some("bash"), Some("/home"));
        assert!(cfg.env.is_empty());
    }

    #[test]
    fn wsl_config_cwd_is_none() {
        // WSL handles cwd internally via --cd, so config.cwd is always None
        let cfg = wsl_config(Some("Arch"), Some("zsh"), Some("/root"));
        assert!(cfg.cwd.is_none());
    }

    #[test]
    fn wsl_config_arg_ordering() {
        // Should be: -d <distro>, --cd <dir>, -- <command>
        let cfg = wsl_config(Some("Alpine"), Some("sh"), Some("/opt"));
        assert_eq!(cfg.args[0], "-d");
        assert_eq!(cfg.args[1], "Alpine");
        assert_eq!(cfg.args[2], "--cd");
        assert_eq!(cfg.args[3], "/opt");
        assert_eq!(cfg.args[4], "--");
        assert_eq!(cfg.args[5], "sh");
    }

    #[test]
    fn wsl_config_distro_only() {
        let cfg = wsl_config(Some("Fedora"), None, None);
        assert_eq!(cfg.args, vec!["-d", "Fedora"]);
    }

    #[test]
    fn wsl_config_cwd_and_command_no_distro() {
        let cfg = wsl_config(None, Some("fish"), Some("/home/user"));
        assert_eq!(cfg.args, vec!["--cd", "/home/user", "--", "fish"]);
    }
}
