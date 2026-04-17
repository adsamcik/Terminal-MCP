# Contributing to terminal-mcp

Thanks for your interest in contributing! terminal-mcp is an MCP server that gives AI agents PTY-backed terminal control, so most contributions touch either the MCP schema, session/PTY lifecycle, or observation/automation tools.

## Getting started

1. Install a recent stable Rust toolchain (MSRV: **1.88**, edition 2024).
2. Install components used by CI: `rustup component add rustfmt clippy`.
3. Clone the repo and build:

   ```bash
   cargo build
   cargo test
   ```

On Windows, the integration and end-to-end test suites launch real `cmd.exe` sessions through ConPTY. They are inherently sleep-based and should be run single-threaded:

```bash
cargo test --test integration_test -- --test-threads=1
cargo test --test e2e_automation -- --test-threads=1 --nocapture
```

On Linux, most of the lower-level unit tests run fine, but the Windows-centric integration suites are skipped or may be flaky — please note your test platform in PRs.

## Project layout

See `.github/copilot-instructions.md` and `.github/context/ARCHITECTURE.md` for the full map. Quick version:

- `src/main.rs` — bootstrap only (logging + `server::run`).
- `src/server.rs` — MCP schema, parameter decoding, and tool routing.
- `src/tools/*.rs` — behavioral logic for each tool category (lifecycle, input, automation, observation, introspection).
- `src/session/*.rs` — session registry (`manager.rs`) and per-session PTY/VT state (`session.rs`).
- `src/terminal/*.rs` — low-level PTY driver and VT parser.
- `tests/*.rs` — black-box integration and end-to-end tests, grouped by area.

Follow the directory-scoped rules in `.github/instructions/*.instructions.md` when modifying code in the corresponding folders.

## Before opening a PR

Please run the same checks CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets
cargo build --all-targets
cargo test
```

When submitting the PR:

- Describe **what** behavior changed and **why**, not just **how**.
- Link to an issue if one exists.
- If you changed MCP schemas or tool behavior, update `README.md` and any relevant docs in `.github/context/`.
- Add regression tests for bug fixes.
- Keep commits focused; prefer small, well-scoped patches over sweeping rewrites.

## Code style

- Rust 2024 edition, formatted with `rustfmt` defaults.
- Prefer targeted fixes and defense-in-depth additions over broad refactors in the same PR.
- Do not disturb the Windows ConPTY startup handshake in `src/terminal/pty_driver.rs` without a clear, tested reason — it is load-bearing.
- Avoid adding comments that simply restate the code; comment when the *why* is non-obvious.

## Reporting bugs

Please open an issue using the bug report template and include, where possible:

- Operating system (Windows version / Linux distro), Rust version.
- The MCP client or host you are running against.
- A minimal reproduction: command(s) used to create the session, tool call sequence, and observed vs expected behavior.
- Relevant stderr log output (set `TERMINAL_MCP_LOG=debug` for more detail).

## Security issues

Please do **not** file public issues for security vulnerabilities. See [`SECURITY.md`](SECURITY.md) for the private reporting process.

## Licensing

By contributing, you agree that your contributions will be dual-licensed under the MIT and Apache-2.0 licenses that cover the rest of the project. See [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By participating, you are expected to uphold it.
