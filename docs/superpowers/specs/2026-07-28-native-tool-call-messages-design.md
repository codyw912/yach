# Native Tool-Call Messages Design

Date: 2026-07-28

Status: approved in shape (owner, 2026-07-28); decisions recorded
below. Reframes the slated "echo-imitation defense" board item: the
evidence says the two behavioral failures are symptoms of a format we
chose, not model quirks to patch.

## Context

Two failures were logged as separate model quirks:

1. **Echo imitation** (nemotron via Zen, 2026-07-26): the model wrote
   prose imitating yach's round-echo format and fabricated a
   `create_text_file` success. The turn "completed" with the file never
   written.
2. **Identical repetition** (haiku, first real eval-gate run
   2026-07-28): the model re-issued the same `edit_text_file` five
   times with identical narration despite each result reporting
   `applied`; `journal.txt` ended with five `beta` lines. Investigated
   in #197, which exonerated the context seams and classified it as
   behavioral.

Both were slated for detect-and-nudge defenses. Reading the provider
path for this design found a shared root cause instead.

## Root cause

`rig_adapter::prompt_from_request` flattens the **entire conversation
into one labeled string** and sends it through rig's `.prompt()`:

```
User:
<prompt>

Assistant:
I'll create the file.
[requested tool calls: create_text_file(journal.txt)]

Tool:
{"arguments":{...},"content":{...},"status":"completed",...}
```

yach never sends native tool-call structure. It only *parses* it on the
receiving side (`StreamedAssistantContent::ToolCall`). The dependency
already supports the sending side — rig 0.38.2 has
`stream_chat(prompt, chat_history)`, `AssistantContent::ToolCall`, and
`UserContent::ToolResult { id, call_id, content }`. Nothing in the
multi-round tool-loop design records a reason for the flattening; it
reads as an early-MVP shortcut that was never revisited.

This explains both failures:

- **Echo imitation is taught, not spontaneous.** Assistant turns in
  that transcript literally contain `[requested tool calls: ...]`. The
  model is shown assistants writing that text, so it writes it — and
  invents the result that would follow. The nearest cohort posture
  (Codex rejecting apply_patch-shaped prose) defends against a format
  it never demonstrates in the first place.
- **Repetition follows from missing structure.** Nothing binds "I
  called X" to "X returned Y"; the result arrives as a `Tool:`-labeled
  text chunk. The provider's trained tool-loop machinery keys on
  `tool_use`/`tool_result` blocks and never engages, so the model
  re-issues calls.

The irony worth recording: the round-echo format exists *as a
workaround* for this missing structure — its code comment cites the
sesh session that ran 161 identical reads into the call backstop. The
workaround for the loop created the imitation surface.

## Goals

- Send provider-native message arrays: real assistant tool calls and
  real tool results, correlated by id.
- Delete the round-echo synthesis and the prose tool rendering it
  requires. The defense is removing the imitable surface, not
  detecting it.
- Keep the session log as the single source for rebuilding provider
  context (resume and post-compaction included).

## Non-goals

- Not the detect-and-nudge defense. It stays in the back pocket, to be
  designed only if measurement shows the structural fix leaves a
  material residue.
- Not compaction slice 2, not the resilience pass. This design touches
  the message representation only.
- Not a provider-capability matrix. If some provider genuinely cannot
  accept native tool messages, that is a model-catalog capability flag
  (quirks-as-data), not a branch in the loop.

## Design

### Message representation

`ProviderMessage.content: String` becomes structured content. The
minimum shape that serves every current provider:

- `User { text }`
- `Assistant { text, tool_calls: Vec<ProviderToolCall> }`
- `ToolResults { results: Vec<ProviderToolResultRef> }` where each
  carries `call_id`, `tool_name`, and the bounded result payload
- `System { text }` (unchanged; still becomes the preamble)

The adapter maps these onto rig `Message::User { content }` /
`Message::Assistant { content }` with `UserContent::ToolResult` and
`AssistantContent::ToolCall`. Tool results ride on user-role messages
because that is rig's (and Anthropic's) shape.

### Correlation ids

`ToolRequestRecorded` already persists `provider_call_id:
Option<String>`, so pairs rebuild from the log directly. When it is
absent — older logs, or a provider that omits ids — synthesize a
deterministic id from `tool_request_id`. Determinism matters for
prompt-cache stability; this is the Codex synthetic-id pattern from
the 2026-07-26 behavioral research.

### Rebuild seam

`provider_messages_from_log` stays the single place that turns session
history into provider context, so resume, cross-turn context, and
post-compaction rebuild all inherit native structure for free. The
in-turn continuation path stops appending a synthesized assistant echo
and appends the real assistant tool calls plus their results instead.

### What gets deleted

- `assistant_round_message` and its `[requested tool calls: ...]`
  rendering.
- The `Role::Tool` prose rendering in the adapter, and the
  `rig_prompt_role_label` transcript flattening.

### Estimator

`estimate_provider_messages_tokens` sums `estimate_text_tokens` over
`message.content`; it grows a small match over structured content
(text + serialized tool arguments/results). This is the chars/4
estimator either way — the hybrid provider-usage upgrade stays queued
and is unaffected.

### Compaction

Compaction operates on session-log events, not `ProviderMessage`, so
it is unaffected structurally. Its summary output remains a text
message. The one interaction to verify is the post-compaction rebuild
(kept-tail events must still produce well-formed tool pairs — a kept
tail that begins mid-pair must drop the orphaned half, which is the
cohort's orphaned-tool-call hygiene arriving naturally).

## Evals first

The point of building evals before the refactor is a before/after
**rate**, not a single sample. Each new task runs through
`just eval-sweep <profiles> <task> <outdir> N` for N repeats to
establish a baseline, then re-runs after the structural change.

**Baseline must span provider *shapes*, not just models** (owner,
2026-07-28): tool-call representation is precisely what differs across
providers, so a baseline covering only Anthropic would miss the
dimension being changed. Coverage: Anthropic native, opencode Zen
(both its messages and chat-completions surfaces), OpenAI, and the
chatgpt-subscription path. That last one has never been exercised in a
real session, so expect this work to be its first real test — a
failure there is a finding, not necessarily a regression from this
change. Each shape's real tool-call behavior joins the quirk-class
test corpus as it lands (the Pi posture from the behavioral research).

Proposed additions to `evals/tasks/`:

- **`tool-call-economy`** — an unambiguous minimal tool plan (create
  one file with given content). The verifier reads
  `.yach-eval/outcome.json` and asserts the tool-call count for the
  edit tool is exactly 1, plus the artifact is correct. This measures
  the repetition class directly; haiku currently exceeds 1
  intermittently.
- **`tool-result-dependence`** — the fixture holds a token in
  `secret.txt`; the task is to read it and write it into `answer.txt`.
  The verifier asserts the exact token round-tripped. A model that
  cannot see tool *result content* cannot pass except by luck, so this
  measures precisely what native `tool_result` blocks are supposed to
  make legible.
- **`multi-round-sequence`** — three-plus dependent rounds (read, edit,
  run, verify) with the verifier asserting final file state and that
  the outcome document shows the expected tool families. Measures loop
  health rather than a single call.
- **`compaction-continuation`** — a fixture carrying a
  `.yach/config.json` with a low compaction threshold and a prompt
  sequence long enough to trigger it; the verifier asserts the final
  artifact proves work from before and after the checkpoint. This
  guards the highest-risk interaction of this refactor.

`notes-tally-fix` already covers the fabricated-success class and
becomes the echo-imitation regression measure; it should join the
sweep with repeats rather than being read as a single pass/fail.

## Slices

1. **Evals + baseline.** Add the four tasks; record baseline rates
   (sweep with repeats) across every provider shape above. No
   production code changes. Profiles for the untested shapes are part
   of this slice's work.
2. **Structured `ProviderMessage` + adapter.** Representation, rebuild
   seam, adapter mapping, estimator, echo synthesis deleted — one
   change (see decisions). Per-shape adapter mapping is the main risk
   surface and needs a test per shape, not one generic test.
3. **Measure and compare.** Re-run the sweep; compare rates per
   provider shape. The detect-and-nudge item is closed or re-opened on
   this evidence.

## Owner decisions (2026-07-28)

1. **Slice 2 lands as one larger change**, not a dual path. The
   representation, rebuild seam, and adapter move together so the
   compiler enforces consistency and no intermediate state sends two
   formats; slice 1's baseline is what catches a regression.
2. **Baseline spans provider shapes, not just models** — Anthropic,
   opencode Zen, OpenAI, chatgpt-subscription. Rationale: this change
   touches provider tool calls, and tool support varies across
   providers, so narrow coverage would measure the wrong dimension.
   Recorded in the evals section above.
3. **The round echo is deleted entirely.** Its only remaining job was
   reconstructing loop context, which native tool calls do properly;
   the TUI already receives round text through streaming, separately
   from the echo. Removing it removes the imitable surface, which is
   the point of the design.
