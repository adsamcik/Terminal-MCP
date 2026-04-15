---
applyTo: "src/server.rs"
description: "Keep MCP routing in server.rs and delegate operational logic."
---

<!-- context-init:managed -->
<!--
context-init:version: 1.0
context-init:generated: 2026-04-12
context-init:source: project-model.json
-->

- Keep this file focused on MCP schema, parameter decoding, tool routing, and stdio service startup.
- Delegate non-routing behavior to `src/tools/*.rs` or `SessionManager` methods; `server.rs` already routes requests into those layers.
- Keep `get_session_info` wired to live session metadata, including the current shell-integration state string.
- See `.github\context\ARCHITECTURE.md` for broader flow details.
