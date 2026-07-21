# Context Compaction Research

Date: 2026-07-20

Input to the context compaction design. Two research passes: how the
cohort (Codex CLI, Claude Code, opencode, Pi) implements compaction, at
source level where open; and the broader research/industry landscape on
agent context management. Gathered before writing the yach design so the
mechanism choice is grounded, per the owner's direction that this is an
active research area we will revisit often.

## The convergent architecture (all four harnesses)

1. **The session log is never truncated.** Compaction appends an entry —
   Pi a `CompactionEntry { summary, firstKeptEntryId, tokensBefore }`,
   Claude Code a `compact_boundary` record plus a synthetic
   `isCompactSummary: true` message, opencode an assistant message with
   `mode: "compaction"` / `summary: true` flags, Codex a rollout item.
   Display history and audit stay intact; only rebuilt model context
   changes.
2. **Post-compaction model context = structured summary + verbatim recent
   tail.** Nobody sends a bare summary.
3. **Manual command with optional focus instructions** (`/compact
   [instructions]`) in all four, alongside the auto trigger.
4. **Auto trigger from token accounting with reserved headroom** — room
   for the response and for the summarization call itself.

## Per-harness specifics

### Pi (source: pi-mono `packages/coding-agent/src/core/compaction/`, docs/compaction.md)

- Trigger: `contextTokens > contextWindow - reserveTokens` (reserve
  default 16,384) — plus **overflow recovery**: a context-overflow API
  error triggers compaction and retries the aborted turn (`reason:
  "manual" | "threshold" | "overflow"`, `willRetry`).
- Keeps a recent tail of `keepRecentTokens` (default 20,000), cutting at
  turn boundaries only (never between a tool call and its result). A
  single turn bigger than the budget becomes a "split turn": history and
  turn-prefix are summarized separately and merged.
- Iterative: the previous summary is passed as context; the summarized
  span restarts at the previous compaction's kept boundary so surviving
  messages get re-summarized rather than dropped.
- Summary schema: Goal / Constraints & Preferences / Progress
  (Done/In Progress/Blocked) / Key Decisions / Next Steps / Critical
  Context, plus cumulative `<read-files>`/`<modified-files>` tracked
  across compactions in the entry's `details`.
- Serialization: messages flattened to `[User]:`/`[Assistant]:` text so
  the summarizer does not continue the conversation; tool results
  truncated to 2,000 chars in the summarization request.
- Extension hook (`session_before_compact`) can cancel or supply a custom
  summary (e.g. from a different model). Branch navigation gets the same
  machinery (`BranchSummaryEntry`).

### opencode (source: `packages/opencode/src/session/compaction.ts`, agent/prompt/compaction.txt)

- **Two-phase: prune, then summarize.** Pruning walks backward marking
  completed tool-call outputs as compacted (`part.state.time.compacted =
  Date.now()` — a timestamp flag, not deletion), skipping the 2 most
  recent turns, protected tools (`skill`), and anything after a prior
  summary. Pruning only proceeds if it frees ≥ `PRUNE_MINIMUM` (20,000
  tokens); `PRUNE_PROTECT` (40,000) guards the recent window.
- Full compaction retains a tail of `DEFAULT_TAIL_TURNS = 2` within a
  preserve-recent budget (2,000–8,000 tokens, up to 25% of usable
  context), appends the compaction-mode message, and publishes
  `Event.Compacted`.
- The prompt frames an **anchored summary**: "If the prompt includes a
  `<previous-summary>` block, treat it as the current anchored summary.
  Update it with the new history by preserving still-true details,
  removing stale details, and merging in new facts."
- Failure: unrecoverable overflow surfaces as `ContextOverflowError` on
  the compaction message.

### Codex CLI (docs/analyses of codex-rs; config keys verified)

- Trigger: `effective_auto_compact_limit = min(user_config_limit,
  context_window * 90%)` — the 90% ceiling is hard (added in v0.100.0
  after backend errors). Defaults: `model_auto_compact_token_limit =
  180_000` on a 200K window. Fires pre-turn and at tool-loop boundaries
  mid-turn.
- **Two implementation paths**: on OpenAI, a server-side
  `POST /v1/responses/compact` returns an opaque AES-encrypted blob the
  client stores without inspecting (anti-injection + opacity tradeoff);
  on other providers, a local summarization prompt produces a plaintext
  summary message. Post-compaction context = one summary message + up to
  20,000 tokens of recent **user** messages; assistant/tool history is
  dropped.
- Per-item truncation exists independently (`tool_output_token_limit =
  16_000`). `/compact [instructions]` (v0.117+). Known failure: a
  v0.112 "death spiral" of repeated compaction without progress.

### Claude Code (docs + reverse-engineering; closed source)

- Multi-tier: **microcompaction first** (since v1.0.68) — bulky tool
  outputs split into a "hot tail" of recent results kept visible and
  "cold storage" spilled to disk files referenced by path, so the agent
  can re-read evicted output on demand — then full summarization when
  clearing is not enough.
- Trigger is model/deployment-dependent (~92–95% of usable window
  historically, with a reserved buffer so the summary call can run;
  configurable via `CLAUDE_CODE_AUTO_COMPACT_WINDOW` and
  `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`, which can only lower it). Countdown
  warning UX ("Context left until auto-compact: N%").
- The summary prompt is a rigid 9-section template (primary request and
  intent; key technical concepts; files and code sections; errors and
  fixes; problem solving; **all user messages** — verbatim, with
  security-relevant instructions preserved verbatim; pending tasks;
  current work; optional next step). After compaction it re-reads the ~5
  most recently accessed files, reinjects todo/plan state, and re-injects
  CLAUDE.md/memory from disk; path-scoped rules are lost until a matching
  file is read again.
- Failure handling worth copying: a **thrashing guard** ("Autocompact is
  thrashing" — stops retrying when context refills to the limit
  repeatedly) and a known deadlock class ("Conversation too long") when
  the window is so full the summarization request itself cannot fit —
  the lesson is to reserve compaction headroom *before* the window is
  exhausted.
- Anthropic has since productized the pattern as API features: context
  editing (`clear_tool_uses` — server-side tool-result clearing), the
  memory tool, and a server-side compaction strategy
  (`compact_20260112`).

## Research landscape (agent-level; model-internals out of scope)

- **Best-evidenced technique is not summarization.** "The Complexity
  Trap" (arXiv:2508.21433, JetBrains, NeurIPS 2025 DL4Code): on
  SWE-bench Verified across 5 model configs, simple **observation
  masking** (replace old tool-result bodies with an elision marker, keep
  the tool call) halves cost and matches or beats LLM summarization;
  their hybrid (mask first, summarize later) gained a further 7–11%.
  ACON (arXiv:2510.00615) reports 26–54% peak-token reduction from
  optimized compression guidelines.
- **The signature failure mode of summarization is instruction loss, not
  detail loss.** "Governance Decay" (arXiv:2606.22528): in-context policy
  constraints silently dropped by compaction — violation rates from 0%
  pre-compaction to 30% average / 59% worst-case. Mitigation: the summary
  schema must restate standing user instructions verbatim (Claude Code's
  "All user messages" section; Pi's Constraints & Preferences). A second
  documented mode: summaries smooth over failing trajectories, so agents
  persist in unproductive loops.
- **Prompt caching is the binding economic constraint.** Caching rewards
  byte-stable append-only prefixes; every compaction (and every
  structural edit of old history) rewrites the prefix and resets the
  cache. Real-trajectory analyses put caching savings at 49–80% of token
  cost. Implication: compact rarely at high thresholds; incremental
  mid-band summarization is the worst of both worlds; time manual
  compaction at task boundaries.
- **Memory architectures**: MemGPT/Letta tiered self-editing memory and
  Mem0-style vector fact stores remain niche for coding harnesses. What
  ships everywhere is the plain-file pattern: notes/todo files the agent
  writes and re-reads (Anthropic memory tool, CLAUDE.md, scratchpads),
  i.e. retrieval by ordinary tool call against durable files.
- **Retrieval affordance on top of summaries** (first-hand observation
  from Claude Code): the continuation message includes the path to the
  full transcript on disk — "read the full transcript at <path> if you
  need specific details" — making lost detail recoverable by an ordinary
  file read. Yach's full-fidelity session log makes the same pattern
  nearly free.

## Implications for yach

- The convergent architecture maps cleanly onto yach's existing pieces:
  a compaction event appended to the JSONL session log (which already
  persists full tool payloads post-#129/#130), rebuilt provider context
  = summary + kept tail (the resume path already rebuilds context from
  the log), and a TUI marker.
- Cut-point discipline matters: never separate a tool call from its
  result (Pi's rule; yach's provider context has the same pairing
  constraint).
- Reserve headroom for the summary call itself and guard against
  thrash loops (Claude Code's two failure classes).
- The evidence ordering suggests tool-result masking is the highest
  value-per-complexity first mechanism, with threshold summarization as
  the second layer — but masking alone cannot recover from true
  overflow, so a shippable slice needs at least a basic summary path or
  a hard-stop story.
- Anchored/iterative summaries (opencode, Pi) prevent repeated-compaction
  degradation from compounding; the schema should restate user
  instructions verbatim (governance-decay mitigation).
- Token accounting precedes everything: yach needs usage-from-provider
  (already reported in session stats) plus an estimate for
  static context + pending turn to compute headroom.

Owner posture: revisit-often topic; the design should keep the mechanism
pluggable (Pi's extension hook is the reference shape) without
committing yach to any single research direction.
