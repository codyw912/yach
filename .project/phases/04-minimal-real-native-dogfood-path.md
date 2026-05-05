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

## Ready chunks

### Chunk 1 — Native-provider error UX plan

- **Why it matters:** Error handling is now the main dogfood usability gap, but protocol/UI choices should be scoped before implementation.
- **Expected files/areas:** `docs/plans/`, `.project/now.md`, possibly `docs/protocol/yach-proto-v0.md` if documenting current behavior.
- **Max scope:** Write a narrow implementation plan comparing existing status events vs typed protocol error event; choose a recommended first slice. No code changes.
- **Validation command:** `git diff --check`.
- **Risk level:** Low.
- **Stop/ask condition:** If the plan would change default backend policy, credential handling, retry behavior, or broad provider UX.
- **Human approval needed:** No for planning; yes before implementing protocol changes.

### Chunk 2 — Narrow status/error copy polish

- **Why it matters:** Users need actionable feedback when native-provider setup or runtime provider calls fail.
- **Expected files/areas:** `crates/yach-cli/src/main.rs`, tests in the same file; maybe `docs/spikes/2026-04-28-rig-provider-evaluation.md` for factual notes.
- **Max scope:** Existing protocol only. Improve setup/runtime status messages and helper tests around normalized provider error kind/copy. No typed protocol error event, no TUI layout work, no retry loop.
- **Validation command:** `just dev cargo fmt && just dev cargo clippy -p yach-cli -p yach-backend --all-targets -- -D warnings && just dev cargo test -p yach-cli -p yach-backend`.
- **Risk level:** Low to medium.
- **Stop/ask condition:** If good UX requires a new protocol event or TUI layout changes beyond status text.
- **Human approval needed:** No.

### Chunk 3 — Native-provider evidence checkpoint

- **Why it matters:** After status/error polish, docs should state exactly what native-provider dogfood supports and what remains experimental.
- **Expected files/areas:** `docs/spikes/2026-04-28-rig-provider-evaluation.md`, `docs/project-os/next-work.md`, `docs/protocol/yach-proto-v0.md`, `.project/now.md`.
- **Max scope:** Factual evidence/status update only; no priority reorder or production/default readiness claim.
- **Validation command:** `git diff --check`.
- **Risk level:** Low.
- **Stop/ask condition:** If docs would declare native-provider production-ready/default or change compatibility policy.
- **Human approval needed:** No for factual updates; yes for policy/default-backend decisions.

## Later candidate chunks

- Typed protocol error event design, if status-only UX proves insufficient.
- Additional approved real-provider failure runs for auth/rate-limit/network timeout.
- Minimal scripted native-provider TUI smoke harness, if manual dogfood evidence becomes too expensive to reproduce.

## Explicit non-goals

- Making native-provider or native backend the default.
- Persisting credentials, raw provider payloads, or provider-owned sessions as canonical yach state.
- Adding retry/backoff policy.
- Adding provider settings/model management UI.
- Adding native tools, resources, permissions, package discovery, or compaction.
- Stabilizing native session JSONL format.
- Supporting Rig OpenAI-compatible streaming before the deferred adapter-path issue is investigated separately.
