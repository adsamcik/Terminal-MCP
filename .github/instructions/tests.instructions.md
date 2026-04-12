---
applyTo: "tests/*.rs"
description: "Maintain the repository's black-box integration and E2E test style."
---

<!-- context-init:managed -->
<!--
context-init:version: 1.0
context-init:generated: 2026-04-12
context-init:source: project-model.json
-->

- Keep `tests/` focused on black-box integration and end-to-end behavior by area (`integration`, `automation`, `lifecycle`, `input`, `observation`, `edge_cases`).
- Preserve the split between these suites and unit tests that live beside implementation under `src/`.
- Expect many current tests to be Windows-centric and `cmd.exe`-oriented with sleep-based settling; changes here should account for that existing harness behavior.
- Use the documented focused test commands from `.github\copilot-instructions.md` or `.github\context\DEVELOPMENT.md` when validating targeted failures.
