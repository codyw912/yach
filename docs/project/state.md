# Project State

Last updated: 2026-09-03

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

- `docs/project/roadmap.md` records the approved outcome sequence. M1,
  **Evidence-driven dogfood loop**, is active: establish a representative,
  repeatable portfolio, resolve its first blocker cohort, and re-run it before
  broadening work.
- The native foundation is strong, but Yach is not yet close to the usability
  bar for daily work. The 2026-07-16 native MVP declaration proves component
  capability—startup, provider prompts, tools, review, persistence, and
  recovery—not sustained effectiveness, long-session resilience, calm
  interaction, safe autonomy, or independent-user readiness.
- The native backend is the only backend: plain `yach` starts an interactive
  native session, with `--backend fixture` as the provider-free development
  and smoke path. Pi remains comparison evidence, not a component.
- Extension lifecycle/runtime primitives remain sufficient; further extension
  packaging, template, npm/git adapter, or TypeScript/Rust host ergonomics work
  should wait unless it directly blocks daily use.
- Sessions use append-only JSONL with restart-safe turn indexing, provider
  transcript resume context, low-frequency metrics, and load/projection
  benchmarks. Defaults live outside repositories under project-keyed user
  state at `~/.yach/sessions/<slug>--<canonical-path-sha256>/`; canonical raw
  path hashing prevents project collisions and keeps worktrees separate.
  `YACH_SESSION_DIR` overrides the directory explicitly.
- Approval mode foundations are live. `review` remains the conservative
  default; `accept-edits` auto-applies only hash-checked project edit
  transactions while bash retains allowlist/ask behavior. Explicit
  `full-access` auto-applies those edits and runs host bash without ordinary
  review after a host-danger confirmation. It is never persisted and resets on
  restart or transcript switch. Modes negotiate over protocol, produce durable
  session and concrete permission evidence, and remain visible in `/status`
  and the status bar. Repository config cannot grant shell/environment
  authority, and permission config is a protected edit path.
- Closely watched `full-access` dogfood drove the first usability correction
  bundle. Applied edits now show bounded changed lines and the next hashline
  snapshot tag. Thinking level is backend-owned rather than cosmetic: explicit
  selections reach provider request controls, persist in session evidence, and
  update the global user-config default for new sessions in any project. A
  resumed session's recorded value wins, while users with no configured
  default keep the prior provider request shape. The TUI no longer captures the
  mouse or uses the alternate screen; completed prior turns move into
  terminal-native scrollback when the next turn starts. Hashline now
  distinguishes unknown, ambiguous, and path-mismatched tags, and proposed
  post-edit content mints the next live tag while retaining live-resource stale
  checks.
- The model-default and session-model-state slice is implemented. User defaults
  and exact session targets are separate; preserving typed
  `~/.yach/config.toml` owns model and thinking defaults; optional immutable
  connection keys disambiguate same-provider defaults while session evidence
  retains UUIDs. Startup, resume, session switch, TUI, headless, and RPC use one
  fail-closed activation path with correlated session/default outcomes.
  `active-model.json` migrates idempotently and the old runtime seam is removed.
  Protocol v0.3.0 carries structured model state, explicit activation intent,
  and prompt-attempt reset. Workspace tests, strict lint, executable RPC
  scenarios, and an isolated
  normal-TUI save-default/restart smoke pass. Design:
  `docs/superpowers/specs/2026-08-26-model-defaults-session-state-design.md`.
- Provider-attempt reliability is implemented. Typed, baked-ID-selected error
  dialects preserve conservative generic fallback; bounded provider metadata
  carries HTTP status, recognized code, Retry-After, timeout phase, and
  classification source without raw payloads. The attempt executor owns three
  total attempts with cancellation-aware 1s/2s waits and a 30s delay budget,
  hard-4xx guards, OpenAI prefix resume, restart reset, and exact-once evidence.
  Protocol v0.3 strictly rejects version mismatch before Ready and adds
  negotiated prompt-attempt reset; TUI/headless retract exact UTF-8 suffixes and
  desynchronize fail-closed on invalid control data. Design:
  `docs/superpowers/specs/2026-08-28-provider-attempt-reliability-design.md`.
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
  the remediation plan; schedule it when M1 dogfood evidence shows backend
  change risk blocking a measured fix. The remediation plan is
  `docs/superpowers/plans/2026-06-11-repository-audit-remediation.md`; the
  historical MVP checkpoint is
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

The current planning surface is sufficient to start roadmap M1: write the
dogfood portfolio design described in `next.md`, run it, and resolve the first
recorded blocker cohort. It is not sufficient for any later milestone; M2-M6
each need their own focused designs once M1 evidence selects the work.

Accepted specs and plans under `docs/superpowers/` remain source material for
the surfaces they implemented (provider tool loop, edit transactions and
evidence, permissions and approval modes, static context, compaction,
extension runtime and the bundled hashline pair, model defaults, provider
reliability). They do not authorize new product scope by themselves, and
their historical "not sufficient for X" lists are superseded by the roadmap's
milestones and non-goals.

One standing constraint carries forward: the crates.io release flow is
formalized but blocked. Packaged `yach-backend` cannot build against registry
Rig because opaque compaction input, ordered raw Responses output, caller-built
native requests, ChatGPT auth guard/fencing, and model listing remain
vendor-only; a version bump alone is not an unblocker. Research:
`docs/project/records/2026-08-24-rig-upstream-reconciliation.md`. The
2026-06-03 MVP convergence and dogfood checkpoint records are historical
inputs to the M1 portfolio, not the active checklist.

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
