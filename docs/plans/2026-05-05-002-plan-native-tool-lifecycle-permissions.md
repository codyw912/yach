# Native Tool Lifecycle and Permission Plan

Date: 2026-05-05
Status: planning recommendation; implementation not started
Related: `.project/phases/05-native-tools-resources-session-hardening.md`, `docs/plans/2026-05-05-001-plan-resource-config-root-policy.md`, `docs/project-os/architecture-invariants.md`

## Goal

Define a yach-owned tool lifecycle before native-provider tool calls can execute local actions or feed tool results back to providers.

The goal is not to implement tools yet. It is to set the minimum safe shape for registry, schema validation, permission checks, execution boundaries, output policy, persistence, and the first safe candidate tool.

## Non-negotiable boundaries

- Provider libraries may surface tool-call requests, but must not execute yach tools.
- Provider call ids are adapter metadata, not canonical yach tool/session ids.
- Tool arguments from a model/provider are untrusted input.
- File/network/process mutation requires explicit owner approval before implementation.
- Tool args/results may be sensitive and must not be persisted raw by default without size/redaction policy.
- `yach-ui` continues to communicate through `yach-proto`, not backend or provider internals.

## Proposed lifecycle

### 1. Register

A native tool registry owns definitions:

- stable yach tool name
- human description
- JSON schema for input
- output policy
- trust/risk classification
- permission requirement
- execution handler boundary

Registry entries should be backend-owned Rust values first. File-loaded/custom tool definitions can come later after resource policy and permission UX mature.

### 2. Request

Provider stream events such as `ProviderStreamEvent::ToolCallStarted`, `ToolCallDelta`, and `ToolCallCompleted` should be translated into a yach-owned pending tool request:

- yach tool request id
- native turn id
- requested yach tool name
- provider call id as metadata only
- raw argument payload kept in memory long enough to validate
- normalized validation/permission state

Unknown tools become rejected pending requests with a normalized reason; they are not executed or passed through blindly.

### 3. Validate

Before permission or execution:

- Parse arguments as JSON object according to the registered schema.
- Reject malformed JSON, wrong types, unknown required fields, oversized arguments, and tool-name mismatches.
- Normalize validation errors without storing raw argument payloads in session logs by default.
- Prefer allowlisted schema features for the first slice instead of a full dynamic schema engine if that keeps validation auditable.

### 4. Authorize

Permission checks run after validation and before execution.

Recommended initial default: **deny by default unless an explicit built-in policy grants the exact safe tool in fixture/test mode**.

Permission policy should be represented separately from tool definitions so future UI approval or config policy can be added without changing provider adapters.

Permission states:

- `allowed` — safe to execute under current policy
- `denied` — reject with user-safe reason
- `needs_approval` — future state once UI approval exists; not first implementation behavior

Do not implement prompt-for-approval UI in the first slice unless separately approved; it crosses protocol/UI scope.

### 5. Execute

Execution should happen through a trait boundary, even if the first handler is in-process:

- input: validated typed arguments plus execution context
- output: structured result with status, content chunks/summary, redaction metadata, and size accounting
- errors: normalized tool error kinds

Recommendation for first implementation: in-process trait boundary with no process/network/file mutation. This keeps the slice small while preserving a future move to subprocess/sandbox handlers.

### 6. Record

Native session records should eventually capture:

- pending tool request id
- yach tool name
- provider call id metadata when relevant
- validation/permission/execution outcome
- redacted/summarized result metadata

Do not store raw tool arguments/results by default. Persist summaries, sizes, error kinds, and redaction notes until a debug policy is explicitly approved.

### 7. Continue provider turn

Provider tool-result continuation is a separate later slice.

First implementation should stop at pending/validated/executed fixture records or backend unit tests. Sending tool results back to Rig/provider SDKs requires approval because it can create loops that expose local data to providers.

## Output and redaction policy

Initial limits should be conservative:

- Cap serialized argument payloads accepted for validation.
- Cap tool result bytes before persistence or provider continuation.
- Persist result summaries and byte counts rather than full content by default.
- Redact secret-looking values, auth headers, API-key-like tokens, and absolute paths where possible in debug strings.
- Treat tool stderr, command output, and file contents as sensitive unless explicitly classified otherwise.

## First safe tool candidate

Recommended first candidate: a fixture-only `echo_metadata` or `resource_path_check` style tool that does **not** read file contents, run processes, use network, or mutate state.

Better product-facing candidate after resource-root helpers exist: a read-only project-root metadata/stat tool that validates a relative path and returns normalized metadata only (exists/type/size), not contents.

Avoid as first tools:

- shell/command execution
- arbitrary file read
- file write/edit
- network fetch
- credential/config inspection
- provider self-introspection

## Protocol impact

No protocol change is required for backend unit-test-only registry/validation work.

Protocol additions become relevant only when the UI must show pending approvals, display tool progress/results, or allow users to approve/deny execution. At that point, prefer general tool request/result events over provider-specific fields.

## Recommended first implementation slice

Add a backend-internal tool registry and validation skeleton with fixture tests.

Expected scope:

- `yach-backend` types for tool definition, risk/permission classifications, pending request, validation result, and normalized tool errors.
- One fixture-safe tool definition.
- Tests for unknown tool, malformed args, schema mismatch, oversized args, denied-by-default behavior, and allowed fixture policy.
- No real provider continuation, no process/file/network mutation, no TUI permission UI, no protocol change.

Suggested validation:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```

## Deferred decisions requiring approval

- Default user-facing permission behavior (`prompt`, `deny`, or config policy).
- Any file, process, network, or destructive tool execution.
- Provider tool-result continuation loop.
- Persisting raw tool arguments/results or raw provider payloads.
- Tool approval UI and protocol events.
- Loading user-defined tools from local files/packages.
