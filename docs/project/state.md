# Project State

Last updated: 2026-07-16

Policy change implemented (2026-07-14, PRs #129/#130): the owner decision
reversed the "provider results are bounded context, not session evidence"
clause of the accepted provider read/search content design. Session logs now
persist bounded provider-visible tool arguments and results
(`argument_content` / `result_content`), resumed transcripts render tool
rows through the live shaping path, and provider requests include prior
tool activity across turns and resume. Design:
`docs/superpowers/specs/2026-07-14-session-tool-payload-persistence-design.md`.
Statements below about evidence never persisting file bodies, search lines,
or directory dumps describe the pre-decision posture and are superseded by
that design. The native provider path also gained baseline stale-evidence
guardrails and recoverable edit tool failures (PR #128).

## Thesis

Yach is a minimal, extensible Rust-native coding harness. The validated
near-term shell is Pi-shaped and should be usable for real coding work out of
the box, but the durable product direction is a yach-owned Rust UI, protocol,
backend runtime, session model, tool loop, extension runtime, and file-first
resource system.

The Pi adapter crates and `--backend pi` were removed on 2026-07-16 (owner
decision): the reference backend served its purpose once the MVP bar was met
on native primitives, and comparison evidence is preserved in dated records
under `docs/benchmarks/`. Pi remains an inspiration, not a component.

## Current Posture

- `main` includes PR #24: native startup profiling, native-default TUI behavior, native-provider tool advertising, one-round provider tool continuation, and earlier native-backend branch wrap-up are merged.
- M0/M1/M2 foundations are considered verified enough for forward planning: workspace, protocol seed, Pi RPC adapter, TUI alpha loop, session/fork groundwork, and performance harness exist.
- The native backend is the only backend: plain `yach` starts an interactive native session, with `--backend native-fixture` as the provider-free dev/smoke path. The Pi adapter crates, `--backend pi`, the Pi-based `run` command, and the `smoke-pi-*` commands were removed on 2026-07-16.
- The MVP bar was declared met on 2026-07-16: every item in
  `docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md` passes
  live, including stale-evidence guardrails with recoverable tool failures
  (#128), session tool payload persistence with live-parity resume (#129,
  #130), budget-safe search (#132), and sensitive-file deny-by-default with
  config overrides (#134). The active posture is daily dogfood use plus
  post-MVP scope selection, starting with a process/shell execution design.
- Extension lifecycle/runtime primitives remain sufficient; further extension
  packaging, template, npm/git adapter, or TypeScript/Rust host ergonomics work
  should wait unless it directly blocks daily use.
- Native sessions now have an append-only JSONL store seam, restart-safe turn indexing, provider transcript resume context, low-frequency session metric events, and append/load/projection benchmark coverage.
- Native read-only project inspection now has backend primitives for path metadata, explicit local-only text context packages, bounded search, a metadata-only project path tool, a backend-only autonomous tool loop that records session evidence while shaping safe provider tool results, backend-only continuation mapping into adapter-ready provider request input, explicit native-provider one-round handling for completed safe read-only tool calls, and schema-only `project_path_info` advertising on explicit native-provider initial requests through `yach.provider_tool_advertising.v1`. Continuation requests strip that advertising so the one-round/fail-closed boundary remains intact.
- Extension-owned tool registration now has a manifest/catalog path, versioned host registration protocol, process-host registration boundary, extension-owned executor routing through the native tool workflow, and policy-gated schema-only provider advertising for safe read-only metadata tools. Extension hosts remain off the default first-frame path; extension-runtime startup profiling shows zero scan starts before first render for both one installed inactive extension and a 50-manifest package-root fixture on the local 100-sample run.
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
- Native startup profiling shows traced Rust `main` to first render is sub-millisecond p95 on the local benchmark run; extension discovery and activation stay off the default first-frame path in the current benchmark evidence.

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

- A 2026-06-11 repository audit identified session durability, CI enforcement,
  and native-runner disk reloads as the first reliability track. The safety-net
  and session-store durability slices have landed: CI now runs fmt/clippy/tests,
  native session appends fsync, corrupt JSONL lines load with warnings, the
  native runner keeps prompt transcript state in memory after startup, and
  startup session load now runs through `spawn_blocking` while preserving
  warning/error events, and extension scan/activation runner state now uses
  async-aware locks. Extension host processes now start with an explicit
  allowlisted environment instead of inheriting provider/API secrets by default.
  Backend structure extraction has started with move-only
  `runner/extension_state.rs`, `runner/local_edit.rs`, and
  `runner/session_state.rs` modules for extension
  scan/activation/lifecycle state, local edit prepare/decision handling, and
  native session log loading/presentation. More move-only extraction remains in
  the remediation plan, but the short-term priority is now a fresh native MVP
  dogfood checkpoint and fixing the first baseline usability blocker it finds.
  The remediation plan is
  `docs/superpowers/plans/2026-06-11-repository-audit-remediation.md`; the MVP
  checkpoint is
  `docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md`.
- Large files, especially `crates/yach-backend/src/runner.rs`, now carry
  enough responsibility that extraction is warranted after session correctness
  and CI are stable.
- Native-provider dogfood can grow into a chat-only path unless tools, resources, persistence, cancellation, and error semantics stay yach-owned.
- Extension runtime work can become a side quest if it continues past the point
  needed for a minimal extensible MVP. The next slices should prioritize native
  dogfood blockers over broader extension packaging or developer UX.
- Local project data exposure needs deny-by-default policy until provider-visible resource rules are explicit.
- Planning docs can become stale if live summaries accumulate history instead of pointing to records.
- Performance claims should stay scoped to measured surfaces; historical Pi comparison evidence lives in dated benchmark records and is no longer reproducible in-repo after the 2026-07-16 adapter removal.

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

The provider-visible read/search content implementation now adds canonical
`read_text_file`, `search_project`, and `list_project_paths` built-ins for the
explicit native-provider path. These use a separate `ReadsLocalContent`
risk/policy path, yach-owned project-root resolution, bounded provider results,
redacted durable session evidence, and provider-visible tool result shaping.
`project_path_info` remains metadata-only, and content tool evidence does not
persist file bodies, search match lines, directory dumps, or raw queries. This
is not sufficient for shell/process tools, broad mutation, network tools,
extension-owned content tools, indexing, LSP, or MCP integration.

Native-provider dogfooding showed the one-round continuation boundary was the
main blocker for practical agent edits. The native-provider path now has a
backend-owned multi-round tool loop for provider-visible read/search/list and
exact/create edit tools. The loop preserves yach-owned validation, permissions,
review, execution, redacted evidence, provider continuation, and
provider-visible tool schemas across rounds. It remains registry-oriented so
future extension-owned tools and explicit built-in replacement can participate
without changing provider-loop semantics. The loop has no artificial default
round cap; configured loop-stop behavior remains available for development or
policy budgets. This is not sufficient for the full extension runtime,
install/package UX, shell/process tools, network tools, broader mutation tools,
sandboxing, or auto-review runtime; those need focused follow-up designs.

The accepted extension runtime and tool replacement design now frames the next
extension work: manifest-first, process-hosted extensions; Pi-like install refs
without making npm part of the Rust runtime; TypeScript and Rust hosts over the
same stdio protocol; post-first-paint discovery/activation; provider-turn tool
availability only after executable registration; and explicit built-in
replacement policy with provenance. Package roots, manifest index/cache,
post-first-paint scan, persistent metadata-tool host invocation,
provider-turn catalog resolution, explicit alias/replacement policy, extension
static-context file contributions, and startup/activation profiling are
implemented as conservative runtime primitives. The current profiling evidence
shows one installed inactive extension and a 50-manifest package-root fixture
start scanning only after first render, with no host spawn before first render.
This is not sufficient for install UX, real host launch, hot reload, broad
mutation tools, shell/process tools, network tools, hidden system prompt
mutation, in-process plugins, sandboxing, or implicit replacement.

A draft extension install and host lifecycle design now narrows the next
extension-runtime work. It recommends staged install records plus host lifecycle:
local-path user/project install records first, then a persistent process-backed
host transport and activation manager, then developer templates, then git/npm
package adapters. The draft keeps install/update/package-manager work out of
startup and preserves the post-first-paint activation boundary.

Local-path extension install records are implemented for user/project scopes.
The CLI can install, remove, enable, disable, list, and doctor local records;
npm/git refs are parsed but remain unavailable adapters. Enabled records feed
the existing post-first-paint manifest scan path without spawning hosts before
first render.

A draft extension activation manager design now narrows the next runtime work.
It recommends a runtime-owned activation state machine, active-registration
projection, background metadata activation after first paint, reload/stop
behavior, and categorical diagnostics. Existing process host/session primitives
are treated as lower-level transport/session pieces, so the next implementation
should focus on manager ownership rather than another transport rewrite.

The first activation-manager implementation slice adds backend activation
diagnostic state for installed, discovered, and blocked extension records and
surfaces that state through `yach extension list` / `doctor`. Discovered
manifests still have zero active registrations until a host activates and
registers tools, so provider-visible extension tools continue to come only from
existing active executable registrations.

Background metadata activation now starts eligible user-scoped
`postFirstPaint` extension hosts after manifest scan, keeps host startup off the
first-render path, stores active registry/executor snapshots for future provider
turns, and routes active extension metadata tools through live stdio host
sessions. Project-scoped extensions remain blocked pending a trust design.
The live activation snapshot also has a backend stop operation that moves an
active extension to `stopped`, removes its provider-visible registry entries,
and drops executor routes so provider turns no longer see or invoke those
tools. The native protocol and TUI now expose a negotiated extension lifecycle
capability plus `/extension-stop <selector>` and `/extension-reload <selector>`.
Stop routes through the running backend's live activation snapshot and reports
completed/not-found/not-active outcomes. Reload resolves the already-discovered
manifest record, schedules host restart work off the backend event loop, removes
stale registry/executor routes before reactivation, and reports completed,
not-found, not-active, or failed outcomes. Live runtime diagnostics are now
available through a protocol snapshot request/response and `/extension-status
[selector]` in the TUI. The TUI also requests a selector-specific diagnostic
snapshot after stop/reload finishes so users can see active/stopped/failed
state, generation, errors, and registered/provider-visible tool names from the
running backend without rescanning from a separate CLI process.

The first extension-first dogfood bundle is implemented. The bundled
`yach.hashline` host uses the public v2 extension protocol to replace native
read/edit advertisement as an all-or-none pair, request bounded file content,
and submit generic multi-file edit proposals. Core still owns path policy,
sensitive-file checks, preview/review, atomic apply with rollback, durable
evidence, and continuation results. A fresh installed `yach` materializes the
versioned bundled manifest under the user's yach directory after first render,
seeds a persisted bundled install record, and launches the host through the
current executable; no separate extension install or PATH lookup is required.
`yach extension list` / `doctor` include the bundle, and persisted
enable/disable state selects the hashline or native pair on the next launch.
Deterministic unit/integration coverage and the stdio RPC scenario exercise the
composed read -> tagged output -> one reviewed edit flow. Actual TUI smokes
confirmed both the active hashline pair and disabled native fallback.

The active MVP convergence record is
`docs/project/records/2026-06-03-mvp-convergence.md`. It defines the near-term
bar as a fast native default that can run real coding sessions with provider
prompts, read/search/list tools, exact/create edit tools, review, continuation,
basic persistence/resume, and recoverable failures. Work that does not move
that usability bar should be deferred unless it blocks MVP dogfooding directly.
The active dogfood checklist is
`docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md`; use it to
record the next live native-provider run and choose the first blocker.

## Currently Relevant Records

- `docs/superpowers/plans/2026-06-11-repository-audit-remediation.md`
- `docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md`
- `docs/project/records/2026-06-03-mvp-convergence.md`
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
- `docs/superpowers/specs/2026-05-18-provider-read-search-content-design.md`
- `docs/superpowers/plans/2026-05-18-provider-read-search-content.md`
- `docs/superpowers/specs/2026-05-18-native-provider-multi-round-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-18-native-provider-multi-round-tool-loop.md`
- `docs/superpowers/specs/2026-05-20-extension-runtime-tool-replacement-design.md`
- `docs/superpowers/plans/2026-05-21-extension-runtime-first-slice.md`
- `docs/superpowers/specs/2026-05-23-extension-install-host-lifecycle-design.md`
- `docs/superpowers/plans/2026-05-23-extension-local-install-records.md`
- `docs/superpowers/specs/2026-06-02-extension-activation-manager-design.md`
- `docs/benchmarks/extension-runtime-profile-2026-05-23.md`
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/benchmarks/native-edit-profile-2026-05-15.md`
- `docs/project/records/2026-05-09-planning-flow-cutover.md`
