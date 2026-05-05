# Real Provider Continuation Adapter Mapping Plan

Date: 2026-05-05
Status: planning recommendation; implementation not started
Related: `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`, `.project/phases/05-native-tools-resources-session-hardening.md`, `docs/spikes/2026-04-28-rig-provider-evaluation.md`

## Goal

Plan how yach should map yach-owned provider-bound tool results into real provider adapter continuation requests without letting provider SDKs execute tools, own sessions, or persist raw local data.

This is a planning-only slice. It does not integrate real continuation into `--backend native-provider`, does not make live provider calls, and does not add file/process/network tools.

## Current baseline

Yach now owns the backend-only fixture half of the loop:

- `ProviderToolCall` preserves provider call id, tool name, and JSON arguments.
- Provider tool calls can become yach-owned `PendingNativeToolRequest`s.
- Tool validation/execution/session records are yach-owned and redacted.
- `NativeProviderToolResult` represents provider-bound tool result content with status, provider call id metadata, byte count, redaction, truncation, and reason fields.
- Fixture continuation tests validate/execute fixture-safe tools and return provider-bound results without provider SDK continuation.

Real provider continuation still needs a provider seam shape and adapter mapping.

## Recommended seam shape

Add a separate backend-owned continuation request type rather than overloading the initial `ProviderRequest` immediately.

Proposed shape:

```rust
pub struct ProviderContinuationRequest {
    pub turn_id: NativeTurnId,
    pub model: ProviderModel,
    pub prior_messages: Vec<ProviderMessage>,
    pub tool_results: Vec<NativeProviderToolResult>,
    pub extensions: Vec<ProviderExtension>,
}
```

Rationale:

- Keeps initial prompt requests simple and dogfood-stable.
- Makes continuation rounds explicit for loop limits and tests.
- Keeps yach-owned tool result metadata visible at the seam.
- Lets each provider adapter translate into SDK-specific tool-result messages below the seam.

Do not expose provider SDK message/thread objects through this type.

## Adapter mapping requirements

For each provider adapter implementation:

- Preserve provider tool call id only as provider metadata needed to pair results.
- Convert yach result status/content into provider-specific tool-result payloads.
- Refuse to continue if a provider requires raw tool arguments/results that yach has redacted or not retained.
- Preserve yach turn id and model metadata in emitted `ProviderStreamEvent`s.
- Surface continuation failures through normalized `ProviderError` kinds.
- Do not mutate `NativeSessionLog` inside provider adapter code.
- Do not execute tools, validate schemas, or decide permissions inside provider adapter code.

## Initial provider target

Do not start with live `--backend native-provider` integration.

Recommended implementation order:

1. Add provider-independent continuation request/result mapping types and fixture tests in `yach-backend`.
2. Add adapter-level unit tests using synthetic provider SDK/tool-result payloads where possible.
3. Only after fixture and adapter mapping tests pass, select one real provider path for explicit no-default continuation dogfood.

Candidate real path later: whichever Rig path exposes tool-result continuation with the least SDK leakage and strongest deterministic tests. Do not assume Anthropic or ChatGPT/Codex until adapter APIs are inspected in code.

## Loop/limit integration

Continuation adapter mapping should receive already-limited `NativeProviderToolResult`s. It should still validate:

- every tool result has provider call id when the target provider requires one,
- provider-bound content length stays below adapter/provider caps,
- redaction/truncation metadata is compatible with provider submission policy,
- continuation round count remains below yach loop policy.

If validation fails, return a normalized provider error before any network call.

## Session and protocol boundaries

Session mutation remains outside provider adapters:

- native runner/loop records tool request, validation, execution, and final turn outcome;
- provider adapter only emits provider stream events;
- native JSONL remains backend-internal/provisional.

No `yach-proto` change is needed for adapter mapping tests. Protocol/UI work is only needed for user-visible approvals, tool progress display, or real interactive tool execution.

## Recommended first implementation slice

Backend-only continuation seam/mapping skeleton:

- Add `ProviderContinuationRequest` or equivalent backend-owned type.
- Add validation helper for provider-bound tool results before adapter submission.
- Add tests for missing provider call id, oversized content, redacted/truncated allowed-vs-rejected policy, and preservation of yach-owned metadata.
- Do not call Rig or real provider SDKs yet.
- Do not integrate with `--backend native-provider`.

Suggested validation:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```

## Stop/approval gates

Stop before:

- real provider SDK continuation mapping,
- live provider calls,
- `--backend native-provider` integration,
- file/process/network tools,
- sending local resource contents to a provider,
- provider SDK owning tool execution/session history,
- protocol/UI approval surfaces,
- raw provider/tool payload persistence.

## Recommendation

Proceed next, if approved, with backend-only continuation request validation/mapping skeleton. This keeps continuation semantics yach-owned and testable before any provider-specific SDK work or live dogfood.
