use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use terminal_mcp::session::{SessionConfig, SessionManager};
use terminal_mcp::tools::{automation, introspection, observation};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter("terminal_mcp=debug")
        .init();

    let mgr = SessionManager::new();
    let stdin = io::stdin();

    println!("terminal-mcp test CLI");
    println!("Type 'help' for available commands\n");

    loop {
        print!("terminal-mcp> ");
        io::stdout().flush().unwrap();

        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap() == 0 {
            break; // EOF
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.splitn(3, ' ').collect();
        let cmd = parts[0];

        let result = match cmd {
            "create" => handle_create(&mgr, &parts).await,
            "list" => handle_list(&mgr).await,
            "send" => handle_send(&mgr, &parts).await,
            "keys" => handle_keys(&mgr, line).await,
            "read" => handle_read(&mgr, &parts).await,
            "screen" => handle_screen(&mgr, line).await,
            "screenshot" => handle_screenshot(&mgr, line).await,
            "scrollback" => handle_scrollback(&mgr, line).await,
            "wait" => handle_wait(&mgr, line).await,
            "idle" => handle_idle(&mgr, line).await,
            "exec" => handle_exec(&mgr, &parts).await,
            "info" => handle_info(&mgr, &parts).await,
            "search" => handle_search(&mgr, &parts).await,
            "close" => handle_close(&mgr, &parts).await,
            "help" => {
                print_help();
                Ok(())
            }
            "quit" | "exit" => break,
            _ => {
                println!("Unknown command: {cmd}. Type 'help'.");
                Ok(())
            }
        };

        if let Err(e) = result {
            eprintln!("ERROR: {e:#}");
        }
    }
}

fn print_help() {
    println!(
        "\
Commands:
  create [command] [--cwd path] [--env KEY=VAL]
      Create a new terminal session (default: system shell)

  list
      List all active sessions

  send <session_id> <text>
      Send text + Enter to a session

  keys <session_id> <key1> [key2] [key3]...
      Send named keystrokes (e.g. Ctrl+C, Up, Tab)

  read <session_id> [--timeout ms]
      Read new output since last read (ANSI-stripped)

  screen <session_id> [--colors] [--diff]
      Show the visible terminal screen grid

  screenshot <session_id> [--theme dark|light] [--out file.png]
      Capture a PNG screenshot to a file

  scrollback <session_id> [--lines N] [--search pattern]
      Read scrollback buffer or search it

  wait <session_id> <pattern> [--timeout ms]
      Wait for a regex pattern to appear in output

  idle <session_id> [--stable ms] [--timeout ms]
      Wait for the terminal to become idle

  exec <session_id> <command>
      Send command + Enter and wait for idle (send_and_wait shortcut)

  info <session_id>
      Show detailed session info (JSON)

  search <session_id> <pattern>
      Regex search across all session output

  close <session_id>
      Close and destroy a session

  help    Show this help
  quit    Exit the CLI"
    );
}

// ── Helpers ────────────────────────────────────────────────────────────

fn require_session_id<'a>(parts: &'a [&'a str]) -> anyhow::Result<&'a str> {
    parts
        .get(1)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Usage: <command> <session_id> ..."))
}

fn parse_flag(args: &str, flag: &str) -> Option<String> {
    let needle = format!("{flag} ");
    if let Some(pos) = args.find(&needle) {
        let rest = &args[pos + needle.len()..];
        let val = rest.split_whitespace().next().unwrap_or("");
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

fn has_flag(args: &str, flag: &str) -> bool {
    args.split_whitespace().any(|w| w == flag)
}

// ── Command handlers ──────────────────────────────────────────────────

async fn handle_create(mgr: &SessionManager, parts: &[&str]) -> anyhow::Result<()> {
    let rest = if parts.len() >= 2 {
        parts[1..].join(" ")
    } else {
        String::new()
    };

    // Parse optional flags from the rest
    let cwd = parse_flag(&rest, "--cwd");
    let mut env = HashMap::new();
    for token in rest.split_whitespace() {
        if token.starts_with("--") {
            continue;
        }
        if let Some((k, v)) = token.split_once('=') {
            env.insert(k.to_string(), v.to_string());
        }
    }

    // The command is the first non-flag token (if any)
    let command = rest
        .split_whitespace()
        .find(|w| !w.starts_with("--") && !w.contains('='))
        .map(|s| {
            // Check if this was actually a flag value
            if rest.contains(&format!("--cwd {s}")) || rest.contains(&format!("--env {s}")) {
                None
            } else {
                Some(s.to_string())
            }
        })
        .flatten();

    let config = SessionConfig {
        command,
        args: Vec::new(),
        cwd,
        env,
        ..Default::default()
    };

    let info = mgr.create_session_async(config).await?;
    println!("{}", serde_json::to_string_pretty(&info)?);
    Ok(())
}

async fn handle_list(mgr: &SessionManager) -> anyhow::Result<()> {
    let sessions = mgr.list_sessions().await;
    if sessions.is_empty() {
        println!("No active sessions.");
    } else {
        for s in &sessions {
            println!("{}", serde_json::to_string_pretty(s)?);
        }
    }
    Ok(())
}

async fn handle_send(mgr: &SessionManager, parts: &[&str]) -> anyhow::Result<()> {
    let session_id = require_session_id(parts)?;
    let text = parts.get(2).copied().unwrap_or("");
    let session = mgr.get_session(session_id)?;
    // Always press Enter for the send command
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0x0d);
    session.write_bytes(&bytes).await?;
    println!("Sent {} bytes (including Enter)", bytes.len());
    Ok(())
}

async fn handle_keys(mgr: &SessionManager, line: &str) -> anyhow::Result<()> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 3 {
        anyhow::bail!("Usage: keys <session_id> <key1> [key2] ...");
    }
    let session_id = tokens[1];
    let key_names: Vec<String> = tokens[2..].iter().map(|s| s.to_string()).collect();
    let session = mgr.get_session(session_id)?;
    let result =
        terminal_mcp::tools::input::handle_send_keys(&session, &key_names).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn handle_read(mgr: &SessionManager, parts: &[&str]) -> anyhow::Result<()> {
    let session_id = require_session_id(parts)?;
    let rest = parts.get(2).copied().unwrap_or("");
    let timeout = parse_flag(rest, "--timeout")
        .and_then(|v| v.parse::<u64>().ok());
    let session = mgr.get_session(session_id)?;
    let resp = observation::handle_read_output(&session, timeout, None).await?;
    if resp.output.is_empty() {
        println!("(no new output)");
    } else {
        print!("{}", resp.output);
        if !resp.output.ends_with('\n') {
            println!();
        }
    }
    println!("--- bytes_read={} idle={}ms ---", resp.bytes_read, resp.idle_duration_ms);
    Ok(())
}

async fn handle_screen(mgr: &SessionManager, line: &str) -> anyhow::Result<()> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 2 {
        anyhow::bail!("Usage: screen <session_id> [--colors] [--diff]");
    }
    let session_id = tokens[1];
    let include_colors = has_flag(line, "--colors");
    let diff_mode = has_flag(line, "--diff");

    let session = mgr.get_session(session_id)?;
    let resp = session
        .with_vt(|vt| {
            observation::get_screen(vt, true, include_colors, None, diff_mode)
        })
        .await;
    println!("{}", resp.screen);
    println!(
        "--- {}x{} cursor=({},{}) alt={} ---",
        resp.rows, resp.cols, resp.cursor.row, resp.cursor.col, resp.is_alternate_screen
    );
    if let Some(changed) = &resp.changed_rows {
        println!("Changed rows: {changed:?}");
    }
    Ok(())
}

async fn handle_screenshot(mgr: &SessionManager, line: &str) -> anyhow::Result<()> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 2 {
        anyhow::bail!("Usage: screenshot <session_id> [--theme dark|light] [--out file.png]");
    }
    let session_id = tokens[1];
    let theme = parse_flag(line, "--theme").unwrap_or_else(|| "dark".to_string());
    let out_file = parse_flag(line, "--out").unwrap_or_else(|| "screenshot.png".to_string());

    let session = mgr.get_session(session_id)?;
    let png_data = session
        .with_vt(|vt| observation::screenshot(vt, &theme, 14, 1.0))
        .await?;

    std::fs::write(&out_file, &png_data)?;
    println!("Screenshot saved to {out_file} ({} bytes)", png_data.len());
    Ok(())
}

async fn handle_scrollback(mgr: &SessionManager, line: &str) -> anyhow::Result<()> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 2 {
        anyhow::bail!("Usage: scrollback <session_id> [--lines N] [--search pattern]");
    }
    let session_id = tokens[1];
    let lines = parse_flag(line, "--lines").and_then(|v| v.parse::<i64>().ok());
    let search = parse_flag(line, "--search");

    let session = mgr.get_session(session_id)?;
    let result = observation::handle_get_scrollback(
        &session,
        lines,
        search.as_deref(),
        None,
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn handle_wait(mgr: &SessionManager, line: &str) -> anyhow::Result<()> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 3 {
        anyhow::bail!("Usage: wait <session_id> <pattern> [--timeout ms]");
    }
    let session_id = tokens[1];
    let pattern = tokens[2];
    let timeout = parse_flag(line, "--timeout")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30000);

    let session = mgr.get_session(session_id)?;
    let result = automation::handle_wait_for(&session, Some(pattern), None, timeout, false, false).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn handle_idle(mgr: &SessionManager, line: &str) -> anyhow::Result<()> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 2 {
        anyhow::bail!("Usage: idle <session_id> [--stable ms] [--timeout ms]");
    }
    let session_id = tokens[1];
    let stable = parse_flag(line, "--stable")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1000);
    let timeout = parse_flag(line, "--timeout")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30000);

    let session = mgr.get_session(session_id)?;
    let result = automation::handle_wait_for_idle(&session, stable, timeout, false).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn handle_exec(mgr: &SessionManager, parts: &[&str]) -> anyhow::Result<()> {
    let session_id = require_session_id(parts)?;
    let command = parts.get(2).copied().unwrap_or("");
    if command.is_empty() {
        anyhow::bail!("Usage: exec <session_id> <command>");
    }

    let session = mgr.get_session(session_id)?;
    let result = automation::handle_send_and_wait(
        &session,
        command,
        true,  // press_enter
        None,  // no pattern → idle detection
        2000,  // 2s timeout
        "both", // return both delta and screen
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn handle_info(mgr: &SessionManager, parts: &[&str]) -> anyhow::Result<()> {
    let session_id = require_session_id(parts)?;
    let session = mgr.get_session(session_id)?;

    let info = session.info().await;
    let idle_ms = session.idle_duration_ms().await;
    let resp = session
        .with_vt(|vt| {
            introspection::build_session_info(
                &info,
                vt,
                &session.config.args,
                session.config.cwd.as_deref(),
                idle_ms,
                "unavailable",
            )
        })
        .await;
    println!("{}", serde_json::to_string_pretty(&resp)?);
    Ok(())
}

async fn handle_search(mgr: &SessionManager, parts: &[&str]) -> anyhow::Result<()> {
    let session_id = require_session_id(parts)?;
    let pattern = parts
        .get(2)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Usage: search <session_id> <pattern>"))?;

    let session = mgr.get_session(session_id)?;
    let matches = session.search_output(pattern).await?;
    if matches.is_empty() {
        println!("No matches found.");
    } else {
        for m in &matches {
            println!("  line {}: {} [match: {}]", m.line_number, m.line, m.match_text);
        }
        println!("--- {} match(es) ---", matches.len());
    }
    Ok(())
}

async fn handle_close(mgr: &SessionManager, parts: &[&str]) -> anyhow::Result<()> {
    let session_id = require_session_id(parts)?;
    mgr.close_session(session_id).await?;
    println!("Session {session_id} closed.");
    Ok(())
}
