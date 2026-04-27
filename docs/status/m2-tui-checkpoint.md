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

M2 is a **verified alpha with caveats**.

The core dogfood loop has automated validation and manual terminal smoke evidence: `yach-cli tui` launches a fullscreen ratatui/crossterm UI, bridges through `yach-proto` to the Pi RPC adapter, renders transcript/tool/input/status regions, supports multiline input, handles Tier A dialogs, exposes model/session/thinking selectors, supports basic current-branch cloning, includes a simple render-performance overlay, uses backend-provided model metadata, provides a readable `/help` overlay, and includes a `tui-dialog-smoke` manual harness.

This does not mean Phase 1 compatibility is complete. Session tree/fork-from-entry UX, cloned-session discoverability, broader Pi compatibility evidence, and user-perceived latency evidence remain later work.

The remaining gaps are no longer blockers for a dogfoodable M2 alpha; they are compatibility, session UX, and performance-evidence follow-ups. Notably, Ctrl+C remains local stop-following rather than true backend cancellation, session picker/fork UX is still partial, and performance evidence still needs user-perceived latency measurements.

## Milestone status

| M2 target | Status | Evidence | Notes / next action |
|---|---|---|---|
| Fullscreen TUI | `verified` | `crates/yach-cli/src/main.rs`, `crates/yach-ui/src/app.rs`, manual TUI smoke 2026-04-27 | Launch/quit terminal restoration passed manual smoke. |
| Transcript pane with streaming support | `verified` | `crates/yach-ui/src/transcript.rs`, `crates/yach-ui/src/app.rs`, manual TUI smoke 2026-04-27 | Deltas coalesce into assistant entries; PageUp/PageDown/End scrolling passed manual smoke. Large-transcript virtualization remains future work. |
| Tool output area | `implemented-unverified` | `crates/yach-ui/src/tool_area.rs`, `crates/yach-ui/src/app.rs` | Active tools and compact completion summaries exist. Expandable details/overflow handling remain future work. |
| Input composer | `verified` | `crates/yach-ui/src/input.rs`, app tests for submit/newline behavior | Uses `ratatui-textarea`; wraps/grows up to a cap; Enter submits and Ctrl+J / Shift+Enter insert newline. Still worth manual terminal validation across terminals. |
| Slash completion | `verified` | `crates/yach-ui/src/slash_commands.rs`, `crates/yach-ui/src/app.rs`, manual TUI smoke 2026-04-27 | Registry-backed slash commands are visible while typing `/`; j/k and arrows move; Tab accepts selected completion; Enter executes exact commands; prefix accidents such as `/clearance` do not execute. |
| Model selector | `verified` | `crates/yach-ui/src/model_selector.rs`, `crates/yach-ui/src/app.rs`, `crates/yach-adapter-pi-rpc/src/parse.rs`, `crates/yach-adapter-pi-rpc/src/serialize.rs`, manual TUI smoke 2026-04-27 | Backend-provided model list plumbing exists via stock RPC `get_available_models`; long lists scroll; j/k and arrows move; selection sends provider/modelId and updates current model only after backend confirmation; status bar shows actual model name. |
| Thinking control | `partial` | `crates/yach-ui/src/thinking_level.rs`, `crates/yach-ui/src/app.rs`, `crates/yach-proto/src/lib.rs` | UI/protocol path exists. Needs stronger adapter test/live evidence that stock Pi accepts the command. |
| Session picker | `partial` | `crates/yach-ui/src/session_picker.rs`, `crates/yach-ui/src/app.rs` | Picker exists but is based on default/observed session ids, not a real recent-session/session-tree source. |
| Session clone/fork | `partial` | `crates/yach-ui/src/app.rs`, `crates/yach-adapter-pi-rpc/src/serialize.rs` | Ctrl+F clones current branch through stock RPC `clone` after at least one user message. Full fork-from-entry, cloned-session visibility, and tree navigation are deferred to M3/session compatibility. |
| Status bar | `verified` | `crates/yach-ui/src/status_bar.rs`, `crates/yach-ui/src/layout.rs`, manual TUI smoke 2026-04-27 | Shows model/session/status/thinking/compaction fields at bottom; actual backend model name is shown after startup state refresh/selection confirmation. True compaction visibility remains incomplete because transcript compaction count is currently stub-like. |
| Basic performance instrumentation | `partial` | `crates/yach-ui/src/perf_metrics.rs`, `crates/yach-ui/src/perf_overlay.rs`, `docs/benchmarks/baseline-2026-04-23.md` | Render duration/total renders are tracked. No p95/p99, startup, keypress-to-paint, heavy tool, large transcript, or Pi comparison evidence yet. |

## Empirical notes

Recent validation reported:

- `just test` passed across the workspace after the hardening fixes.
- `just lint` passed with Clippy `-D warnings` after the hardening fixes.
- `just run print-capabilities` printed stock RPC capabilities including prompt streaming, dialogs, notifications, status entries, widgets, and session forking.
- `just run smoke-pi-rpc` reported success for initialization, state/model/session/stats/messages/dialog smoke operations after correcting current-branch duplication to stock RPC's `clone` command and model selection to stock RPC's provider/modelId shape.
- `just run smoke-pi-rpc-prompt` and `just run smoke-pi-rpc-tool` passed in preflight smoke.

Manual fullscreen TUI smoke on `feat/m2-tui-hardening` reported:

- Pass: launch/quit terminal restoration, basic streaming, Ctrl+J multiline input, local Ctrl+C stop-following, slash commands/completion, selectors/perf except model caveats, and transcript scrolling.
- Not observed: live backend dialogs.
- Found and fixed in follow-up hardening: `Ctrl+M` is not a reliable model-selector binding because terminals commonly encode it as Enter/CR; blank/whitespace-only prompts could be submitted after that path; `Ctrl+F` initially used an invalid `fork_session` command and then the lower-level `fork` command with a session id rather than an entry id. Model selector access now uses `Alt+M`, blank prompts are ignored/cleared, current-branch duplication serializes as stock RPC `clone`, clone responses show `session cloned`, and cloning is blocked until the visible transcript has at least one user message so fresh sessions do not surface Pi's entry-not-found error. True fork-from-entry and visible session-tree confirmation remain broader session work.
- Follow-up basic-loop polish replaces the static model selector source with stock RPC `get_available_models`, sends `set_model` with provider/modelId, avoids optimistic local model changes until backend confirmation, refreshes backend state after TUI startup so the status bar can show the actual model name instead of the placeholder, displays model names in the status bar when available, keeps the selected model visible while scrolling long model lists, supports j/k plus arrow movement in list selectors, and renders dialog input/editor fields with `ratatui_textarea` so Unicode-safe cursor movement uses the same non-shifting cursor behavior as the normal composer.
- Follow-up basic-loop polish adds a readable `/help` overlay with vim-style `q` close support, documents j/k selector movement, and makes slash-command completion visible while typing `/` with Tab accepting the selected completion.
- Follow-up basic-loop polish adds `just run tui-dialog-smoke`, a scripted in-process TUI backend that requests confirm/input/select/editor dialogs for manual validation; editor dialogs now use reliable `Ctrl+J` newline and `Enter` submit semantics.

These checks validate compile-time/unit behavior, the stock RPC smoke path, and manual TUI/dialog dogfood behavior for the M2 basic loop. They do not yet provide user-perceived latency measurements or broader M3 compatibility evidence.

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
| Tier A dialogs in TUI | `verified` | Confirm/input/editor/select modes exist, have unit coverage, and passed manual `tui-dialog-smoke` validation. Editor uses Ctrl+J newline and Enter submit; input/editor rendering reuses `ratatui_textarea` cursor behavior for Unicode-safe edits. |
| Notifications/status/widgets/title | `implemented-unverified` | Adapter maps events; UI status/tool surfaces exist; rich component behavior remains out of scope for stock RPC. `/help` now uses a readable overlay. |
| Model/session switching/forking | `partial` | Backend-provided model list plumbing exists; basic session select paths exist; Ctrl+F duplicates the current active branch via stock RPC `clone` after at least one user message, with a status confirmation. Session picker does not yet show the cloned session; real fork-from-entry, recent-session list, tree navigation, stats/export remain incomplete. |
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

Accepted remaining alpha caveats after M2 verification:

- Session picker/fork UX remains intentionally limited: Ctrl+F duplicates current active branch through stock RPC `clone`, but cloned sessions are not yet visible in a real session tree/picker.
- Transcript virtualization and large-transcript performance are not proven.
- User-perceived latency measurements are still absent.
- Ctrl+C is local stop-following, not backend cancellation.

### Important alpha usability gaps

- Add visible session clone/fork confirmation and a real session list/tree so cloned sessions are discoverable from the picker.
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

M2 can now be treated as a verified alpha for normal dogfooding, with the caveats listed above.

Recommended next step: **broader compatibility evidence pass**.

Use `docs/project-os/compatibility.md` to expand evidence for Tier A/session/resource surfaces, then plan the M3 session tree/fork compatibility work from that evidence.
