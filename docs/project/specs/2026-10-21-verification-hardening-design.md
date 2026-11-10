# Verification hardening

**Outcome:** linear:CEW-147
**Status:** approved direction in conversation, 2026-10-21

## Problem and approved scope

The first M1 cohort exposed false-confidence risks in the existing verification
flow: waits match old scrollback, fixture echo can look like successful provider
resume, and invoking `/compact` need not create a checkpoint. Static captures
also do not establish modal focus or rejection behavior. The owner approved
fresh-state waits first, stronger resume/compaction checks, modal transitions and
a human affected-view checklist, followed by reassessment of the open gate.

This is verification infrastructure, not a new testing framework or a product
behavior change. The agreed scope is now nontrivial because it includes a shared
local protocol fixture. This spec and its numbered plan must precede further
implementation. Earlier baseline research and any partial helper edits remain
unverified until the full checks below run.

## Existing seams and decisions

- Extend `.agents/skills/verify-yach/scripts/lib/common.sh`; leave the copied
  `lib/tmux-tty.sh` unchanged. Existing drives keep private HOME, workspace and
  sessions, and the bench-enabled real binary.
- Keep the scripted resume recipe as transcript-persistence/fixture-continuation
  coverage. Add a distinct provider-backed recipe; do not silently repin its
  claim to fixture echo.
- A scripted FIFO provider ignores requests, so cannot prove history use. Use
  a std-only Rust loopback HTTP fixture for deterministic protocol evidence;
  Rust and jq already exist in the declared environment. Bind only
  `127.0.0.1:0`, use an ephemeral port, no external network or real credentials.
  Its response depends on validated request history, not just call sequence.
- Local protocol proofs do not count as live provider families. Portfolio v2
  acceptance, severity and same-input rerun rules stay unchanged.

## Fresh-state UI contract

`ui_wait PATTERN [SECONDS]` matches a current visible pane, not scrollback, and
requires qualifying settled frames. `ui_send TEXT PATTERN [SECONDS]` captures
the frame before sending literal text and Enter, then requires a changed frame
matching PATTERN and settling. `ui_keys PATTERN KEY...` does the same for keys.
Regex matching retains per-line extended-regex semantics. A timeout fails with
current-pane diagnostic evidence. Readiness requires the expected modal or
composer, not arbitrary delays. Settled pixels are capture readiness, not proof
that a provider or tool completed: durable assertions remain necessary.

Migrate existing startup/prompt/review/resume interactions to this contract.
The shared tmux driver remains a reusable unchanged dependency.

## Semantic recipes

### Provider resume

Drive an initial request through the local real adapter, complete it, quit,
and resume the same persisted session. Capture the restored transcript before
input. Complete a distinct second request; require a new succeeded turn-purpose
provider attempt, same session identity, and the fixture's validation that the
actual second request contains the restored first exchange. Fixture echo,
retained model labels, old reply text and message counts alone fail acceptance.
If explicit model re-selection is necessary, record it; do not hide it as
transparent provider restoration.

### Compaction

Create deterministic foldable history in isolated state, with a standing
instruction in the prefix and a kept recent tail. Invoke the actual TUI's
`/compact`. Require a new persisted `CompactionCheckpoint` and successful
summary attempt. The local server validates the summary request contains the
standing instruction, returns a summary retaining it, and allows continuation
only when that summary instruction reaches the effective request and the
original folded entry does not. Require successful continuation and correlated
wire/session evidence. No-op compaction is a distinct failed success criterion,
not a pass. This recipe proves summary compaction, not provider-native compact.

## Modal transitions and human handoff

A real-TUI edit rejection must show the two-option edit controls, move Down
once to Reject, reject, leave the original file bytes unchanged, and persist
the denial. The command review's three-option navigation is not a substitute.
A model selector must open, dismiss with Escape, and return focus to a composer
that can submit a unique prompt and complete it. Capture these transitions.

README's existing TUI design review gains a concise table: affected view/modal,
entry/action/exit keys, expected state/durable effect, evidence, agent result,
human result. Human checks remain unverified until a person performs them.
No new human approval is required for unrelated unchanged views.

## Verification and acceptance

- Baseline `just test` and the existing seven-drive pack passed on clean main
  `c27911fe7a11459ab329ba4a4598782477eae8c3` before implementation.
- A real-tmux negative control must show old scrollback satisfies the legacy
  wait but cannot satisfy the new current-frame transition contract. Exercise
  delayed fresh state and timeout as well.
- Reject fixture-only resume, absent checkpoint, omitted standing instruction,
  folded-history replay, and an edit rejection that mutates the file.
- Run all integrated drives through the declared development environment;
  validate real session/request/disk effects, not source text or mock echoes.
- Final `just test`, committed hk-check, shell syntax/shellcheck as available,
  complete smoke pack, independent code review and different-family runtime
  verification. One heavy build/check job at a time; no worker builds/checks
  while parent integrates.
- Update skill index/feature docs after smoke proof. Do not change the copied
  tmux driver or product behavior to force a verification pass.

## Cohort reassessment

Only after improvements pass, re-read the open cohort record and investigate
each blocker: Anthropic Messages availability, L5 error classification,
compaction checkpoint, live provider resume, and outstanding person-required
assessments. Use improved synchronization and semantic evidence where relevant.
Record new runs and changed prerequisites honestly. Do not claim local protocol
coverage closes the live gate; do not implement an unapproved product redesign.
