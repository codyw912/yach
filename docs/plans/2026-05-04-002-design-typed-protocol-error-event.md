# Typed Protocol Error Event Design

Date: 2026-05-04
Status: design recommendation; implementation requires owner approval
Related: `docs/plans/2026-05-04-001-feat-native-provider-error-ux-plan.md`, `docs/protocol/yach-proto-v0.md`, `.project/phases/04-minimal-real-native-dogfood-path.md`

## Goal

Decide whether yach should add a typed protocol error surface after the native-provider status-only error UX polish.

The design must preserve the core invariant: `yach-ui` talks yach-owned protocol events, not Pi RPC, provider SDKs, or native backend internals.

## Current behavior

Current `ServerEvent` error-ish surfaces are:

- `StatusUpdated { message }` for general human-readable status/errors;
- `PromptFinished { outcome: Failed, message }` for prompt-scoped failure/cancel/completion;
- transport-level `BackendEvent::Disconnected { reason }` outside the serializable protocol event enum.

Native-provider setup/runtime failures currently stay on existing `StatusUpdated` / `PromptFinished` events with concise copy and normalized provider-kind hints. This is enough for the current explicit dogfood mode, but it is not enough for typed retry/help/error inspection semantics.

## Options

### Option A — Keep status-only until concrete UX pressure

Continue using `StatusUpdated` and `PromptFinished.message` for native-provider errors.

Pros:

- No protocol churn.
- Lowest compatibility risk.
- Good fit while native-provider remains explicit and experimental.

Cons:

- No typed actionability for retry/help.
- UI cannot distinguish setup/provider/protocol errors without string conventions.
- Error correlation is implicit.

Use when: dogfood needs only readable status and persisted logs.

### Option B — Prompt-scoped typed failure details

Add optional structured detail to `PromptFinished`, e.g. `error: Option<ProtocolError>` while keeping `message` as user-facing copy.

Candidate shape:

```rust
pub struct ProtocolError {
    pub domain: ProtocolErrorDomain,
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
    pub retryable: Option<bool>,
    pub redacted_debug: Option<String>,
}

pub enum ProtocolErrorDomain {
    Backend,
    Provider,
    Protocol,
    Tool,
    Resource,
    Unknown,
}
```

Pros:

- Correlates naturally with prompt lifecycle.
- UI can render prompt failure details without parsing strings.
- Less broad than a general error bus.

Cons:

- Does not cover setup failures before a prompt starts.
- Requires serialization compatibility decisions for adding fields.
- May tempt provider taxonomy into `yach-proto` too early.

Use when: prompt-level retry/help/error rendering is the immediate need.

### Option C — General `ServerEvent::ErrorRaised(ProtocolError)`

Add a standalone server error event with optional prompt/session/correlation fields.

Candidate shape:

```rust
pub struct ProtocolError {
    pub domain: ProtocolErrorDomain,
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub retryable: Option<bool>,
    pub redacted_debug: Option<String>,
}

pub enum ServerEvent {
    // ...
    ErrorRaised(ProtocolError),
}
```

Pros:

- Covers setup failures, prompt failures, tool/resource failures, and protocol errors.
- Does not overload prompt lifecycle completion.
- Can coexist with `StatusUpdated` for human-readable summaries.

Cons:

- Broader semantic surface and more policy decisions.
- Needs UI behavior for deduping with status/prompt-finished messages.
- Requires Pi adapter policy: when to emit typed errors versus status only.

Use when: non-prompt setup/config/tool/resource errors need structured UI behavior.

## Recommendation

Do not implement a typed protocol error event yet.

When implementation becomes justified, prefer **Option C: `ServerEvent::ErrorRaised(ProtocolError)`** over changing `PromptFinished` first.

Rationale:

- Native-provider setup failures can happen before a prompt exists, so a prompt-only shape would be incomplete.
- A general event can represent provider, backend, protocol, tool, and resource errors while keeping provider-specific details in `code` / redacted debug rather than enum variants.
- `PromptFinished { outcome: Failed, message }` can remain the lifecycle boundary; `ErrorRaised` can carry optional structured detail near the same turn without replacing lifecycle semantics.
- Pi RPC can continue emitting `StatusUpdated` until a concrete mapping is needed; native backend can adopt `ErrorRaised` first behind negotiated behavior or conservative UI handling.

## Proposed implementation gates

Implement only after at least one condition is true:

1. native-provider dogfood shows status-only failures are insufficient;
2. UI needs retry/help/detail affordances;
3. tools/resources introduce structured errors that status text cannot represent safely;
4. owner explicitly approves protocol expansion.

## Implementation sketch if approved later

1. Add `ProtocolErrorDomain` and `ProtocolError` to `yach-proto`.
2. Add `ServerEvent::ErrorRaised(ProtocolError)`.
3. Add JSONL roundtrip tests.
4. Update `yach-ui` to store/show latest error without depending on provider strings.
5. Update native-provider runner to emit both:
   - `ErrorRaised(...)` for structured detail;
   - existing `PromptFinished { outcome: Failed, message }` for lifecycle.
6. Keep Pi adapter unchanged unless a narrow mapping is obvious.
7. Document that `redacted_debug` is optional, redacted, and not for ordinary status display.

## Non-goals

- No implementation in this design chunk.
- No provider-specific enum variants in `yach-proto`.
- No retry/backoff behavior.
- No credential persistence or raw provider payload persistence.
- No default backend policy change.

## Validation for this design chunk

```bash
git diff --check
```

## Stop / ask conditions for future implementation

Ask before:

- adding `ServerEvent::ErrorRaised` or changing protocol compatibility expectations;
- exposing redacted debug in the TUI;
- mapping provider-specific error taxonomies into stable protocol enums;
- adding retry/help actions;
- changing Pi adapter error semantics.
