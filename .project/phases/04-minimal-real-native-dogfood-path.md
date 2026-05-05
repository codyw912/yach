# Phase 4 — Minimal Real Native Dogfood Path

Last updated: 2026-05-04

## Goal

Make explicit `yach tui --backend native-provider` dogfoodable for a narrow real-provider chat loop while preserving yach-owned sessions, protocol boundaries, inspectable persistence, and reversible backend selection.

This phase is successful when a developer can use the native-provider backend for constrained success, failure, and cancellation dogfood without provider SDK types leaking into `yach-ui`, `yach-proto`, or canonical native session records.

## Why it matters

The project now has native fixture lifecycle coverage and real-provider evidence for Anthropic and ChatGPT/Codex subscription paths through the yach-owned provider seam. The next risk is not proving providers can answer; it is making the explicit native-provider dogfood loop understandable, recoverable, and evidence-backed without prematurely expanding into provider settings, retry policy, tools, resources, or default backend changes.

## Dependencies / entry criteria

- `yach tui --backend native-provider` exists and is explicitly selected; Pi remains default.
- `run_provider_request(...)` consumes yach-owned `ProviderRequest` and emits yach-owned `ProviderStreamEvent`.
- Anthropic and ChatGPT/Codex subscription happy-path diagnostics have succeeded.
- Invalid-model real-provider evidence exists for both supported provider paths.
- Native-provider cancellation has been dogfooded with `YACH_NATIVE_PROVIDER_TEST_DELAY_MS`.
- Native session records remain backend-internal/provisional and inspectable.

## Expected end-state

By the end of this phase:

- Native-provider startup/status clearly identifies provider/model and unsupported surfaces.
- Provider setup and runtime failures produce actionable, redacted, user-facing status copy.
- Native-provider failure/cancel/success turns persist enough normalized reason/metadata to inspect what happened without storing secrets or raw payloads.
- Late/stale provider events after cancel/failure do not corrupt a later turn.
- Evidence docs list exact supported provider paths, commands, limitations, and stop conditions.
- No default backend change, broad provider settings UI, retry loop, credential persistence, raw payload persistence, tools/resources, or provider-specific protocol commitment is introduced.

## Workstreams

### 1. Error presentation and status copy

- Keep using existing protocol events unless a typed protocol error event is explicitly planned and approved.
- Make setup/runtime/provider errors understandable from the TUI/status stream.
- Include normalized provider error kind where helpful.
- Keep debug detail redacted and avoid raw provider payload persistence.

### 2. Failure persistence and inspection

- Preserve normalized failure reason in native session logs.
- Keep provider metadata on assistant entries for successful turns.
- Avoid promising stability for session JSONL field names.
- Add tests around any new persistence shape or copy helper.

### 3. Cancellation/stale event hardening

- Ensure active native-provider turns cannot complete after cancel/failure.
- Keep concurrent prompt rejection narrow and explicit.
- Add targeted tests only when a concrete stale-event path is reproducible.

### 4. Evidence and docs checkpoint

- Update `docs/spikes/2026-04-28-rig-provider-evaluation.md` with factual supported/failing provider evidence.
- Update `docs/protocol/yach-proto-v0.md` only for actual protocol behavior, not internal implementation detail.
- Keep `.project/now.md` as the current execution panel and avoid broad backlog expansion.

## Key decisions needed

1. Whether narrow native-provider error presentation should stay on existing `StatusUpdated` / `PromptFinished` events for now.
2. Whether a typed protocol error event is needed before broader native dogfood.
3. How much redacted debug detail should be visible in TUI status versus only persisted in native session logs.
4. Which additional real-provider failure modes, if any, justify approved manual evidence runs.

## Risks and mitigations

- **Protocol churn:** Prefer existing status/finish events for narrow dogfood; plan typed error events separately if needed.
- **Credential/debug leakage:** Never persist credentials or raw payloads; keep manual evidence redacted.
- **Provider UX scope creep:** Stop before provider settings UI, model management, retry policy, or default backend decisions.
- **False stability:** Label native-provider path experimental and explicit; keep session records provisional.
- **Adapter leakage:** Keep Rig/provider types below `yach-backend` and out of UI/protocol/session records.

## Validation strategy

For docs/planning chunks:

```bash
git diff --check
```

For CLI/backend-only implementation chunks:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings
just dev cargo test -p yach-backend -p yach-cli
```

For protocol/UI-impacting implementation chunks:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-proto -p yach-ui -p yach-cli -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-proto -p yach-ui -p yach-cli -p yach-backend
```

Optional final confidence:

```bash
just dev cargo clippy --workspace --all-targets -- -D warnings
just dev cargo test --workspace
```

Manual evidence remains approval-gated when it requires real provider credentials/network calls.

## Acceptance criteria

- Explicit native-provider dogfood remains reversible and non-default.
- Supported provider paths and limitations are documented with evidence.
- Provider errors are normalized, redacted, and actionable enough for dogfood debugging.
- Failure/cancel/success turns are inspectable in native session logs without secrets/raw payloads.
- `yach-ui` remains protocol-only; provider SDK types do not leak past backend adapter code.
- Stop conditions are documented before any credential persistence, retry policy, tools/resources, or default-backend work.

## Completed chunks

### Chunk 1 — Native-provider error UX plan

Completed in `docs/plans/2026-05-04-001-feat-native-provider-error-ux-plan.md`. Recommendation: keep first polish slice on existing `StatusUpdated` / `PromptFinished`; defer typed protocol error events until dogfood proves status-only UX insufficient.

### Chunk 2 — Narrow status/error copy polish

Completed in `crates/yach-cli/src/main.rs` with existing protocol events only. Setup failures are prefixed as native-provider setup failures; runtime failures include snake_case normalized error kind plus concise hints; failed-turn reasons continue to persist normalized kind plus redacted debug context.

### Chunk 3 — Native-provider evidence checkpoint

Completed via `docs/spikes/2026-04-28-rig-provider-evaluation.md`, `docs/project-os/next-work.md`, `docs/protocol/yach-proto-v0.md`, and `.project/now.md`. The checkpoint records status-only error UX and no default backend, retry, raw payload persistence, typed protocol error event, or broad provider UX.

## Ready chunks

### Chunk 4 — Typed protocol error event design

- **Why it matters:** Status-only provider error UX is intentionally narrow. Before adding a typed error surface, yach needs a design that preserves `yach-proto` ownership, Pi compatibility/reference behavior, redaction policy, prompt/session correlation, and native-provider dogfood needs.
- **Expected files/areas:** `docs/plans/`, `docs/protocol/yach-proto-v0.md`, `.project/now.md`; inspect `crates/yach-proto/src/lib.rs`, `crates/yach-ui/`, and `crates/yach-adapter-pi-rpc/` for constraints, but do not implement protocol changes in this chunk.
- **Max scope:** Planning/design only. Compare status-only, prompt-scoped typed errors, and general server error events; recommend whether/when to implement. No code changes, no event additions, no UI work.
- **Validation command:** `git diff --check`.
- **Risk level:** Medium due future protocol semantics, but low implementation risk because this chunk is planning only.
- **Stop/ask condition:** Stop before adding or committing protocol code, changing compatibility/default-backend policy, or requiring provider credentials/network tests.
- **Human approval needed:** No for this design chunk; yes before implementation of protocol changes.

### Chunk 5 — Native-provider smoke harness feasibility plan

- **Why it matters:** Manual native-provider TUI dogfood is useful but expensive to reproduce. A feasibility pass can decide whether a scripted no-secret harness can cover setup/status/cancel/error UX without real provider credentials.
- **Expected files/areas:** `docs/plans/`, existing bench/smoke harness docs if relevant, `crates/yach-cli/src/main.rs`, `crates/yach-ui/`, `.project/now.md`.
- **Max scope:** Planning/feasibility only. Identify whether to use fixture native, delayed native-provider, fake provider stream injection, or existing TUI harnesses. No new harness implementation and no real provider calls.
- **Validation command:** `git diff --check`.
- **Risk level:** Low.
- **Stop/ask condition:** Stop if feasibility requires credentials, network calls, production-like provider setup, or broad TUI harness architecture.
- **Human approval needed:** No.

## Later candidate chunks

- Implement typed protocol error event only after owner approval of Chunk 4 design.
- Implement minimal scripted native-provider TUI smoke harness only after Chunk 5 feasibility identifies a narrow no-secret path.
- Additional approved real-provider failure runs for auth/rate-limit/network timeout.

## Explicit non-goals

- Making native-provider or native backend the default.
- Persisting credentials, raw provider payloads, or provider-owned sessions as canonical yach state.
- Adding retry/backoff policy.
- Adding provider settings/model management UI.
- Adding native tools, resources, permissions, package discovery, or compaction.
- Stabilizing native session JSONL format.
- Supporting Rig OpenAI-compatible streaming before the deferred adapter-path issue is investigated separately.
