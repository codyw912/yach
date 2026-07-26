# Behavioral-Fixes Cohort Research (2026-07-26)

How the comparison harnesses handle model behavioral quirks: how many
model-specific workarounds each has accumulated, what they look like,
and where the disciplined ones put them. Trigger: the first cross-model
rotation run (nemotron via opencode Zen, chat-completions shape)
surfaced a model imitating yach's own assistant-round tool-call echo
format in prose and fabricating a `create_text_file` success result —
the turn completed with half the work not done. The owner flagged a
standing principle the same day: be careful about accumulating nudges
and model-specific fixes; the base project stays lightweight and
minimal. This research asks whether the cohort avoided accumulation
(it did not) and what discipline looks like.

Method: one source-reading research agent over openai/codex (codex-rs),
badlogic/pi-mono, and sst/opencode, with citations spot-verified.

## Summary table

| | Codex | Pi | opencode |
|---|---|---|---|
| Distinct workarounds | ~25 | ~42 | ~55-65 |
| Quirk home | server-supplied `ModelInfo` catalog + standalone `apply-patch` crate + one history-normalization pass | declarative `compat` flag tables in provider adapters + shared repair utils | one 1,832-line `provider/transform.ts` + models.dev catalog |
| Quirk encoding | data flags; code reads capabilities | data flags auto-detected from provider/base-URL, overridable | imperative model-ID substring branches |
| Core-loop leakage | minimal (one slug table; TUI cosmetics) | zero model-name branches in the agent loop | moderate (leaks into 4 core session files) |
| Prose imitation defense (the nemotron scenario) | closest analog only: rejects apply_patch-shaped strings instead of accepting (`invocation.rs:149`) | none | none |

## Findings

1. **Accumulation is universal and proportional to provider surface.**
   One provider ≈ 25 workarounds (Codex), ~15 providers ≈ 42 (Pi),
   dozens ≈ 55-65 (opencode). No harness avoided it; quirk handling is
   part of what a multi-provider harness is. The governing question is
   placement and growth rate, not existence.
2. **A convergent baseline trio appears in all three**: orphaned
   tool-call healing with synthetic results (Codex `normalize.rs:20`
   with cache-stable synthetic IDs; Pi `transform-messages.ts:158`
   "No result provided" error results; opencode `message-v2.ts:349`
   "[Tool execution was interrupted]"), malformed-tool-JSON tolerance
   (Codex error-result-not-crash; Pi 3-tier JSON repair + partial-json;
   opencode case repair + reroute to a synthetic `invalid` tool whose
   result feeds the validation error back as corrective feedback), and
   empty/aborted-turn replay hygiene. These are baseline harness
   hygiene, not patch accumulation.
3. **Placement discipline separates the cohort.** Codex expresses
   per-model behavior as data on a catalog struct (`shell_type`,
   `tool_mode`, `truncation_policy`, `input_modalities`, ...) that the
   core reads as capability flags — one hardcoded slug branch survives
   in core. Pi's `detectCompat()` emits ~22 flags per provider and the
   agent loop has zero model-name branches; Pi also uniquely
   institutionalizes accumulation: every new provider must join
   dedicated quirk-class regression suites (empty-response, overflow,
   unicode-surrogate, tool-call-without-result, cross-provider
   handoff). opencode is the cautionary tale: imperative substring
   matching concentrated in one giant transform file (with a
   self-aware "fix this stupid inefficient" TODO) and leaking into
   session code — it works but grows fastest.
4. **Nobody defends against prose imitation of tool calls or
   fabricated tool results** — the nemotron incident is novel relative
   to this cohort. The nearest postures both refuse to accept
   format-shaped text: Codex detects apply_patch imitations and
   rejects raw patch-shaped strings rather than applying them;
   opencode converts malformed calls into corrective feedback the
   model must answer. Reject-or-feedback, never silent acceptance.

## Owner strategy (decided 2026-07-26)

1. **Quirks-as-data from day one**: future model/provider quirks are
   expressed as capability flags in the model catalog (hydration
   design), never model-name branches in the loop. Adopted early,
   before accumulating imperative-branch debt.
2. **Pi's regression-suite pattern joins the provider rotation**: each
   new provider's real failure shapes land in the quirk-class test
   corpus (started with the provider-error classification tests).
3. **The convergent baseline trio is in-scope for core** (not counted
   against the lean budget); yach already carries replay hygiene
   (failed-turn exclusion) and truncated-result acceptance — orphaned
   tool-call healing is the gap to close when it first bites.
4. **Echo-imitation defense is format-level, not model-level**, so it
   clears the lean bar in principle — but it touches the round-echo
   format that carries loop-prevention weight, so it gets a design
   note (detect echo-format text in a final response → reject and
   nudge once, Codex-posture) rather than a quick patch.
