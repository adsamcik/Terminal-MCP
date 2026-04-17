<!--
context-init:version: 3.1.0
context-init:generated: 2026-04-17T00:00:00Z
context-init:file: PATTERNS
-->
<!-- context-init:managed -->

# terminal-mcp Patterns

## Conventions to follow

| Area | Convention | Evidence | Status |
| --- | --- | --- | --- |
| Module layout | Public areas are split into focused folders, with small `mod.rs` facades where a folder exposes a compact public surface. | `src\session\mod.rs:1-5`, `src\terminal\mod.rs:1-5`, `src\tools\mod.rs:1-5` | Follow |
| Server boundary | Keep `src\server.rs` thin: define MCP schema and delegate behavior into tool/session modules. | `src\server.rs:256-733`, `src\tools\lifecycle.rs:14-53`, `src\tools\input.rs:16-74`, `src\tools\automation.rs:110-316`, `src\tools\observation.rs:455-567` | Follow |
| Handler naming | Tool handlers are named `handle_*` and return `Result<serde_json::Value>` or a serializable response type. | `src\tools\lifecycle.rs:14-53`, `src\tools\input.rs:16-74`, `src\tools\automation.rs:110-316`, `src\tools\observation.rs:455-522` | Follow |
| Shared state | Long-lived runtime state is stored behind `Arc` plus mutexes; session registry uses `DashMap<SessionId, Arc<Session>>`. | `src\session\manager.rs:17-19`, `src\session\session.rs:170-195`, `src\terminal\pty_driver.rs:40-55` | Follow |
| Blocking I/O bridge | Blocking PTY reads/writes use `spawn_blocking` and channel handoff, while higher-level waits poll session state in async loops. | `src\terminal\pty_driver.rs:149-151`, `src\terminal\pty_driver.rs:168-180`, `src\tools\automation.rs:173-271`, `src\tools\observation.rs:465-481` | Follow |
| JSON payload style | Responses are assembled with `serde_json::json!` or `serde_json::to_value`, not handwritten strings. | `src\tools\lifecycle.rs:32-52`, `src\tools\input.rs:49-73`, `src\tools\automation.rs:285-315` | Follow |
| Internal imports | Internal modules generally import siblings through `crate::...` paths. | `src\tools\lifecycle.rs:4-5`, `src\tools\input.rs:4-5`, `src\tools\automation.rs:14-16`, `src\tools\observation.rs:13-15` | Follow |
| Testing layout | Keep fast unit tests next to implementation, and use `tests\*.rs` for black-box / end-to-end flows. | `src\tools\automation.rs:318-338`, `src\scrollback.rs:160-220`, `tests\integration_test.rs:1-80`, `tests\e2e_input.rs:1-80`, `tests\e2e_observation.rs:1-60` | Follow |
| Platform assumptions | Current end-to-end coverage is Windows-first and often uses `cmd.exe` plus sleep-based settling. | `tests\integration_test.rs:8-18`, `tests\e2e_input.rs:16-27`, `tests\e2e_observation.rs:14-35` | Follow |
| Shell integration maturity | Shell integration and error-detection modules exist, and session info now reports the live integration state. Treat availability as environment-dependent rather than globally wired. | `src\shell_integration.rs:80-257`, `src\error_detection.rs:104-188`, `src\session\session.rs:573-583`, `src\server.rs:674-703` | Evolving |

## Practical guidance

- Add new MCP behavior in `src\tools\*.rs` first, then keep the matching `src\server.rs` method as a thin adapter.
- If you introduce new shared runtime state, match the existing `Arc<Mutex<_>>` style used by `Session` and `PtyDriver` instead of mixing ownership models.
- Preserve the existing observation split: raw PTY bytes feed `Session`, `VtParser` owns screen state, and observation handlers format client-facing responses.
- For tests that exercise real PTY behavior, follow the existing `SessionManager` + spawned session pattern from `tests\integration_test.rs` and `tests\e2e_*.rs`.

## Things not to assume

- Do not assume shell integration will be active for every shell just because `src\shell_integration.rs` exists; rely on the reported live state.
- Do not assume exact process exit codes are always available from `read_output`; observation now returns `null` when EOF did not yield a concrete code (`src\tools\observation.rs:504-522`).
- Do not assume README feature claims always reflect current wiring; verify against server/session bootstrap code when changing lifecycle behavior.
