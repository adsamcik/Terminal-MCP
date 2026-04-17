# terminal-mcp

An MCP (Model Context Protocol) server for interactive terminal session management. Enables AI agents to run, observe, and control interactive CLI applications — shells, REPLs, TUI apps, debuggers — through a set of well-defined tools.

Built in Rust on top of [`portable-pty`](https://crates.io/crates/portable-pty) (ConPTY on Windows, `forkpty` on Linux) and the [`vt100`](https://crates.io/crates/vt100) terminal state parser.

## Features

- **Multiple concurrent PTY sessions** — Windows ConPTY, Linux, and WSL (via `wsl.exe` under ConPTY)
- **Named key input** — arrows, Ctrl+X, function keys, Alt combos, with application cursor mode awareness
- **Screen observation** — read the terminal as a text grid with optional color spans and compact per-glyph style maps
- **PNG screenshots** — monospace font rendering via embedded Cousine font (fontdue + tiny-skia)
- **Delta output** — read only new output since the last read, with ANSI stripping
- **Scrollback history** — ring buffer with tail, range, and regex search
- **`send_and_wait` compound tool** — send input and wait for output in one call (60–70% latency reduction vs separate `send_text` + `wait_for` + `read_output`)
- **3-layer shell prompt detection** — OSC 133/633 sequences, regex heuristics, cursor stability analysis
- **Error pattern detection** — recognises compiler errors, stack traces, and common failure indicators
- **TUI app support** — alternate screen detection, highlight/selection tracking, diff mode
- **Idle session tracking** — idle durations are available for inspection; opt into automatic eviction of idle sessions via `TERMINAL_MCP_IDLE_TIMEOUT` (disabled by default)

## Quick Start

```bash
# Build
cargo build --release

# Run (MCP stdio transport — connect your MCP client to stdin/stdout)
./target/release/terminal-mcp
```

The server communicates over **stdin/stdout** using the MCP JSON-RPC protocol. Logs go to **stderr** in JSON format.

### MCP Client Configuration

Add to your MCP client config (e.g. `mcp.json`):

```json
{
  "mcpServers": {
    "terminal": {
      "command": "path/to/terminal-mcp"
    }
  }
}
```

## Shell integration

Shell integration uses **OSC 133** (FinalTerm / iTerm2) and **OSC 633** (VS Code terminal integration) escape sequences that a shell emits to mark prompt start (`A`), prompt end / input ready (`B`), command start (`C`), and command finish with an exit code (`D;<code>`). When these markers flow through the PTY, terminal-mcp parses them in `src/shell_integration.rs::process_osc` and derives the session's `shell_integration_state`, the current shell phase, and — when the shell reports one — a definite last exit code without falling back to screen-scraping heuristics. OSC 7 (`file://host/path`) is also recognised to track the working directory. (See `src/shell_integration.rs` lines 102–168 and `src/session/session.rs` lines 315–326 for the wiring.)

### Supported shells

| Shell       | Auto-detect markers (OSC 133 / 633) | Auto-inject | Notes                                                                                                                                                                                                                   |
|-------------|-------------------------------------|-------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| bash        | ✅                                  | ❌          | Injection script exists in-tree (`BASH_INTEGRATION`, `src/shell_integration.rs` L318–331) but is not wired into session startup today — use your existing shell integration (e.g. VS Code) or source the script yourself. |
| zsh         | ✅                                  | ❌          | Injection script exists (`ZSH_INTEGRATION`, L333–344); same "not auto-sourced" caveat as bash.                                                                                                                           |
| fish        | ✅                                  | ❌          | Injection script exists (`FISH_INTEGRATION`, L346–356); same caveat.                                                                                                                                                     |
| PowerShell  | ✅                                  | ❌          | Injection script exists (`POWERSHELL_INTEGRATION`, L358–369); same caveat.                                                                                                                                               |
| cmd.exe     | ✅ (if another integration emits them) | ❌        | No injection script provided; `ShellType::Cmd` returns `None` from `injection_script` (L227).                                                                                                                            |
| Other / unknown | ✅ (if markers happen to be emitted) | ❌       | Falls back to regex + cursor-stability heuristics for prompt detection.                                                                                                                                                   |

"Auto-detect" means terminal-mcp recognises and acts on OSC 133/633 markers whenever they arrive in PTY output — no configuration required. "Auto-inject" means terminal-mcp *itself* installs the integration into the spawned shell; today no code path does this, so the `injected` state below is currently unreachable without external wiring.

### `shell_integration_state` values

- `detecting` — initial state. The server has not yet observed any OSC 133/633 markers and has not given up on detection. Sessions that never receive markers stay here.
- `active` — at least one OSC 133 or OSC 633 payload has arrived from the child process, meaning an external integration (e.g. VS Code's, or a user-sourced script) is already emitting markers. Exit codes reported via OSC 133;D are now trusted.
- `injected` — the server itself injected an integration script into the shell. Defined in code (`IntegrationStatus::Injected`, `src/shell_integration.rs` L31) and honoured by prompt detection, but **no production call site sets this state today** — treat it as reserved for a future auto-inject feature.
- `unavailable` — detection was actively given up on. Backed by `ShellIntegration::mark_unavailable` (L252), which is **currently only exercised by unit tests**; live sessions do not transition to this state today and remain in `detecting` indefinitely when no markers appear.

### Limitations

- **Shells not in the supported list rely on heuristics only.** Prompt detection falls back to a regex-match on the last non-empty screen line plus cursor-position stability (`is_at_prompt`, L177–216). Heuristics can produce `Probable` or `Unknown` results but never a `Definite` exit code.
- **No auto-injection in the current release.** Even for bash / zsh / fish / PowerShell, the bundled injection scripts are not sourced into the spawned shell automatically. Users who want `active` state must rely on an existing marker-emitting integration (VS Code terminal, iTerm2 shell integration, Starship with FTCS enabled, a manually-sourced script, etc.).
- **`unavailable` is not reached automatically.** There is no detection timeout in production code, so a session that never sees markers remains `detecting` for its lifetime.
- **Injection (once wired) may fail in locked-down environments** — restricted `PROMPT_COMMAND`, read-only profiles, `set -u`, `noprofile`/`norc` flags, or security policies that block DEBUG traps can all prevent the marker-emitting functions from running.
- **Exit-code reporting depends on markers when available.** With OSC 133;D (or 633;D) the exit code is authoritative. Without it, `read_output` falls back to whatever the reader task captured at EOF, which may be `exit_code = null` if the process is still running or the exact code was never observed.
- **OSC 7 cwd tracking requires the shell to emit it.** The integration scripts do, but sessions without an active integration will not populate the cwd field even if the shell changes directories.
- **Marker source is not authenticated.** Any program writing to the PTY can emit OSC 133/633 sequences and flip the session into `active` state; this is inherent to terminal escape sequences, not a terminal-mcp bug.

### How to check

Call the `get_session_info` MCP tool — the response includes a `shell_integration` field whose value is one of the strings above (see `src/server.rs` L685 and `src/tools/introspection.rs` L226). The `create_session` response does **not** include this field; its `SessionInfo` payload (`src/session/session.rs` L58–66) is limited to identity, pid, command, size, status, and `created_at`. Poll `get_session_info` after the shell has had a chance to print its first prompt to see a meaningful value.

## Configuration

| Environment Variable | Description | Default |
|---|---|---|
| `TERMINAL_MCP_LOG` | Tracing filter (e.g. `debug`, `terminal_mcp=trace`) | `info` |
| `TERMINAL_MCP_IDLE_TIMEOUT` | When set, spawn a background task that evicts sessions idle longer than this duration. Accepts plain integer seconds (`3600`) or humantime strings (`1h`, `30m`, `90s`). Unparsable values log a warning and leave cleanup disabled. | unset (disabled) |

## Security / Trust Model

terminal-mcp is **powerful by design**: any client that can call `create_session`, `send_text`, `send_keys`, or `send_and_wait` can run arbitrary commands as the OS user running the server.

Use it as a **trusted local stdio server** for same-user automation. Do **not** expose it to untrusted or multi-tenant clients without external isolation.

Recommended operating posture:

- Run it as an unprivileged user.
- Keep credentials and sensitive working directories out of the default shell environment when possible.
- Use OS or container isolation if you need a stronger boundary than local same-user trust.
- Expect session dimensions, scrollback, and screenshot rendering to be bounded by server-side safety limits.
- When the transport exposes caller identity, session listing and lookup are scoped to that caller. The bundled stdio server still assumes a single trusted local client.

## MCP Tools Reference

terminal-mcp exposes **15 tools** organised into three tiers:

### Overview

| Tool | Category | Description |
|---|---|---|
| `create_session` | Lifecycle | Create a new PTY session (shell or command) |
| `close_session` | Lifecycle | Terminate a session by ID |
| `list_sessions` | Lifecycle | List active sessions visible to the caller |
| `send_text` | Input | Type raw text into a session |
| `send_keys` | Input | Send named keystrokes (Ctrl+C, arrows, F-keys) |
| `send_and_wait` | Automation | Send input + wait for output (primary command execution tool) |
| `read_output` | Observation | Read new delta output since last read (ANSI stripped) |
| `get_screen` | Observation | Get visible screen as text grid (for TUI apps) |
| `screenshot` | Observation | Capture PNG screenshot of terminal |
| `get_scrollback` | Observation | Read scrollback buffer with optional search |
| `wait_for` | Automation | Wait for a regex pattern to appear (or disappear) |
| `wait_for_idle` | Automation | Wait until output stops changing |
| `wait_for_exit` | Automation | Wait for the child process to exit and return its exit code |
| `get_session_info` | Introspection | Get session metadata, terminal modes, and capabilities |
| `search_output` | Introspection | Regex search in scrollback history with context |

### Tool Details

---

#### `create_session`

Create a new interactive terminal session with a PTY. Spawns the user's default shell or a specified command.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `command` | string | No | User's shell | Command to run |
| `args` | string[] | No | `[]` | Command arguments |
| `cwd` | string | No | Inherited | Working directory |
| `env` | object | No | `{}` | Additional environment variables |
| `rows` | number | No | `24` | Terminal height in rows (clamped to safe server limits) |
| `cols` | number | No | `80` | Terminal width in columns (clamped to safe server limits) |
| `scrollback` | number | No | `1000` | Scrollback lines to retain (clamped to safe server limits) |

**Example:**

```json
{
  "name": "create_session",
  "arguments": {
    "command": "bash",
    "cwd": "/home/user/project",
    "rows": 40,
    "cols": 120,
    "scrollback": 5000
  }
}
```

**Returns:**

```json
{
  "session_id": "7f3f5bc2f1a84d4b8c1d0bb2f2f7d6c1",
  "pid": 12345,
  "command": "bash",
  "rows": 40,
  "cols": 120,
  "status": "Running",
  "created_at": "2025-07-12T10:30:00Z"
}
```

---

#### `send_and_wait`

**The primary tool for command execution.** Sends input to a terminal and waits for the output to settle or match a pattern. Combines `send_text` + `wait_for` + `read_output` into one efficient call. Each call starts from a fresh unread-output baseline so prior unread session backlog does not contaminate the result. When `wait_for` is omitted, `screen` / `both` mode prefers waiting for the visible screen to settle before falling back to idle, and `delta` mode prefers prompt return for interactive shell sessions while still avoiding echo-only early returns.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `input` | string | Yes | — | Text to send (typically a command) |
| `press_enter` | bool | No | `true` | Press Enter after input |
| `wait_for` | string | No | Screen settle, prompt return, or post-input output | Regex pattern to wait for |
| `timeout_ms` | number | No | `30000` | Max wait time in ms |
| `output_mode` | string | No | `"delta"` | `"delta"`, `"screen"`, or `"both"` |

**Example — Run a command and get output:**

```json
{
  "name": "send_and_wait",
  "arguments": {
    "session_id": "a1b2c3d4",
    "input": "cargo test",
    "timeout_ms": 60000
  }
}
```

**Example — Wait for a specific prompt:**

```json
{
  "name": "send_and_wait",
  "arguments": {
    "session_id": "a1b2c3d4",
    "input": "python3",
    "wait_for": ">>>",
    "output_mode": "screen"
  }
}
```

**Returns:**

```json
{
  "matched": true,
  "match_text": ">>>",
  "timed_out": false,
  "output": "Python 3.12.0 ...\n>>> "
}
```

---

#### `get_screen`

Get the current terminal screen contents as a text grid. Best for TUI/full-screen apps (editors, debuggers, htop). For streaming command output, use `read_output` instead.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `include_colors` | bool | No | `false` | Include color spans, highlights, and a compact per-glyph style palette/grid |
| `include_cursor` | bool | No | `true` | Mark cursor position with `▏` |
| `diff_mode` | bool | No | `false` | Return only changed row indices since last call |

**Example:**

```json
{
  "name": "get_screen",
  "arguments": {
    "session_id": "a1b2c3d4",
    "include_colors": true
  }
}
```

**Returns:**

```json
{
  "screen": "user@host:~/project$ ▏\n\n...",
  "rows": 24,
  "cols": 80,
  "cursor": { "row": 0, "col": 22, "visible": true },
  "is_alternate_screen": false,
  "title": "bash",
  "color_spans": [
    { "row": 0, "col": 0, "len": 4, "fg": "green", "bold": true }
  ],
  "glyph_styles": {
    "palette": [
      {},
      { "fg": "green", "bold": true }
    ],
    "rows": [
      [1, 1, 1, 1, 0, 0, 0, null]
    ]
  },
  "highlights": [
    { "row": 0, "col": 0, "len": 4, "inverse": true }
  ]
}
```

---

#### `screenshot`

Capture a PNG screenshot of the terminal screen rendered with an embedded monospace font (Cousine). Returns an MCP image content block.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `theme` | string | No | `"dark"` | Color theme (`"dark"` or `"light"`) |
| `font_size` | number | No | `14` | Font size in pixels (bounded by server limits) |
| `scale` | number | No | `1.0` | Render scale (e.g. `2.0` for retina, bounded by server limits) |

**Example:**

```json
{
  "name": "screenshot",
  "arguments": {
    "session_id": "a1b2c3d4",
    "theme": "dark",
    "scale": 2.0
  }
}
```

---

#### `send_text`

Type raw text into a terminal session. Characters are sent as-is (UTF-8). Small text-entry sends are paced like real typing so raw-input TUIs receive text entry instead of a pasted chunk; use `delay_between_ms` when you need an even slower cadence. For control keys, navigation, or function keys, use `send_keys` instead.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `text` | string | Yes | — | Text to type |
| `press_enter` | bool | No | `false` | Press Enter after typing |
| `delay_between_ms` | number | No | — | Additional delay between typed characters; `send_text` already applies a small built-in pace for normal text entry when omitted |

---

#### `send_keys`

Send named keystrokes to a terminal session. Handles application cursor mode automatically (SS3 vs CSI arrow key encoding).

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `keys` | string[] | Yes | — | Sequence of named keys |

**Supported keys:** `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Insert`, `Delete`, `Enter`, `Tab`, `Escape`, `Backspace`, `Space`, `F1`–`F12`, `Ctrl+A`–`Ctrl+Z`, `Ctrl+[`, `Ctrl+\\`, `Ctrl+]`, `Alt+A`–`Alt+Z`, `Shift+Tab`, `Shift+Up`/`Down`/`Left`/`Right`, `Ctrl+Up`/`Down`/`Left`/`Right`, and more.

**Example — Navigate a TUI menu:**

```json
{
  "name": "send_keys",
  "arguments": {
    "session_id": "a1b2c3d4",
    "keys": ["Down", "Down", "Enter"]
  }
}
```

**Example — Interrupt a running process:**

```json
{
  "name": "send_keys",
  "arguments": {
    "session_id": "a1b2c3d4",
    "keys": ["Ctrl+C"]
  }
}
```

---

#### `read_output`

Read new output since the last read (delta mode). ANSI escape codes are stripped. Polls for output up to the timeout. Best for streaming command output; for TUI apps use `get_screen`.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `timeout_ms` | number | No | `5000` | Max ms to wait for new output |
| `max_bytes` | number | No | `16384` | Maximum bytes to return |

---

#### `get_scrollback`

Read scrollback buffer content that has scrolled above the visible screen. Supports tail access and regex search.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `lines` | number | No | `-100` | Lines to return (negative = from bottom) |
| `search` | string | No | — | Regex pattern to search in scrollback |

---

#### `wait_for`

Wait for a regex pattern to appear (or disappear) in terminal output, or wait for a target number of new output lines. Does not send any input.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `pattern` | string | No* | — | Regex pattern to match |
| `line_count` | number | No* | — | Wait for this many new output lines instead of matching a pattern |
| `timeout_ms` | number | No | `30000` | Max wait time in ms |
| `on_screen` | bool | No | `false` | Match against screen buffer instead of stream |
| `invert` | bool | No | `false` | Wait for pattern to *disappear* |

\* Provide at least one of `pattern` or `line_count`. If both are set, pattern matching wins.

**Example — Wait for output lines instead of a pattern:**

```json
{
  "name": "wait_for",
  "arguments": {
    "session_id": "a1b2c3d4",
    "line_count": 10,
    "timeout_ms": 10000
  }
}
```

---

#### `wait_for_idle`

Wait until the terminal has been idle for a specified duration. By default this watches for new output; set `screen_stable` to watch for screen changes instead, which is more reliable for some TUI apps.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `stable_ms` | number | No | `1000` | Consider idle after this many ms of silence |
| `timeout_ms` | number | No | `30000` | Max overall wait time |
| `screen_stable` | bool | No | `false` | Wait for the visible screen to stop changing instead of waiting for output silence |

---

#### `get_session_info`

Get detailed session metadata including PID, command, terminal size, status, terminal modes (alternate screen, application cursor, bracketed paste, mouse mode), and the full capability manifest of supported keys.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |

---

#### `close_session`

Terminate a session by ID and release its PTY resources.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Session to close |

---

#### `list_sessions`

List active terminal sessions visible to the caller, including their current status, command, and terminal size. Takes no parameters.

---

#### `search_output`

Search scrollback history using a regex pattern. Returns matching lines with surrounding context.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `session_id` | string | Yes | — | Target session |
| `pattern` | string | Yes | — | Regex pattern |
| `max_results` | number | No | `50` | Max matches to return |
| `context_lines` | number | No | `2` | Lines of context around each match |

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                    MCP Client (AI Agent)                  │
│              (JSON-RPC over stdin/stdout)                 │
└───────────────────────┬──────────────────────────────────┘
                        │
┌───────────────────────▼──────────────────────────────────┐
│                  TerminalMcpServer                        │
│            (rmcp ToolRouter, 15 tools)                    │
├──────────────────────────────────────────────────────────┤
│                  SessionManager                           │
│            (DashMap<SessionId, Arc<Session>>)              │
├────────┬────────────┬────────────┬───────────────────────┤
│Session │  Session   │  Session   │  ...                   │
│  ┌─────┴──────┐     │            │                        │
│  │ PtyDriver  │ PTY process (bash, python, vim, ...)      │
│  │ (ConPTY /  │◄──► stdin/stdout                          │
│  │  forkpty)  │                                           │
│  ├────────────┤                                           │
│  │ VtParser   │ Terminal state (vt100 crate)               │
│  │ (screen,   │ - Screen grid + colors                    │
│  │  cursor,   │ - Cursor position & visibility            │
│  │  modes)    │ - Alternate screen detection               │
│  ├────────────┤ - Application cursor mode                  │
│  │ Scrollback │ Ring buffer of scrolled-off lines          │
│  │ Buffer     │ with regex search                          │
│  ├────────────┤                                           │
│  │ Shell      │ OSC 133/633 + regex + cursor stability     │
│  │ Integration│                                           │
│  ├────────────┤                                           │
│  │ Error      │ RegexSet pattern matching for compiler     │
│  │ Detection  │ errors, stack traces, failures             │
│  └────────────┘                                           │
└──────────────────────────────────────────────────────────┘
```

## Typical Workflows

### Running a build command

```
create_session(command: "bash", cwd: "/project")
  → session_id: "abc123"

send_and_wait(session_id: "abc123", input: "cargo build", timeout_ms: 120000)
  → { matched: true, output: "Compiling ...\nFinished ..." }

close_session(session_id: "abc123")
```

### Interacting with a REPL

```
create_session(command: "python3")
  → session_id: "py01"

send_and_wait(session_id: "py01", input: "import math", wait_for: ">>>")
send_and_wait(session_id: "py01", input: "math.pi", wait_for: ">>>")
  → { output: "3.141592653589793\n>>> " }

send_keys(session_id: "py01", keys: ["Ctrl+D"])  # exit
close_session(session_id: "py01")
```

### Navigating a TUI application

```
create_session(command: "htop")
  → session_id: "tui01"

get_screen(session_id: "tui01")
  → { is_alternate_screen: true, screen: "..." }

send_keys(session_id: "tui01", keys: ["F6"])                        # sort menu
get_screen(session_id: "tui01", include_colors: true)               # see selection highlighting
send_keys(session_id: "tui01", keys: ["Down", "Down", "Enter"])

send_keys(session_id: "tui01", keys: ["q"])                         # quit
close_session(session_id: "tui01")
```

## Platforms

| Platform | PTY Backend | Status |
|---|---|---|
| **Windows** | ConPTY | ✅ Primary |
| **Linux** | forkpty | ✅ Supported |
| **WSL** | wsl.exe under ConPTY | ✅ Supported |

## Building

```bash
cargo build --release
```

The binary is at `target/release/terminal-mcp` (or `terminal-mcp.exe` on Windows).

## Testing

```bash
cargo test
```

## Contributing

Contributions are welcome. Please read [`CONTRIBUTING.md`](CONTRIBUTING.md) for the development setup, test commands, and PR checklist. This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).

For security issues, please follow the private reporting process in [`SECURITY.md`](SECURITY.md) instead of opening a public issue.

## License

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual-licensed as above, without any additional terms or conditions.
