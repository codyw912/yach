# Native Agent Edit Tool Surface Design

Date: 2026-05-15
Status: proposed

## Context

Native edit work now has the local pieces needed for safe file mutation:

- `NativeEditEngine::preview` validates create/modify text-file requests and
  returns bounded diff summaries without writing files.
- `NativeEditEngine::apply` remains crate-local and consumes prepared
  transactions through guarded writes.
- `NativeEditAccess` owns pending previews, records permission decisions, and
  applies or rejects prepared transactions.
- The protocol has a local edit prepare, preview, decision, and finish
  lifecycle used by the temporary `/debug-edit` TUI harness.
- Provider-visible mutation is still unavailable because current provider
  advertising only accepts safe read-only metadata tools.

The remaining product gap is the real edit surface. Users should describe work
to the agent; the agent should choose yach-owned tools; yach should validate,
preview, route review, apply or reject, and record evidence. A user-facing slash
command is useful as a manual harness, but it is not the product surface for
code edits.

Comparable harnesses point at the same broad shape:

- Pi exposes familiar coding-agent tools such as `read`, `bash`, `edit`,
  `write`, `grep`, `find`, and `ls`, and its extension UX favors lightweight
  install/drop-in workflows. Yach should preserve the familiar surface where it
  helps UX, but keep execution authority in the Rust core.
- OpenCode groups all file modification tools under one edit permission family:
  `edit`, `write`, and patch-style tools do not get separate bypassable safety
  knobs. Yach should follow that grouping for mutation permission.
- Codex separates sandbox boundary, approval policy, and reviewer. Its
  `auto_review` changes who reviews eligible approval requests; it does not
  expand the sandbox or grant permission by itself. Yach's permission model
  should keep the same separation.

References:

- OpenCode permissions: <https://opencode.ai/docs/permissions/>
- OpenCode tools: <https://opencode.ai/docs/tools/>
- Codex sandbox defaults and auto-review: <https://developers.openai.com/codex/concepts/sandboxing#configure-defaults>
- Codex approvals and security presets: <https://developers.openai.com/codex/agent-approvals-security#common-sandbox-and-approval-combinations>
- Pi package example showing familiar built-in tool names: <https://pi.dev/packages/pi-agent-selector>

## Goal

Design the first agent-selected edit tool surface for native yach.

The design should specify how agents discover, select, and invoke yach-owned
edit tools while preserving:

- core-owned file mutation enforcement;
- permission and reviewer routing;
- durable redacted tool/edit evidence;
- provider advertising boundaries;
- extension compatibility;
- fast TUI first paint.

## Non-Goals

- No implementation in this slice.
- No broad provider-advertised mutation surface beyond the narrow first edit
  schemas designed here.
- No multi-round provider tool loop redesign.
- No direct exposure of `NativeEditEngine::apply`.
- No overwrite-capable `write` tool in the first agent edit surface.
- No delete, rename, chmod, directory creation, binary edit, or multi-operation
  atomicity.
- No shell/process execution tools.
- No network tools.
- No extension-owned mutation tool implementation.
- No working auto-review agent or subagent runtime.
- No sandbox implementation.
- No broad config or permission UI.
- No removal of the temporary `/debug-edit` harness.

## Design Principles

### Tool UX Can Be Familiar, Enforcement Must Be Yach-Owned

The model-facing names should be easy for agents to understand, but yach should
not copy another harness's safety boundary blindly.

The first mutation tools should compile into `NativeEditTransactionRequest`
values and then enter `NativeEditAccess`. They should not expose arbitrary file
handles, raw `PathBuf` writes, provider-owned executors, extension-owned writes,
or a public apply API.

### One Mutation Permission Family

All file mutation surfaces should be governed by one permission family. The
first implementation can call this `EditTransaction` internally and expose it as
`edit` or `file_mutation` in future config/UI.

This family should cover:

- exact-replacement edits;
- create-new-file writes;
- future patch-style edits;
- future overwrite writes;
- future extension tools that request edits.

The important property is that a future `write` or `apply_patch` tool must not
be able to bypass a user's edit permission mode just because it has a different
tool name.

### Agent Selection Requires Provider-Visible Schemas

Yach currently has one provider-advertised metadata tool path. That path is
deliberately limited to `ReadsLocalMetadata`.

For the current native-provider architecture, the agent is the model behind the
provider adapter. If a tool is never projected into a provider request, the
agent cannot discover or select it. The first real agent edit surface therefore
must include policy-gated provider-visible schemas for the narrow built-in edit
tools.

The safety boundary should be yach-owned execution, not provider invisibility.
Provider advertising remains schema-only: the provider may request
`edit_text_file` or `create_text_file`, but yach still owns validation,
permission routing, preview, apply/reject, evidence, result shaping, and
continuation.

Provider-visible mutation beyond these first exact-create schemas can come
later, but it needs separate designs for broader write/patch/delete/rename
semantics, shell/process coupling, and extension-owned mutation.

### Review Is Part Of Invocation, Not A Separate Manual Command

An edit tool call should produce the same lifecycle users already see in the
local access harness:

```text
agent chooses edit tool
  -> yach validates tool arguments
  -> yach prepares edit transaction
  -> yach records tool + permission + prepared-edit evidence
  -> yach routes review based on permission mode and reviewer
  -> yach applies or rejects
  -> yach records final tool + edit evidence
  -> agent receives a bounded result
```

The TUI can render this as a transcript-native diff review card or a focused
review panel, but the source of truth remains backend protocol events and
session evidence.

### Fast Startup Remains Non-Negotiable

Built-in edit tool definitions should be cheap static definitions. They may be
available to the runtime when the first agent turn is built, but they should not
force extension scanning, extension host activation, file reads, provider
initialization, or config-heavy work onto the TUI first-render path.

The existing startup evidence supports keeping this boundary strict:

- native first render after Rust `main` is about `0.54ms` p95 in the
  2026-05-12 traced run;
- inactive extension setup stayed within the same sub-millisecond envelope,
  with `tui_first_render_end_since_main` p95 delta of `+0.024ms`.

## Approach Options

### Option A: Promote `/debug-edit` Into `/edit`

The simplest path is to rename the manual harness and let users trigger edits
through a slash command.

This is rejected. It reinforces the wrong product surface: users should ask the
agent to do work, and the agent should invoke tools. A manual slash command can
remain for smoke testing, but it should not become the main edit UX.

### Option B: Hidden Built-In Agent Edit Tools

Register built-in edit tool definitions as hidden from provider advertising and
route them through a separate local agent tool loop.

This is rejected for the current architecture. It sounds safer, but it leaves
the agent no way to discover or select the tools unless yach first builds a
second local planning/runtime layer. That may become useful later, but it is not
the native-provider path in front of us.

### Option C: Policy-Gated Provider-Visible Edit Tools

Register narrow built-in edit tool definitions in the native tool catalog with
`NativeToolRisk::MutatesLocalState` and provider visibility gated by explicit
session/profile policy. The native provider request builder advertises only
these accepted schemas when the edit tool surface is enabled, and execution
routes through `NativeEditAccess`.

This is the recommended option. It is the only option here that is actually
agent-selected in the current provider-backed runtime. It keeps execution and
permission enforcement in yach, makes the provider-visible schema narrow and
inspectable, reuses registry/evidence concepts, and creates the seam future
extension mutation tools can target.

## Recommended Shape

### Built-In Tool Set

Add two first-class built-in mutation tool definitions:

- `edit_text_file`
- `create_text_file`

`edit_text_file` should perform exact text replacement in an existing UTF-8 text
file. The provider-facing schema should not require `expected_sha256`; yach
should compute the current file hash during request normalization and pass it
into `NativeEditTransactionRequest` before preview.

```json
{
  "path": "crates/example/src/lib.rs",
  "find": "old text",
  "replace": "new text"
}
```

`create_text_file` should create a new UTF-8 text file and fail if the target
already exists:

```json
{
  "path": "docs/example.md",
  "content": "new file content\n"
}
```

This schema assumes the agent knows the text it wants to replace. Until a
provider-visible read/search content surface is designed and enabled, practical
use will be limited to user-supplied snippets, static context already included
in the prompt, or injected test tool calls. The implementation plan should call
that limitation out explicitly rather than pretending this edit tool alone is a
complete coding-agent tool set.

The first surface should not include a broad `write` tool because overwrite
semantics have a larger blast radius and do not map as tightly to the existing
guarded transaction model. A later `write_text_file` can still be added under
the same mutation permission family after overwrite review semantics are
explicit.

The model-facing prompt can describe these as the current edit tools and can
mention that broader write/patch/delete/rename tools are unavailable.

### Tool Definition Metadata

The built-in definitions should use the existing catalog vocabulary:

```rust
NativeToolDefinition {
    name: "edit_text_file",
    risk: NativeToolRisk::MutatesLocalState,
    owner: NativeToolOwner::BuiltIn,
    provider_visibility: ProviderToolVisibility::Visible,
    input_schema: ...
}
```

Provider advertising should continue to reject arbitrary mutation definitions.
This design should extend the advertising allowlist only for the canonical
built-in `edit_text_file` and `create_text_file` definitions when the current
session/profile policy enables provider-visible edits and the native runner has
an executable route.

The runtime should expose provider-advertisable agent tools through a projection
that is stricter than the raw registry. A future name could be:

```rust
NativeAgentToolCatalog::available_for_turn(...)
```

That projection should filter by:

- accepted built-in and extension definitions;
- current session/profile policy;
- runtime support for the executor route;
- provider visibility and current backend availability;
- risk class;
- model/tool-loop capability.

The existing `NativeToolRegistry::validate_request` is not enough for this path
as-is because `NativeToolPermissionPolicy` correctly denies
`MutatesLocalState`. The implementation should split schema lookup/validation
from permission classification, then route mutation authorization through
`NativePermissionDecisionEngine` and `NativeEditAccess`.

### Invocation Boundary

Add a small execution path for local agent tool calls, conceptually:

```rust
pub struct PendingNativeAgentToolRequest {
    pub request_id: String,
    pub turn_id: NativeTurnId,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub provider_call_id: String,
    pub actor: NativePermissionActor,
}

pub enum NativeAgentToolInvocation {
    Completed(NativeAgentToolResult),
    NeedsUserReview(NativeAgentToolReviewRequest),
    Denied(NativeAgentToolResult),
    Failed(NativeAgentToolResult),
}
```

For edit tools, invocation should:

1. validate the tool name and schema through a schema-only registry path;
2. convert arguments into a `NativeEditTransactionRequest`, computing
   `expected_sha256` inside yach for `edit_text_file`;
3. build a `NativePermissionRequest` with
   `NativePermissionCapability::EditTransaction`;
4. call `NativeEditAccess::prepare`;
5. persist tool request evidence, permission evidence, and prepared-edit
   evidence before surfacing review;
6. return either an immediate result for allowed/denied/failure cases or a
   review request for user/auto-review routing.

The edit access facade should learn to carry an optional
`NativeToolRequestId`. That lets `EditTransactionPrepared` and
`EditTransactionFinished` correlate with `ToolRequestRecorded` and
`ToolExecutionFinished`.

Because these calls originate from provider tool calls, the provider call ID
must be preserved through the final bounded tool result so the continuation can
pair the result with the provider request.

### Review And Apply

The first implementation can continue to route `Ask` and `AutoReview` to the
user because auto-review has no runtime yet. The data model should still keep
`AutoReview` as the configured mode/reviewer route so later implementation does
not need to alter edit semantics.

For `Allow`, the runtime may apply immediately, but it must persist tool
request evidence, permission decision evidence, prepared-edit evidence, and a
write-ahead `ApplyStarted` edit event before mutation. The existing
`apply_with_evidence_sink` path is the right final apply integration point, but
the agent path must ensure the prepare-side evidence has already been flushed in
the same way the local UI path does before apply.

For `Ask`, the runtime should pause the tool call and emit a review request to
the TUI. The TUI should show the diff summary, path, operation, decision ID, and
tool name. The user can apply or reject. Apply should call back with the same
preview ID and permission decision ID.

For `Deny`, no preview should be applied. The implementation can deny before
preparing a transaction, which keeps after-images out of memory for blocked
requests. If a later reviewer route denies after preview, evidence should show
the prepared edit and denied outcome. If validation failed before a transaction
exists, evidence should record only safe categorical failure summaries.

### Result Shaping

Agent-visible edit results should be bounded and redacted. They may include:

- tool request ID;
- preview ID or transaction ID when safe;
- outcome: `applied`, `rejected`, `denied`, `validation_failed`, or `failed`;
- relative path;
- operation kind;
- before/after hashes where applicable;
- byte counts;
- hunk count;
- whether the diff summary was truncated;
- categorical reason label.

They must not include:

- full file bodies;
- raw request JSON;
- absolute host paths;
- resolved symlink targets;
- raw provider payloads;
- extension process output;
- unbounded diff text.

### Provider Relationship

The first agent edit tool implementation should add provider-visible schemas
only for the canonical built-in `edit_text_file` and `create_text_file` tools,
and only when policy enables the agent edit surface for that session.

The provider-visible result should be a bounded tool result, not raw diff or
file content. It should preserve the provider call ID, report final outcome,
and include only the redacted result fields listed above.

Review pausing should fit the existing one-round continuation boundary:

1. provider returns one or more tool calls;
2. yach accepts at most the implementation-plan-approved edit call shape;
3. yach prepares and either applies immediately, denies, fails, or pauses for
   user review;
4. after final decision, yach sends one bounded tool result continuation;
5. continuation requests still strip advertising and do not open an unbounded
   multi-round tool loop.

Provider-visible mutation beyond this first exact/create surface should use a
new design and should answer these questions first:

- Does the provider receive a preview, a redacted result, or both?
- How are cancellation and stale previews handled?
- What happens if the provider requests another edit after one is denied?
- How are provider tool-call IDs correlated with edit transaction IDs?

Until then, broad mutating tool definitions should remain absent from
`yach.provider_tool_advertising.v1`.

### Extension Relationship

Extensions should eventually be able to contribute edit-capable tools, but they
should do so by compiling their intent into yach-owned edit transactions. They
should not receive direct workspace write handles or authority to append session
evidence.

The future extension mutation path should require:

- manifest-declared mutation capability;
- accepted host registration with `NativeToolRisk::MutatesLocalState`;
- policy classification by core;
- invocation through the same agent tool pipeline;
- yach-owned preview/apply/reject;
- yach-owned evidence;
- core final-deny authority;
- explicit prevention of extension self-approval.

No extension host should be activated before first paint solely to make mutation
tools available. Extension-provided mutation tools can appear after activation
for later turns, consistent with the existing extension startup direction.

### UI Relationship

The temporary `/debug-edit` command should remain clearly labeled as a manual
harness. The real surface should be transcript-native:

- the agent requests an edit tool;
- the transcript shows the tool call and diff preview;
- the user applies or rejects when review is needed;
- the transcript records the final tool outcome.

This keeps user intent conversational while still making mutations visible and
inspectable.

The existing local edit protocol can be reused or generalized, but the
implementation plan should avoid baking "local UI manually requested this" into
agent edit review messages. A likely protocol evolution is to add a generic
tool review event and let local edit previews be one review payload type.

## Testing Strategy

The implementation plan should include tests for:

- built-in edit tool definitions are present in the agent tool catalog;
- edit tools are provider-advertised only when the session policy enables the
  canonical built-in schemas;
- `edit_text_file` validates required fields and rejects oversized or unknown
  fields;
- `edit_text_file` computes `expected_sha256` during yach-owned normalization;
- `create_text_file` validates required fields and rejects overwrite attempts;
- denied edit permissions record tool and permission evidence without applying;
- ask-mode edit permissions produce a review request and do not apply until
  approval;
- allow-mode edit permissions persist tool request, permission, prepared-edit,
  and `ApplyStarted` evidence before writing;
- applied/rejected/failed outcomes correlate tool request ID with edit evidence;
- provider call IDs are preserved in bounded continuation results;
- provider transcript projection ignores edit and permission evidence;
- extension mutation registrations remain rejected until the future extension
  mutation design changes policy.

## Acceptance Criteria

This design is accepted when it is clear that:

- the product edit surface is agent-selected tools, not a user slash command;
- the first mutation tools are narrow `edit_text_file` and `create_text_file`;
- all mutation tools share one edit/file-mutation permission family;
- invocation routes through `NativeEditAccess`, not public apply or raw writes;
- tool evidence and edit evidence are correlated by tool request ID;
- provider advertising is policy-gated to the canonical first edit schemas
  rather than opened to arbitrary mutation tools;
- extension mutation remains future-scoped but has a compatible target seam;
- startup performance constraints are preserved.

## Follow-Up

After this spec is accepted, write an implementation plan for the canonical
policy-gated `edit_text_file` and `create_text_file` provider-visible schemas
and the native agent tool invocation/review path.

Production edit tracing should come after the concrete agent tool surface is
implemented or at least planned, so trace IDs and timings match the final
review/apply states.
