# Next Work

Last updated: 2026-08-24. The full open-item queue lives in `board.md`
(one line per item with status); this file carries narrative and
rationale. Sections below the 2026-08-03 block predate the
2026-07-31..08-03 arc — the board and `records/` are authoritative
where they disagree.

## Recommended Next Move (2026-08-24)

The first normal-TUI provider dogfood session exercised the bundled hashline
pair in another repository and exposed concrete seam failures rather than a
need for more extension abstraction. The patch contract did not make
`+`-prefixed PUT bodies sufficiently explicit, live registration lost the
manifest version in provenance, extension-authored failures were durably
classified as completed, and the review/transcript presentation obscured
decisions and successful tool evidence.

Those blockers are fixed on the current stack. Hashline now gives actionable
patch grammar before and after malformed calls; `tool.result` carries typed
completed/failed status with a required categorical failure reason; live
registration preserves the manifest version; and reviews use a vertical
Approve/Reject stack with Up/Down and `j`/`k` selection. Edit previews use
changed hunks with four lines of context. Prompts and each tool call/result have
separate full-width surfaces with configurable padding and outcome backgrounds;
assistant prose keeps a distinct bright `•` marker, and completed tools expose
bounded output previews. One strict Pi-inspired theme owns all UI colors and
transcript spacing, with fixed dark defaults plus personal, project, and
explicit-file overrides. Real fixture and provider normal-TUI smokes confirmed
the themed transcript flow, visible read output, review navigation in both
directions, approved-edit write behavior, and the distinct final assistant
response. Follow-up owner dogfood also fixed opaque extension-resource failures
and status-line information density: missing files retain native recovery
guidance without duplicate excerpts; session ID moved to `/status`; and the
always-visible line now pairs model with thinking level and context percentage
with the configured model window.

The malformed-patch recovery path now has a deterministic RPC provider
scenario: the mock provider omits a required `+`, observes the typed
`malformed_patch` result and correction guidance, retries with a valid patch,
and reaches one reviewed apply without an early write. Resume owner dogfood in
the normal TUI and take the next naturally observed blocker; a longer
multi-tool session remains useful, but manually coercing a real provider into a
malformed call is not a correctness gate.

The latest owner dogfood also removed repository-local session state. Default
TUI, headless, and RPC transcripts now live under collision-resistant,
project-keyed `~/.yach/sessions/` directories, so using Yach does not create a
`.yach` folder or a gitignore obligation. Existing project-local logs are a
clean cutover and remain untouched. The same run showed that two malformed
edits were model-authored `CUT N.=M:` calls, but the tool incorrectly blamed
missing PUT `+` prefixes; hashline now reports the exact CUT grammar error.

Approval modes slice 1 is implemented. Repository configuration can no longer
grant shell/environment authority, permission files are protected from
provider edits, and project mode preference lives in private user state.
Negotiated correlated protocol events expose conservative-default `review` and
`accept-edits`. `/approval` now opens a keyboard picker and works during active
turns; the backend-owned session cell applies a change to future tool requests,
including later rounds in the same turn, without changing a pending review.
All status surfaces show the active mode. `accept-edits` auto-applies only
hash-checked Yach edit transactions—host bash retains its user-state
allowlist/ask policy. Changes persist durable session evidence and
unnegotiated requests fail explicitly.

The next approval slice should add explicit session-only `full-access` first.
It directly removes the remaining autonomous-work blocker by allowing host
commands without ordinary review, while stating plainly that Yach has no
process/filesystem sandbox. The mode is never persisted, requires a danger
confirmation on every activation, keeps environment hygiene and execution
bounds, and records the exact policy reason for every command. `yach run
--full-auto` should converge on the same backend mode instead of auto-clicking
reviews. Scoped grants, `plan`, auto-review, and sandboxing remain separate
follow-ups. Proposed design:
`docs/superpowers/specs/2026-08-24-full-access-approval-design.md`; foundational
design: `docs/superpowers/specs/2026-08-24-approval-modes-design.md`; research:
`records/2026-08-24-approval-modes-cohort-research.md`.

Release flow is now formalized with the agreed evidence policy and remains
blocked on vendored Rig. Issue
[#2269](https://github.com/0xPlaygrounds/rig/issues/2269) is partially upstream:
merged #2295 in Rig 0.42 fixes blocking replay's message `phase`. It does not
provide opaque compaction input, terminal ordered raw `response.output`, or
caller-built native Responses requests; ChatGPT auth guard/fencing and model
listing also remain vendor-only. Isolated 0.42 and current-main probes still
fail. Keep publication blocked and upstream the three coherent patch families
before a release; a 0.42 migration alone is not an unblocker. Record:
`records/2026-08-24-rig-upstream-reconciliation.md`.

## Recommended Next Move (2026-08-09)

The OpenAI Responses provider-native compactor, one-shot private profile-runner
boundary, portable environment scrub, and structured provider-failure
accounting have landed (#239-#241). The first two matrix attempts remain
excluded diagnostic evidence: one exposed the Bash `compgen` portability
defect, and one exposed fail-open accounting after provider quota exhaustion.

The clean `2026-08-09-responses-native-compactor-rerun2` matrix is complete:
124/124 valid cells passed, with one zen-deepseek provider-invalid attempt
excluded. That attempt completed two turns and two compactions before an
intermittent `invalid_request`; the other four identical repeats completed end
to end. The run consumed 1,238,547 input and 71,075 output tokens over 5,610
cell-seconds (1h 33m 30s). It found no behavioral regression. Do not patch or
rerun it.

The release-evidence policy changed after reviewing this cost against the
matrix's job:

- Normal releases use deterministic checks, `just eval-validate`, and one
- pinned live `just eval-gate` pass over every task. The live portion has
  a two-minute target; fixed repeated sampling is not on the green path.
- A first behavioral miss gets two targeted reruns in fresh workspaces. Block
  only when at least two of three valid attempts fail.
- Provider-invalid attempts do not vote. Retry once, then use one fallback
  live profile for the provider-neutral gate and report degraded coverage.
  Setup, verifier, and harness failures remain hard failures.
- Compatibility canaries run only for affected Anthropic, OpenAI Responses,
  or OpenAI-compatible wire paths, with one stable representative, one initial
  repetition, and only relevant tasks.
- Repeated provider/model matrices are experiment-driven only: intentional
  behavioral measurements, new provider/model characterization, or
  investigations. They are not ordinary release gates.
- Keep one live profile for now. A scripted provider can be reconsidered later
  if its investment becomes worthwhile; major releases may retain live
  evidence even then.

`eval-gate` now enforces the adaptive vote, bounded provider retry, sticky
fallback, hard evidence failures, and normalized model propagation through its
driver checks. Live checks use the same provider-invalid retry/fallback path,
including outages that begin only after task cells finish. The
resolver-neutral regression exercises those paths without live calls.

Next in line, in order of readiness:

1. ~~Publish the masking slice 2 stack~~ — **MERGED 2026-08-11 (#242)**.
   The owner-run live
   gate passed 2026-08-11: 8/8 tasks + 5/5 checks, first attempt, 81s live;
   masking-reclaim reclaimed 11,060 net tokens (exact match to review-time
   arithmetic) and proved post-mask re-read continuation. Measurement:
   `records/2026-08-11-masking-slice2-measurement.md`.
2. ~~ChatGPT subscription/OAuth lifecycle~~ — **implemented 2026-08-17**
   (spec: `specs/2026-08-11-chatgpt-subscription-design.md`). `/connect`
   probes, adopts, or starts device-code login; the TUI shows the URI
   and code; Esc aborts. Live login is owner-verified. Codex catalog
   bootstrap is pinned to `codex-models.json` and sends the snapshot's
   listed `client_version`. The leftover env-var exit line remains open.

3. **UX sprint** (board: "UX sprint (deliberate batch)") or later
   provider product slices (role-based routing; each needs design).

Working conventions that carried the arc (fresh sessions should keep them):
spec-first via `docs/superpowers/specs/`, subagent-driven execution with
per-task review plus a final whole-branch review, and owner rulings recorded
inline in board items rather than left in chat.
## Recommended Next Move (2026-07-20, superseded)

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
(harness loop vs provider adapter vs SDK layer). Related (2026-07-24,
from the #169 classifier fix): provider-error classification is a
keyword ladder over Rig's stringified errors — Rig flattens the HTTP
status and the provider's typed JSON error body into prose before yach
sees them (its streaming path does `format!("SSE Error: {e}")`). The
resilience pass should add a structured tier ahead of the keywords:
scrape the HTTP status and the JSON error body back out of the debug
string and classify on typed fields (`error.type`/`code` plus status)
first, keeping the keyword ladder as the fallback. Two error-body
dialects (Anthropic-shaped and OpenAI-shaped) cover nearly every
provider; the dialect belongs in the model catalog alongside context
windows. Keep `ProviderErrorKind` small and policy-shaped — new kinds
only when the harness would act differently. Also research where the
cohort sits on provider-integration ownership — own HTTP client (Codex)
vs SDK/middleware layer (opencode on the Vercel AI SDK, yach on Rig) —
as input to how long Rig stays load-bearing.

Owner-flagged (2026-07-22): context-tracker research. DONE 2026-07-25 —
see `docs/project/records/2026-07-25-context-system-harness-research.md`
(cohort sweep: Codex, opencode, Pi, Claude Code, nanocodex). Headline
findings: provider-reported usage is the cohort-consensus source of
truth, with chars/4 only estimating the unreported tail (Codex/Pi
hybrid); only Pi routes meter and trigger through one shared accounting
(which skips failed turns — the fix pattern for the 2026-07-24
meter flip-flop finding); compaction is tiered (tool-output
clearing/pruning before full summarization); overflow recovery is
one-shot and guarded everywhere. The record's "Implications for yach"
section lists eight concrete adoption items, ordered; item 1 (shared
completed-turns accounting for meter + trigger) is the immediate fix,
item 2 (hybrid provider-usage accounting) ties into model-catalog
hydration.

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
seam — the seam was designed for exactly this kind of swap. Concrete
provider lineup for the post-milestone rotation (2026-07-24):
OpenAI/ChatGPT (API and subscription paths), opencode Zen (black), and
Fireworks (firepass) — three distinct provider shapes (native OpenAI,
aggregator-wrapped, OpenAI-compatible) whose real failure strings should
also grow the provider-error classification test corpus as they land.
Also added to the harness-comparison cohort (2026-07-24): nanocodex
(https://github.com/gakonst/nanocodex), a minimal Codex-derived harness
— useful as a distilled view of Codex's architecture choices.

First cross-model rotation findings (2026-07-26, nemotron via opencode
Zen over the chat-completions shape): the wire shape, tool loop,
full-auto approvals, and edits all worked end to end, and Zen's
rate-limit phrasing classified correctly as `rate_limited`. One new
failure class: the model imitated yach's own assistant-round tool-call
echo format in prose and fabricated a `create_text_file` success
result — the turn completed with the README never written. The session
log is earmarked as the first cross-model fixture. Owner strategy for
behavioral fixes (2026-07-26, research:
`records/2026-07-26-behavioral-fixes-cohort-research.md`): the base
stays lean — quirks are expressed as capability data in the model
catalog, never model-name branches in the loop (Codex posture);
each rotated provider joins the quirk-class test corpus (Pi posture);
the cohort's convergent baseline trio (orphaned-tool-call healing,
malformed-tool-JSON tolerance, replay hygiene) counts as core hygiene;
and the echo-imitation defense (detect echo-format text in a final
response, reject and nudge once — no cohort harness has one) gets a
deliberate design note rather than a quick patch, since it touches the
round-echo format that carries loop-prevention weight.

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
status-bar context meter. Validated live twice (2026-07-22 armed at 10%,
2026-07-25 at production settings: 160,848 -> 4,160 tokens on a real
session with a clean continuation both times); the trigger now also runs
between tool rounds, not just at turn start. Next: the masking pre-pass
(slice 2). Known simplification to revisit: trigger accounting uses the
chars/4 estimate over assembled messages rather than provider-reported
usage — the harness research record
(`docs/project/records/2026-07-25-context-system-harness-research.md`)
slates the hybrid upgrade. Confirmed gap from the 2026-07-25 production
compaction (checkpoint compaction-2 kept only the triggering user
prompt): cut points are turn boundaries, so a turn larger than
`keep_recent_tokens` keeps nothing verbatim, and a turn larger than the
usable window cannot be compacted at all. Pi solves this with split-turn
summarization (the oversized turn's prefix gets its own secondary
summary and the cut can land mid-turn); design-scale work, slated with
slice 2 alongside the record's other adoption items (one-shot overflow
recovery flag, compaction-request-overflow fallback, summary
carry-forward anchor, post-compaction meter honesty).

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
