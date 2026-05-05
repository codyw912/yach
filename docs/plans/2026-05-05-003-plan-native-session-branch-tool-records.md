# Native Session Branch and Tool Record Shape Plan

Date: 2026-05-05
Status: planning recommendation; implementation not started
Related: `.project/phases/05-native-tools-resources-session-hardening.md`, `docs/plans/2026-05-05-001-plan-resource-config-root-policy.md`, `docs/plans/2026-05-05-002-plan-native-tool-lifecycle-permissions.md`, `docs/protocol/yach-proto-v0.md`

## Goal

Sketch provisional backend-internal native session records for branches, tool calls/results, provider metadata, and outcomes before resource/tool implementation starts writing richer logs.

The goal is not to stabilize the JSONL format or add migration tooling. It is to prevent accidental coupling to provider-hosted sessions, Pi session schemas, or UI-only branch summaries.

## Current baseline

`yach-backend` currently has backend-internal append-only JSONL records:

- `EntryAppended` with session id, entry id, optional parent entry id, turn id, role, text, and optional provider metadata.
- `TurnFinished` with session id, turn id, outcome, and optional reason.

This is enough for minimal native/native-provider dogfood prompts, provider metadata preservation, failure/cancel markers, and reload tests. It is not enough to represent tool lifecycle, branch/fork events, resource reads, or redacted debug evidence without overloading text entries.

## Shape principles

- Native ids are canonical for native sessions; provider response ids/call ids are metadata only.
- JSONL remains append-only and provisional.
- Records should separate user/assistant transcript text from tool/resource/debug events.
- Persist metadata, outcomes, redaction notes, and sizes by default; avoid raw local contents, tool args/results, credentials, and raw provider payloads.
- Branch/fork records should describe yach-owned structure, not copy Pi session internals.
- UI-visible protocol events are added only when the TUI needs behavior; backend records can come first.

## Proposed identifiers

Add or reserve the following yach-owned ids when implementation pressure arrives:

- `NativeBranchId` — yach-owned branch/thread id within a native session.
- `NativeToolRequestId` — yach-owned id for a requested tool execution.
- `NativeResourceReadId` — yach-owned id for a resource access attempt.
- `NativeRecordId` or sequence number — optional later helper for stable references within an append-only log.

Provider ids remain annotations:

- provider response id on assistant turn metadata
- provider tool call id on tool request metadata
- provider model/provider names as metadata

## Proposed record additions

### Branch records

`BranchCreated`:

- session id
- branch id
- optional parent branch id
- fork point entry id or turn id
- creation reason (`new_session`, `fork`, `tool_experiment`, `import`, `unknown`)

`BranchSelected` may be deferred unless backend-side active branch selection becomes persistent.

First branch implementation should not promise a user-visible session tree. It should only preserve enough parent/fork provenance for later reconstruction.

### Turn records

Keep `TurnFinished`, but consider adding a `TurnStarted` record once tools/resources can occur before assistant text is appended.

`TurnStarted` fields:

- session id
- turn id
- branch id
- parent entry id
- provider/model metadata when known

This avoids deriving active turn structure solely from text entries.

### Tool records

`ToolRequestRecorded`:

- session id
- turn id
- tool request id
- yach tool name
- provider call id metadata, if any
- validation state
- permission state
- redacted argument summary and byte count, not raw args by default

`ToolExecutionFinished`:

- session id
- turn id
- tool request id
- outcome (`completed`, `failed`, `denied`, `cancelled`, `validation_failed`)
- normalized error kind/reason when applicable
- result summary, byte count, truncation/redaction markers

Avoid recording tool result continuation to a provider until the provider loop is explicitly implemented.

### Resource records

`ResourceAccessRecorded`:

- session id
- turn id or tool request id if access happens inside a tool
- resource read id
- root class (`project`, `user`, `generated`, `compat_import`)
- relative/display path after policy normalization
- access kind (`metadata`, `read`, `discovery`)
- provider visibility decision (`never`, `explicit`, `allowed_by_policy`)
- outcome and byte count
- redaction/truncation notes

First resource implementation may not need session records if it is helper/test-only. Add records when a dogfood workflow actually performs resource access during a turn.

### Debug/evidence records

If needed later, add a deliberately explicit `DebugEvidenceRecorded` record with:

- feature/debug mode name
- redaction version
- retained fields summary
- retention note

Do not add this until debug payload policy is approved.

## Migration and stability cautions

- Keep docs and code comments saying native JSONL is backend-internal/provisional.
- Do not add import/export or migration commands in the first branch/tool record slice.
- If record names change during Phase 5, update tests and docs rather than adding compatibility shims too early.
- Only introduce migrations after dogfood data matters enough to preserve across versions.

## Protocol impact

No `yach-proto` change is required for backend-internal record additions.

Potential future protocol events:

- branch tree/list updates when the TUI needs native session navigation
- pending tool approval requests when permission UI exists
- tool progress/result display events when native tool execution is user-visible
- resource approval/status events when resource reads become interactive

Until then, use existing prompt/status/session-message events for native dogfood where sufficient.

## Recommended first implementation slice

After resource/tool planning is accepted, start with one of these small backend-only slices:

1. Add id/types and provisional `NativeSessionEvent` variants for tool request/result records with serialization tests only.
2. Add `TurnStarted` + optional branch id to new records while preserving existing minimal dogfood records.
3. Add resource access record variants only when a resource helper is used inside a native turn.

Recommended first choice: tool request/result record serialization tests after the tool registry skeleton exists, because tool calls create immediate pressure for non-text records.

Suggested validation:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```

## Deferred decisions requiring approval

- Declaring native JSONL stable or user-editable.
- Session migration/import tooling.
- User-visible branch tree policy.
- Persisting raw tool args/results, resource contents, or provider payloads.
- Provider-hosted session/thread synchronization.
- Protocol-level tool/resource/session tree events.
