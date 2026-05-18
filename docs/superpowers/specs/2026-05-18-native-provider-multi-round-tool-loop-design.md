# Native Provider Multi-Round Tool Loop Design

Date: 2026-05-18
Status: accepted

## Context

Native-provider dogfooding exposed a structural limit in the current provider
tool path. The runner can advertise and execute read/search/list and exact edit
tools, but the orchestration remains a one-round continuation:

1. send an initial provider request;
2. execute the first batch of tool calls;
3. send one continuation with tool results;
4. require the continuation to produce final assistant text.

That works for simple single-step tool use, but it fails normal coding-agent
flows such as read a file, then edit it. The provider can receive the read
result and decide that another tool call is needed, but yach currently treats
that second-round tool call as an error. Guard-message tweaks and mutation
intent heuristics are fragile because they try to patch over the missing state
machine instead of representing the actual agent workflow.

The next design should replace the one-round boundary with a bounded
multi-round provider tool loop while preserving yach-owned tool execution,
permission, review, evidence, and future extension compatibility.

## Goal

Design a backend-owned native-provider tool loop that can repeatedly:

- collect provider tool calls;
- resolve them through yach's active tool catalog;
- validate, authorize, review, execute, and shape results;
- persist redacted evidence before provider continuation;
- send another provider request with tool results and current provider-visible
  tool schemas;
- stop only on final assistant text, cancellation, denial, failure, or a hard
  limit.

The loop must be registry-first. Built-in tools are the default implementations,
but the loop should not assume tools are permanently hardcoded into core.
Extensions must be able to contribute new tools, and future explicit policy
must allow extensions to replace built-in tool names.

## Non-Goals

- No implementation in this slice.
- No full extension runtime, installer, package manager, hot reload, or host
  lifecycle design.
- No shell/process tools.
- No network or web-fetch tools.
- No broad mutation tools beyond the existing exact/create edit schemas.
- No working auto-review reviewer/subagent runtime.
- No sandbox implementation.
- No provider-owned tool execution.
- No unbounded provider result bodies, transcript writes, or session evidence.

## Architecture

The native-provider turn should become a backend-owned state machine:

```text
initial provider request
  -> collect provider response
  -> if tool calls: execute approved tool batch
  -> append bounded tool results
  -> next provider request
  -> repeat until terminal state
```

Before the first provider request, yach builds a resolved provider-turn tool
catalog. The catalog combines:

- core built-ins such as `read_text_file`, `search_project`,
  `list_project_paths`, `edit_text_file`, and `create_text_file`;
- activated extension tools such as future `fffind` or `ffgrep`;
- future explicit user/profile override choices.

The provider only sees the resolved, policy-approved, provider-visible schemas
for the turn. The loop executes by resolved definition and route, not by a
closed list of built-in names. Built-ins and extensions differ by provenance and
executor route; they do not get separate provider-loop semantics.

This updates the earlier extension-tool-registration direction. That design
rejected built-in name collisions for the first safe registration surface. For
the multi-round loop and future runtime work, built-in replacement should be a
first-class capability, but only through explicit override policy. Accidental
collisions still fail closed.

## Tool Resolution And Replacement

The active catalog should support three collision/replacement states:

- `deny`: default for tool name collisions;
- `alias_only`: extension registers only under its own name, such as `ffgrep`;
- `replace_builtin`: extension replaces a built-in name for the configured
  profile/session.

Resolved tool metadata should retain provenance even when a built-in is
replaced:

- provider-facing tool name;
- owner, such as `BuiltIn` or `Extension { extension_id }`;
- implementation label;
- risk class;
- whether this definition replaced a built-in;
- original built-in name when applicable.

Session evidence should persist enough provenance for later inspection. A user
should be able to tell whether `search_project` was handled by core yach or by
an extension-backed implementation.

The loop should not load or activate extension hosts on the TUI first-paint
path. Extension runtime work should decide when provider-visible extension
tools become available for a turn. This loop design only requires that once a
tool is resolved into the active catalog with an executable route, it can
participate in every tool round.

## Loop Semantics

The loop terminal states are:

- provider returns final assistant text with no tool calls;
- provider request or stream fails;
- provider stream is cancelled;
- provider stream ends without completion;
- user cancels the turn;
- tool validation, permission, execution, or result shaping fails;
- review denies or rejects a tool call in a stop-the-turn mode;
- a configured limit is reached.

Provider responses should be buffered until yach knows whether they contain
tool calls. If a provider response contains tool calls, any assistant text from
that same response is not committed as final assistant text. It may be recorded
as diagnostic provider text later if useful, but it must not appear as the
answer in the user transcript.

Provider-visible tool schemas should remain available on every loop round. The
current one-round guard that says no additional tools are available becomes
incorrect. Replace it with a per-round harness message shaped like:

```text
Yach executed exactly the tool results included in this continuation. You may
call more advertised tools if more work is required, or answer only from
executed evidence. Do not claim local effects unless they are present in the
tool results.
```

The key behavioral rule is: no model-authored success unless the required tool
evidence exists. If yach stops because of a limit, denial, failed tool, or
unsupported next action, the user-visible transcript should receive a
harness-authored blocked/failed/denied/cancelled result instead of relying on
the model to narrate what happened.

## Limits

The first implementation should use conservative hard limits:

- `max_tool_rounds`: total provider responses that may contain tool calls,
  initially `4`;
- `max_tool_calls_per_round`: initially the current value, `4`;
- `max_total_tool_calls`: initially `12`;
- `max_result_bytes_per_tool`: initially `64 KiB` for provider-visible content;
- `max_total_result_bytes`: initially `256 KiB`;
- review wait: no hard timeout while the user is actively reviewing, but
  cancellation must interrupt the wait.

The loop should record which limit stopped the turn. Limit stops are harness
outcomes, not model text.

## Permissions And Reviews

Every tool call in every round must pass through the same yach-owned pipeline:

```text
schema validation
  -> risk classification
  -> permission policy
  -> optional reviewer
  -> execution
  -> result shaping
  -> evidence
  -> provider result
```

Permission is evaluated per resolved tool definition. If an extension replaces
`search_project`, it still carries a read-content risk class. If an extension
eventually replaces `edit_text_file`, it still lands in the mutation permission
family and review path.

Risk behavior should remain separated:

- metadata tools can run without UI review when policy allows;
- content reads are separately configurable;
- mutation tools use edit review/approval unless explicitly allowed;
- shell/process/network tools remain out of scope until their own specs define
  sandbox and approval semantics;
- auto-review is reserved as a future reviewer strategy, not implemented here.

Extension-host execution must remain bounded by yach:

- timeout;
- maximum result bytes;
- structured error mapping;
- no direct session writes;
- no direct provider calls;
- no hidden provider-visible tool calls from inside the extension host.

## Data Flow And Evidence

Each executed tool should create durable evidence before its result is sent back
to the provider. Evidence should include:

- tool request recorded;
- validation and permission decision;
- review requested, waited, approved, or rejected for reviewed tools;
- execution finished with bounded summary;
- edit trace records for mutation;
- provider continuation trace for result submission.

Multi-round tracing should add a generic provider tool-loop round concept. Each
provider request/response should be correlated by:

- session id;
- turn id;
- provider request index;
- provider response id when available;
- tool round index;
- provider tool call id;
- native tool request id;
- edit trace id when applicable;
- extension id or owner when applicable.

Provider-visible result content may include bounded file text or search snippets
when policy allows. Durable session evidence should continue to store summaries
and redactions only, not file bodies, search result bodies, raw queries, raw
extension outputs, or raw provider payloads.

If the loop fails, the transcript should show a harness-authored outcome:
blocked, failed, denied, cancelled, or limit reached. That outcome should be
distinguishable from model-generated assistant text.

## Error Handling

Fail closed on:

- unknown tool names;
- malformed arguments;
- disallowed risk class;
- denied permission;
- rejected review when configured as stop-the-turn;
- missing executable route;
- extension host timeout or crash;
- oversized tool result;
- aggregate result budget exhaustion;
- provider request failure;
- provider stream cancellation or incomplete completion;
- invalid provider continuation mapping.

For errors that are safe for the model to see, yach may return a bounded tool
result describing the failure if that helps the model recover within limits. For
errors that should stop the turn, yach should finish with a harness-authored
failure/blocked outcome. The implementation plan should make this distinction
explicit for each risk class.

## Testing

Use fake provider requesters for most coverage. Required design-level test
cases:

- no-tool response still completes in one provider request;
- read -> edit -> final answer works across multiple provider requests;
- multiple tool calls in one round preserve order and evidence;
- second and later rounds still advertise tools;
- loop limits stop with harness-authored outcomes;
- denied or rejected edits stop or return a bounded denial result
  consistently;
- extension-owned tool executes through the same loop;
- configured extension replacement of a built-in records provenance;
- accidental built-in collision fails closed;
- provider failure, incomplete stream, cancelled stream, oversized result, and
  malformed arguments all stop deterministically.

Dogfood validation should repeat the failed real-session case:

```text
Use read_text_file to inspect dogfood-provider-edit.txt, then replace "ok" with
"passed".
```

Expected behavior: yach should allow the provider to read, receive the content,
request the edit in a later round, route review, apply or reject based on user
decision, and then produce final assistant text only after the edit tool result
exists.

## Follow-Up Work

The next likely design priority after this loop is the extension runtime and
install/package UX. That design should cover:

- language-agnostic extension host activation;
- TypeScript and Rust extension ergonomics;
- Pi-like install/drop-in UX where practical;
- explicit override policy for replacing built-in tools;
- provider-visible extension tool availability before a turn;
- startup performance guarantees.

Other separate designs are still needed for:

- shell/process tools;
- network and web tools;
- broader mutation tools;
- sandboxing;
- auto-review reviewer runtime;
- production performance profiling for multi-round tool loops, including
  provider round count, tool latency, review wait, extension-host latency, and
  aggregate result size.

## Acceptance Criteria

This design is sufficient when it can drive an implementation plan that:

- replaces the one-round native-provider continuation boundary with a bounded
  multi-round tool loop;
- keeps yach in charge of validation, permissions, review, execution, evidence,
  and provider continuation;
- keeps provider-visible tool schemas available across rounds;
- preserves the ability for extensions to add and explicitly replace tools;
- stops deterministically with harness-authored outcomes when work cannot
  continue safely.
