---
applyTo: "src/session/*.rs"
description: "Own session registry, lifecycle, and shared runtime state here."
---

<!-- context-init:managed -->
<!--
context-init:version: 1.0
context-init:generated: 2026-04-12
context-init:source: project-model.json
-->

- Keep registry concerns in `manager.rs` and per-session PTY/VT/log state in `session.rs`.
- Follow the existing shared-state pattern: `Arc` plus mutex-protected fields for PTY handles, VT state, output logs, read cursor, scrollback, and activity tracking.
- Preserve close/drop semantics that cancel the reader task and kill the child process on shutdown.
- If touching cleanup behavior, keep README and context docs aligned with the current host-controlled cleanup wiring; the task exists, but server startup does not begin it automatically.
