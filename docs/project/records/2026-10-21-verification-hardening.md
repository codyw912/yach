# Verification hardening and M1 gate reassessment

**Outcome:** linear:CEW-147
**Spec:** `../specs/2026-10-21-verification-hardening-design.md`
**Plan:** `../plans/2026-10-21-verification-hardening.md`
**Result:** verification improvements exercised; cohort gate remains open.

## Changes and evidence

The copied tmux driver is unchanged. Yach-specific helpers now inspect only the
current pane, require changed frames after actions and settled qualifying
frames, and print current-pane diagnostics on timeout. Existing prompt and
resume drives poll durable completed turns instead of transient footer counts;
restored history is checked before input, then retained via session evidence
rather than expected to remain in the visible pane after archival.

Added three real-TUI recipes:

- `drive-provider-resume`: same session, pre-input restored transcript, explicit
  model search/selection plus Thinking Level confirmation, then a new provider
  attempt whose actual request contains the original exchange. This is provider
  **recovery**, not automatic restoration.
- `drive-compaction`: empty-history no-op retained separately; a new manual
  summary checkpoint correlated to its request, then a continuation that carries
  the retained standing instruction and excludes folded original input.
- `drive-modal-transitions`: one Down selects edit Reject before Enter; file
  bytes remain unchanged and matching denial persists. Model-selector Escape
  returns focus to a composer that completes a new provider-backed turn.

One std-only Rust loopback Responses fixture serves the semantic recipes. It
checks actual request context before responding and correlates POST request IDs
with persisted attempts. It uses dummy local credentials and ephemeral localhost
ports. These proofs are not live provider-family or model-quality evidence.
Fresh private catalog caches prevent unrelated network refresh in new recipes.
README now has an affected-view handoff with separate agent/human results.

### Verification receipts

Evidence root: `~/.local/state/sjujperpowers/evidence/yach/`.

| Check | Observed result | Evidence |
| --- | --- | --- |
| Baseline `just test` and original seven-drive pack | exit 0 | baseline test output retained in execution ledger; `20261021T172320Z-3858360` through `20261021T172324Z-3858983` |
| Real-tmux stale-screen regression probe | legacy wait falsely accepted offscreen history; new helpers reject it, unchanged frames and transient matches; accept delayed fresh matching state | `/tmp/yach-fresh-ui-probe.VeBhFe/{before,final}.log` |
| `just test` | exit 0, zero failures; one ignored perf test | controller command receipt; committed hk receipt `/tmp/yach-verification-hardening-hk.json` separately below |
| Committed `hk-check` in dev shell | exit 0, cargo-fmt and cargo-clippy passed | `/tmp/yach-verification-hardening-hk.json` |
| Complete ten-drive pack after final identity fix | exit 0 with cleanup | `20261021T182147Z-3895939` through `20261021T182206Z-3897351` |
| Shell syntax and shellcheck, warning level | exit 0 after explicit sourced-variable contract correction | controller receipt |
| Eight semantic negative controls | all rejected, exit 1 each | `hardening-negative-20261021/`: missing continuation attempt, restored history, missing/unrelated request ID, missing checkpoint, missing summary instruction, replayed folded input, reused continuation turn/entry identities |
| Separate-agent verifier, same OpenAI family, at published head `cc7b9407` on main `0ae65e8a` | full ten-drive pack and cleanup exit 0; ten negative controls rejected | `20261110T044544Z-1196652` through `20261110T044602Z-1198073`; `hardening-verify-independent/` |
| Different-family verifier (Anthropic Claude) at `cc7b9407` | provider-resume, compaction, modal-transitions and cleanup exit 0; evidence inspected against each claim; ten negative controls rejected, unmodified evidence accepted | `20261110T050351Z-1214379`, `20261110T050357Z-1214746`, `20261110T050401Z-1215059`; `hardening-verify-anthropic/` |
| Guidance consuming-agent scenario | identifies exact new recipes and required evidence; refuses fixture-only resume/no-op as passes; reports manual recovery limit | execution ledger |

Final code review found a false positive when a resumed process reused old turn
or entry identities. The checker now requires fresh identities and exact reply
parent linkage; the suffix-only identity-remapping negative control rejects the
corruption while preserving otherwise valid request traffic and old prefix.

All commands used the declared development environment. Workers did not overlap
builds or runtime runs. Raw request/session files remain outside the repository.

The implementation workers ran on OpenAI models. The final runtime check came
from a verifier on an Anthropic model, confirmed from its recorded model ID
rather than its role name, which meets the spec's different-family criterion.
It ran the three new drives and their negative controls; the other seven
drives ran under the same-family verifier.

### Failures that improved the proof

Initial real runs correctly failed fixture-only resume. Recovery needed a
nonempty **full model ID** search: blank queries show curated rows, while broad
vendor-name queries also match many unrelated models. Model selection opens a
Thinking Level confirmation, which must finish before new prompt input.
Automatic restoration was not fixed or claimed by this change.

The local Responses wire contains string assistant content as well as typed
user-content arrays; the fixture now accepts both real protocol forms while
preserving exact history predicates. Compaction's successful status can be
immediately overwritten by another status; its proof now polls the durable new
checkpoint and summary attempt, with the visible compaction-count marker used
only for UI readiness. Unnecessary large padding was removed: the deterministic
one-token kept-tail configuration already makes short history foldable.

Independent verification also observed the resumed selector marking the model
`active` while the footer reported no configured backend. That misleading
interaction is recorded, not accepted as provider restoration. A fixture GET
model-list request can share a stage-derived ID with its following POST;
correlation intentionally filters POSTs, so it cannot supply attempt evidence.

## Open cohort gate: after the improvements

This section supplements, not rewrites, the failed first observations in
`2026-10-21-m1-cohort-1.md`. Portfolio v2 remains unchanged. Local deterministic
proofs are adjacent evidence, not a live-family substitute or a same-input
classification repeat.

| Original blocker | New evidence / disposition |
| --- | --- |
| Anthropic Messages availability, S1 | Gateway listing refreshed: 654 models, zero `anthropic-messages`; categories remain 634 devin-agent, 8 Codex Responses, 10 Responses, 1 image, 1 TTS. Anthropic success rows remain blocked. No credential/header regression inferred, no alternate family substituted. |
| L5 classification, S2 (`yach#wvbm`) | Still open. No product classification code changed; original both-family `invalid_request` attempts remain the evidence. The strict `unavailable_model` assertion is unchanged. These injections were not rerun merely to confirm known failures. |
| Live compaction, S2 | Local checkpoint/context reconstruction now has executable proof, including rejection of no-op and folded replay. The original live coding session still has no successful compaction continuation under its original acceptance; changing the kept-tail budget for the local fixture does not resolve it. Next product decision: how manual compaction should behave when all retained history fits its keep budget; then rerun the original live task without silently modifying portfolio inputs. |
| Compat live resume, S1 | **New successful manual recovery** on the original session, detailed below. The portfolio explicitly permits re-selection if recorded. This resolves the compat provider-backed continuation criterion, not automatic restoration, the unavailable Anthropic row, or required human assessment. |
| Human assessments | Still outstanding. All new visual notes are assistant observations. The README checklist makes missing human results explicit rather than substituting agent review. |

### Original compat session: actual provider recovery

- Original binary: cohort build `55ef9756163388447426c8c95982516c29419ae7`
  (signature-only rewrite `29579ddf31c864960ccde287746e9c3e1c3dfd9f`), unchanged
  copied binary used for the original session.
- Session: `session-3782212-1792599565299051607`; original private HOME/session
  directory and dedicated ylog workspace retained. No files changed by the
  recovery prompt. Existing ylog workspace contents were not reset or scored as
  a new coding scenario.
- Before-input capture shows the restored exchange. `/model`, search
  `openai-codex/gpt-6-sol`, select the exact
  `openai-compatible/openai-codex/gpt-6-sol [Environment]` row, confirm Thinking
  Level `off`, then submit the original L6 prompt:
  “Reply with exactly COHORT_RESUME_OK. Do not use tools or change any files.”
- New `turn-2` completes with one succeeded provider attempt for the approved
  model, request ID `ccf6d4c5-d608-448b-9611-6e48664cf268`, retry index 0. Screen
  shows `COHORT_RESUME_OK`, not fixture echo. Session ID remains unchanged.
- Evidence: `hardening-live-resume-20261021/{before-inspector.json,
  after-inspector.json,01-restored.txt,03-selection.txt,
  04-reactivated.txt,05-provider-completed.txt,session.jsonl}`.
- An earlier broad `gpt-6-sol` search only exposed unrelated Devin fusion rows
  first; no such row was activated. Exact model-ID search resolved it. This is
  an operator-recovery interaction finding, not a second-family substitution.

## Recommended order for closing the gate

1. Restore an approved Anthropic Messages route/model prerequisite; record any
   changed gateway input and run the blocked live family rows.
2. Fix `yach#wvbm` under its own product-code scope, then rerun both exact L5
   injections and the full scripted pack.
3. Decide/fix manual compaction's small-history behavior and rerun the original
   live checkpoint-plus-continuation task. Local forced-cut proof is not enough.
4. Retain explicit provider re-selection as the observed resume workaround;
   investigate automatic restoration and misleading active-model label as
   separate product findings, without hiding the workaround.
5. Have a person perform the outstanding interaction/autonomy/restoration
   assessments. Only then evaluate the portfolio's no-open-S1/S2 rerun gate.
