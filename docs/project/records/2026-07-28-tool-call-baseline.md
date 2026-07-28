# Tool-Call Baseline (2026-07-28)

Pre-refactor measurement for the native tool-call messages design
(`specs/2026-07-28-native-tool-call-messages-design.md`), slice 1. The
point is a falsifiable before/after: rates recorded here are what the
structural change must improve.

Method: 100 cells — 5 tasks x 4 provider profiles x 5 repeats, via
`just eval-sweep`, every cell verified on artifacts. No cell failed to
launch. Raw rows and per-cell artifacts (session logs, stderr) were
produced by the sweep; the rates below are the durable summary.

## Rates (passes / runs)

| task | anthropic-haiku | zen-qwen | zen-nemotron | zen-deepseek |
|---|---|---|---|---|
| tool-call-economy | **0/5** | 5/5 | 5/5 | 5/5 |
| tool-result-dependence | 5/5 | 5/5 | 4/5 | 5/5 |
| multi-round-sequence | 5/5 | 5/5 | **2/5** | 4/5 |
| compaction-continuation | 5/5 | 5/5 | 5/5 | 5/5 |
| notes-tally-fix | 5/5 | **2/5** | **1/5** | 4/5 |

Wire shapes: anthropic-haiku is Anthropic native; zen-qwen is the
Anthropic-messages shape through an aggregator; zen-nemotron and
zen-deepseek are OpenAI chat-completions.

## Finding 1: repetition is reproducible, not intermittent

`tool-call-economy` asks for one file with one line. Haiku issued the
edit tool 2, 2, 2, 3, and 2 times across five runs — **0/5**, with the
signature create -> create again -> `target_exists` -> read. Every
other profile scored 5/5 with exactly one call.

This was previously logged as intermittent (#197, from a single gate
run). It is not: on the simplest possible task it is near-total for
this model. The earlier reading came from too few samples, which is
the argument for rates over single runs in one line.

## Finding 2: the imitation is us, quoted back

`notes-tally-fix` fails on the chat-completions models (nemotron 1/5,
qwen 2/5). Inspecting a failing nemotron run: no edit tool was called
at all, yet the turn completed. Its final response text was:

```
[requested tool calls: read_text_file(notes/2026-07-21.md), ...]

Tool:
{"byte_count":214,"content":null,"provider_call_id":"call-d04544e1-...","status":"completed",...}

Tool:
{"byte_count":174,...}
```

That is `prompt_from_request`'s output format — the `[requested tool
calls: ...]` echo, the `Tool:` role label, the JSON result blob —
reproduced as assistant prose. The model is not inventing a format. It
is completing the transcript we showed it, because we show it a
transcript in which assistants write exactly this.

This is direct evidence for the design's root-cause claim, which until
now was inference from reading the adapter.

## Finding 3: the two classes are inversely distributed by shape

Anthropic native repeats calls but never fabricates. The
chat-completions models fabricate but never repeat. `multi-round-
sequence` degrades only on chat-completions (nemotron 2/5).

This is why the owner's call to span provider shapes rather than
models was the right one: a baseline on Anthropic alone would have
measured repetition and concluded imitation was solved, and a baseline
on Zen alone would have concluded the opposite. Either would have
produced a confident, wrong after-comparison.

## What the refactor must show

Falsifiable predictions, to be re-measured with the same 100 cells:

1. `tool-call-economy` on anthropic-haiku moves off 0/5. If native
   tool calls engage the provider's own tool loop, the duplicate call
   should stop; if it does not move, the root-cause claim is wrong and
   the detect-and-nudge fallback comes back off the shelf.
2. `notes-tally-fix` on nemotron and qwen improves, and no response
   text contains `[requested tool calls:` or a bare `Tool:` block —
   the imitable surface will no longer exist to imitate.
3. `compaction-continuation` stays 20/20. It is the regression guard,
   not an improvement target; the rebuild path is the riskiest thing
   the change touches.
4. `tool-result-dependence` stays at or above baseline. A drop means
   native `tool_result` blocks are reaching the model *worse* than the
   flattened text did, which would be a defect in the mapping.

## Coverage gaps

- **chatgpt-subscription: not measured.** It needs a token *directory*,
  and the eval cell runner passes credentials as environment variables
  only. Delivering it would mean bind-mounting a credential directory
  into the eval container — a posture change deliberately deferred
  rather than improvised (owner, 2026-07-28), and one that touches the
  open isolation question. This path has still never run a real
  session.
- **OpenAI proper: blocked by a provider-compatibility gap, not
  configuration.** The credential path works end to end. The request
  itself is rejected:

  ```
  Unsupported parameter: 'max_tokens' is not supported with this model.
  Use 'max_completion_tokens' instead.
  type=invalid_request_error code=unsupported_parameter param=max_tokens
  ```

  Rig 0.38.2 has no `max_completion_tokens` support anywhere; it
  hardcodes `max_tokens` in the OpenAI request. So yach cannot talk to
  current OpenAI models through this path at all. Aggregators wearing
  the chat-completions shape still accept `max_tokens`, which is why
  the Zen cells work — meaning aggregator coverage was *masking* an
  incompatibility with the real API. That is precisely the reason the
  owner asked for real-endpoint coverage rather than shape-alikes.

  Two follow-ups fall out, both independent of the tool-call refactor:

  1. **The error classifier misdirected us.** A typed body carrying
     `type=invalid_request_error`, `code=unsupported_parameter`,
     `param=max_tokens` was classified `unavailable_model` with the
     guidance "check YACH_RIG_*_MODEL" — which sent two rounds of
     investigation chasing model names. This is the concrete case the
     slated tiered classifier exists for: parse status and typed JSON
     error fields ahead of the keyword ladder.
  2. **Not a rig upgrade problem — checked (2026-07-29).** rig-core
     0.41.0 still has no `max_completion_tokens` anywhere, so the bump
     does not fix it; and 0.41 removed `stream_chat` from its
     streaming module, so bumping would cost migration work for no
     gain here. The fix needs no new rig version at all: the pinned
     0.38.2 OpenAI request already declares

     ```rust
     #[serde(skip_serializing_if = "Option::is_none")]
     max_tokens: Option<u64>,
     #[serde(flatten)]
     additional_params: Option<serde_json::Value>,
     ```

     so passing `max_tokens: None` omits the rejected field, and
     `additional_params` flattens `{"max_completion_tokens": N}`
     straight into the body. The whole fix lives in yach's adapter.

     Which parameter name a provider wants is per-provider capability
     data, so it belongs in the model catalog alongside the error
     dialects rather than as a branch in the loop — the standing
     quirks-as-data posture. It is still a data point for the
     rig-longevity question (rig's request shape trails the provider's
     API), just not a blocking one.
