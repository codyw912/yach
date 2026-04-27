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

Update after hardening branch: the highest-priority code hardening items are now implemented with unit coverage, but M2 still needs manual/live TUI smoke evidence and performance/compatibility measurements before it should be called a verified alpha.

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

Recent validation reported:

- `just test` passed across the workspace after the hardening fixes.
- `just lint` passed with Clippy `-D warnings` after the hardening fixes.
- `just run print-capabilities` printed stock RPC capabilities including prompt streaming, dialogs, notifications, status entries, widgets, and session forking.
- `just run smoke-pi-rpc` reported success for initialization, state/model/session/stats/messages/dialog smoke operations after correcting the fork command serialization to stock RPC's `fork` command.
- `just run smoke-pi-rpc-prompt` and `just run smoke-pi-rpc-tool` passed in preflight smoke.

Manual fullscreen TUI smoke on `feat/m2-tui-hardening` reported:

- Pass: launch/quit terminal restoration, basic streaming, Ctrl+J multiline input, local Ctrl+C stop-following, slash commands/completion, selectors/perf except model caveats, and transcript scrolling.
- Not observed: live backend dialogs.
- Found and fixed in follow-up hardening: `Ctrl+M` is not a reliable model-selector binding because terminals commonly encode it as Enter/CR; blank/whitespace-only prompts could be submitted after that path; `Ctrl+F` initially used an invalid `fork_session` command and then the lower-level `fork` command with a session id rather than an entry id. Model selector access now has `Alt+M` and `F2`, blank prompts are ignored/cleared, current-branch duplication serializes as stock RPC `clone`, clone responses show `session cloned`, and cloning is blocked until the visible transcript has at least one user message so fresh sessions do not surface Pi's entry-not-found error. True fork-from-entry and visible session-tree confirmation remain broader session work.
- Remaining manual caveat: model selection still depends on a static placeholder list rather than a backend-provided model list; selecting an unavailable model can produce backend rejection such as `Model not found`. Treat model selection as partial until the selector is backed by real model metadata or visibly marked alpha-static.

These checks validate compile-time/unit behavior, the stock RPC smoke path, and a first manual TUI dogfood pass. They do not yet provide dialog evidence or user-perceived latency measurements.

## Architecture validation

What is matching the intended architecture well:

- The UI does not directly speak Pi RPC; `yach-cli` bridges Pi RPC adapter events into `yach-proto` client/server events.
- `yach-ui` owns app state, rendering, input modes, transcript state, active tools, dialogs, and overlays.
- The adapter remains isolated in `crates/yach-adapter-pi-rpc`, with parse/serialize/session responsibilities separated.
- The M2 code continues the PRD’s “Pi-shaped Rust shell first” strategy rather than jumping to a native Rust backend.

Drift/risk signals to address before broader alpha dogfood:

- TUI/backend channels are currently unbounded, while the PRD calls for bounded queues/backpressure before serious performance validation.
- Transcript rendering still works from the full entry list; virtualization/large-transcript behavior is not proven.
- Ctrl+C now uses honest local stop-following semantics rather than claiming unsupported backend cancellation; first manual live-stream validation passed, but it is still not true backend cancellation.
- Unknown backend events are mapped to status/degraded status instead of fatal parser errors for well-formed unknown methods.
- Session/model/thinking controls are blocked while backend work is busy; full backend-confirmed selector rollback remains future work.

## Compatibility snapshot

M2 adds a real UI surface on top of the M1 RPC adapter, but most M3 compatibility goals remain unproven.

| Compatibility area | Status | Evidence / notes |
|---|---|---|
| Prompt streaming through TUI | `implemented-unverified` | UI handles `PromptDelta`; first manual smoke passed for basic streaming and local stop-following. Needs broader dogfood/perf evidence before verified. |
| Tier A dialogs in TUI | `implemented-unverified` | Confirm/input/editor/select modes exist and have unit coverage; live dialog was not observed in manual smoke. |
| Notifications/status/widgets/title | `implemented-unverified` | Adapter maps events; UI status/tool surfaces exist; rich component behavior remains out of scope for stock RPC. `/help` currently uses the compact status bar and can be hard to read. |
| Session switching/forking | `partial` | Basic select paths exist and Ctrl+F duplicates the current active branch via stock RPC `clone` after at least one user message, with a status confirmation. Session picker does not yet show the cloned session; real fork-from-entry, recent-session list, tree navigation, stats/export remain incomplete. |
| Settings/resources/packages | `planned` | No new M2 evidence. Still M3. |
| Existing Pi session files/tree | `planned` | No new M2 evidence. Still M3. |
| Rich UI surfaces / SDK sidecar | `deferred` | Still M4. |

## UX backlog snapshot

| Backlog item | Status | Notes |
|---|---|---|
| Adopt `ratatui-textarea` | `verified` | Dependency and composer integration exist. |
| Expand/wrap long prompts | `implemented-unverified` | Composer wraps/grows; needs manual terminal validation. |
| Multiline input | `verified` | Ctrl+J passed manual smoke. Shift+Enter remains terminal-dependent and was not observed in the manual terminal used for smoke. |
| User/assistant separation | `partial` | Role-specific prefixes/styles exist; readability polish remains. |
| Continuation alignment | `partial` | Basic continuation indentation exists; needs visual verification. |
| Completed tool rows compact summary | `verified` | Completion summary behavior exists and is unit-tested. |
| Bottom status placement | `verified` | Status bar renders below input and lifecycle noise is filtered. |

## Current gaps

### Highest-priority hardening before calling M2 complete

Implemented with unit/lint verification:

- Terminal cleanup guard restores raw mode, alternate screen, and cursor visibility on guarded exits.
- Ctrl+C during streaming now means local stop-following; stale deltas/tool starts do not revive the cancelled visible stream.
- Model/session/thinking/fork controls are blocked while backend work is busy, and prompt deltas are filtered against active/effective session context.
- Dialog text cursor handling is UTF-8 boundary-safe for multibyte input, and dialogs queue FIFO instead of overwriting an active request.
- Unknown/noncritical Pi RPC methods map to status/degraded status instead of fatal parser errors.
- Slash command completion/execution is registry-backed and exact-match; prefix accidents such as `/clearance` no longer execute destructive commands.
- Minimal PageUp/PageDown/End transcript scrolling exists without stealing Up/Down from prompt editing.

Still needed before calling M2 complete:

- Focused manual re-smoke for `Alt+M`/`F2` model selector access, ignored blank prompts after Ctrl+M/Enter ambiguity, and `Ctrl+F` fork after the follow-up fixes.
- Live dialog UX remains unobserved; keep as `implemented-unverified` until a dialog-producing workflow or test harness exercises it.
- Decide whether remaining alpha caveats such as static model list, limited session picker, and non-virtualized transcript are acceptable for M2 or need another hardening pass.

### Important alpha usability gaps

- Replace the static model selector list with backend-provided model metadata or visibly mark unavailable/static choices.
- Add visible session clone/fork confirmation and a real session list/tree so cloned sessions are discoverable from the picker.
- Make `/help` readable outside the narrow status bar, for example via a transient help overlay or transcript/system entry.
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

Recommended next step: **manual M2 TUI smoke and checkpoint refresh**.

The hardening pass has landed at the code/unit-test level. After manual live validation, rerun this checkpoint and decide whether M2 can move from `implemented-unverified / partial` to `verified alpha`, or whether another focused hardening pass is needed.
