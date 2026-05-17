# Production Edit Tracing Design

Date: 2026-05-17
Status: proposed

## Context

Native edit work now has a real provider-originated agent edit path:

- `edit_text_file` and `create_text_file` are policy-gated provider-visible
  schemas in the native-provider path.
- Provider tool calls are validated by yach, normalized into
  `NativeEditTransactionRequest`, routed through `NativeEditAccess`, reviewed,
  applied or rejected, and returned as bounded provider continuation results.
- Durable session evidence already records generic tool requests/finishes,
  permission decisions, prepared edit transactions, and finished edit
  transactions.
- Local benchmark profiling exists for edit preview, apply, evidence summary,
  session append, and end-to-end harness phases.

The remaining observability gap is production traceability. A slow or failed
agent edit should be explainable from the session log without reconstructing
control flow from several event families by hand, and without leaking file
bodies, raw provider arguments, absolute paths, or full diffs.

The earlier benchmark/trace design intentionally deferred production tracing
until a real user-facing edit path existed. That condition is now met for
provider-originated agent edits.

## Goal

Design low-volume production tracing for native edit operations that records
the lifecycle and timings of provider-originated agent edits while preserving
yach-owned safety, redaction, and startup boundaries.

The design should make it possible to answer:

- which edit attempt a tool call, permission decision, preview, apply/reject,
  and provider continuation belonged to;
- whether time was spent in validation, normalization, preview, user/reviewer
  wait, apply/reject, result shaping, or provider continuation;
- which categorical phase failed;
- whether an edit was blocked before preview, rejected by review, failed during
  apply, or successfully continued back to the provider.

## Non-Goals

- No implementation in this slice.
- No provider-visible tool schema changes.
- No new edit, write, patch, delete, rename, shell, process, or network tools.
- No extension-owned mutation implementation.
- No auto-review runtime implementation.
- No sandbox implementation.
- No new TUI review UI.
- No high-frequency tracing framework.
- No wall-clock timeline or distributed tracing backend.
- No raw provider argument logging, file body logging, absolute path logging,
  or full diff logging.
- No performance claims against other harnesses.

## Approach Options

### Option A: Extend `MetricRecorded`

Record edit phase timings as `NativeSessionEvent::MetricRecorded` events with
attributes for tool request, transaction, phase, and outcome.

This reuses the existing metric event shape, but it blurs two concerns.
`MetricRecorded` is a low-frequency summary metric. Edit tracing needs a
lifecycle record with optional IDs, states, categorical reasons, and ordering
that users may inspect as evidence. Encoding that into metric attributes makes
the contract harder to validate and easier to misuse.

### Option B: Add A Distinct Edit Trace Event Family

Add a new durable session event variant for bounded edit trace records. Each
record captures one edit lifecycle phase, correlates to existing IDs, stores a
duration and categorical outcome, and remains ignored by provider transcript
projection.

This is the recommended option. It keeps metrics summary-oriented, keeps edit
evidence focused on local file effects, and gives production diagnostics a
clear contract. The event volume is low: one agent edit attempt produces a
small fixed set of phase records.

### Option C: Keep Tracing External To Session Logs

Rely on `yach-bench native-edit-profile-report`, local profiler tools, and
ad-hoc debug logs.

This remains useful for optimization work, but it does not help diagnose real
agent edit sessions after the fact. The production runtime already has
redacted session evidence; edit tracing should be part of that durable story.

## Recommended Shape

### Trace Event Model

Add a distinct session event variant:

```rust
NativeSessionEvent::EditTraceRecorded {
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    trace: NativeEditTraceRecord,
}
```

The trace record should be structured, bounded, and redacted:

```rust
pub struct NativeEditTraceId(pub String);

pub struct NativeEditTraceRecord {
    pub trace_id: NativeEditTraceId,
    pub phase: NativeEditTracePhase,
    pub source: NativeEditTraceSource,
    pub tool_name: Option<String>,
    pub tool_request_id: Option<NativeToolRequestId>,
    pub provider_call_id: Option<String>,
    pub preview_id: Option<NativeEditPreviewId>,
    pub permission_decision_id: Option<NativePermissionDecisionId>,
    pub transaction_id: Option<NativeEditTransactionId>,
    pub outcome: NativeEditTraceOutcome,
    pub duration_ms: u64,
    pub reason_label: Option<String>,
    pub attributes: Vec<NativeMetricAttribute>,
}
```

`trace_id` is minted when an agent edit attempt enters the yach-owned edit
path. It exists before a transaction ID, so validation and normalization
failures can still be correlated. `tool_request_id` remains the bridge to
generic tool evidence, `provider_call_id` remains the bridge to provider tool
calls, `permission_decision_id` remains the bridge to review, and
`transaction_id` remains the bridge to edit evidence once preview succeeds.

`attributes` should use the same low-cardinality shape as metrics. Allowed
initial attributes are small categorical values such as:

- `operation=edit_text_file|create_text_file`;
- `review_state=allowed|needs_user_approval|auto_review_unavailable`;
- `decision=apply|reject`;
- `continuation=sent|skipped`;
- `evidence_persisted=true|false`.

Do not add path strings, raw arguments, file bodies, model text, full diff
content, absolute paths, extension stdout, or provider debug payloads to trace
attributes.

`reason_label` must be categorical and allowlisted. It should reuse existing
snake-case labels such as `malformed_arguments`, `permission_denied`,
`preview_failed`, `apply_failed`, `user_rejected`, `tool_round_result_too_large`,
or `tool_continuation_validation_failed`. It must not contain provider debug
strings, model text, local paths, raw arguments, file bodies, or arbitrary
error messages.

Initial trace bounds should be explicit:

- `tool_name`: canonical registry tool names only, at most 64 bytes;
- `provider_call_id`: at most 256 UTF-8 bytes after bounded-string
  normalization;
- `reason_label`: allowlisted snake-case labels, at most 64 bytes;
- `attributes`: at most 8 items;
- attribute keys: at most 48 bytes;
- attribute values: at most 128 bytes.

If a bounded field exceeds its limit, the implementation should truncate or
replace it with a categorical sentinel in a way that does not leak raw local
content. The implementation plan should define the exact helper and tests.

### Phase Vocabulary

Use explicit snake-case phases:

- `tool_validation`
- `argument_normalization`
- `permission_decision`
- `preview`
- `review_wait`
- `apply`
- `reject`
- `result_shaping`
- `provider_continuation`

Each phase record should represent a completed phase with a duration measured
from `std::time::Instant` and stored as saturated milliseconds. Event order in
the JSONL log is the lifecycle order; no wall-clock timestamp is required in
this slice.

`review_wait` records user or reviewer wait time separately from compute time.
This prevents a slow approval decision from looking like a slow preview or
apply. Later auto-review can reuse the same phase with
`review_state=auto_review` or a more specific reviewer attribute.

### Outcome Vocabulary

Use a small categorical outcome enum:

```rust
pub enum NativeEditTraceOutcome {
    Started,
    Completed,
    Failed,
    Denied,
    Rejected,
    Cancelled,
    Skipped,
}
```

Most phase records should be `completed` or `failed`. `denied` is for policy or
permission denial before apply. `rejected` is for user/reviewer rejection.
`cancelled` is for dropped review channels or cancelled turns. `skipped` is
for phases that are intentionally absent, such as provider continuation after a
denied edit.

The implementation plan may omit `started` if every phase is recorded only at
completion. It should include `started` only if a write-ahead record is needed
for a phase that can leave externally visible effects before completion.

### Lifecycle Mapping

Provider-originated agent edits should record these phases:

1. `tool_validation`: wraps schema-only registry validation plus required
   provider call ID checks.
2. `argument_normalization`: wraps conversion from provider arguments into
   `NativeEditTransactionRequest`, including existing-text hash computation for
   `edit_text_file`.
3. `permission_decision`: wraps the permission decision made inside
   `NativeEditAccess::prepare`.
4. `preview`: wraps `NativeEditEngine::preview` through `NativeEditAccess`.
5. `review_wait`: wraps waiting for a user/reviewer decision when the edit is
   not auto-applied by policy.
6. `apply` or `reject`: wraps `NativeEditAccess::apply_with_evidence_sink` or
   `NativeEditAccess::reject`.
7. `result_shaping`: wraps creation of the bounded provider tool result.
8. `provider_continuation`: wraps building and sending the provider
   continuation request after all tool results for the round are ready.

The design intentionally traces phases around existing owned seams rather than
adding provider-owned or UI-owned tracing responsibilities.

Provider continuation is a tool-round boundary, not an edit-only primitive. If
one provider round contains multiple edit tool calls, the implementation should
record the continuation observation against each edit trace that contributed a
tool result, using the same continuation duration and a low-cardinality
attribute such as `tool_result_count=N`. This keeps per-edit diagnostics easy
to query without introducing a separate round trace ID in the first slice.

This requires the agent-edit handoff types to carry trace identity through the
whole provider round. A future implementation should not infer trace IDs by
searching prior log events. Instead, `NativeAgentEditToolPrepared` and
`PendingAgentEditToolReview` should carry `NativeEditTraceId`, including
completed, denied, and needs-review paths, so `native_runner.rs` can time
`review_wait` and `provider_continuation` against the correct edit traces.

### Persistence Semantics

Trace records are durable diagnostics, not a new authorization boundary.

- Trace records should be appended to the same native session log as other
  session events.
- Trace append failures must not cause yach to apply an edit without the
  existing write-ahead edit evidence.
- Trace append failures after a successful apply must not rewrite history or
  pretend the edit failed.
- When a trace record is batched with existing pre-apply evidence, the existing
  evidence persistence behavior remains authoritative.
- When a trace record is best-effort after an already visible outcome, failure
  should be surfaced as bounded diagnostic state, not as a second mutation.

This preserves the current safety rule: edit evidence and guarded apply remain
the local-effect authority; trace events explain timing and lifecycle.

### Projection And Replay

Provider transcript projection must ignore `EditTraceRecorded`, just as it
already ignores metrics, permission decisions, and edit evidence. Trace events
must not become provider context.

Session resume should treat trace events as turn-associated metadata:

- `next_turn_index()` should consider their `turn_id`;
- transcript projections should ignore them;
- JSONL load/write should round-trip them;
- tests should prove trace records do not appear in provider messages.

### Startup Boundary

Production edit tracing must not move work onto TUI first paint.

All trace types can be compiled into the backend, but trace IDs and timers
should be created only when an edit tool call is actually processed. The design
does not require extension scanning, provider initialization, config-heavy
loading, file reads, or benchmark setup during startup.

### Relationship To Metrics And Benchmarks

`MetricRecorded` remains the right place for coarse summaries such as
`native_prompt_total`. Edit trace events are the right place for edit
lifecycle diagnostics.

Future aggregate metrics can be derived from trace events or emitted
separately if needed, but the first production tracing implementation should
not duplicate every trace phase as a metric.

Bench report phase names should stay aligned with production trace phase names
where practical. The benchmark report remains the optimization tool; production
trace records remain the after-the-fact session diagnostic.

### Extension Compatibility

The trace source enum should leave room for future edit initiators:

```rust
pub enum NativeEditTraceSource {
    ProviderTool,
    LocalUi,
    ExtensionTool,
}
```

The first implementation should only emit `ProviderTool` for
provider-originated agent edits. Local `/debug-edit` tracing can be deferred,
because that harness is manual and not the product edit surface.

Future extension mutation tools should use the same trace record family after
they compile intent into yach-owned edit transactions. Extensions should not
emit authoritative trace events directly; the core runtime should emit records
around core-owned validation, permission, preview, review, apply, and result
boundaries.

## Implementation Planning Notes

A later implementation plan should likely touch:

- `crates/yach-backend/src/session.rs` for trace IDs, record types, session
  event variants, turn indexing, and JSONL round-trip tests;
- `crates/yach-backend/src/agent_edit_tools.rs` for trace ID creation and
  validation/normalization/prepare/apply/reject/result-shaping phase records;
- `crates/yach-backend/src/edit_access.rs` for returning or accepting enough
  phase metadata to correlate permission decisions, preview IDs, and
  transaction IDs without exposing prepared transactions;
- `crates/yach-backend/src/native_runner.rs` for `review_wait` and
  `provider_continuation` timing around the provider one-round path;
- provider replay tests to prove trace events are ignored by provider message
  projection;
- focused agent edit tests to prove successful apply, user rejection,
  validation failure, and apply failure all emit bounded correlated trace
  records.

The implementation should not make `NativeEditEngine::apply` public, should
not add provider schemas, and should not change the current edit permission
policy.

Because `NativeEditAccess::prepare` currently combines permission decision and
preview, the implementation plan must add an explicit correlation seam rather
than scraping side-effected log events. Acceptable shapes include returning a
small diagnostics struct on success and structured denial/preview-failure
errors that include the trace-relevant decision ID where one exists, or
splitting internal permission and preview phases behind crate-local helpers.
Denied and preview-failed paths must still emit correlated trace records even
when no transaction ID exists.

## Testing

The implementation plan should include tests for:

- JSONL round-trip of `EditTraceRecorded`;
- `next_turn_index()` includes trace events;
- provider transcript projection ignores trace events;
- successful provider-originated agent edit records a single trace ID across
  validation, normalization, preview, apply, result shaping, and continuation;
- ask-mode review records `review_wait` and then `apply` or `reject` with the
  same trace ID;
- validation failure records a trace ID even when no transaction ID exists;
- trace records do not include raw tool arguments, file bodies, absolute paths,
  or diff bodies;
- trace append behavior does not weaken existing write-ahead edit evidence.

Final verification for the implementation should include focused backend tests
for agent edit tracing, provider message projection, session JSONL
compatibility, plus the workspace `just test` and `just lint` gates.

## Follow-Up Work

- Write an implementation plan for this tracing design after review.
- Add aggregate edit latency reports only if production traces reveal a need.
- Design provider-visible read/search content separately if edit usefulness is
  blocked by context acquisition.
- Design broader mutation tools separately if exact/create edits are too
  narrow.
