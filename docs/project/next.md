# Next Work

Last updated: 2026-07-20

## Recommended Next Move

The MVP bar was declared met on 2026-07-16: every checkpoint item passes
live, including the 2026-07-16 sensitive-file verification (denied read of
.env.local with actionable guidance; search excludes denied files).

Shell execution v1 slices 1 and 2 are implemented: the bash tool
(cohort-consensus schema) behind the executor seam, host executor with
review-every-command default, parse-aware auto-run allowlists from
.yach/config.json, workdir validation, timeout clamping, process-group
kill, env stripping, bounded head+tail output persisted as tool payload,
and live output streaming (ToolCallOutput protocol event, line-buffered
and cap-shared with the persisted capture, rendered as a bounded tail
under the running tool row). First bash dogfood passed 2026-07-20 (cargo
check/build through review).

Dogfood hardening (from the first sesh sessions, 2026-07-21): the tool
loop budget is now a runaway backstop (200 total calls) instead of a
working cap, but budget exhaustion is still turn-fatal; adopt the
Pi-style graceful degradation — error tool results ("budget exhausted;
summarize and respond") that let the model wrap up instead of aborting.
Mid-turn context overflow likewise has no recovery (overflow compaction
only covers the turn's first request); both belong to the same
resilience pass. Owner-flagged (2026-07-21): the resilience pass should
start with design research on how the cohort handles provider-failure
recovery — retry/backoff policies (yach's fixed 2x1s/5s from #159 is a
stopgap), rate-limit-aware delays (Retry-After), stream-stall timeouts vs
time-to-first-token, partial-stream salvage, and where retries belong
(harness loop vs provider adapter vs SDK layer).

Owner-flagged (2026-07-22): context-tracker research. As the context
system builds out, dig into how the cohort harnesses track and update
their context meters — source of truth (provider usage fields on stream
events vs client-side estimates like yach's chars/4), update cadence
(per token, per round, per turn), and how tracker accounting stays
consistent with the compaction trigger's. Yach now refreshes per tool
round from the assembled continuation context; provider-reported usage
is the likely upgrade and ties into model-catalog hydration.

Recommended next move: the slice-1 leftovers — command permission-decision
evidence (the permission summary types are edit-shaped today) and
persisting the tool request before the review wait for durability across
crashes mid-review — then the context compaction design. The baseline
system prompt was cohort-checked and deliberately leaned out on 2026-07-20
(`docs/project/records/2026-07-20-baseline-prompt-cohort-check.md`); the
deeper prompt/instructions design pass and any tone/formatting tuning wait
for the aesthetics/UX sprint after core functionality stabilizes.

Owner-flagged from sesh dogfood (2026-07-21), needing design work rather
than quick fixes:

- **Approval model beyond review-everything.** Approving every edit and
  command is not sustainable as the only mode, especially as work trends
  toward long-running, largely autonomous sessions. Gating must remain
  possible — the need is a richer policy surface (per-tool/per-risk auto
  modes, session-scoped grants, the Codex/Claude Code sandbox-backed
  auto postures already sketched in the shell design's
  AutoReviewUnavailable seam), designed deliberately.
- **Mid-turn progress visibility.** Even with streamed round text (fixed
  2026-07-21) and tool rows, long turns are hard to follow; the cohort
  leans on plan/todo surfaces, richer tool grouping, and progress
  narration. Belongs with the UX sprint but may need loop support.

Owner-slated for that UX sprint (2026-07-20): expandable/collapsible tool
output rows — finished tool rows show a compact summary by default (today's
header + bounded tail) with a way to expand and inspect the full captured
output on demand, the cohort norm, so command output does not clog the
transcript but stays inspectable. Also slated (2026-07-21): a status-bar
design pass — the bar is too noisy (dogfood finding: the generated session
id pushed the context meter off-screen; the id is now trimmed to a tail as
a stopgap, but layout, what earns a slot, and overflow behavior need a
deliberate pass). Also slated (2026-07-22): an unfocused-input indicator —
the prompt cursor looks the same whether or not the terminal pane has
focus, which is confusing across tmux panes; the input should visibly
change (dimmed/hollow cursor or border) when the terminal reports focus
lost. And inline approvals — the pop-up review dialog adds friction for
routine file-edit and tool-call approvals; the cohort norm is an inline
prompt rendered in the transcript flow, keeping pop-ups for things that
genuinely warrant modal attention.

Owner-flagged (2026-07-22): cross-model dogfood coverage. All dogfood so
far has run Anthropic models; different model families trigger different
harness failure scenarios (tool-call shapes, thinking/empty-response
behavior, streaming patterns, truncation habits), so the dogfood rotation
should deliberately cycle other families — the chatgpt-subscription
provider path exists but is untested in real sessions — before assuming a
failure class is fixed rather than merely not elicited. Related
(2026-07-22): OpenAI's Responses API offers server-side compaction
(encrypted reasoning/state kept provider-side, as Codex uses natively);
harnesses that manage context client-side reportedly do worse on long
OpenAI-model runs, and a Pi extension re-enables the server-side path
(https://github.com/algal/pi-openai-server-compaction, with a
native-vs-text benchmark in its repo). When OpenAI models land in yach,
evaluate a provider-native compactor behind the existing NativeCompactor
seam — the seam was designed for exactly this kind of swap.

Owner-flagged for near-term thinking (2026-07-21, not yet scheduled):
model-catalog hydration. Four stopgaps now wait on it (max_tokens 32k,
context_window 200k, the curated /model list, Pi-style truncated-tool-call
recovery), and the hydration mechanism itself is a real design fork with
different freshness/offline/trust tradeoffs — cohort divergence: opencode
fetches models.dev metadata at runtime; Codex bakes a models.json catalog
into each release; Pi bundles a static in-code registry; the Anthropic
Models API serves ceilings live but is single-provider. Start the eventual
work with a design session on the hydration source, not the data shape.

The isolation landscape (OS sandboxing, containers, hermetic/virtual
filesystems) is deliberately open by owner decision: the seam keeps every
door open, the research record
(`docs/project/records/2026-07-16-execution-isolation-research.md`) is the
exploration input, and each direction gets its own design if pursued.

The deny-by-default design (`docs/superpowers/specs/2026-07-14-sensitive-file-deny-design.md`)
is implemented: a single NativeSensitivePathPolicy chokepoint consulted by
read/search/list and the edit engine, globset gitignore-style matching,
visible built-in default patterns with allow carve-outs, JSON config at
user/project scope with fail-closed invalid-pattern handling, recoverable
sensitive_path_denied tool failures, silent search/list exclusion with a
denied_paths_excluded evidence marker, and 0700/0600 session log
permissions. Cohort research showed no comparison harness ships a real
default deny, so this is ahead of common practice.

## Completed: 2026-07-14 Stale-Evidence Guardrails And Payload Persistence

PR #128 added baseline system-prompt guardrails and made recoverable edit
preview failures (target_exists, hash_mismatch, ...) return failed tool
results with actionable guidance instead of aborting the provider turn.
PRs #129/#130 implemented the session tool payload persistence design:
session logs persist provider-visible tool arguments and results, resumed
transcripts render tool rows through the live shaping path, and provider
requests include prior tool activity across turns and resume. Session store
benchmarks with content-bearing logs are recorded in
`docs/benchmarks/native-session-store-2026-07-14.md`.

## Completed: 2026-07-14 Live Dogfood Rerun

The live checkpoint rerun passed: session separation (#124) and list-path
visibility (#123) are confirmed fixed, cross-session `/resume` hydration
works, and credless launch now fails recoverably (#126). Remaining findings
are recorded in the checkpoint; stale evidence is first, resumed-transcript
tool-output display fidelity and list/search preview caps follow.

Why: the audit safety net, CI, session-store durability, in-memory native
runner transcript state, off-reactor startup session load, async-aware
extension scan/activation state, and extension host env hardening have landed.
The first native-runner structure extractions also reduced immediate backend
change risk. At this point, more refactoring is less important than proving the
native default can support a baseline MVP loop: launch, prompt, read/search/list
project files, approve exact/create edits, see recoverable failures, cancel, and
resume enough session state to continue work.

Use the checkpoint as a pass/fail discovery run. The immediate recheck should
verify that plain `yach tui` starts a fresh native session, `/resume` hydrates
only the selected prior session, `--resume` chooses the latest existing native
session, and the model no longer reasons from stale default-session evidence
after local dogfood files are deleted. Fix only P0/P1 dogfood blockers before
broadening platform work. Candidate blocker categories include provider setup
confusion, TUI review rough edges, resume/session UX gaps, missing or misleading
tool evidence, bad failure/status messages, and anything that makes the native
path unsuitable for daily coding.

Resolved 2026-07-14: the 2026-07-13 no-secret run found that credless
`yach tui` exited with `native provider setup failed` before first render
after the native provider became the default backend. The default TUI now
launches without provider credentials, surfaces the setup error as backend
status with relaunch guidance, fails prompts with that error instead of
fixture text, and passes the no-secret startup-profile check again. The live
dogfood rerun is the remaining gate.

Relevant sources:

- `docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md`
- `docs/superpowers/plans/2026-06-11-repository-audit-remediation.md`
- `docs/superpowers/specs/2026-06-02-extension-activation-manager-design.md`
- `docs/project/records/2026-06-03-mvp-convergence.md`
- `docs/superpowers/plans/2026-05-21-extension-runtime-first-slice.md`
- `docs/superpowers/specs/2026-05-20-extension-runtime-tool-replacement-design.md`
- `docs/superpowers/specs/2026-05-23-extension-install-host-lifecycle-design.md`
- `docs/superpowers/plans/2026-05-23-extension-local-install-records.md`
- `docs/benchmarks/extension-runtime-profile-2026-05-23.md`
- `docs/superpowers/specs/2026-05-12-extension-tool-registration-design.md`
- `docs/superpowers/plans/2026-05-12-extension-tool-registration.md`
- `docs/superpowers/specs/2026-05-18-native-provider-multi-round-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-18-native-provider-multi-round-tool-loop.md`
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/specs/2026-05-13-native-static-context-design.md`
- `docs/superpowers/plans/2026-05-13-native-static-context.md`

## Near-Term Alternative

### Context Compaction

Designed 2026-07-20:
`docs/superpowers/specs/2026-07-20-context-compaction-design.md`, grounded
in the cohort + research-landscape record
(`docs/project/records/2026-07-20-context-compaction-research.md`). Owner
decisions: two-tier architecture (summary checkpoints first, deterministic
tool-result masking as slice 2), a pluggable `NativeCompactor` seam for
novel approaches, session log never truncated, revisit-often posture.
Slice 1 is implemented (2026-07-21): checkpoint event and context rebuild
(#149); auto trigger with headroom accounting, summary pass, overflow
recovery, and thrash guard (#150); `/compact [instructions]` and the
status-bar context meter. Next: live dogfood past the threshold to judge
continuation quality, then the masking pre-pass (slice 2). Known
simplification to revisit: trigger accounting uses the chars/4 estimate
over assembled messages rather than provider-reported usage.

### Per-Turn Output Token Budget (Owner-Flagged Revisit)

The interactive `max_tokens` default is 32,000 (cohort-modal, within every
current Claude ceiling), owner-decided 2026-07-16 as a stopgap, not a
settled design. Revisit alongside a model-catalog design: per-model
ceilings would allow the Pi/Codex model-max posture (headroom is free on
Anthropic), and the provider loop should adopt Pi-style truncated-tool-call
recovery (error tool results plus loop continuation instead of a failed
turn). Research:
`docs/project/records/2026-07-16-max-output-tokens-research.md`.

### Session Tool Payload Persistence (Owner-Decided Policy Change)

The 2026-07-14 owner decision reverses the "provider results are bounded
context, not session evidence" clause: session logs should be the full
model-visible transcript so resume is replay-fidelity, not compacted
history. The draft design is
`docs/superpowers/specs/2026-07-14-session-tool-payload-persistence-design.md`:
additive `argument_content`/`result_content` fields on tool session events,
hydration rendering through the live shaping path, and tool-role messages in
resumed provider context. Implement after (or alongside) the guardrail
slices; the resume-context half also mitigates stale evidence after resume.

### Backend Structure Extraction

Continue move-only backend structure extraction in
`docs/superpowers/plans/2026-06-11-repository-audit-remediation.md`, starting
from the next cohesive `crates/yach-backend/src/native_runner.rs` responsibility
after `native_runner/extension_state.rs`, `native_runner/local_edit.rs`, and
`native_runner/session_state.rs`.

Why: the audit's structural concern remains real, but it is no longer the best
short-term path to MVP. Resume this as move-only work after the native dogfood
checkpoint identifies and clears baseline usability blockers.

### Extension Developer/Package UX

Write a focused extension developer/package UX design before implementing
templates, TypeScript/Rust host packaging, git refs, or npm adapter behavior.

Why: this work matters for the extensible harness vision, but stop/reload/live
status now give enough extension lifecycle surface for MVP convergence. Package
and developer UX should resume after the native default is usable for real work.

### Provider Tool Guardrails

Add provider-loop guardrails that reduce model-authored claims of local effects
when tool evidence is missing.

Why: dogfooding showed the backend loop is correct, but older prompts let the
model claim an edit after only reading. This is less important than extension
runtime work because the stronger prompt and existing tool evidence path
produce correct behavior, but the provider-loop harness can still become more
defensive.

Relevant sources:

- `docs/superpowers/specs/2026-05-18-native-provider-multi-round-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-18-native-provider-multi-round-tool-loop.md`

## Not Ready Without a New Spec

- Provider-advertised file mutation beyond the canonical exact/create edit
  schemas, extension-owned mutation tools, broad write/patch/delete/rename
  tools, or multi-operation edit atomicity.
- Process or shell execution tools.
- Network tools.
- Npm/git extension package adapters, developer templates,
  package-manager integration, or cross-language host packaging beyond the
  accepted manifest-first, metadata-first runtime/replacement design.
- A working auto-review reviewer/subagent runtime.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
