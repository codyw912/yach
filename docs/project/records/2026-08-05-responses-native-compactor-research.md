# Responses Provider-Native Compactor Research

Date: 2026-08-05

This revisits the board item to evaluate an OpenAI Responses provider-native
compactor now that yach has an OpenAI Responses path. Scope is the current API,
the actual (not merely designed) yach seam, Rig 0.41, and the comparison cohort.
The decision is about preserving the opaque provider artifact while retaining
yach's append-only session record and provider-independent fallback.

## What OpenAI Responses offers now

OpenAI has three distinct context-state mechanisms; only the last two are
provider-native compaction.

1. **State references.** A Responses request may carry `previous_response_id`
to thread from a prior response; the [conversation-state guide](https://developers.openai.com/api/docs/guides/conversation-state#passing-context-from-the-previous-response)
says to send only the new user input on the next request. The same guide says
that response objects are retained for 30 days by default and `store: false`
disables that retention; durable [Conversations](https://developers.openai.com/api/reference/resources/conversations/methods/create)
and their items do not have that 30-day TTL. It also says prior-chain input
tokens remain billable. This is server-side state, not compaction. In
particular, top-level `instructions` are request-local and are not carried by
`previous_response_id` ([text-generation guide](https://developers.openai.com/api/docs/guides/text#message-roles-and-instruction-following)).
2. **Automatic server compaction.** On `POST /responses`,
`context_management: [{"type":"compaction", "compact_threshold": N}]`
automatically compacts when rendered tokens cross `compact_threshold`.
OpenAI's [Compaction guide](https://developers.openai.com/api/docs/guides/compaction#server-side-compaction)
and [platform create reference](https://platform.openai.com/docs/api-reference/responses/create)
explicitly document the `context_management` object; the guide says it emits an encrypted, opaque compaction item and that no
separate compact request is needed. In stateless-array mode that item is
carried into the next request; in `previous_response_id` mode the client sends
only the newest user message. The guide recommends `store: false` for a
ZDR-friendly stateless path, and says not to edit the item.
3. **Explicit stateless compaction.** `POST /responses/compact` accepts a full
currently-fitting input window and returns the canonical next window. That
output is opaque, can contain retained items in addition to the compaction
item, and must be passed to the next `/responses` call **as-is**—not pruned or
rendered as a human summary ([guide](https://developers.openai.com/api/docs/guides/compaction#standalone-compact-endpoint);
[platform compact reference](https://platform.openai.com/docs/api-reference/responses/compact)).
It is therefore the API shape that matches yach's existing manual/threshold
orchestration: yach decides *when*, OpenAI produces the replacement context.

This is not a prompt-cache or truncation feature. The current retention policy
also distinguishes them: `/v1/responses` application state is retained for 30
days by default or with `store=true`; ZDR forces `store=false`; and the policy
specifically says server-side compaction retains no data with `store=false`
([OpenAI data controls](https://developers.openai.com/api/docs/guides/your-data#v1responses)).

For comparison, Anthropic now has a beta server-side equivalent:
`context_management.edits: [{"type":"compact_20260112"}]` with beta header
`compact-2026-01-12`. It emits a `compaction` block, removes earlier blocks on
subsequent appended-history requests, supports a token trigger and custom
instructions, and is ZDR-eligible for the listed models
([Anthropic Compaction](https://platform.claude.com/docs/en/build-with-claude/compaction)).
This confirms provider-native compaction is no longer unique to OpenAI, but it
does not make a generic cross-provider wire abstraction safe: the artifacts
and replay rules are provider-specific.

## Yach: the implemented seam and slice 1

The design calls the intended plug-in `NativeCompactor`
([design, lines 25-28](../specs/2026-07-20-context-compaction-design.md#L25-L28) and [trait sketch, lines 104-107](../specs/2026-07-20-context-compaction-design.md#L104-L107)),
but there is **no symbol named `NativeCompactor` in the workspace**. The public
implemented seam is `Compactor`:

```rust
pub trait Compactor: Send + Sync {
    fn compact(&self, preparation: CompactionPreparation) -> CompactionFuture;
}
```

It receives an owned `CompactionPreparation`—serialized fold, optional prior
summary and details, kept-boundary id, pre-cut estimate, reason
(`Threshold | Manual | Overflow`), and optional `/compact` focus—and returns
`CompactionOutcome { summary, details }` or `SummaryFailed`
([`crates/yach-backend/src/compaction.rs:473-508`](../../../crates/yach-backend/src/compaction.rs#L473-L508);
[`session.rs:34-42`](../../../crates/yach-backend/src/session.rs#L34-L42)).
Core deliberately owns cut selection, accounting, and checkpoint writes.

Slice 1 behaves as follows.

- `CompactionCheckpoint` appends `summary`, `first_kept_entry_id`, before/after
  estimates, reason, compactor label, and arbitrary `details`; the log is not
  truncated ([`session.rs:368-385`](../../../crates/yach-backend/src/session.rs#L368-L385)).
  The newest checkpoint reconstructs model context as one summary plus events
  from its kept boundary ([`runner.rs:2326-2465`](../../../crates/yach-backend/src/runner.rs#L2326-L2465)).
- Defaults enable `summary`, reserve 16,384 tokens, keep 20,000, and trigger at
  90% ([`compaction.rs:19-45`](../../../crates/yach-backend/src/compaction.rs#L19-L45)).
  Cut selection folds from the prior checkpoint boundary, retains a recent tail
  at a user-entry boundary, and does not split a tool call from its result
  ([`compaction.rs:269-348`](../../../crates/yach-backend/src/compaction.rs#L269-L348)).
- Before a turn, the runner estimates provider-visible history plus static
  context, applies the threshold, compacts, rebuilds, and rejects only a
  post-compaction context that still exceeds the usable window
  ([`runner.rs:3316-3381`](../../../crates/yach-backend/src/runner.rs#L3316-L3381)).
  `/compact [focus]` maps to `CompactionRequested`, refuses during an active
  prompt, then runs the same orchestration with reason `Manual`
  ([`runner.rs:1068-1141`](../../../crates/yach-backend/src/runner.rs#L1068-L1141);
  [`yach-ui/src/slash_commands.rs:66-70`](../../../crates/yach-ui/src/slash_commands.rs#L66-L70)).
- Today the runner does **not dispatch the trait**: it rejects every configured
  name except `"summary"`, serializes the old range, makes a normal one-message
  summary request, and appends the checkpoint
  ([`runner.rs:3922-4053`](../../../crates/yach-backend/src/runner.rs#L3922-L4053)).
  Thus `Compactor` is the right shape in `compaction.rs`, but not yet the live
  extension point. The current text summary has a fixed eight-section,
  anchored-iteration prompt ([`compaction.rs:510-552`](../../../crates/yach-backend/src/compaction.rs#L510-L552)).

The seam is close, but it is insufficient unchanged for Responses compaction:
a `String` summary becomes a generic provider text message, whereas OpenAI
requires an ordered, opaque replacement-input array. `details: Value` can
persist that array, but context rebuilding and request construction must replay
it only on the matching OpenAI Responses transport. That is an explicit seam
completion, not a JSON blob hidden inside the existing summary string.

## OpenAI wiring and Rig 0.41

`RigProviderConfig::OpenAi` builds `rig::providers::openai::Client`, then its
`completion_model`; the `OpenAiCompatible` branch explicitly switches to chat
completions ([`crates/yach-backend/src/rig_adapter.rs:278-317`](../../../crates/yach-backend/src/rig_adapter.rs#L278-L317)).
Rig 0.41 documents Responses as its default OpenAI completion API
([versioned source](https://docs.rs/crate/rig-core/0.41.0/source/src/providers/openai/responses_api/mod.rs#1-14)).

Rig's typed `AdditionalParameters` exposes `previous_response_id`, `store`,
and `truncation`, among other normal-create fields
([versioned source](https://docs.rs/crate/rig-core/0.41.0/source/src/providers/openai/responses_api/mod.rs#1633-1683));
its `TruncationStrategy::Auto` is middle-item dropping, not server compaction
([versioned source](https://docs.rs/crate/rig-core/0.41.0/source/src/providers/openai/responses_api/mod.rs#1703-1712)).
It has neither a typed `context_management` field nor a `/responses/compact`
client operation in this source surface. yach can flatten arbitrary JSON into
a normal Rig completion request ([`rig_adapter.rs:667-689`](../../../crates/yach-backend/src/rig_adapter.rs#L667-L689)), so it could *send* an automatic-compaction parameter, but it cannot preserve
its required artifact: the adapter discards `RawStreamingChoice::Unknown` and
reasoning/additional-parameter events ([`rig_adapter.rs:190-237`](../../../crates/yach-backend/src/rig_adapter.rs#L190-L237)).

Conclusion: use a narrow yach-owned `reqwest` Responses implementation beside
Rig for explicit compaction and native replay. Do not fork Rig, and do not
make `previous_response_id` the normal session authority. yach already owns a
full append-only log, resume, cut decisions, and cross-provider fallback; a
30-day provider reference would weaken those properties.

## Cohort: native artifact versus local reduction

| Harness | Current compaction posture | Evidence |
| --- | --- | --- |
| Codex CLI | Provider capability selects Remote V2/V1; unsupported providers take a local summarization task. Its remote source has distinct `remote_v2`, `remote`, and `local` paths. A 2026 user report shows why model-level capability and local fallback are mandatory: `/responses/compact` failed for an otherwise usable `gpt-5.5`; treat that as reported reliability evidence, not API specification. | [source](https://github.com/openai/codex/blob/main/codex-rs/core/src/tasks/compact.rs), [issue #19400](https://github.com/openai/codex/issues/19400) |
| opencode | Local two-stage reduction: marks older completed tool outputs compacted after protected/recent budgets, then generates a summary retaining a tail. It has no provider-native compaction artifact in this path. | [source](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/session/compaction.ts) |
| Claude Code | Its public UX describes auto-compact and `/compact [focus]` as replacing the conversation with a structured summary. Claude Code is closed source; this is not evidence that it calls Anthropic's new beta API. Anthropic's API now separately supports the native compaction block described above. | [Claude Code docs](https://code.claude.com/docs/en/context-window), [Anthropic API](https://platform.claude.com/docs/en/build-with-claude/compaction) |
| Pi | Core Pi is local summarization with an extension hook. The maintained third-party OpenAI extension runs native Responses compaction while also keeping a portable text summary; its own benchmark reports a higher recall result but also more compaction output and larger downstream context, so it is directional rather than an apples-to-apples quality claim. | [Pi core](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/core/compaction/compaction.ts), [extension/readme](https://github.com/algal/pi-openai-server-compaction) |
| omp (Oh My Pi) | Local `shake`, text-summary, and default `snapcompact` strategies coexist with native OpenAI Responses paths. Its documented V2 stream or `/responses/compact` artifact is persisted as `preserveData.openaiRemoteCompaction`; native failure falls back to local summary. Its model roles choose models and overflow can promote to a configured larger-context target before compacting—orthogonal controls, not server state. | [omp compaction docs](https://github.com/can1357/oh-my-pi/blob/main/docs/compaction.md), [models](https://github.com/can1357/oh-my-pi/blob/main/docs/models.md) |

The cohort consensus is not "replace the log with provider state." It is
**keep an append-only local record, retain a portable local reduction, and add
a provider-native artifact only when the next turn can replay it exactly**.
Codex and omp additionally demonstrate the non-negotiable fallback: native
support is provider- and model-capability-specific, and endpoint failures must
not strand a long session.

## Recommendation: build now, as a narrow OpenAI capability

Build `openai-responses` now behind the intended compactor dispatch, but make
this a complete provider-native path rather than a new summary prompt:

1. Make the live runner dispatch `Compactor`; add an OpenAI-only outcome that
   contains the opaque returned replacement input plus portable summary/details.
   Persist it in checkpoint `details` with an explicit provider/model/wire
   version, never in `summary`.
2. Use raw authenticated `POST /v1/responses/compact` for manual, threshold,
   and overflow cuts. Reuse the exact current system instructions, tool
   definitions, and ordered provider items; store and replay the returned
   window verbatim only to compatible OpenAI Responses models.
3. Keep the current summary compactor as the default and mandatory fallback;
   select native compaction from a model capability, not merely provider name.
   On native unsupported/timeout/decode failure, leave the original log intact
   and run the normal summary path. Do not adopt `previous_response_id` as the
   canonical session mechanism.
4. Defer `context_management` automatic server compaction and Conversations:
   they would require every normal streamed turn to retain opaque output items,
   duplicate yach's trigger policy, and add 30-day/retention semantics without
   improving the explicit seam's first slice.

The deciding factors are decisive API support (`/responses/compact` is
stateless and ZDR-compatible), a real cohort implementation (omp), and yach's
existing durable checkpoint architecture. The costs are equally concrete: the
present seam is only partly wired, Rig cannot carry native artifacts end to
end, artifacts are model-specific and opaque, and the Codex report shows the
endpoint/model matrix can lag ordinary inference. Build only with a portable
fallback and exact replay; without both, wait rather than ship a fragile
OpenAI-only continuation.
