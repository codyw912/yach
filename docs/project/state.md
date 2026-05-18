# Project State

Last updated: 2026-05-17

## Thesis

Yach is a Rust-native coding harness. The validated near-term shell is Pi-shaped, but the durable product direction is a yach-owned Rust UI, protocol, backend runtime, session model, tool loop, and file-first resource system.

Pi remains useful as a compatibility/reference backend. It is not the long-term architecture target.

## Current Posture

- `main` includes PR #24: native startup profiling, native-default TUI behavior, native-provider tool advertising, one-round provider tool continuation, and earlier native-backend branch wrap-up are merged.
- M0/M1/M2 foundations are considered verified enough for forward planning: workspace, protocol seed, Pi RPC adapter, TUI alpha loop, session/fork groundwork, and performance harness exist.
- Native backend work is now the default `yach tui` path. Pi remains available only as an explicit comparison/reference backend via `--backend pi`; Native MVP work is framed around yach-owned backend primitives rather than Pi compatibility.
- Native sessions now have an append-only JSONL store seam, restart-safe turn indexing, provider transcript resume context, low-frequency session metric events, and append/load/projection benchmark coverage.
- Native read-only project inspection now has backend primitives for path metadata, explicit local-only text context packages, bounded search, a metadata-only project path tool, a backend-only autonomous tool loop that records session evidence while shaping safe provider tool results, backend-only continuation mapping into adapter-ready provider request input, explicit native-provider one-round handling for completed safe read-only tool calls, and schema-only `project_path_info` advertising on explicit native-provider initial requests through `yach.provider_tool_advertising.v1`. Continuation requests strip that advertising so the one-round/fail-closed boundary remains intact.
- Extension-owned tool registration now has a manifest/catalog path, versioned host registration protocol, process-host registration boundary, extension-owned executor routing through the native tool workflow, and policy-gated schema-only provider advertising for safe read-only metadata tools. Extension hosts remain off the default first-frame path; inactive-extension startup profiling shows `tui_first_render_end_since_main` p95 delta of +0.024ms on the local 100-sample run.
- Native static context assembly now supports core `AGENTS.md` discovery plus explicit project-root `.yach/APPEND_SYSTEM.md`, injects accepted context into native provider requests with redacted evidence, and keeps extension static context limited to manifest metadata for a later contribution slice.
- Native edit transactions now have merged backend primitives for preview,
  guarded apply, redacted session evidence, and a backend-local harness.
  `NativeEditEngine::preview` validates create/modify requests, mints
  yach-owned edit IDs, enforces project-root and metadata path policy, rejects
  symlink paths, checks expected hashes, applies exact hunks in memory, rejects
  duplicate targets, and returns bounded diff summaries without writing files.
  `NativeEditEngine::apply` remains crate-internal, consumes a prepared
  transaction, hard-rejects multi-operation apply, performs guarded
  create/modify writes, and returns structured apply metadata. The harness
  records prepared/finished edit evidence without registering mutation tools or
  advertising edit/write capabilities to providers. Native edit profiling now
  has Criterion coverage and a `yach-bench native-edit-profile-report` mode for
  preview, apply, evidence summary, session append, and end-to-end harness
  phases.
- The planning-flow cutover is complete: `docs/project/` is the active planning fast path, while cockpit and Project OS docs are reference-only.
- Native startup profiling shows traced Rust `main` to first render is sub-millisecond p95 on the local benchmark run; extension discovery and activation should stay off the default first-frame path.

## Architecture Beliefs

- `yach-proto` is the UI/backend seam.
- The TUI should not speak Pi RPC, provider SDK, or native backend internals directly.
- Yach owns sessions, tools, resources, protocol events, and user-facing runtime semantics.
- Provider libraries can sit below yach-owned seams, but they do not own sessions, tool execution, or canonical transcript state.
- File-first configuration and inspectable local state remain product values.
- Compatibility and performance claims need evidence, not assumptions.

## Profiling And Traceability

Yach should be designed so performance work can use real tools and evidence.

- Core primitives should have clear measurable boundaries: provider requests, stream handling, tool validation/execution, file read/search/edit, verification commands, session append/load/projection, and TUI render/update.
- Correlation IDs should flow across layers where they exist: session ID, turn ID, entry ID, tool request ID, provider response ID, and future edit or verification IDs.
- Instrumentation should stay low-noise. Canonical session logs should keep durable evidence, while high-frequency metrics should be summarized or stored separately if they become necessary.

For each Native MVP slice, ask: can this be benchmarked in isolation, and can we explain a slow run after the fact?

## Current Risks

- Native-provider dogfood can grow into a chat-only path unless tools, resources, persistence, cancellation, and error semantics stay yach-owned.
- Local project data exposure needs deny-by-default policy until provider-visible resource rules are explicit.
- Planning docs can become stale if live summaries accumulate history instead of pointing to records.
- Same-machine Pi comparison evidence is still imperfect, so performance claims should stay scoped to measured surfaces.

## Plan Sufficiency

The current planning surface is sufficient to continue Native MVP implementation from the accepted MVP definition.

The accepted provider tool advertising plan is sufficient for schema-only `project_path_info` advertising behind explicit native-provider opt-in. It is not sufficient for file writes, process execution, network tools, extension runtime implementation, provider-native tool-result block support, or additional default-backend changes. Those need dedicated Superpowers specs/plans and explicit approval.

The accepted extension tool registration design and implementation plan are now implemented for the first extension-owned tool contribution surface. They are not sufficient for broader extension runtime work such as context providers, install UX, hot reload, higher-risk tools, file mutation, shell/process tools, network tools, or approval UI; those need focused specs/plans.

The accepted native static context design and implementation plan are now implemented for core `AGENTS.md`, project-root `.yach/APPEND_SYSTEM.md`, provider request injection, redacted evidence, extension manifest metadata placeholders, and assembly benchmarks. They are not sufficient for extension-provided context activation, project-file selectors, prompt replay, or broader extension runtime behavior; those need focused specs/plans.

The accepted native edit transactions, edit evidence, and benchmark/trace
designs now cover preview/apply/harness behavior plus local Criterion and
report-mode profiling. They are sufficient as the basis for designing local
CLI/TUI edit access on top of the native edit transaction/evidence boundary.
They are not sufficient for provider-advertised edit tools, extension-owned
mutation tools, production edit tracing, delete/rename, shell/process tools,
network tools, verification actions, or multi-operation atomicity; those need
focused follow-up specs/plans.

The accepted native edit local access design and implementation plan frame
local edit UX as the first consumer of a generic permission/reviewer pipeline.
The generic permission model, durable permission evidence, and backend-owned
edit access facade are implemented, and yach-owned protocol DTOs/events now
cover local edit prepare, preview, decision, and finish messages. The native
runner now wires those events to the backend facade, persists redacted
permission/edit evidence, advertises local edit capability to the UI, and keeps
provider-visible mutation unavailable. The TUI has a temporary `/debug-edit`
manual harness that gates on local edit capability, emits local prepare
requests, correlates preview and finish responses, supports apply/reject review
decisions, and avoids exposing edit/write tools to providers. This is not the
product edit surface; actual edit usage should come through agent-selected
tools once mutation tools are explicitly designed and exposed. Cross-crate
verification for the local edit access work now passes workspace tests, strict
workspace clippy, provider replay coverage for ignoring local edit evidence,
provider tool advertising coverage, and local edit protocol JSONL compatibility.
The accepted native edit local access plan is complete. It is not sufficient
for the real agent edit tool surface, a working auto-review agent, sandboxing,
provider-visible mutation, extension-owned mutation tools, or broad
permission/config UI; those need follow-up designs.

The native agent edit tool surface implementation now provides policy-gated
provider-visible canonical `edit_text_file` and `create_text_file` schemas for
the native-provider path. Provider-originated edit calls route through
yach-owned schema validation, permission routing, `NativeEditAccess`
preview/apply/reject, redacted tool/edit evidence with provider-call
correlation, and bounded provider continuation results. The temporary
`/debug-edit` harness remains a manual local test surface, not the product edit
surface. This is not sufficient for broad `write`/patch/delete/rename tools,
extension-owned mutation, shell/process tools, network tools, sandboxing, or a
working auto-review runtime.

The production edit tracing implementation now records bounded durable
`EditTraceRecorded` session events for provider-originated agent edits. Trace
records correlate validation, normalization, permission, preview, review wait,
apply/reject, result shaping, and provider continuation phases through a
`NativeEditTraceId` plus existing tool request, provider call, permission,
preview, and transaction IDs. Trace records are ignored by provider transcript
projection and remain diagnostic-only; redacted tool/edit evidence remains the
authoritative record of local effects. This is not sufficient for broader
mutation tools, extension-owned mutation, auto-review runtime, sandboxing, or
provider-visible read/search content tools.

## Currently Relevant Records

- `docs/superpowers/specs/2026-05-09-planning-flow-cutover-design.md`
- `docs/superpowers/plans/2026-05-09-planning-flow-cutover.md`
- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/plans/2026-05-10-native-session-store-resume-metrics.md`
- `docs/superpowers/plans/2026-05-11-native-read-search-context.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-tool-loop.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-provider-continuation-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-provider-continuation.md`
- `docs/superpowers/specs/2026-05-11-native-provider-one-round-tools-design.md`
- `docs/superpowers/plans/2026-05-11-native-provider-one-round-tools.md`
- `docs/superpowers/specs/2026-05-11-native-provider-tool-advertising-design.md`
- `docs/superpowers/plans/2026-05-11-native-provider-tool-advertising.md`
- `docs/superpowers/specs/2026-05-12-extension-tool-registration-design.md`
- `docs/superpowers/plans/2026-05-12-extension-tool-registration.md`
- `docs/superpowers/specs/2026-05-13-native-static-context-design.md`
- `docs/superpowers/plans/2026-05-13-native-static-context.md`
- `docs/superpowers/specs/2026-05-13-native-edit-transactions-design.md`
- `docs/superpowers/plans/2026-05-13-native-edit-transactions-preview.md`
- `docs/superpowers/plans/2026-05-14-native-edit-transactions-apply.md`
- `docs/superpowers/specs/2026-05-14-native-edit-evidence-harness-design.md`
- `docs/superpowers/plans/2026-05-14-native-edit-evidence-harness.md`
- `docs/superpowers/specs/2026-05-15-native-edit-benchmark-trace-design.md`
- `docs/superpowers/plans/2026-05-15-native-edit-benchmark-trace.md`
- `docs/superpowers/specs/2026-05-15-native-edit-local-access-design.md`
- `docs/superpowers/plans/2026-05-15-native-edit-local-access.md`
- `docs/superpowers/specs/2026-05-15-native-agent-edit-tool-surface-design.md`
- `docs/superpowers/plans/2026-05-15-native-agent-edit-tools.md`
- `docs/superpowers/specs/2026-05-17-production-edit-tracing-design.md`
- `docs/superpowers/plans/2026-05-17-production-edit-tracing.md`
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/benchmarks/native-edit-profile-2026-05-15.md`
- `docs/project/records/2026-05-09-planning-flow-cutover.md`
