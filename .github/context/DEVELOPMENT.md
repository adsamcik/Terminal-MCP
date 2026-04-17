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
| Format check | `cargo fmt --all -- --check` | `.github\workflows\ci.yml` (fmt job) |
| Lint | `cargo clippy --all-targets` | `.github\workflows\ci.yml` (clippy job, Linux + Windows) |

Formatting and linting are not declared in `Cargo.toml` but are gated by CI (`.github\workflows\ci.yml`). The CI pipeline does **not** pass `-D warnings`, so intentionally-retained API surface does not break the build; new clippy warnings in your diff should still be addressed.

## Continuous integration and releases

| Workflow | File | Trigger | What it does |
| --- | --- | --- | --- |
| CI | `.github\workflows\ci.yml` | `push`/`pull_request` to `main`, manual dispatch | `rustfmt` check (Linux), `clippy` + `cargo build` (Linux + Windows), full `cargo test --all-targets -- --test-threads=1` (Windows only, since PTY/ConPTY integration tests are Windows-centric). |
| Release | `.github\workflows\release.yml` | Tag push matching `v*.*.*`, manual dispatch with tag | Builds `--release --locked` binaries for `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-gnu`, packages them with `README.md`, `CHANGELOG.md`, and both LICENSE files as `.zip` / `.tar.gz`, emits SHA-256 checksums, extracts the matching `## [X.Y.Z]` section from `CHANGELOG.md` as release notes, and publishes a GitHub Release (pre-release when the tag contains `-`). |
| Dependabot | `.github\dependabot.yml` | Weekly | Opens PRs for `cargo` and `github-actions` ecosystems (max 5 open PRs each). |

`main` is a protected branch: all changes go through PRs. To cut a release, update `## [Unreleased]` in `CHANGELOG.md` under a new `## [X.Y.Z]` heading (land via PR), then tag and push:

```bash
git tag v0.2.0 -m "v0.2.0"
git push origin v0.2.0
```

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
| `get_session_info` says shell integration is unavailable. | That is now a live status, not a hardcoded placeholder. Check whether the shell emitted OSC 133/633 markers or whether injected shell integration is expected in that environment (`src\shell_integration.rs`, `src\session\session.rs`, `src\server.rs:511-520`). |
| `read_output` returns `exit_code = null` for an exited process. | That means the reader never captured an exact exit code at EOF. Use `wait_for_exit` or `get_session_info` if you need an explicit code after process termination (`src\tools\observation.rs:304-310`, `src\session\session.rs`). |
| Idle sessions are not auto-cleaned. | Cleanup logic exists in `src\session\manager.rs:112-147`, but bootstrap only shows server construction and stdio serving in `src\server.rs:600-609`; hosts must start cleanup explicitly if they want it. |
| E2E tests are flaky on non-Windows platforms. | Current black-box tests are written around `cmd.exe` plus `sleep`-based settling (`tests\integration_test.rs:8-18`, `tests\e2e_input.rs:16-27`, `tests\e2e_observation.rs:14-35`). |
