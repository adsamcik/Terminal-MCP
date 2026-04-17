<!-- context-init:managed -->
<!--
context-init:version: 1.0
context-init:generated: 2026-04-12
context-init:source: .github/context/project-model.json
context-init:scope: github-copilot
-->

# terminal-mcp

Rust MCP stdio server for PTY-backed terminal session management. Keep this file terse; use `.github\context\ARCHITECTURE.md`, `.github\context\PATTERNS.md`, and `.github\context\DEVELOPMENT.md` for deeper reference.

## Project overview

- Entry flow: `src\main.rs` initializes stderr JSON tracing, then hands off to `server::run`.
- Core domains: `src\server.rs`, `src\session\`, `src\terminal\`, `src\tools\`, `src\screenshot.rs`.
- Primary behaviors: session lifecycle, PTY I/O, VT screen parsing, automation, observation, introspection.

## Tech stack

- Rust
- Tokio
- rmcp
- portable-pty
- vt100
- serde / serde_json
- fontdue + tiny-skia
- dashmap

## Commands

| Task | Command |
| --- | --- |
| Run server | `cargo run` |
| Run helper CLI | `cargo run --bin test-cli` |
| Release build | `cargo build --release` |
| Full tests | `cargo test` |
| Focused integration | `cargo test --test integration_test -- --test-threads=1` |
| Focused E2E | `cargo test --test e2e_automation -- --test-threads=1 --nocapture` |
| Format check (CI-gated) | `cargo fmt --all -- --check` |
| Lint (CI-gated) | `cargo clippy --all-targets` |

## Rules to follow

- Keep `src\main.rs` bootstrap-only: logging init plus handoff to `server::run`.
- Keep `src\server.rs` as the MCP router/schema layer; move operational logic into `src\tools\*.rs` or session types.
- Keep session registry and lifecycle state in `src\session\*.rs`; keep PTY + VT mechanics in `src\terminal\*.rs`.
- Preserve the small `mod.rs` façade pattern in `src\session`, `src\terminal`, and `src\tools`; `src\lib.rs` exposes top-level modules instead of re-exporting them.
- When sharing runtime state across tasks, follow the existing `Arc` + mutex pattern used by session and PTY code.
- Match the current test layout: unit tests beside implementation plus black-box integration/E2E tests under `tests\`.
- `main` is protected: do not push directly. Open a PR; CI (`.github\workflows\ci.yml`) must pass. Dependabot opens PRs weekly for `cargo` and `github-actions` ecosystems.
- Update `CHANGELOG.md` under `## [Unreleased]` for any user-visible behavior change; releases are cut by tagging `v*.*.*`, which fires `.github\workflows\release.yml` to publish GitHub Release binaries for Windows + Linux.

## Env and runtime

- `TERMINAL_MCP_LOG` controls stderr JSON tracing; default is `info`.
- If `create_session` omits a command, shell lookup falls back to `COMSPEC` on Windows and `SHELL` on non-Windows.

## Gotchas

- Windows ConPTY requires the initial cursor/DSR handshake injected in `src\terminal\pty_driver.rs`; avoid removing it.
- `SessionInfo.created_at` is now reported as an RFC 3339 timestamp string, matching the README examples.
- `read_output` returns `exit_code = null` when the exact exit code was never observed at EOF.
- Idle cleanup support exists in `SessionManager`, but server startup does not begin cleanup automatically; keep docs explicit that hosts must opt in.
- Shell integration state is reported live as `"detecting"`, `"active"`, `"injected"`, or `"unavailable"`.
- Most integration/E2E tests are Windows-centric, `cmd.exe`-oriented, and use sleep-based settling.
- `send_and_wait` semantics (see `src\tools\automation.rs` and `README.md`): always resets the unread delta before sending input; in `screen`/`both` mode without a pattern it waits for a meaningful visible screen change (longer settle window for slow-start TUIs); in `delta` mode on an interactive shell it prefers prompt return over raw idle; it never completes on echo alone.
- `read_output` / `get_screen` report cursor visibility from the live VT state; `get_screen(include_cursor=true)` suppresses the cursor marker when the cursor is hidden.
- `Cargo.toml` carries `publish = false` — the crate is released as GitHub binaries only, not to crates.io.

## Targeted path rules

- `src\server.rs` → `.github\instructions\server.instructions.md`
- `src\tools\*.rs` → `.github\instructions\tools.instructions.md`
- `src\session\*.rs` → `.github\instructions\session.instructions.md`
- `src\terminal\*.rs` → `.github\instructions\terminal.instructions.md`
- `tests\*.rs` → `.github\instructions\tests.instructions.md`
