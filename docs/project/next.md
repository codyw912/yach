# Next Work

Last updated: 2026-07-14

## Recommended Next Move

Recommended next move: design and implement sensitive-file deny-by-default
for the provider-visible file tools, before daily dogfooding puts real
credentials at risk.

Why now: the 2026-07-14 confirming dogfood pass succeeded (duplicate-create
recovery, resume display parity; the search-budget blocker it found is fixed
in #132), so the MVP bar is effectively met pending a trivial live search
re-check. But `.env.local` with live API keys is currently readable and
searchable by provider-visible tools, and with payload persistence anything
the model reads also lands in plaintext session logs. Cohort research
(`docs/project/records/2026-07-14-sensitive-file-harness-research.md`) shows
no harness ships a real default deny — yach adopting one is ahead of common
practice and consistent with its deny-by-default posture. The research
record carries the recommended shape: a single path-authorization
chokepoint, deny-first precedence, a visible overridable default pattern set
(`.env*` with example-file allows, key material, credential stores), and
restrictive session-dir permissions. Needs a focused Superpowers design
first (new permission/policy surface).

After that, the next planning decision is post-MVP scope. The most likely
first gap in daily use is process/shell execution (running tests and builds
from the harness), which is on the "Not Ready Without a New Spec" list and
needs a focused design before implementation. Deferred stale-evidence
hardening (Cline-style post-edit content in edit results, file-change
notifications) remains available if dogfooding shows the prompt guardrails
are not enough for haiku-class models.

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
