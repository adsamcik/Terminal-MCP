<!--
context-init:version: 3.1.0
context-init:generated: 2026-04-12T00:00:00Z
context-init:file: DEVELOPMENT
-->
<!-- context-init:managed -->

# terminal-mcp Development

## Prerequisites

- Rust toolchain with Cargo (`Cargo.toml:1-34`).
- A PTY-capable shell on the target OS. If `command` is omitted, default shell lookup falls back to `COMSPEC` / `cmd.exe` on Windows and `SHELL` / `/bin/sh` elsewhere (`src\terminal\pty_driver.rs:95-100`, `src\terminal\pty_driver.rs:293-302`).
- Embedded screenshot fonts are compile-time inputs because `src\screenshot.rs` pulls `assets\Cousine-*.ttf` in with `include_bytes!` (`src\screenshot.rs:10-12`).

## Common commands

| Task | Command | Source |
| --- | --- | --- |
| Debug run the server | `cargo run` | Binary entry at `src\main.rs:16-32` |
| Run the helper CLI | `cargo run --bin test-cli` | `src\bin\test_cli.rs:7-66` |
| Release build | `cargo build --release` | `README.md:23-29` |
| Full test suite | `cargo test` | `README.md:506-510`, `tests\*.rs` |
| Integration test | `cargo test --test integration_test -- --test-threads=1` | `tests\integration_test.rs:1-2` |
| Automation E2E | `cargo test --test e2e_automation -- --test-threads=1 --nocapture` | `tests\e2e_automation.rs:1-2` |
| Lifecycle E2E | `cargo test --test e2e_lifecycle -- --test-threads=1 --nocapture` | `tests\e2e_lifecycle.rs:1-2` |
| Observation E2E | `cargo test --test e2e_observation -- --test-threads=1 --nocapture` | `tests\e2e_observation.rs:1-4` |
| Input E2E | `cargo test --test e2e_input -- --test-threads=1 --nocapture` | `tests\e2e_input.rs:1-2` |
| Edge-case E2E | `cargo test --test e2e_edge_cases -- --test-threads=1 --nocapture` | `tests\e2e_edge_cases.rs:1-2` |

No dedicated lint or typecheck commands are defined in `Cargo.toml:1-34`.

## Environment variables

| Variable | Required | Purpose | Default | Source |
| --- | --- | --- | --- | --- |
| `TERMINAL_MCP_LOG` | No | Controls tracing filter for stderr JSON logs. | `info` | `README.md:47-52`, `src\main.rs:18-20` |
| `COMSPEC` | No | Default shell when `create_session` omits `command` on Windows. | `cmd.exe` fallback | `src\terminal\pty_driver.rs:295-298` |
| `SHELL` | No | Default shell when `create_session` omits `command` on non-Windows platforms. | `/bin/sh` fallback | `src\terminal\pty_driver.rs:299-302` |

## Troubleshooting

| Symptom | Check |
| --- | --- |
| New Windows sessions start but never emit output. | Confirm the ConPTY handshake path in `src\terminal\pty_driver.rs:133-143` is still intact; the code explicitly writes a cursor/device-response sequence before normal output begins. |
| `get_session_info` says shell integration is unavailable. | This is current behavior, not necessarily a broken environment: `src\server.rs:511-520` hardcodes `"unavailable"` even though `src\shell_integration.rs` exists. |
| `read_output` reports `exit_code = 0` for an exited process. | That value is synthetic; `src\tools\observation.rs:304-310` documents that exact exit codes are not available from that path. |
| Idle sessions are not auto-cleaned. | Cleanup logic exists in `src\session\manager.rs:112-147`, but bootstrap only shows server construction and stdio serving in `src\server.rs:600-609`; verify lifecycle wiring before relying on README feature text. |
| E2E tests are flaky on non-Windows platforms. | Current black-box tests are written around `cmd.exe` plus `sleep`-based settling (`tests\integration_test.rs:8-18`, `tests\e2e_input.rs:16-27`, `tests\e2e_observation.rs:14-35`). |
