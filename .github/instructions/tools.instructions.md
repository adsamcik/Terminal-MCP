---
applyTo: "src/tools/*.rs"
description: "Implement category-specific tool handlers here."
---

<!-- context-init:managed -->
<!--
context-init:version: 1.0
context-init:generated: 2026-04-12
context-init:source: project-model.json
-->

- Keep category-specific MCP tool logic in this directory: lifecycle, input, automation, observation, and introspection.
- Match existing boundaries: lifecycle handlers take `SessionManager`, input/automation/observation handlers take `Session`, and helper modules stay pure/serializable.
- Keep `server.rs` thin by moving new operational branches here instead of expanding router logic.
- When changing observation flows, keep delta-output consumption and VT snapshot reads compatible with current `read_output` and `get_screen` behavior.
