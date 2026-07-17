# Max Output Tokens Across the Harness Cohort

Date: 2026-07-16

Question: how do the standard-cohort harnesses (Codex CLI, Claude Code,
opencode, Pi) size the per-turn output-token budget, and what does the
Anthropic API itself constrain? Context: yach's interactive default was 128
(a smoke-test bound that leaked into the TUI path, fixed to 8192 in the
first-response-truncation fix), and 8192 is itself an unjustified number.

Method: one research subagent per harness plus one on the Anthropic API
docs. Codex, opencode, and Pi findings are from clones of the current
sources; Claude Code is closed-source, so its findings are from official
docs plus maintainer-visible issue reports.

## Findings

| Harness | Per-turn output budget | Where it comes from | User config | On truncation |
| --- | --- | --- | --- | --- |
| Codex CLI | None — field omitted; the API's per-model default (model max) applies | n/a (a `model_max_output_tokens` config key existed but was telemetry-only, never sent on the wire; removed in #7100, 2025-11) | None (no key, env var, or flag) | `response.incomplete` becomes a retryable stream error: full-prompt retry with backoff (default 5 attempts), then the error is shown verbatim; deterministic truncation just fails 5 times |
| Claude Code | ~32,000 default (older docs: "Default: 32,000. Maximum: 64,000"; current docs: "defaults and caps vary by model") | Internal per-model table | `CLAUDE_CODE_MAX_OUTPUT_TOKENS`, out-of-range values silently clamped | Visible API error in the TUI ("response exceeded the 32000 output token maximum"); no auto-continue |
| opencode | `min(model.limit.output, 32_000)`, falling back to 32k when the model limit is unknown | models.dev catalog metadata; per-model config override | `OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX` env var, per-model `limit.output` config, plugin hook (some bundled plugins omit the field entirely) | Stop reason normalized to `length` and recorded; no retry or auto-continue |
| Pi | Model's full catalog max output, clamped to remaining context window (context − estimate − 4096 safety) | models.dev `limit.output` (4096 fallback) baked into generated catalogs; custom models default 16384 | Per-model `maxTokens` in model config; no env/runtime knob | Best-in-cohort: a `length`-stopped response containing tool calls executes none of them — each gets an error tool result telling the model its arguments may be truncated and to re-issue — and the loop continues; `length` without tool calls ends the turn |

Two postures, evenly split: a deliberate ~32k per-turn budget (Claude Code,
opencode) versus the model's own maximum (Codex by omission, Pi
explicitly). Nobody defaults anywhere near 8k, and nobody auto-continues a
truncated response.

## Anthropic API constraints (what yach must respect)

- `max_tokens` is REQUIRED on the Messages API; exceeding the model's
  ceiling is a hard 400, not a clamp. So unlike Codex-on-OpenAI, "omit the
  field and get the model max" is not available — yach must pick a number,
  and a number above a given model's ceiling breaks that model.
- Current ceilings: 128k for the Claude 5 family and Opus 4.6+; 64k for
  Haiku 4.5, Sonnet 4.5, Opus 4.5; 32k for Opus 4.1. Queryable at runtime
  via the Models API (`GET /v1/models/{id}` returns `max_tokens`).
- Unused headroom is free: output-token rate limits count only actual
  generated tokens ("no rate limit downside to setting a higher
  `max_tokens` value"), and billing is on actual output.
- Thinking counts INSIDE `max_tokens`, and newer models think adaptively by
  default — a tight budget can be consumed by thinking before any visible
  output. This is exactly the 128-token failure mode, and it argues against
  small budgets generally.
- Large `max_tokens` requires streaming (SDKs reject non-streaming requests
  projected past 10 minutes). yach always streams, so no constraint.

## Assessment for yach

- 8192 is below every cohort default and leaves thinking + a large edit in
  one turn at real risk of truncation. It should be raised.
- 32,000 is the cohort's modal deliberate budget and is ≤ every current
  Claude model ceiling (equal to Opus 4.1's), so it is universally safe to
  send today without per-model metadata, which yach does not yet have.
- The model-max posture (Pi/Codex) is strictly better on Anthropic economics
  (headroom is free) but requires per-model ceiling knowledge to avoid 400s;
  that belongs to a future model-catalog design, not a constant.
- Pi's truncation recovery (error tool results + continue, instead of a
  failed turn) is the standout behavior worth adopting; yach currently
  fails the turn with a truncation-naming error message.

## Decision (owner, 2026-07-16)

Default 32,000, bounds 1024–128,000, still overridable via
`YACH_RIG_PROVIDER_MAX_TOKENS`. Explicitly a revisit-later topic, not a
settled design: the model-max posture needs per-model ceiling knowledge,
which belongs to a future model-catalog design. Follow-up candidates when
revisited: derive the budget from model metadata; Pi-style
truncated-tool-call recovery in the provider loop (error tool results plus
loop continuation instead of a failed turn).
