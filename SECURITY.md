# Security Policy

## Supported Versions

`terminal-mcp` is pre-1.0 software. Security fixes are made on the latest `main` branch and released as new `0.x` versions. Older tagged versions are not maintained.

## Threat Model

`terminal-mcp` is designed as a **trusted local stdio server** for same-user automation. By design, any client that can call `create_session`, `send_text`, `send_keys`, or `send_and_wait` can run arbitrary commands as the OS user running the server. This is not a vulnerability — it is the intended capability.

In-scope security concerns include, for example:

- Path traversal or injection in parameters that are **not** the command body (session IDs, filters, file paths used by the server itself).
- Memory safety bugs (panics on crafted input, unsound `unsafe`, use-after-free in PTY handling).
- Denial-of-service vectors that cause the server to crash or hang on malformed JSON-RPC input.
- Information disclosure between concurrent sessions running under the same server process.

Out of scope:

- Abuse of the documented command-execution capability by a trusted client.
- Running `terminal-mcp` exposed to untrusted or multi-tenant clients without external isolation.

## Reporting a Vulnerability

Please do **not** open public GitHub issues for suspected security problems.

Report privately via GitHub's **Security Advisories** on the repository:

- <https://github.com/adsamcik/Terminal-MCP/security/advisories/new>

If that is unavailable, contact the maintainer through their GitHub profile at <https://github.com/adsamcik>.

When reporting, please include:

- A clear description of the issue and its impact.
- Steps or a minimal reproduction (tool calls, inputs, expected vs actual behavior).
- Affected version / commit SHA and platform (Windows / Linux).
- Any suggested mitigation, if you have one.

You can expect an initial acknowledgement within a reasonable timeframe. We will coordinate a fix and disclosure timeline with you before any public announcement.
