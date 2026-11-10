# Verification Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development or sjujperpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate stale-screen verification success and add semantic resume,
compaction and modal-transition proofs before reassessing the M1 cohort gate.

**Architecture:** Extend yach-specific shell helpers around the unchanged tmux
driver. Add a shared local request-validating protocol fixture and real-TUI
recipes; preserve durable session/disk assertions alongside captures.

**Tech Stack:** Bash, tmux, jq, existing Rust toolchain and bench-enabled yach.

**Spec:** `docs/project/specs/2026-10-21-verification-hardening-design.md`
**Source:** linear:CEW-147

## Global Constraints

- Leave copied `scripts/lib/tmux-tty.sh` unchanged.
- No real credentials, external network, new runtime dependency or product
  behavior fix in deterministic recipes; HTTP binds `127.0.0.1:0`.
- Existing portfolio v2 acceptance, severity and rerun rules stay unchanged.
- Local protocol proofs do not establish live-family coverage or human scores.
- Workers skip builds/lint/tests/formatters while editing shared files. Parent
  runs checks after integration, one heavy job at a time. A throwaway isolated
  tmux probe may establish the fresh-state regression without project runtime.
- Keep raw evidence outside repository and clean up owned processes reliably.
- Spec is binding; any implementation discovery requiring broader behavior
  changes must be recorded before changing that scope.

### Task 1: Current-frame synchronization

**Files:** modify `scripts/lib/common.sh`, `drive-prompt-reply`,
`drive-tool-review`, `drive-resume`, and `launch` only if needed, all beneath
`.agents/skills/verify-yach/`. Do not modify the shared tmux driver.

**Produces:** shell APIs consumed by Tasks 2 and 3:

```bash
ui_wait PATTERN [SECONDS]
ui_send TEXT PATTERN [SECONDS]
ui_keys PATTERN KEY...
```

`ui_wait` requires settled current visible pane matches. Action helpers capture
pre-action state, act once, require changed matching frame, then settling.
Extended regexes match per line. Timeout prints current state and fails.
Startup verifies model and composer readiness. Capture evidence uses the visible
pane rather than accepting off-screen history.

- [ ] Establish RED using real tmux: put a marker into history but outside the
  visible pane; legacy wait accepts it incorrectly. Record exit status and
  capture outside the repository.
- [ ] Implement helper contract and migrate existing UI sequences. Example:

```bash
ui_send 'count the lines' 'Approve once' 15
ui_keys 'Counted the lines\.' Enter
```

- [ ] Establish GREEN with old-history rejection, delayed matching frame,
  missing-state timeout, unchanged-frame timeout, literal single send, and
  keyed transition; retain observed results. Settled frames alone do not prove
  provider completion.
- [ ] Parent runs migrated drives and reviews the helper plus caller diff for
  both specification compliance and correctness before final acceptance.
### Task 2: Provider-backed resume and compaction semantics

**Files:** create `scripts/drive-provider-resume`, `scripts/drive-compaction`,
shared std-only Rust loopback fixture beneath `scripts/lib/`, and
`features/provider-resume.md`, `features/compaction.md`.

**Consumes:** Task 1 APIs. **Produces:** executable credless TUI recipes with
numbered captures, session inspector/JSONL, server request validation and exit
status. Fixture runs through declared Rust environment; no dependency installs.

- [ ] Establish RED acceptance: fixture-only resume with zero new provider
  attempts fails; no-op compaction with no new checkpoint fails. Use actual
  previous cohort evidence or explicit negative controlled evidence, not
  source-string assertions.
- [ ] Implement one ephemeral loopback server that accepts actual adapter
  requests and emits valid Responses SSE following runner's existing HTTP
  integration fixture. It saves requests and validation results. Reply payloads
  are conditional on request predicates; the server must fail malformed/missing
  history instead of accepting any sequence.
- [ ] Provider resume drive: start private TUI with loopback adapter, complete
  first turn, retain session identity, quit, resume, capture restored transcript
  before input, submit distinct continuation. Require same session plus a new
  successful provider attempt and request containing the original exchange.
  Record any explicit model re-selection. Fixture echo fails.
- [ ] Compaction drive: create foldable history containing standing instruction
  and kept tail, invoke `/compact`, require a new durable checkpoint and summary
  attempt, and submit continuation. Validate summary request contains original
  instruction and continuation contains compacted instruction but not original
  folded entry. Gate the reply on those predicates.

```bash
jq -e 'select(.type == "provider_attempt_finished" and
  .purpose == "turn" and .outcome == "succeeded")' SESSION.jsonl
jq -e 'select(.type == "compaction_checkpoint")' SESSION.jsonl
```

These selectors are existence signals only: scripts must additionally correlate
new events to continuation/checkpoint and verify complete session/request
relationships. Do not use existence of an earlier successful attempt as proof.

- [ ] Parent runs both real-TUI drives and negative controls removing checkpoint,
  provider continuation, standing instruction, and folded-history exclusion;
  every wrong behavior must be rejected. Review compliance and code quality.

### Task 3: Modal transition proof and human checklist

**Files:** create `scripts/drive-modal-transitions`,
`features/modal-transitions.md`; modify README TUI design-review subsection.

**Consumes:** Task 1 APIs and existing hashline tool-round helpers.

- [ ] Write edit-rejection scenario first, using read-minted file tag and
  proposal. Prove command three-option navigation does not establish edit
  two-option behavior.
- [ ] Drive edit proposal in review mode; observe pending two-option review,
  Down once selects Reject, Enter denies it. Assert original file bytes and
  persisted rejection/denial, not just a label.
- [ ] Open model selector, observe its controls, Escape, observe composer,
  submit unique prompt and require fresh actual scripted completion.
- [ ] Add affected-view table to existing human handoff, with entry/action/exit
  keys, expected state/durable effect, evidence, agent result, human result.
  Leave person checks unverified until performed.
- [ ] Parent runs modal drive and negative controls; review spec and quality.

### Task 4: Integration, verification and cohort reassessment

**Files:** skill index and affected feature docs; dated evidence record under
`docs/project/records/`. Do not edit roadmap or rewrite prior failed outcomes.

- [ ] Update SKILL.md full pack/table and truthful provider-resume/compaction
  limits after observing smoke evidence. Keep old transcript-only recipe label
  explicit. Validate guidance via consuming agent application scenario.
- [ ] Run complete declared-environment pack, preserving initial failure status
  and always cleaning up:

```bash
just dev-shell 's=.agents/skills/verify-yach/scripts;
$s/doctor && $s/launch && $s/drive-prompt-reply && $s/drive-tool-review &&
$s/drive-resume && $s/drive-headless && $s/drive-provider-failure &&
$s/drive-cancellation && $s/drive-multi-file-edit && $s/drive-provider-resume &&
$s/drive-compaction && $s/drive-modal-transitions;
rc=$?; $s/cleanup; crc=$?; [ "$rc" -ne 0 ] || rc=$crc;
echo "exit status: $rc"; [ "$rc" -eq 0 ]'
```

- [ ] Run `just test` then committed hk-check in dev shell. Run shell syntax and
  shellcheck where available. Refresh bench binary after commands replacing it.
- [ ] Independent final code review and different-family runtime verifier run
  assigned recipes without overlapping builds. Fix actionable correctness gaps,
  retain refusal/negative evidence, and run covering smoke after fixes.
- [ ] Reassess all original cohort blockers with declared inputs: gateway
  Anthropic Messages availability, L5 mismatch, compaction, live resume, human
  assessment gaps. Perform reachable scenarios using improved synchronization;
  distinguish changed prerequisites/new runs from classification repeats.
  Record concrete unavailable prerequisites without inventing a pass.
- [ ] Commit coherent results with `jj describe` then `jj new`; keep Kata tasks
  open until configured publication milestone; no automatic push or merge.
  Run final status and `agent-checkpoint push` before handoff, reporting the
  reference plus verification, open blockers and required human actions.
