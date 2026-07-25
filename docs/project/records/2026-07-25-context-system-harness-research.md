# Context-System Harness Research (2026-07-25)

Cohort research on how the comparison harnesses track context, drive their
context meters, trigger and run compaction, recover from overflow, and
handle failed turns. Trigger: two live dogfood findings in the sesh
sessions — the yach meter freezing mid-turn (fixed in #166) and the meter
flip-flopping between accountings around a failed turn (89%→73% on resume,
found 2026-07-24, unfixed as of this record) — plus the context-tracker
research item flagged in #167.

Method: one research subagent per harness reading actual source — Codex
CLI (openai/codex, codex-rs) with nanocodex (gakonst/nanocodex) as a
distilled comparison, opencode (sst/opencode), Pi (badlogic/pi-mono
v0.82.0) — and one doing public-record research on Claude Code (closed
source; docs, changelog, issues, reverse-engineering writeups, with
confidence markers). File:line citations below refer to each project's
repo at its 2026-07-25 head.

## Summary table

| Dimension | Codex CLI | opencode | Pi | Claude Code | yach today |
| --- | --- | --- | --- | --- | --- |
| Accounting source | provider usage + bytes/4 tail | provider usage only | provider usage + chars/4 tail | provider usage (display), estimate (trigger) | chars/4 estimate only |
| Meter numerator | last response `total_tokens` (+est tail for trigger) | last assistant msg token sum | last valid usage + est tail | input+cache tokens of last response | log/provider-msg estimate |
| Meter denominator | window − fixed 12k baseline | raw window | raw window | raw full window | usable (window − max_out − reserve) |
| Meter cadence | per completed response | per step-finish (tool round) | per assistant message | per assistant message (300ms debounce) | per tool round (#166) |
| Trigger threshold | min(config, 90% window) | count ≥ input_limit − reserved (~20k) | tokens > window − 16,384 | model-dependent; proactive at boundary or reactive at limit | 90% of usable, keep_recent floor (#165) |
| Trigger == meter accounting? | no (different adjustments) | numerator yes, denominator no | yes (same function) | no (documented as intentional) | no (log vs provider-msgs — the 07-24 bug) |
| Failed turns feed forward? | yes, + abort marker | no (errored); aborted partially | no at request-build (persisted in log) | interrupted: yes, with synthetic markers | no |
| Overflow recovery | fail turn, peg meter full, compact next turn | compact + replay last user msg | one-shot compact-and-retry | reactive compact seeded by overflow size; head-truncate retries | compact-and-retry (reachable since #169) |
| Provider integration | own HTTP/WS stack | Vercel AI SDK + models.dev | own layer (~45 providers, ~8 wire protocols) | first-party SDK (closed) | Rig |
| Error classification | typed enums from typed JSON `code` | typed SDK errors + ~30-regex overflow fallback | regex tables over normalized error strings | n/a (closed) | keyword ladder (tiered design slated, #170) |

## Cohort findings

### Accounting: provider-reported usage is the consensus source of truth

Every harness anchors on the provider's usage fields; none uses a real
tokenizer client-side. The estimators that exist are all chars-or-bytes/4
and are confined to the *gap* the provider hasn't reported yet:

- Codex: `get_total_token_usage` = last reported `total_tokens` + bytes/4
  estimates for items appended after the last model-generated item
  (`core/src/context_manager/history.rs:295-315`); estimator comment:
  "coarse lower bound, not a tokenizer-accurate count".
- Pi: identical shape — `estimateContextTokens` = last valid assistant
  usage + chars/4 for trailing messages
  (`packages/coding-agent/src/core/compaction/compaction.ts:198-230`).
- opencode: pure provider usage, written per AI-SDK `step-finish` onto the
  persisted assistant message (`session/processor.ts:438-456`); chars/4
  exists only for compaction cut-point planning.
- Claude Code: status-line context data comes "from the most recent API
  response" (documented); a client-side estimate feeds the compaction
  trigger (changelog v2.1.75 fixed it over-counting thinking/tool_use and
  triggering premature compaction).

yach is the outlier: chars/4 for everything. The upgrade path is the
Codex/Pi hybrid — provider usage as the anchor, estimate only the
unreported tail. Anthropic's `message_start`/`message_delta` usage fields
carry what's needed; Pi's Anthropic mapping computes `total` from
components (`packages/ai/src/api/anthropic-messages.ts:577-585`).

### Meter/trigger consistency: only Pi actually has it

Pi routes meter and trigger through the same accounting function, which
skips errored/aborted/zero-usage assistant messages
(`compaction.ts:153-167`) — so its meter cannot flip-flop around failed
turns the way yach's did on 2026-07-24. opencode shares the numerator but
not the denominator (meter: raw window; trigger: input-limit minus
reserved) and has yach's exact failed-turn divergence: errored messages
are dropped from requests (`message-v2.ts:248-256`) but still drive the
meter (`sidebar/context.tsx:20`). Codex and Claude Code both run
deliberately different meter and trigger accountings (Codex: fixed 12k
baseline for display vs 90%-of-window hybrid check; Claude Code:
documented that `used_percentage` "always uses the model's full context
window" while compaction math reserves separately).

Denominator postures split: raw window (opencode, Pi, Claude Code) vs
adjusted window (Codex fixed baseline; yach usable-window). yach's
usable-window meter is defensible — it is the honest fraction of what a
request may actually occupy — but the choice should be documented, and
the *numerator* must be consistent between meter and trigger, which is
the actual bug.

### Post-compaction meter honesty

Pi reports `{tokens: null}` and renders `?` until the next assistant
response arrives ("context token count is unknown until the next LLM
response", `agent-session.ts:3163-3189`). Codex recomputes a client-side
estimate immediately and corrects on the next reported usage. Claude
Code's `current_usage` goes null after `/compact` until the next call.
All three avoid showing a stale pre-compaction number.

### Compaction is tiered; full summarization is the last resort

- Claude Code layers "microcompact" first: old tool results replaced with
  `[Old tool result content cleared]`, tool results >50K chars offloaded
  to disk, reportedly a server-side cache-edit deletion tier (issue
  #42542), then full summarization.
- opencode prunes old tool outputs (protecting the newest 40k tokens) and
  renders pruned ones as `[Old tool result content cleared]`
  (`message-v2.ts:293-295`, `compaction.ts:243-287`).
- Codex trims oversized tool outputs before remote compaction requests.
- Pi goes straight to summarization but keeps the keep-tail verbatim.

Common summary mechanics across Codex local, opencode, Pi, Claude Code:
fixed structured template; conversation serialized into a *user* message
(Pi wraps in `<conversation>` tags "so model doesn't try to continue
it"); previous summary carried forward as an anchor with an UPDATE
instruction (Pi `<previous-summary>`, opencode same); summarizer runs
with tools disabled (opencode throws on tool calls during summary);
tool outputs truncated in the summarizer's input (opencode: 2,000 chars).
Model choice: Pi and Codex use the session model; opencode allows a
configured compaction agent/model, defaulting to the session model.
Claude Code post-compaction re-reads up to ~5 recently-important files
(~50K budget) rather than trusting the summary (inferred).

### Overflow recovery: guarded, single-shot, never transient-retried

All three OSS harnesses explicitly exclude overflow from transient
retry (opencode `retry.ts:69-70`; Pi "handled by compaction instead";
Codex `is_retryable` → false). Recovery shapes differ:

- Pi: one compact-and-retry attempt, gated by an
  `_overflowRecoveryAttempted` flag that resets on the next valid
  response; the failed error message is stripped from the retry context;
  second overflow → actionable terminal error. Also detects "silent"
  overflows (success responses whose input exceeds the window, and
  length-stops with zero output at ~99% input).
- opencode: compact, then *replay the last user message*; a compaction
  request that itself overflows is terminal ("Session too large to
  compact").
- Codex: the failed request is not retried at all — usage is pegged to
  the full window so the meter reads 0% left and the *next* turn's
  pre-sampling check compacts. During compaction itself: drop the oldest
  item and retry until it fits.
- Claude Code: reactive compaction seeds the summarize attempt from the
  overflow size parsed out of the API error; compact-request overflow →
  head truncation with up to 3 retries; a circuit breaker stops
  auto-compaction after 3 consecutive failures and a thrash detector
  stops repeated compact-refill cycles (changelog v2.1.76/v2.1.89).

yach's #165 thrash guard is a milder single-check version; the one-shot
recovery flag and the compaction-request-overflow fallback are both
missing.

### Failed turns: exclude-from-request is the majority, with markers

Pi persists failed/aborted messages to the session file but drops them at
request-build time ("incomplete turns that shouldn't be replayed",
`transform-messages.ts:189-197`) — same posture as yach. opencode drops
errored turns, keeps aborted ones only if they produced substantive
parts, and synthesizes `[Tool execution was interrupted]` results.
Codex is the outlier: everything recorded before the failure stays in
context, plus an explicit model-visible "turn aborted" marker, with
history normalization healing dangling tool calls. Claude Code patches
interrupted turns with synthetic `[Request interrupted by user]`
messages that do feed forward.

### Server-side compaction and state (OpenAI Responses)

Codex is stateless-by-default (`store: false` for openai proper,
encrypted reasoning replayed by the client each turn);
`previous_response_id` is only a websocket bytes-on-the-wire
optimization with full-resend fallback. Its remote-v2 compaction sends a
`compaction_trigger` item and receives an opaque server `Compaction`
item that replaces summarized history. nanocodex inverts the posture:
server-side state is load-bearing (`store: true`, delta-only requests),
no meter, no local summarization fallback, no overflow recovery — the
90% pre-emptive trigger is the entire strategy. The Pi server-compaction
extension plugs into a `session_before_compact` hook that may return a
complete replacement compaction result — structurally the same seam as
yach's NativeCompactor; its README benchmarks 78% exact recall for
server-side vs 48% for Pi's text summaries.

### Error classification (bears on #170's tiered design)

Codex proves the typed ceiling: owning the wire protocol, it parses the
provider's error JSON by `code` into typed enums
(`context_length_exceeded` → `ApiError::ContextWindowExceeded`). But
both opencode (typed AI-SDK errors available) and Pi (owns ~45
providers) still keep regex/pattern tables for overflow phrasings —
opencode ~30 patterns including "prompt is too long", Pi ~23
per-provider regexes with exclusion lists. Across N providers the
phrasing tier is unavoidable; the tiered design slated in #170 (typed
fields first, keywords as fallback) matches what the cohort converged
on independently.

## Implications for yach

1. **Fix the failed-turn meter divergence with Pi's pattern**: one
   shared accounting path for meter and trigger that skips
   non-completed turns — the same filter `native_provider_messages_from_log`
   already applies. Cheapest correct fix for the 07-24 finding.
2. **Adopt hybrid accounting as the context-tracker upgrade**:
   provider-reported usage as anchor + chars/4 only for the unreported
   tail. Requires plumbing usage fields through the Rig adapter (rig
   exposes `GetTokenUsage`) and ties into model-catalog hydration.
3. **Post-compaction honesty**: show `?` (or an explicitly-estimated
   marker) until the next provider-reported usage, rather than a stale
   or silently-estimated percent.
4. **Harden overflow recovery**: add Pi's one-shot recovery flag, and a
   fallback for the compaction request itself overflowing (Codex's
   drop-oldest loop is the simplest shape).
5. **Compaction tiers for slice 2+**: tool-output clearing/pruning
   before full summarization is the cohort norm and is exactly the
   masking pre-pass already slated; disk-offload of oversized tool
   results (Claude Code) is a natural extension.
6. **Summary carry-forward anchor**: on re-compaction, pass the previous
   summary with an UPDATE instruction (Pi/opencode) instead of
   re-summarizing from scratch.
7. **NativeCompactor seam validated**: both Codex remote-v2 and the Pi
   extension model provider-native compaction as "replace the
   summarization step, keep the orchestration" — the seam's shape is
   right; a provider-native compactor needs an opaque-blob slot in the
   checkpoint (Pi uses a generic `details` field).
8. **Silent-overflow heuristics** (Pi): when cross-model dogfood starts,
   some providers overflow as HTTP-200 success or empty length-stops;
   detection needs usage-vs-window checks, not just error classification.
