# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Refreshed `file:line` citations in `.github/context/ARCHITECTURE.md` and
  `.github/context/PATTERNS.md` after formatter-driven reflow.
- Contributor checklist in `.github/PULL_REQUEST_TEMPLATE.md` refreshed to
  cover fmt, clippy, tests, CHANGELOG, and docs expectations.

### Added

- GitHub Actions release workflow that builds and uploads prebuilt
  `terminal-mcp` binaries for Windows (`x86_64-pc-windows-msvc`, zip) and
  Linux (`x86_64-unknown-linux-gnu`, tar.gz) — plus SHA-256 checksums —
  whenever a `v*.*.*` tag is pushed, and creates a GitHub Release with
  notes extracted from this changelog.
- `send_and_wait` now resets its unread delta before sending input, so stale
  startup output or backlog from prior commands no longer leaks into the new
  command's result or triggers premature idle completion.
- `send_and_wait` in `screen` / `both` mode waits for a meaningful visible
  screen change before considering an idle completion, with a longer settle
  window for slow-start launched applications (e.g. full-screen TUIs) than for
  fast navigation flows.
- `send_and_wait` in `delta` mode on interactive shell sessions prefers prompt
  return over raw output idle, preventing bursty shell commands from completing
  between output pauses.
- Regression tests for delayed output, bursty prompt-return, slow-start screen
  launches, streamed screen updates, screen stability timing, stale unread
  output, and hidden-cursor observation.

### Fixed

- `send_text` now reliably types character-by-character into raw-input apps on Windows. Previously the PTY write of multi-byte strings could arrive as a single chunk, causing raw-mode consumers (e.g. `node --interactive`, TUIs) to see the input as a paste. A small inter-character delay now guarantees one chunk per character.
- `read_output` and `get_screen` now agree on cursor visibility by reading from
  the VT parser's live state.
- `get_screen(include_cursor=true)` no longer injects a synthetic cursor marker
  when the VT cursor is hidden.
- `send_and_wait` no longer completes on echoed input alone in `delta` mode
  without a pattern; it waits for post-input non-echo output before treating
  idle as completion.

## [0.1.0] - Initial development

- Initial implementation of the MCP terminal session server over stdio.
- Windows ConPTY, Linux, and WSL support via `portable-pty`.
- Session lifecycle, named-key input, screen observation, PNG screenshots,
  delta output, scrollback, `send_and_wait`, shell integration detection, and
  idle session reaping.
