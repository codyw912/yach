# Phase 2 — Fixture Native Dogfood Runner

Last updated: 2026-05-03

## Goal

Make `yach tui --backend native` a constrained, explicit, no-network dogfood mode that exercises native backend lifecycle semantics through the real TUI/protocol path before adding provider SDK dependencies or credentials.

The phase is successful when native mode proves that yach-owned prompt lifecycle, persistence, cancellation, errors, and slow-consumer behavior can be represented cleanly without leaking provider/Pi details into `yach-ui` or `yach-proto`.

## Why it matters

A fixture runner is the lowest-risk bridge between architecture seams and real native backend dogfooding. It lets the project validate UI/backend control flow, typed lifecycle events, native persistence, and backpressure policy before the complexity of provider APIs, credentials, tool calls, or resource loading enters the system.

## Dependencies / entry criteria

- `yach-backend` crate and backend session launch seam exist.
- `yach-proto` remains the UI/backend boundary.
- Pi remains default backend; native mode is explicitly selected.
- Native session/event log is provisional and inspectable.
- Provider request/event/error seam exists, but no provider SDK dependency is required.
- Current branch has already implemented first-pass native fixture streaming, failure/cancel persistence, and prompt lifecycle events.

## Expected end-state

By the end of this phase:

- `yach tui --backend native` is explicit, reversible, and visibly identified in the TUI.
- Native mode has clear limitation/status behavior for unsupported features.
- Fixture prompts cover success, failure, cancellation, empty stream or equivalent no-output completion, and slow-consumer/backpressure behavior.
- Every active fixture turn has a yach-owned turn/request id carried through stream events and persisted records.
- Prompt lifecycle is explicit: started/delta/finished/failed/cancelled, with stale or late events ignored or recorded according to policy.
- `.yach/native-sessions/default.jsonl` or successor fixture log remains inspectable and clearly provisional.
- Bounded internal queue/backpressure behavior is specified and tested; unbounded outer UI channels, if still present, are documented as outside the solved boundary.
- No provider SDK dependency, network call, credential path, tool execution, or resource loading is added in this phase.

## Workstreams

### 1. Native runner UX contract

- Keep backend selection explicit (`pi` default, `native` opt-in).
- Show active native dogfood status/model/session limitations in protocol-visible surfaces.
- Return clear user-facing status for unsupported native actions instead of silent no-ops.
- Ensure native startup failure does not silently fall back to Pi when `native` was explicitly selected.

### 2. Prompt lifecycle and persistence

- Preserve typed lifecycle events for completion, failure, and cancellation.
- Persist enough fixture turn state to inspect prompt, assistant deltas/output, finish state, and error/cancel reason.
- Keep native session record format backend-internal/provisional.
- Add reload/blank-line/error-tolerance coverage only where it informs dogfood reliability.

### 3. Cancellation and stale-event policy

- Ctrl+C/native cancel should mark the active native turn cancelled and return UI to idle.
- Dropped/closed UI receivers should cancel or fail the active fixture turn rather than completing it.
- Late events after cancel/failure should not corrupt the next turn.
- Tests should prove state transitions, not just happy-path text streaming.

### 4. Bounded queue/backpressure semantics

- Introduce the smallest backend-owned queue/helper/policy needed to test slow consumers.
- Decide which events are never dropped: lifecycle, tool-call placeholders, completion/failure/cancellation.
- Decide which events may be coalesced or sampled: text deltas and raw debug payloads.
- On full queue, either await with cancellation support or fail the stream with a structured backpressure error.
- Keep protocol changes narrow; prefer backend-internal fixture tests unless UI needs a typed event.

### 5. Error envelope and user-facing mapping

- Ensure fixture failures map to structured backend/provider-style errors with actionable copy.
- Distinguish fixture failure, cancellation, malformed/empty stream, and backpressure where useful.
- Avoid broad provider taxonomy work unless fixture pressure exposes a concrete gap.

### 6. Evidence and docs checkpoint

- Record the final phase status in `.project/now.md` at wrap.
- Update `docs/protocol/yach-proto-v0.md` only for actual protocol commitments.
- Run a factual U7 docs checkpoint after a clean implementation slice to reconcile project OS docs with cockpit state.

## Key decisions needed

1. **Backpressure policy:** await-with-cancel vs fail-with-structured-backpressure-error when the bounded internal queue fills.
2. **Text delta handling:** preserve every fixture delta vs allow coalescing under slow consumer pressure.
3. **Lifecycle event durability:** exactly which started/finished/failed/cancelled records are persisted for provisional session logs.
4. **UI boundary scope:** whether current unbounded UI channels are explicitly out of scope for this phase or need narrow instrumentation.
5. **Error vocabulary:** whether existing provider/backend error types are sufficient for fixture backpressure and malformed stream cases.

## Risks and mitigations

- **Over-designing runtime abstractions:** Keep helpers small and fixture-driven; split crates/modules only after concrete consumers exist.
- **Protocol churn:** Avoid broad event changes; add protocol only when UI-visible behavior needs a typed boundary.
- **False backpressure confidence:** Document if policy stops at backend-internal queues while outer UI channel remains unbounded.
- **Session format ossification:** Label records provisional and avoid migration promises.
- **Provider creep:** Stop before real SDKs, credentials, network calls, or provider-specific settings.
- **UI regression:** Validate Pi path when protocol/UI behavior changes, not just native path.

## Validation strategy

Primary commands for backend/CLI-only changes:

```bash
just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings
just dev cargo test -p yach-backend -p yach-cli
```

For protocol/UI-impacting changes:

```bash
just dev cargo clippy -p yach-proto -p yach-adapter-pi-rpc -p yach-ui -p yach-cli --all-targets -- -D warnings
just dev cargo test -p yach-proto -p yach-adapter-pi-rpc -p yach-ui -p yach-cli
```

Optional final confidence:

```bash
just dev cargo test --workspace
```

Manual/scripted smoke evidence should cover:

- native backend launch/status;
- fixture success prompt;
- fixture failure prompt;
- fixture cancellation path;
- receiver-drop or slow-consumer/backpressure path;
- persisted JSONL inspection/reload where applicable.

## Acceptance criteria

- Native fixture mode is usable through the existing TUI with explicit `--backend native` selection.
- Success, failure, cancellation, and slow-consumer/backpressure paths have automated coverage.
- Active turn lifecycle cannot silently complete after cancel/failure/receiver drop.
- User-visible errors/statuses are clear enough for dogfood debugging.
- Native session log remains inspectable and provisional.
- No provider SDK dependency, API credential path, network provider call, tools, or resource loading is introduced.
- `yach-ui` remains independent from native backend internals and provider/Pi APIs.
- Validation commands pass before wrap.

## Candidate chunks

### Chunk 1 — Bounded queue/backpressure fixture policy

- **Why it matters:** Current native lifecycle semantics cover finish/cancel, but real provider streams need a tested policy for slow consumers.
- **Expected files/areas:** `crates/yach-backend/src/lib.rs`; possibly `crates/yach-cli/src/main.rs`; tests near native fixture runner/queue helper.
- **Max scope:** Backend-owned bounded queue/helper and fixture tests for slow consumer behavior; no provider SDKs or credentials.
- **Validation command:** `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings` and `just dev cargo test -p yach-backend -p yach-cli`.
- **Risk level:** Medium.
- **Stop/ask condition:** If solution requires broad protocol changes, UI channel redesign, or real provider semantics.
- **Human approval needed:** No.

### Chunk 2 — Native fixture error envelope refinement

- **Why it matters:** Provider adapter work needs native dogfood errors that can represent failure, malformed stream, cancellation, and backpressure without provider-specific leakage.
- **Expected files/areas:** `crates/yach-backend/src/lib.rs`, `crates/yach-cli/src/main.rs`, possibly `docs/protocol/yach-proto-v0.md` if UI-visible types change.
- **Max scope:** Narrow error variants/copy/tests needed by fixture runner only.
- **Validation command:** Backend/CLI clippy/tests; add proto/UI validation if protocol changes.
- **Risk level:** Medium.
- **Stop/ask condition:** If error taxonomy expands into full provider matrix or credential/setup errors.
- **Human approval needed:** No.

### Chunk 3 — Native dogfood smoke/evidence checkpoint

- **Why it matters:** The phase should produce evidence that future provider work can trust the native runner lifecycle.
- **Expected files/areas:** `docs/protocol/yach-proto-v0.md`, `docs/project-os/next-work.md`, possibly a new evidence/status doc if current repo conventions require it, plus `.project/now.md` at wrap.
- **Max scope:** Factual status/evidence update after implementation passes; no priority reorder without owner approval.
- **Validation command:** `git diff --check`; code validation should already have passed in prior chunks.
- **Risk level:** Low.
- **Stop/ask condition:** If docs would change committed priority order or declare native mode production-ready/default.
- **Human approval needed:** No for factual updates; yes for priority/default-backend decisions.

## Explicit non-goals

- Adding Rig, Siumai, GenAI, OpenAI, Anthropic, Gemini, or any other provider SDK dependency.
- Reading provider credentials, making network calls, or adding model/provider configuration UI.
- Implementing native tools, resource loading, package discovery, compaction, retry, or rich session tree navigation.
- Stabilizing native session/event JSONL format.
- Making native backend default.
- Reworking Pi RPC process IO placement unless a narrow testable need appears.
