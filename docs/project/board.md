# Work Board

Last updated: 2026-07-28. One line per open item, grouped by thread.
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
- **DONE 2026-07-28** — Eval portfolio decided (spec:
  `docs/superpowers/specs/2026-07-28-eval-portfolio-design.md`):
  in-repo Harbor-format tasks under `evals/`, dual-runner (local
  `just eval-gate` / yacht custom-eval course), three tracks
  (regression gate now, rotate verifier-awareness, cross-harness
  comparison later). Founding principle holds: verifiers assert on
  file state and outcome-document fields, never response prose.
- **active** — Build eval portfolio: assets + `eval-validate` (#195),
  `eval-gate` (#196), and the provider-matrix `eval-sweep` (one cell
  per profile × repeat, profile-owned model, verifier-scored rows in
  results.tsv) all landed 2026-07-28. First real gate run:
  notes-tally-fix and notes-explore reward 1; session-continuation
  reward 0 — a genuine catch (below). Remaining: the yacht custom-eval
  course config, waiting on Harbor-course packaging.
- **RESOLVED as behavioral 2026-07-28** — Session-continuation
  repetition (caught by the eval gate's first real run: the same
  find/replace applied 5 times, journal.txt got 5 betas). Investigated
  same day: the harness is exonerated at both context seams — the
  initial turn context assembled from the real hydrated log is correct
  (prior turn once, structured tool payloads, no duplication), and the
  in-turn round continuation (echo + tool results) is correct over a
  hydrated log too (new regression test
  `native_provider_agent_rounds_echo_survives_hydrated_session_log`;
  live path confirmed to route through the tested seam). Decisive:
  turn 1 repeated its create in a completely fresh session, so the
  `--session` correlation was coincidental. Class: model-behavioral
  repetition (haiku re-issuing an action despite visible applied
  results, identical narration amplifying round over round) — same
  family as the sesh "161 identical reads" finding and the slated
  echo-imitation defense. Defense (identical-consecutive-call
  detection → reject/nudge once, format-level) belongs in that design
  note, not a quick patch. Gate artifacts preserved under
  `evals/.gate/session-continuation/`; expect this task to be
  intermittent on small models until a defense lands.
- **DONE 2026-07-28** — Backend naming cleanup (owner-flagged): the
  dogfood era is retired from the live code surface. Status lines lead
  with the fact (`backend: anthropic/claude-haiku-4-5; ...`,
  `turn_start`), the `native dogfood:` prefix is gone from every
  status/error message, symbols dropped it
  (`NativeRunnerConfig`, `NativeProviderConfig`, `run_native_loop`,
  `BackendMetadata::native()`), and the handshake ids are
  `yach-native` / `yach-native-provider`. Comments citing real dogfood
  sessions and findings were deliberately kept — those document why
  code exists. Also removed the `BackendKind::PiRpc` /
  `BackendMetadata::pi_rpc()` vestige (dead since the 2026-07-16 Pi
  backend removal, test-only).
- **DONE 2026-07-28** — `native` retired from every surface a human
  reads (owner decision: no users yet, so no migration owed).
  Status/error strings dropped it (`provider failed (...)`,
  `turn_end provider`, `resource path ...`); sessions moved to
  `.yach/sessions/` (both edit-guard deny rules moved with the path);
  the `--backend` flag is now just `fixture`, since `native` and
  `native-provider` both selected the default. Where the extension
  contrast would actually live is `BackendKind::Native`, which stays.
- **DONE 2026-07-28** — `Native*` prefix stripped (owner committed):
  166 types and 315 functions, all crates. Collision review came back
  empty against sibling names, existing types, and `yach_proto`; no
  serde attribute ever exposed a type name, so the persisted session
  format is untouched. The module confusion resolved by naming each
  file for what it holds: the old 132-line `runner.rs` was pure
  `Backend*` items (channels, session, metadata) and became
  `backend.rs`, freeing `runner.rs` for the actual runner (formerly
  `native_runner.rs`, with `native_runner/` -> `runner/`). Six
  functions kept a qualifier where stripping would have collided, and
  five locals were renamed for what they actually distinguish
  (`native_session_id` -> `typed_session_id`, since it is the typed
  form of an in-scope `session_id`). Benchmark labels in `yach-bench`
  deliberately keep their historical spelling so reports stay
  comparable with `docs/benchmarks/`. Also retired the last Pi-era
  test naming (`default_rpc_handshake` -> `default_backend_handshake`).
- **queued** — `BackendCapabilities.tool_execution` is still `false`
  for the native runner even though tools execute (found during the
  naming cleanup; a behavior flag rather than naming, so left alone).
- **queued** — Harbor-course packaging (now the comparison-track
  prerequisite): musl static artifacts (x86_64/aarch64) + sha256.
  Note: yacht's recorded-baseline comparisons (ADR 0018) landed
  2026-07-28 (yacht #258), so version-vs-version yach comparisons
  re-run only the new vessel once a course logbook exists.
- **open** — yacht entry 9 (nondeterministic agent-prompt response
  contract) pending on yacht's side; until then preflights carry a
  self-report prompt workaround.
- **MEASURED 2026-07-30** — Native tool-call messages implemented and
  re-measured (`records/2026-07-30-tool-call-after-measurement.md`,
  PR #204): 82/100 to 95/100. The disconfirming prediction resolved in
  favour of the root cause — `tool-call-economy` on haiku moved 0/5 to
  5/5 — and responses reproducing yach's own transcript format went
  from 38 to 0 across 100 outcome documents. `notes-tally-fix` on the
  chat-completions models went 7/15 to 14/15. One counter-result:
  `compaction-continuation` on nemotron 5/5 to 3/5, both failures the
  model reading and then never calling a write tool, with compaction
  firing correctly; unsettled, wants higher n on that cell before it
  is called either way. Detect-and-nudge stays shelved on this
  evidence.
- **next (spec in review)** — Native tool-call messages
  (`specs/2026-07-28-native-tool-call-messages-design.md`). REFRAMES
  the echo-imitation defense: root-cause reading found yach flattens
  the whole conversation into one labeled string and never sends
  native tool-call structure, though rig 0.38.2 supports it and yach
  already parses it inbound. That one gap explains both behavioral
  failures — the transcript shows assistants writing
  `[requested tool calls: ...]`, so imitation is taught rather than
  spontaneous; and with no `tool_use`/`tool_result` binding the
  provider's trained tool loop never engages, so calls repeat. The
  round echo was itself a workaround for the missing structure (its
  comment cites the sesh 161-identical-reads run). Plan: evals +
  baseline rates first, then the structural change, then compare.
  BASELINE RECORDED 2026-07-28 (100 cells, 5 tasks x 4 profiles x 5
  repeats): `records/2026-07-28-tool-call-baseline.md`. Headline —
  haiku repeats the edit call 0/5 on the simplest task (reproducible,
  not intermittent as #197 read it), and a failing nemotron run's
  response text reproduces yach's own flattened transcript verbatim
  (`[requested tool calls: ...]`, `Tool:` labels, JSON blobs) with no
  edit tool called — direct evidence for the root cause. The two
  classes are inversely distributed by wire shape, which is why
  per-shape coverage was the right call. Gaps: chatgpt-subscription
  deferred (needs a token dir the cell runner cannot deliver); OpenAI
  pending a model id.
  Owner decisions 2026-07-28: one larger slice (no dual path); the
  baseline spans provider *shapes* — Anthropic, Zen, OpenAI,
  chatgpt-subscription — since tool support varies per provider and
  that is the dimension being changed; the round echo is deleted
  entirely. Note: the chatgpt-subscription path has never run a real
  session, so slice 1 is its first real exercise.
- **slated (fallback only)** — Detect-and-nudge echo defense: reject
  echo-format/fabricated tool-call text once (Codex reject-posture).
  Kept only in case native tool messages leave a material residue;
  closed or re-opened on measurement, not opinion. Research:
  `records/2026-07-26-behavioral-fixes-cohort-research.md`.
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

- **queued** — Upgrade rig to current, as a focused update (owner
  principle, 2026-07-29: do not build around an older version of a
  fast-moving dependency while yach is still early; the
  `additional_params` workaround below is explicitly "works for now",
  not the resting state). Known cost from the 0.41.0 survey: it drops
  `stream_chat` from the streaming module, so the streaming path needs
  rework; it adds no `max_completion_tokens`, so it fixes nothing on
  its own. Sequencing matters — the native tool-call messages work
  rewrites the same adapter surface, so doing both blind at once would
  confound the before/after measurement. Cleanest order is: land the
  tool-call refactor against its existing baseline, measure, then
  upgrade as its own change and re-run the same evals as the
  regression check. The eval portfolio is what makes that upgrade
  safe to attempt at all. Related open question below (own thin layer
  vs middleware) may be answered by how painful this proves.
- **next** — yach cannot talk to current OpenAI models (found
  2026-07-29 on the real endpoint's first exercise, baseline record):
  rig sends `max_tokens`, which those models reject in favour of
  `max_completion_tokens`, and rig 0.38.2 has no support for it.
  Aggregators still accept `max_tokens`, so Zen coverage was masking
  the gap. Rig 0.41.0 checked 2026-07-29: still no
  `max_completion_tokens`, and it drops `stream_chat` from the
  streaming module, so the bump costs migration work and fixes
  nothing. No new rig needed — the pinned 0.38.2 request already skips
  `max_tokens` when `None` and flattens `additional_params` into the
  body, so the fix is entirely in yach's adapter. Which parameter name
  a provider wants is capability data for the model catalog, not a
  loop branch.
- **slated** — Tiered provider-error classifier: parse status + typed
  JSON error body ahead of the keyword ladder; per-provider dialects in
  the model catalog. CONCRETE CASE 2026-07-29: a body carrying
  `type=invalid_request_error code=unsupported_parameter
  param=max_tokens` was classified `unavailable_model` with guidance
  "check YACH_RIG_*_MODEL", sending two rounds of investigation after
  a model name that was never wrong. The typed fields needed to
  classify it correctly were all present in the body.
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

## Release flow

- **DONE 2026-07-27** — Publication: repo flipped to build-in-public;
  yach 0.1.0 published to crates.io (yach-proto, yach-ui,
  yach-backend, yach) after metadata prep and a clean history audit
  (#189). No launch; onboarding polish deliberately deferred to the
  catalog/provider-surface work.
- **queued** — Release flow formalization: a `just publish` recipe
  (enforces publishing from a synced working copy — cargo publish
  fails on jj's checked-out change otherwise), version-bump
  conventions, and install docs (`cargo install yach`) in the README.

## Open owner questions (no schedule)

- **open** — Execution isolation landscape (sandboxing, containers,
  hermetic filesystems) — deliberately undecided.
- **open** — Rig longevity / provider-integration ownership (own thin
  layer vs middleware; Codex/Pi own theirs, opencode delegates).
