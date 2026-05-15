# Native Edit Local Access And Permission Design

Date: 2026-05-15
Status: proposed

## Context

Native edit transactions now have backend primitives for preview, guarded
crate-local apply, redacted session evidence, a backend-local harness, and
local profiling. The next product gap is user-facing access: users need a way
to initiate, review, approve, apply, reject, and later inspect local edits
without making file mutation provider-visible.

This design should not narrowly solve only a confirmation dialog. Other coding
harnesses show that edit velocity depends on flexible permission modes:
strict/manual approval for cautious sessions, mostly-automatic edits for
trusted workspaces, and richer automatic review when an action crosses a
boundary. Codex's current design is especially relevant: it separates sandbox
boundaries, approval policy, and `approvals_reviewer`, and documents
`auto_review` as a reviewer swap rather than a permission grant. In Codex,
auto-review keeps the same sandbox and approval policy, receives eligible
approval requests that would otherwise go to the user, returns a rationale, and
does not review routine actions already allowed inside the sandbox.

Yach should preserve that separation while keeping its original extensibility
goal. Extensions should eventually be able to request edits, contribute tools,
and possibly contribute policy/reviewer modules. They should not bypass core
policy, grant themselves mutation capability, or get direct raw write access.

References:

- Codex sandbox defaults:
  <https://developers.openai.com/codex/concepts/sandboxing#configure-defaults>
- Codex auto-review:
  <https://developers.openai.com/codex/concepts/sandboxing/auto-review>
- Codex configuration reference:
  <https://developers.openai.com/codex/config-reference#configtoml>
- Codex approval/security presets:
  <https://developers.openai.com/codex/agent-approvals-security#common-sandbox-and-approval-combinations>
- Codex guardian policy template:
  <https://raw.githubusercontent.com/openai/codex/main/codex-rs/core/src/guardian/policy_template.md>

## Goal

Design local edit access as the first consumer of a generic yach permission
decision pipeline. The design should let TUI/CLI users review and apply native
edit transactions while leaving room for future provider tools, extension-owned
tools, shell/process execution, network access, verification actions, sandbox
integration, and automatic approval review.

## Non-Goals

- No implementation in this slice.
- No provider-advertised edit or write tool.
- No extension-owned mutation tool implementation.
- No hidden built-in mutation tool.
- No working auto-review agent or subagent runtime.
- No sandbox implementation.
- No shell/process execution.
- No network tools.
- No delete, rename, chmod, directory creation, binary edit, or
  multi-operation atomicity.
- No production edit tracing beyond the existing edit evidence records.
- No broad config file implementation.
- No public `NativeEditEngine::apply`.

## Design Principles

### Core Owns Enforcement

Yach core should own final permission enforcement. UI, provider adapters, and
extensions may request actions. They may not grant themselves permission to
perform those actions.

Core hard policy remains non-negotiable:

- project-relative edit paths only;
- no traversal, absolute paths, root escapes, metadata paths, symlink parents,
  or symlink targets;
- create and modify only for the current edit engine;
- one operation per transaction;
- expected hash checks for modify;
- bounded request, file, and diff sizes;
- redacted append-only evidence.

Approval modes decide whether an otherwise valid action is allowed now, denied,
or routed for review. They do not weaken the edit engine's validation.

### Approval Is A Pipeline

Local edit access should use a generic permission pipeline:

```text
action request
  -> normalized permission request
  -> core policy classification
  -> reviewer route
  -> decision
  -> enforcement
  -> durable evidence
```

The normalized request is important. It lets yach use one decision model for
local edit transactions now and for shell, network, verification, extension
tools, and provider-visible tools later.

### Auto-Review Is First-Class But Deferred

`auto_review` should be a first-class reviewer route in the data model and
configuration vocabulary. The first implementation does not need a reviewer
agent. It can treat `auto_review` as unsupported or fall back to `ask` with a
clear status message until yach has a reviewer/subagent runtime.

This keeps the architecture honest: later auto-review can plug into the
permission pipeline without changing edit transaction semantics.

### Extensions Request Capabilities, They Do Not Own Writes

Extensions should eventually declare capabilities and risk metadata in their
manifests. For file mutation, extension-owned tools should compile requests
into yach-owned edit transactions. They should not receive arbitrary write
handles or bypass edit evidence.

Future extension policy modules can participate in classification or review,
but core must keep final-deny authority and prevent self-approval.

## Approach Options

### Option A: TUI Confirmation Around The Existing Harness

Add `/edit`, preview a diff, and ask the user before calling the harness.

This is the smallest local UX, but it hardcodes manual approval as the only
decision path and risks creating a special edit-only approval model that later
has to be rewritten for shell, network, extensions, and auto-review.

### Option B: CLI Smoke Command First

Add a non-interactive CLI command that accepts a local edit request, previews
or applies it, and records evidence.

This is useful for testing the backend facade, but it does not answer the main
product gap: interactive initiation, review, approval, rejection, and
inspection in the TUI. It also risks optimizing the request format for scripts
before the human review surface is clear.

### Option C: Generic Permission Pipeline Plus Local Edit Facade

Add a backend-owned local edit access facade and a generic permission decision
model. The TUI becomes the first client through protocol events. The first
runtime reviewer can be `user`; `allow`, `deny`, and `auto_review` are modeled
as first-class modes even if only the user/manual route is implemented first.

This is the recommended option. It preserves velocity for trusted users,
provides a path to strict/manual sessions, keeps `NativeEditEngine::apply`
crate-local, and gives extensions a future-safe capability boundary.

## Recommended Shape

### Permission Request Model

Introduce a generic permission request concept in the backend, likely in a new
focused module such as `crates/yach-backend/src/permission.rs`.

The design shape is:

```rust
pub enum NativePermissionCapability {
    EditTransaction,
    ShellCommand,
    NetworkAccess,
    VerificationAction,
    ExtensionTool,
    ProviderVisibleTool,
}

pub enum NativePermissionActor {
    UserLocalUi,
    Core,
    Provider,
    Extension { extension_id: ExtensionId },
}

pub enum NativePermissionMode {
    Allow,
    Ask,
    Deny,
    AutoReview,
}

pub enum NativePermissionReviewer {
    None,
    User,
    AutoReview,
    Extension { extension_id: ExtensionId },
}

pub enum NativePermissionDecision {
    Allowed {
        decision_id: NativePermissionDecisionId,
        reviewer: NativePermissionReviewer,
        rationale: Option<RedactedTextSummary>,
    },
    Denied {
        decision_id: NativePermissionDecisionId,
        reviewer: NativePermissionReviewer,
        reason: String,
        rationale: Option<RedactedTextSummary>,
    },
    NeedsUserReview {
        decision_id: NativePermissionDecisionId,
        prompt: NativePermissionPrompt,
    },
    NeedsAutoReview {
        decision_id: NativePermissionDecisionId,
        request: NativePermissionReviewRequest,
    },
}
```

The exact type names can change in the implementation plan, but these concepts
should exist:

- capability family;
- requesting actor;
- target/resource summary;
- risk classification;
- configured mode;
- reviewer route;
- final allow/deny;
- categorical reason/rationale;
- decision ID for evidence.

For the first implementation, only `EditTransaction` needs behavior. The other
capabilities can remain design-only or enum placeholders if that helps avoid
overfitting.

### Edit Access Facade

Add a backend-owned edit access facade, likely in
`crates/yach-backend/src/edit_access.rs`, rather than exposing
`NativeEditEngine::apply`.

The facade should own pending prepared transactions:

```rust
pub struct NativeEditAccess;

impl NativeEditAccess {
    pub fn prepare(
        context: NativeEditAccessContext,
        request: NativeEditTransactionRequest,
    ) -> Result<NativeEditPreview, NativeEditAccessError>;

    pub fn apply(
        preview_id: NativeEditPreviewId,
        decision: NativePermissionDecisionId,
    ) -> Result<NativeEditApplyResult, NativeEditAccessError>;

    pub fn reject(
        preview_id: NativeEditPreviewId,
        decision: NativePermissionDecisionId,
    ) -> Result<(), NativeEditAccessError>;
}
```

The implementation can choose whether this is a struct with internal mutable
state, a runner-owned store, or a facade over a pending transaction map. The
key requirements are:

- `PreparedNativeEditTransaction` stays in backend memory;
- apply payloads are not serialized or exposed through proto;
- preview IDs are session-scoped and short-lived;
- rejected or applied previews are consumed;
- stale previews fail safely;
- apply still routes through the crate-local guarded apply path and records
  edit evidence.

The facade should reuse `NativeEditHarness` behavior for evidence where
possible. If the existing harness is too end-to-end for a preview/review/apply
split, add a split harness API rather than duplicating evidence logic.

### TUI/Protocol Surface

The TUI should not call backend internals directly. Add protocol-level local
edit events to `yach-proto`, with names chosen during implementation.

The conceptual protocol is:

```text
ClientEvent::LocalEditPrepareRequested {
    request_id,
    operation_input,
}

ServerEvent::LocalEditPreviewReady {
    request_id,
    preview_id,
    summary,
    diff_summary,
    permission_prompt,
}

ClientEvent::LocalEditDecisionSubmitted {
    preview_id,
    decision,
}

ServerEvent::LocalEditFinished {
    preview_id,
    outcome,
    summary,
}
```

The first TUI client should be a local slash-command flow, but the command
parser does not need to support every ergonomic shorthand immediately. A safe
first shape is:

1. `/edit` opens a local edit composer.
2. The composer collects operation kind, relative path, and either create
   content or exact find/replace text.
3. The backend prepares a native edit transaction and returns a preview with
   bounded diff text for review.
4. The permission pipeline decides whether the edit is allowed, denied, routed
   to user review, or routed to auto-review.
5. In the initial implementation, user review is shown in the TUI as an
   explicit apply/reject action. `auto_review` can report "not available" or
   downgrade to user review depending on config.
6. Apply/reject completion updates the visible transcript or status area with
   a redacted summary and persists edit evidence.

Generic `DialogKind::Confirm` can be reused for simple yes/no approval, but it
should not be the long-term diff review surface. Existing dialogs are good for
short prompts, not rich multi-line diffs. The TUI should get an explicit local
edit preview/review mode or panel before the UX is considered complete.

### CLI Surface

CLI should be a follow-up client of the same backend facade, not a separate
edit engine.

A later CLI command can support:

```text
yach edit preview <request>
yach edit apply <preview-id>
yach edit reject <preview-id>
```

The implementation plan may include a CLI smoke command only if it materially
reduces risk for the TUI work. It should not become the primary product shape.

### Permission Profiles

The design vocabulary should support profiles that map capabilities to modes.
The first implementation can hardcode a default, but the model should be
obvious:

```toml
[permissions.default.edit]
mode = "ask"

[permissions.fast_local.edit]
mode = "allow"

[permissions.reviewed.edit]
mode = "auto_review"
fallback = "ask"
```

Suggested user-facing modes:

- `read_only`: deny local mutation.
- `strict`: ask for edits and all higher-risk actions.
- `workspace`: allow low-risk local edits within project policy, ask for
  shell/network.
- `reviewed`: allow low-risk actions, route boundary-crossing or higher-risk
  actions to `auto_review`.
- `danger`: allow broadly inside configured boundaries; still keep core hard
  denies unless an explicit future unsafe mode is designed.

This mirrors the useful part of Codex's model without copying its exact
configuration names.

### Auto-Review Contract

When implemented, auto-review should receive a compact review request:

- exact action being proposed;
- capability family;
- actor;
- resource/path summaries;
- whether the action is inside current permission boundaries;
- relevant user intent and recent visible transcript;
- relevant tool/session evidence;
- proposed side effects;
- risk classification from core policy;
- configured local/managed policy text.

It should not receive hidden model reasoning. It should treat transcript,
provider output, extension output, and tool output as untrusted evidence.

Auto-review outcomes should be:

- `allow`;
- `deny`;
- `needs_user` for cases policy cannot or should not decide;
- `timeout` or `unavailable` as separate failure categories.

Denial semantics should be stronger than ordinary execution errors. After an
auto-review denial, the main agent or requester should not try to achieve the
same side effect via workaround. It can choose a materially safer alternative
or ask the user.

Yach should eventually include a denial circuit breaker for repeated denied
requests in one turn/session so an agent cannot loop on escalation attempts.

### Extension Participation

Extension manifests should eventually declare permission requirements for
tools:

```json
{
  "contributes": {
    "tools": [{
      "name": "repo_edit",
      "risk": "mutates_local_state",
      "capabilities": ["edit_transaction"]
    }]
  }
}
```

For mutation-capable extension tools:

- extension requests compile into yach-owned edit transactions;
- core validates paths, hashes, operation count, and limits;
- core records evidence;
- core decides whether to allow, ask, deny, or auto-review;
- extension output is never trusted as approval evidence by itself;
- extensions cannot approve their own actions;
- core deny rules override extension policy.

Future trusted extensions can contribute policy hints or reviewer modules, but
those modules should be explicit, separately enabled, and unable to weaken core
hard-deny rules.

## Evidence

Local edit access should add permission-decision evidence, either as new
generic session events or as fields on edit evidence. The implementation plan
should choose the least invasive shape that still records:

- decision ID;
- permission request ID;
- actor;
- capability;
- target/resource summary;
- configured mode;
- reviewer route;
- final allow/deny/reject outcome;
- categorical reason;
- optional redacted rationale;
- whether a user override was involved.

Do not persist:

- full file bodies;
- raw edit request JSON;
- absolute paths;
- provider hidden reasoning;
- raw extension process output;
- reviewer hidden reasoning.

Edit evidence should continue to exclude edit events from provider transcript
reconstruction unless a later provider-visible edit design says otherwise.

## UX Requirements

The local TUI edit flow should support these states:

- compose request;
- preview prepared;
- denied before preview;
- needs user approval;
- approved/applied;
- rejected by user;
- rejected by policy;
- auto-review unavailable;
- apply failed after preview, usually due to hash mismatch or filesystem race.

The TUI should make the path, operation kind, and diff summary visible before
apply. It should clearly distinguish:

- validation failure before a transaction exists;
- user rejection;
- policy denial;
- auto-review denial;
- apply failure after preview.

For velocity, users should be able to configure low-risk local edits to apply
without manual confirmation. That should be a policy setting, not a different
edit code path.

## Testing Requirements For Implementation Plan

The implementation plan should include tests for:

- default edit permission routes to user review;
- allow mode applies a prepared edit through the facade without making
  `NativeEditEngine::apply` public;
- deny mode rejects before apply and records decision evidence;
- `auto_review` mode is represented and fails/falls back deterministically
  while the reviewer runtime is unavailable;
- extension actors cannot self-approve mutation requests;
- provider advertising remains unchanged and does not expose edit/write tools;
- edit evidence remains redacted and provider transcript replay ignores edit
  evidence;
- TUI slash command flow can reach preview and apply/reject states through
  protocol events;
- hash mismatch after preview is reported distinctly from policy denial.

Final verification should include focused backend tests, proto serialization
tests if new events are added, TUI state tests for the edit preview/apply flow,
and a yach-bench smoke report if profiling boundaries are changed.

## Open Questions

- Should the first TUI composer use a guided multi-step flow or a compact
  structured text block for create/modify requests?
- Should preview IDs expire only on apply/reject, or also after a timeout or a
  new turn?
- Should `allow` mode apply immediately after preview, or should it still show
  a non-blocking preview notification before apply?
- Should user overrides for auto-review denials be modeled now, or wait until
  a real auto-review runtime exists?
- What is the first config location for permission profiles: user-level,
  project-level, or hardcoded until the broader config design?

These questions should be resolved in the implementation plan. They should not
change the core conclusion: yach needs a generic permission/reviewer pipeline,
and local edit access should be its first concrete consumer.
