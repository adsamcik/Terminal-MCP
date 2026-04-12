---
applyTo: "src/terminal/*.rs"
description: "Keep low-level PTY and VT parsing concerns in terminal modules."
---

<!-- context-init:managed -->
<!--
context-init:version: 1.0
context-init:generated: 2026-04-12
context-init:source: project-model.json
-->

- Restrict this directory to low-level terminal abstractions: PTY child/process I/O and VT parser or screen-state logic.
- Preserve the async bridge from blocking PTY reads into Tokio tasks: `PtyDriver` uses `spawn_blocking` plus an `mpsc` channel handoff.
- Do not remove the Windows-specific initial cursor/DSR handshake in `pty_driver.rs` unless the ConPTY startup flow is redesigned and revalidated.
- Keep parser-facing state changes compatible with observation features that read screen text, cursor position, colors, and diffs.
