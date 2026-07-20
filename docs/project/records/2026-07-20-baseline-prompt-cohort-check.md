# Baseline Prompt Cohort Check (2026-07-20)

Quick comparison of cohort harness system prompts (Codex CLI, Claude Code,
opencode, Pi) before landing the proportionality clause in
`NATIVE_PROVIDER_BASELINE_GUIDANCE`. Trigger: a bare "hello" caused three
AGENTS.md-driven orientation file reads before responding. Scope: a sanity
check that our baseline guidance is in-family, not a full prompt design
pass — that deeper pass is deliberately deferred (see Follow-ups).

## What we checked

Three questions, against each harness's shipped default prompt (sources:
openai/codex main; anomalyco/opencode dev, `packages/opencode/src/session/prompt/`;
badlogic/pi-mono main, `packages/coding-agent/src/core/system-prompt.ts`;
Claude Code's own injected context):

1. Does the prompt tell the model to match effort to the request and answer
   conversational prompts directly?
2. How are project instruction files (AGENTS.md etc.) framed — compliance
   weight and scope?
3. Are there tool-evidence / stale-file-state guardrails like ours?

## Findings

### Effort-matching for conversational prompts

- **Codex CLI**: present in every prompt generation. "Do not use plans for
  simple or single-step queries that you can just do or answer immediately";
  "For casual chit-chat, just chat"; "In casual conversation, you just talk
  like a person"; greetings and one-offs get plain sentences, no structure.
  Newest default prompt scopes autonomy by request type and says to "verify
  it in proportion to risk". Nuance: none of them ban tools for simple
  requests — one line even says to fulfill "what time is it" by running
  `date`.
- **opencode**: the fallback `default.txt` (classic Claude Code lineage) has
  the strongest version: "If you can answer in 1-3 sentences or a short
  paragraph, please do"; "answer their question first, and not immediately
  jump into taking actions"; "One word answers are best." The
  Claude-specific `anthropic.txt` (the effective default for Claude models,
  ~1,335 words) drops the aggressive brevity rules but keeps CLI
  conciseness; it has no greeting/tool guidance.
- **Claude Code**: "Match the response to the question: a simple question
  gets a direct answer in prose, not headers and sections."
- **Pi**: nothing. Entire default prompt is ~350 words: identity line, tool
  list, 8 guideline bullets ("Be concise in your responses"), self-docs
  pointers, cwd.

### Project instruction file framing

- **Codex CLI**: strongest precedent for scoping. Its AGENTS.md spec anchors
  compliance to "every file you touch in the final patch" and limits style
  rules to "code within the AGENTS.md file's scope" — obligations attach to
  work, not conversation. Newest prompts drop the spec from the base prompt
  entirely and inject AGENTS.md as a separate message with no compliance
  framing.
- **Claude Code**: strong compliance language on injected instructions
  ("you MUST follow them"), but the block ends with a relevance
  counterweight: the context "may or may not be relevant to your tasks…
  should not respond to this context unless it is highly relevant".
- **opencode**: bare neutral header, `Instructions from: <path>`, no
  compliance or scoping language. Nested AGENTS.md files discovered during
  reads arrive inside tool results wrapped in system-reminder tags.
- **Pi**: neutral wrapper ("Project-specific instructions and guidelines:"),
  no compliance or scoping language.

No harness mandates applying instruction files to every message; two of four
explicitly scope them to work/relevance.

### Tool-evidence / stale-state guardrails

None of the cohort has "tool results are the only source of truth"
language. Closest: opencode's "investigate to find the truth first rather
than instinctively confirming the user's beliefs"; Codex's dirty-worktree
guidance ("If this happens, STOP IMMEDIATELY…", softened in newer prompts)
and compaction-staleness checks. Codex leans the other way on economy: "Do
not waste tokens by re-reading files after calling `apply_patch` on them.
The tool call will fail if it didn't work." Read-before-edit is enforced
mechanically by tools in opencode, not by prompt text.

Our stale-state paragraph is therefore a yach-specific addition (motivated
by cheap-model dogfooding: without it, models asserted filesystem state from
stale conversation memory). It only mandates re-checking on failure or
before asserting file state, so it does not conflict with Codex-style
"trust tool success" economy.

## Assessment

The proportionality clause added to `NATIVE_PROVIDER_BASELINE_GUIDANCE`
("Match effort to the request… Project instructions in context describe how
to carry out real work, not a checklist to run before every response.") is
in-family: between Pi's silence and opencode-default's aggressive brevity,
closest to Codex's current defaults and Claude Code's relevance scoping.

Owner decision (2026-07-20): where yach was heavier than the cohort — the
stale-state paragraph — lean it out, in keeping with the goal of staying
closer to Pi in spirit. The paragraph was compressed to the two behaviors
that earned their place in dogfooding (verify before asserting remembered
file state; re-check and adapt on failed calls instead of repeating) and the
elaboration ("only source of truth", the enumeration of outdated evidence
kinds) was dropped.

Size context: yach's baseline guidance is ~90 words against cohort baseline
prompts of ~350 (Pi), ~1,100–1,300 (Codex codex-variants, opencode
anthropic), and ~2,800–3,900 (Codex general prompts). We are far below the
cohort median, which is intended — yach's prompt surface stays deliberately
small until the deeper pass.

## Follow-ups (deferred to the deeper prompt pass)

- Whole-prompt design: tone/formatting rules, proactiveness boundaries,
  verification-in-proportion-to-risk, parallel tool-call guidance — the
  cohort carries these; yach currently says nothing.
- Consider Codex-style explicit scoping in how yach *wraps* AGENTS.md
  content (today: rendered under a title header with no framing either
  way), rather than relying solely on the baseline counterweight.
- Revisit whether the (now leaner) stale-state guidance can drop further as
  we move to stronger default models (it exists for cheap-model steering).
