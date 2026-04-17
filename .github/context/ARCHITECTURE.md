<!--
context-init:version: 3.1.0
context-init:generated: 2026-04-17T00:00:00Z
context-init:file: ARCHITECTURE
-->
<!-- context-init:managed -->

# terminal-mcp Architecture

## System overview

`terminal-mcp` is a single Cargo package that exposes an MCP server over stdio. `src\main.rs` initializes JSON tracing on stderr and hands off to `server::run` (`src\main.rs:16-33`). `src\server.rs` defines MCP tool schemas, wires the tool router, and serves the stdio transport (`src\server.rs:26-228`, `src\server.rs:771-788`).

```text
MCP client (stdin/stdout)
        |
        v
src\main.rs -> src\server.rs
                  |
                  +--> src\tools\lifecycle.rs
                  +--> src\tools\input.rs
                  +--> src\tools\automation.rs
                  +--> src\tools\observation.rs
                  +--> src\tools\introspection.rs
                              |
                              v
                    src\session\manager.rs
                              |
                              v
                       src\session\session.rs
                         |              |
                         v              v
          src\terminal\pty_driver.rs   src\terminal\vt_parser.rs
                         |
                         v
                    child shell / command
```

## Component map

| Component | Purpose | Key files |
| --- | --- | --- |
| Server bootstrap | Starts tracing and the stdio MCP service. | `src\main.rs:16-33`, `src\server.rs:771-788` |
| MCP router | Declares tool parameter types and delegates tool calls to handlers. | `src\server.rs:26-228`, `src\server.rs:256-733` |
| Session registry | Owns the live session map and close/list/create operations. | `src\session\manager.rs:17-222` |
| Session runtime | Couples PTY, VT parser, output log, scrollback, and idle tracking for one session. | `src\session\session.rs:170-281`, `src\session\session.rs:688-722` |
| PTY backend | Spawns a PTY-backed child process, forwards blocking I/O into async code, resizes, and kills. | `src\terminal\pty_driver.rs:40-277` |
| Terminal model | Tracks screen contents, cursor, title, colors, diff snapshots, and terminal modes. | `src\terminal\vt_parser.rs:96-348` |
| Tool handlers | Implements lifecycle, input, automation, observation, and introspection behavior. | `src\tools\lifecycle.rs:14-53`, `src\tools\input.rs:16-74`, `src\tools\automation.rs:110-316`, `src\tools\observation.rs:349-567`, `src\tools\introspection.rs:60-270` |
| Screenshot renderer | Renders VT state into PNG using embedded Cousine fonts. | `src\screenshot.rs:10-50`, `src\screenshot.rs:184-251`, `assets\Cousine-Regular.ttf`, `assets\Cousine-Bold.ttf` |
| Scrollback + helpers | Supports regex search, buffered history, key translation, shell/error helpers, and WSL support. | `src\scrollback.rs:32-156`, `src\keys.rs:8-164`, `src\shell_integration.rs:80-257`, `src\error_detection.rs:104-188`, `src\wsl.rs:12-113` |

## Primary flows

### 1. Server startup

1. `main` configures `tracing_subscriber` from `TERMINAL_MCP_LOG` and logs startup (`src\main.rs:18-30`).
2. `server::run` creates `TerminalMcpServer`, binds stdio transport, and waits on the MCP service (`src\server.rs:771-788`).

### 2. Session creation

1. `create_session` in `src\server.rs` deserializes params and delegates to the lifecycle handler (`src\server.rs:302-324`).
2. `handle_create_session` builds `SessionConfig` and calls `SessionManager::create_session_async` (`src\tools\lifecycle.rs:14-33`).
3. `Session::new` creates PTY + VT state, then spawns the background reader task (`src\session\session.rs:241-281`).
4. `PtyDriver::spawn` opens the PTY, resolves the default shell when needed, spawns the child, and bridges PTY reads into Tokio via `spawn_blocking` + `mpsc` (`src\terminal\pty_driver.rs:77-165`).

### 3. Output observation

1. The session reader task continuously feeds PTY bytes into the VT parser, raw output log, scrollback buffer, and idle timestamp (`src\session\session.rs:285-348`).
2. `handle_read_output` consumes unread bytes, strips ANSI, and returns cursor/idle metadata (`src\tools\observation.rs:455-522`).
3. `get_screen` reads the VT snapshot and can include cursor markers, colors, highlights, and changed-row diffs (`src\tools\observation.rs:356-432`).

### 4. Automation

1. `send_and_wait` routes through `src\server.rs` into `handle_send_and_wait` (`src\server.rs:436-461`, `src\tools\automation.rs:110-316`).
2. The handler writes bytes to the session, then either polls for a regex match or, without an explicit pattern, uses screen-settle detection for screen-oriented calls and prompt-return detection for interactive shell delta calls before falling back to idle when appropriate (`src\tools\automation.rs:162-271`).
3. The response returns delta output, screen output, or both, depending on `output_mode` (`src\tools\automation.rs:285-316`).

### 5. Session shutdown

1. `close_session` delegates from `src\server.rs` to the lifecycle handler (`src\server.rs:328-352`, `src\tools\lifecycle.rs:35-44`).
2. `SessionManager` removes the session from the `DashMap` and either unwraps it for graceful close or force-kills the PTY if extra `Arc` refs still exist (`src\session\manager.rs:75-98`).
3. `Session::close` / `Drop` cancels the reader task and kills the child process (`src\session\session.rs:696-722`).

## Boundaries and responsibilities

- `src\server.rs` should stay as the MCP boundary: schema, tool registration, session lookup, and response serialization.
- `src\tools\*.rs` hold user-visible behavior for each tool family.
- `src\session\*.rs` own long-lived runtime state and session lifecycle.
- `src\terminal\*.rs` are low-level terminal abstractions: PTY process control and VT parsing.

## Architecture notes to keep in mind

- Windows ConPTY support depends on an explicit initial handshake written in `PtyDriver::spawn`; without it, session output may never start flowing (`src\terminal\pty_driver.rs:133-143`).
- Shell integration is tracked in session state and surfaced through `get_session_info` as `"detecting"`, `"active"`, `"injected"`, or `"unavailable"` (`src\shell_integration.rs:80-257`, `src\session\session.rs:573-583`, `src\server.rs:674-703`).
- Idle-session cleanup logic exists in `SessionManager::start_cleanup_task`, but bootstrap still only constructs the server and starts stdio serving; cleanup remains host-controlled rather than auto-started at server startup (`src\session\manager.rs:172-221`, `src\server.rs:771-788`, `README.md:19`).
