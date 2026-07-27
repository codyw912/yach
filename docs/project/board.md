# Work Board

Last updated: 2026-07-26. One line per open item, grouped by thread.
`next.md` carries the narrative and rationale; this file is the queue.
Statuses: **active** (being worked), **next** (agreed order), **queued**
(concrete, unscheduled), **slated** (needs design first), **open**
(owner question, no schedule).

## Provider rotation — active

- **DONE 2026-07-26** — Rotation automation phase 1: `yach run`
  headless driver (#177, spec approved + container-isolation addendum),
  `yach-runtime` image, `run-isolated` + provider-matrix `rotate`
  recipes with the secret-reference profile-runner hook (#179, #181),
  openai-compatible provider + anthropic base-URL override (#178),
  provider-reported usage capture (#180, agent-path fix #181).
  Validated: runs-2 sweep 2026-07-26 — 4/4 cells fully correct across
  Anthropic direct, Zen messages (qwen), and Zen chat-completions
  (deepseek, nemotron), with real usage on every path.
- **active** — Continue rotating providers/models: Fireworks, more Zen
  families, the untested chatgpt-subscription path; grow the
  quirk-class corpus. Findings so far: laguna rate-limits classified
  correctly; nemotron echo-imitation is intermittent (2 of 3 runs).
- **DONE 2026-07-27** — yacht custom-harness hookup: yach runs as a
  declared harness end to end (preflight passed, task attempts
  measured, real provider usage in yacht's scorecard). Done
  outsider-style from the yach side; 9-entry friction log delivered,
  driving yacht fixes (#252 mounts/UX) and the `evidence_map` design
  (ADR 0017, #253/#254 — declaration-mapped harness-native json, per
  our recommendation). Owner decision en route: yach emits only
  standard formats — no consumer schemas (#185 closed; #186/#187 made
  the native document mapping-complete and line-oriented, and removed
  all yacht-schema code). Eval workspace: `~/dev/yach-evals`
  (uv-managed, config + preflight prompt + friction log).
- **next** — Decide the eval portfolio (founding principle, owner
  2026-07-27: assert on artifacts, not utterances): harness-regression
  evals (rotation scenarios as Harbor-format custom tasks) and
  cross-harness comparison (yach vs Pi vs Claude Code on identical
  courses via yacht's built-in adapters).
- **queued** — Harbor-course packaging when yacht's slice 2 firms up:
  musl static artifacts (x86_64/aarch64) + sha256.
- **open** — yacht entry 9 (nondeterministic agent-prompt response
  contract) pending on yacht's side; until then preflights carry a
  self-report prompt workaround.
- **slated** — Echo-imitation defense design note: detect
  echo-format/fabricated tool-call text in a final response, reject and
  nudge once (format-level, Codex reject-posture; novel vs the cohort).
  Intermittent (2/3 nemotron runs), so validation needs repeated runs.
  Research: `records/2026-07-26-behavioral-fixes-cohort-research.md`.
- **queued** — Orphaned tool-call healing with synthetic results — the
  one cohort-convergent baseline repair yach lacks; adopt when it
  first bites (or with the resilience pass).
- **active** — Pick the successor dogfood project (sesh finished all 6
  milestones 2026-07-25).
- **queued** — Evaluate an OpenAI Responses provider-native compactor
  behind the NativeCompactor seam once OpenAI models land.

## Context system

- **queued** — Compaction slice 2: masking pre-pass (deterministic
  tool-result clearing before summarization; cohort norm).
- **slated** — Split-turn summarization: turns larger than
  `keep_recent_tokens` keep nothing verbatim; larger than the window
  cannot compact (confirmed live 2026-07-25; Pi reference design).
- **queued** — Overflow hardening: one-shot recovery flag;
  compaction-request-overflow fallback (drop-oldest retry).
- **queued** — Summary carry-forward anchor on re-compaction
  (previous summary + UPDATE instruction, Pi/opencode pattern).
- **queued** — Post-compaction meter honesty: show unknown until the
  next real estimate instead of a stale number.
- **slated** — Hybrid accounting: provider-reported usage as anchor,
  chars/4 only for the unreported tail; coupled to model-catalog
  hydration. Research:
  `records/2026-07-25-context-system-harness-research.md`.

## Resilience pass (design research first)

- **slated** — Tiered provider-error classifier: parse status + typed
  JSON error body ahead of the keyword ladder; per-provider dialects in
  the model catalog.
- **slated** — Retry/backoff design: replace the fixed 2x1s/5s ladder;
  Retry-After awareness; partial-stream salvage; where retries live.
- **queued** — Richer user-facing provider-error surfacing (show the
  provider's actual message, e.g. billing, not a generic failure).
- **queued** — Graceful tool-budget exhaustion: error tool results that
  let the model wrap up instead of failing the turn.
- **queued** — Silent-overflow heuristics (success responses exceeding
  the window; zero-output length-stops) once multi-provider data exists.

## Model catalog

- **slated** — Model-catalog hydration design; unblocks four stopgaps:
  per-model context windows, per-model output budgets, curated /model
  list, truncated-tool-call recovery. Error dialects join it (above).
- **slated** — Provider/model product surface (owner-flagged
  2026-07-26): the `YACH_RIG_*` env wiring is explicitly a stopgap;
  design a friendlier surface for connecting providers and picking
  models — auth/connect flows, provider config, model discovery —
  with cohort examples (opencode `/connect` + models.dev catalog, Pi's
  provider registry, Claude Code `/login`). Couples with model-catalog
  hydration.

## Slice-1 leftovers (small)

- **queued** — Command permission-decision evidence (summary types are
  edit-shaped today).
- **queued** — Persist the tool request before the review wait
  (durability across crashes mid-review).
- **queued** — Commit a sanitized real-session JSONL as a compaction
  test fixture (snapshots earmarked 2026-07-22/25).

## UX sprint (deliberate batch)

- **slated** — Inline approvals for routine edit/tool review; pop-ups
  only for genuinely modal moments.
- **slated** — Approval model beyond review-everything (per-tool/risk
  auto modes, session grants, sandbox-backed postures).
- **queued** — Unfocused-input indicator (tmux pane confusion).
- **queued** — Expandable/collapsible tool output rows.
- **queued** — Status-bar design pass (layout, slot economy, overflow;
  includes >100% context-meter display semantics).
- **slated** — Mid-turn progress visibility (plan/todo surfaces, tool
  grouping, narration; may need loop support).
- **slated** — Deeper system-prompt/instructions design pass
  (follow-ups in `records/2026-07-20-baseline-prompt-cohort-check.md`).

## Open owner questions (no schedule)

- **open** — Execution isolation landscape (sandboxing, containers,
  hermetic filesystems) — deliberately undecided.
- **open** — Rig longevity / provider-integration ownership (own thin
  layer vs middleware; Codex/Pi own theirs, opencode delegates).
