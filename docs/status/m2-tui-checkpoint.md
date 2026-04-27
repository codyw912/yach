# M2 TUI checkpoint

Date: 2026-04-26

## Scope of this checkpoint

This checkpoint audits current implementation state against the M2 TUI alpha target from `PRD-v0.1.md` and `docs/plans/2026-04-21-m2-tui-alpha-design.md`.

It covers:

- fullscreen TUI launch and Pi RPC bridge
- transcript, tool area, input composer, status bar
- slash commands and selectors
- dialogs
- basic performance instrumentation
- relevant UX backlog items from `docs/plans/2026-04-24-tui-ux-backlog.md`

It does not claim completion of broader Phase 1 compatibility work such as Pi settings/resources, existing Pi session files, full tree navigation, canonical extension suites, or rich SDK sidecar parity.

## Summary

M2 is **implemented-unverified / partial**, not complete.

The core TUI alpha exists: `yach-cli tui` launches a fullscreen ratatui/crossterm UI, bridges through `yach-proto` to the Pi RPC adapter, renders transcript/tool/input/status regions, supports multiline input, handles Tier A dialogs, exposes model/session/thinking selectors, supports basic session forking, and includes a simple render-performance overlay.

The remaining M2 gaps are mostly alpha hardening and verification rather than absence of a TUI. The most important gaps are terminal cleanup safety, real cancellation semantics, stream/session correlation, transcript scrolling, slash command consistency, stronger model/session data sources, non-fatal handling for unknown backend events, and performance evidence that measures user-perceived latency.

## Milestone status

| M2 target | Status | Evidence | Notes / next action |
|---|---|---|---|
| Fullscreen TUI | `implemented-unverified` | `crates/yach-cli/src/main.rs`, `crates/yach-ui/src/app.rs` | Launch path and terminal mode code exist. Needs manual/recorded smoke or automated terminal harness before calling verified. |
| Transcript pane with streaming support | `implemented-unverified` | `crates/yach-ui/src/transcript.rs`, `crates/yach-ui/src/app.rs` | Deltas coalesce into assistant entries. Transcript scrolling/virtualization is still missing. |
| Tool output area | `implemented-unverified` | `crates/yach-ui/src/tool_area.rs`, `crates/yach-ui/src/app.rs` | Active tools and compact completion summaries exist. Expandable details/overflow handling remain future work. |
| Input composer | `verified` | `crates/yach-ui/src/input.rs`, app tests for submit/newline behavior | Uses `ratatui-textarea`; wraps/grows up to a cap; Enter submits and Ctrl+J / Shift+Enter insert newline. Still worth manual terminal validation across terminals. |
| Slash completion | `partial` | `crates/yach-ui/src/slash_commands.rs`, `crates/yach-ui/src/app.rs` | Completion exists but omits executable commands such as `/fork`, `/thinking`, and `/perf`; command execution uses prefix matching. |
| Model selector | `partial` | `crates/yach-ui/src/model_selector.rs`, `crates/yach-ui/src/app.rs` | Static model list and RPC event path exist. Needs dynamic source or explicit alpha-static caveat plus backend confirmation/rollback semantics. |
| Thinking control | `partial` | `crates/yach-ui/src/thinking_level.rs`, `crates/yach-ui/src/app.rs`, `crates/yach-proto/src/lib.rs` | UI/protocol path exists. Needs stronger adapter test/live evidence that stock Pi accepts the command. |
| Session picker | `partial` | `crates/yach-ui/src/session_picker.rs`, `crates/yach-ui/src/app.rs` | Picker exists but is based on default/observed session ids, not a real recent-session/session-tree source. |
| Session fork | `implemented-unverified` | `crates/yach-ui/src/app.rs`, `crates/yach-adapter-pi-rpc/src/session.rs` | Capability-gated basic fork path exists and smoke coverage has exercised RPC fork. Full tree navigation is M3/Phase 1 broader scope. |
| Status bar | `implemented-unverified` | `crates/yach-ui/src/status_bar.rs`, `crates/yach-ui/src/layout.rs` | Shows model/session/status/thinking/compaction fields at bottom. True compaction visibility remains incomplete because transcript compaction count is currently stub-like. |
| Basic performance instrumentation | `partial` | `crates/yach-ui/src/perf_metrics.rs`, `crates/yach-ui/src/perf_overlay.rs`, `docs/benchmarks/baseline-2026-04-23.md` | Render duration/total renders are tracked. No p95/p99, startup, keypress-to-paint, heavy tool, large transcript, or Pi comparison evidence yet. |

## Empirical notes

Recent merged branch validation reported:

- `just test` passed across the workspace: 78 unit tests.
- `just lint` passed with Clippy `-D warnings`.
- `just run print-capabilities` printed stock RPC capabilities including prompt streaming, dialogs, notifications, status entries, widgets, and session forking.
- `just run smoke-pi-rpc` reported success for initialization, state/model/session/stats/messages/dialog smoke operations.

These checks validate compile-time/unit behavior and the stock RPC smoke path. They do not replace manual fullscreen TUI dogfooding or user-perceived latency measurements.

## Architecture validation

What is matching the intended architecture well:

- The UI does not directly speak Pi RPC; `yach-cli` bridges Pi RPC adapter events into `yach-proto` client/server events.
- `yach-ui` owns app state, rendering, input modes, transcript state, active tools, dialogs, and overlays.
- The adapter remains isolated in `crates/yach-adapter-pi-rpc`, with parse/serialize/session responsibilities separated.
- The M2 code continues the PRD’s “Pi-shaped Rust shell first” strategy rather than jumping to a native Rust backend.

Drift/risk signals to address before broader alpha dogfood:

- TUI/backend channels are currently unbounded, while the PRD calls for bounded queues/backpressure before serious performance validation.
- Transcript rendering still works from the full entry list; virtualization/large-transcript behavior is not proven.
- Ctrl+C currently behaves like local UI cancellation rather than proven backend/tool cancellation.
- Unknown backend events can still be brittle if parsed as fatal disconnects instead of non-fatal noise/status.
- Session/model/thinking controls are optimistic and not clearly correlated with backend confirmation.

## Compatibility snapshot

M2 adds a real UI surface on top of the M1 RPC adapter, but most M3 compatibility goals remain unproven.

| Compatibility area | Status | Evidence / notes |
|---|---|---|
| Prompt streaming through TUI | `implemented-unverified` | UI handles `PromptDelta`; needs live/manual TUI evidence. |
| Tier A dialogs in TUI | `implemented-unverified` | Confirm/input/editor/select modes exist and have unit coverage; needs manual UX validation. |
| Notifications/status/widgets/title | `implemented-unverified` | Adapter maps events; UI status/tool surfaces exist; rich component behavior remains out of scope for stock RPC. |
| Session switching/forking | `partial` | Basic select/fork paths exist; real recent-session list, tree navigation, stats/export remain incomplete. |
| Settings/resources/packages | `planned` | No new M2 evidence. Still M3. |
| Existing Pi session files/tree | `planned` | No new M2 evidence. Still M3. |
| Rich UI surfaces / SDK sidecar | `deferred` | Still M4. |

## UX backlog snapshot

| Backlog item | Status | Notes |
|---|---|---|
| Adopt `ratatui-textarea` | `verified` | Dependency and composer integration exist. |
| Expand/wrap long prompts | `implemented-unverified` | Composer wraps/grows; needs manual terminal validation. |
| Multiline input | `verified` | Ctrl+J and Shift+Enter behavior is covered by tests. |
| User/assistant separation | `partial` | Role-specific prefixes/styles exist; readability polish remains. |
| Continuation alignment | `partial` | Basic continuation indentation exists; needs visual verification. |
| Completed tool rows compact summary | `verified` | Completion summary behavior exists and is unit-tested. |
| Bottom status placement | `verified` | Status bar renders below input and lifecycle noise is filtered. |

## Current gaps

### Highest-priority hardening before calling M2 complete

- Add a terminal cleanup guard so raw mode, alternate screen, and cursor visibility are restored on render/runtime errors.
- Clarify and implement stream cancellation semantics. If backend cancellation is not available yet, relabel Ctrl+C behavior and prevent stale deltas from reviving a cancelled local stream.
- Prevent model/session/thinking changes during active streams or filter/correlate deltas by active session/stream.
- Fix dialog text cursor handling for non-ASCII input, or reuse `ratatui-textarea` for dialog text/editor input.
- Make unknown/noncritical Pi RPC events non-fatal or explicitly document fatal behavior.

### Important alpha usability gaps

- Add minimal transcript scroll controls independent of textarea cursor movement.
- Align slash completion with executable commands and require exact command matching for destructive commands such as `/clear` and `/quit`.
- Drain and surface Pi child stderr in a bounded way.
- Add startup/init timeout or visible startup progress.
- Add overflow signaling for active tool rows.
- Make model/session/thinking selectors show pending/confirmed state or roll back on backend rejection.

### Performance evidence gaps

- Startup-to-first-frame or startup-to-interactive measurement.
- Keypress-to-paint measurement while idle and while streaming.
- p95/p99 render latency, not just average render duration.
- Heavy tool output and large transcript replay.
- Same-machine comparison against Pi for at least one important tail-latency workload.

## Suggested next step

M2 should remain `in-progress` until the highest-priority hardening items are resolved or explicitly deferred with alpha caveats.

Recommended next plan: **M2 alpha hardening pass**, focused on:

1. terminal cleanup guard,
2. cancellation/stream-state semantics,
3. session/model/thinking controls during streaming,
4. dialog Unicode safety,
5. slash command consistency,
6. minimal transcript scrolling.

After that pass, rerun this checkpoint and decide whether M2 can move from `implemented-unverified / partial` to `verified alpha`.
