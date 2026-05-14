# Native Edit Evidence And Local Harness Design

Date: 2026-05-14
Status: proposed

## Context

Native edit transactions now have two backend primitives: `NativeEditEngine::preview`
validates and summarizes create/modify requests without writing files, and
`NativeEditEngine::apply` consumes a prepared transaction and performs one
guarded create or modify operation. Apply is intentionally `pub(crate)` because
prepared transactions carry private after-images and should not become a public
full-file write API.

The next gap is not provider-visible mutation. It is a local, yach-owned path
that exercises preview/apply through runtime-shaped context and records durable
edit evidence. Generic tool evidence is useful, but file mutation needs
first-class local-effect records: users and future agents should be able to see
which relative paths changed, which hashes were expected and produced, which
transaction ID correlates the work, and whether content was redacted or
truncated.

## Goal

Add a backend-local edit harness and redacted session evidence model for native
edit transactions, without advertising mutation tools to providers or granting
extensions direct write capability.

## Non-Goals

- No provider-advertised edit or write tool.
- No extension-owned mutation tools.
- No approval UI.
- No CLI command in this slice.
- No public `NativeEditEngine::apply`.
- No delete, rename, chmod, directory creation, binary edit, or shell/process
  execution.
- No multi-operation atomicity.
- No Criterion benchmarks in the first evidence/harness slice.

## Recommended Shape

Use an explicit edit evidence model plus a crate-local harness wrapper.

The harness should accept:

- `NativeResourceRoot`;
- `NativeEditTransactionRequest`;
- `NativeEditPolicy`;
- `NativeSessionLog`;
- `NativeSessionId`;
- `NativeTurnId`;
- optional `NativeToolRequestId`.

It should call `NativeEditEngine::preview`, record a prepared edit event when
preview succeeds, then call `NativeEditEngine::apply` and record a finished edit
event. If preview fails before a transaction exists, it should record a finished
event with no transaction ID, a categorical reason, and no raw request payload.

Keep the harness outside `NativeToolRegistry` and outside provider continuation
for this slice. That preserves the current fail-closed policy where
`NativeToolRisk::MutatesLocalState` is denied and provider advertising only
routes safe read-only metadata tools.

## Approach Options

### Option A: Evidence events only

Add `NativeSessionEvent` variants and tests, but no wrapper that actually uses
preview/apply.

This is too abstract. It would prove JSONL compatibility, but not the runtime
flow that must record evidence in the right order and failure cases.

### Option B: Hidden native tool now

Register a hidden built-in edit tool with `NativeToolRisk::MutatesLocalState`
and route it through `NativeToolContinuationWorkflow`.

This moves too close to provider and extension policy before approval semantics
are designed. Hidden visibility reduces provider exposure, but the registry and
tool workflow are currently tuned for read-only provider continuations.

### Option C: Backend-local harness plus edit events

Add explicit edit session events and a crate-local harness that wraps
preview/apply without registering a tool.

This is the recommended option. It exercises the mutation primitive through a
runtime-shaped seam, records durable evidence, and leaves a straightforward
path to later CLI, hidden tool, provider-visible tool, and extension integration
without relaxing current provider safety.

## Evidence Model

Add edit-specific session evidence rather than overloading generic tool
payloads. The minimum event shape is:

```rust
NativeSessionEvent::EditTransactionPrepared {
    session_id,
    turn_id,
    tool_request_id,
    transaction_id,
    summary,
}

NativeSessionEvent::EditTransactionFinished {
    session_id,
    turn_id,
    tool_request_id,
    transaction_id,
    outcome,
    reason,
    summary,
}
```

`summary` should contain only bounded, local-effect metadata:

- operation count;
- operation kind;
- relative path;
- before/after hashes where applicable;
- before/after byte counts;
- hunk count for modify operations;
- bytes written after apply;
- diff summary as a redacted/truncatable payload summary.

Do not persist full file bodies, raw edit request JSON, absolute paths, resolved
`PathBuf` values, raw provider arguments, or extension process output.

The finished event should use categorical outcomes:

- `completed`;
- `validation_failed`;
- `failed`.

Preview failures should be `validation_failed` with `transaction_id: None` and
`summary: None` unless a later engine change returns a safe partial summary.
Apply failures after a successful preview should include the transaction ID and
the prepared summary, with `bytes_written` omitted.

## Harness Boundary

The first harness can be crate-local:

```rust
pub(crate) struct NativeEditHarness;

impl NativeEditHarness {
    pub(crate) fn preview_and_apply(
        root: &NativeResourceRoot,
        request: NativeEditTransactionRequest,
        policy: &NativeEditPolicy,
        log: &mut NativeSessionLog,
        context: &NativeEditHarnessContext,
    ) -> Result<NativeEditApplyResult, NativeEditError>;
}
```

`NativeEditHarnessContext` should carry `session_id`, `turn_id`, and optional
`tool_request_id`. The optional tool request ID is for future wrapping by local
or hidden tools; the first implementation can use `None`.

The harness should not:

- create a `NativeToolDefinition`;
- modify `NativeToolRegistry`;
- change provider advertising;
- produce provider continuation results;
- expose full after-images;
- make `NativeEditEngine::apply` public.

## Error Labels

Session evidence should use stable, snake_case reason labels derived from
`NativeEditError`, including:

- `empty_transaction`;
- `too_many_operations`;
- `transaction_too_large`;
- `create_disabled`;
- `modify_disabled`;
- `absolute_path`;
- `path_traversal`;
- `path_outside_root`;
- `parent_missing`;
- `target_missing`;
- `target_exists`;
- `duplicate_target`;
- `symlink_rejected`;
- `expected_file`;
- `unsupported_metadata_path`;
- `unsupported_file_type`;
- `not_utf8`;
- `file_too_large`;
- `hunk_not_found`;
- `hunk_ambiguous`;
- `empty_hunks`;
- `empty_find`;
- `hash_mismatch`;
- `io`.

Labels should not include local filesystem paths. Safe relative paths already
belong in operation summaries when validation reached a prepared transaction.

## Relationship To Future Tools

A future hidden built-in edit tool can wrap the harness and populate
`tool_request_id`, while still recording generic tool request/execution events
for consistency. The edit-specific events remain the durable local-effect
record.

A future provider-visible edit or write tool must still get its own design for
tool schema, approval UX, result shaping, continuation behavior, cancellation,
and provider-visible diff limits.

Extensions should eventually route mutation-capable tools through this same
yach-owned transaction boundary or an equivalent capability boundary. They
should not gain direct workspace write access during startup or manifest
loading.

## Acceptance Criteria

- Native session logs can persist and reload edit prepared/finished events.
- Edit evidence records relative paths, operation kinds, hashes, byte counts,
  hunk counts, bytes written, bounded diff summaries, outcomes, and categorical
  failure reasons.
- Full file bodies and raw request payloads are not persisted.
- A crate-local harness records `validation_failed` evidence when preview
  fails before apply.
- The harness records prepared and completed evidence when preview/apply
  succeeds.
- The harness records prepared and failed evidence when apply fails after a
  successful preview.
- No provider advertising, registry, extension, approval UI, or public apply
  surface changes are introduced.

