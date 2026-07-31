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
- **slated (needs a design pass)** — Return tool results as text, not
  JSON objects. Cohort research 2026-07-31 across nine harnesses —
  with a read tool: pi, opencode, Crush, OpenHands, Claude Code, cline;
  without one (reads go through shell `cat`): Codex, goose, nanocodex;
  aider noted separately as having no tool-call path at all.
  **5 of 6 read tools return text**, the lone exception (cline) ships an
  undeclared envelope and is predominantly run against Anthropic — the
  models that handled ours fine — so it is evidence someone else
  carries the same unmitigated risk, not that it is safe.

  **Weighted reading (owner, 2026-07-31).** Weigh Codex, nanocodex, pi
  and opencode; Claude Code cannot be leaned on at all since it is not
  open source and the entry above rests on documentation only. cline,
  aider, goose, Crush and OpenHands are not comparisons the owner would
  weight — recorded for completeness, not as evidence. Add omp to the
  cohort where it deviates from vanilla pi (gaining adoption); it has
  not been looked at yet and is the one gap in this reading.

  Narrowed to those four the conclusion holds and gets cleaner: on
  reads it is text or no read tool at all (pi returns byte-exact text,
  opencode returns text delimited with tags and a gutter, Codex and
  nanocodex have no read tool); on shell it is three text against one
  declared-JSON (nanocodex). The single heaviest point is that Codex
  *removed* our exact exec shape on purpose — a reversal, not an
  absence.

  The strongest precedent is a deletion: Codex PR 22706 (2026-05-18)
  removed `{"output":…,"metadata":{exit_code,duration_seconds}}` — our
  `bash` shape almost exactly — for plain `Exit code:/Wall time:/Output:`,
  because "responses are already plain text for model consumption".
  goose computes a structured `ShellOutput`, declares an output schema
  for it, and all three provider formatters discard it and ship text.
  MCP's `CallToolResult` agrees in writing: `structuredContent` is a
  *sibling* of `content`, never a wrapper.

  Two further findings that shape the design:
  - **Errors must be legible in the text.** Only pi and Crush set
    Anthropic's `is_error`; OpenAI has no error slot at all, so a
    failure that is only structural is invisible there.
  - **The split is on delimiting, not text-vs-JSON.** opencode and
    Crush wrap file text in `<file>`-style tags with a line gutter; pi
    is the only harness returning byte-exact contents. yach's envelope
    has five sibling keys and nothing marking which holds the file, and
    `read_text_file` declares no output schema — undeclared structure
    is the actual defect.

  Note for the eval: most harnesses would also fail
  `tool-result-dependence` as written, by writing gutter-prefixed text
  rather than a JSON object. That is a difference in kind worth
  encoding rather than papering over.
- **next** — Slim the tool-result payload, now with evidence (2026-07-31).
  The native tool-call change deliberately kept the result payload
  byte-identical so the before/after measurement isolated the
  structural variable, noting that slimming it was a separate,
  separately measured change. The OpenAI cells then failed
  `compaction-continuation` 3/5 by writing the whole blob into the
  answer file —
  `{"byte_count":22,...,"text":"marigold-7731-lantern\n",...}` — where
  Anthropic and Zen models extract the `text` field. Same payload,
  different legibility per provider. The embedded `provider_call_id` is
  also redundant now that the block carries its own id. Re-measure per
  provider shape, since the point is that they differ.
- **queued** — `notes-explore` is brittle by construction: it runs
  without `--full-auto` to exercise the default approval posture, so
  *any* review-gated call fails the turn, and the instruction only
  forbids modifying files. A model choosing `bash ls` fails it while
  behaving reasonably (seen 2026-07-31). The harness is correct; the
  task conflates "no writes happened" with "the model avoided gated
  tools". Steer the instruction away from shell, or assert on the
  approval-required outcome deliberately rather than incidentally.
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

- **slated (owner leaning 2026-07-31: migrate to the canonical
  endpoint; needs a design pass, not a patch)** — Use rig's Responses
  API surface for OpenAI proper
  (upstream exploration, 2026-07-31). `openai::Client` in rig-core
  **defaults to the Responses API** and has since ~0.30; the
  chat-completions surface is the opt-in `.completions_api()`, which
  yach calls deliberately. On the default path rig already maps
  `max_tokens` -> `max_output_tokens`, so the parameter gap only exists
  because yach treats OpenAI proper as just another
  openai-compatible endpoint. Consequences: the
  `max_completion_tokens` workaround is correct but its motivating case
  disappears on the Responses path; and this unblocks the already
  queued OpenAI Responses provider-native compactor behind the
  `Compactor` seam, whose whole premise is the server-side state that
  API offers. Needs a provider-variant decision, so it couples to the
  provider/model product surface item.

  Questions the design pass must answer before any code, so it does not
  start blind: does rig's Responses path carry tool calls in a shape
  the native tool-call mapping can target (that API models function
  calls and output items differently from chat-completions
  `tool_use`/`tool_result`); do the raw streaming events differ enough
  to affect the collector; and does the existing
  `ChatGptSubscription` variant already ride a Responses-shaped path,
  since rig's `providers/chatgpt` uses `max_output_tokens` — if so,
  there may be one surface here rather than two.
- **open (owner 2026-07-31: not until it blocks us)** — Report the
  chat-completions parameter gap upstream to rig.
  `.completions_api()` + real OpenAI + `max_tokens` is a genuine 400
  and no issue exists (searched 2026-07-31: zero hits across issues,
  PRs and discussions), and rig has name-mapping precedent (ollama PR
  2185 maps `max_tokens` -> `options.num_predict`), so it would likely
  be accepted. But the `additional_params` workaround needs no rig
  change, and the Responses migration above would remove our exposure
  entirely. File it when something is actually blocked, and only with
  the behavior verified first-hand rather than inferred — the bar we
  failed three times on 2026-07-30/31 before this same gap was
  understood properly.
- **next (spec in review)** — Rig upgrade: own the loop
  (`specs/2026-07-31-rig-upgrade-own-the-loop-design.md`). Drops the
  Agent abstraction for `rig-core`s model-level seam
  (`completion_request(..).build()` + `CompletionModel::stream`), which
  is the level yach already operates at. `CompletionRequestBuilder`
  carries everything the branches need and `apply_rig_tool_definitions`
  survives untouched; the native tool-call mapping is unaffected since
  it uses `completion::message` types that survive the split. Main
  risks: the raw-event collector is assumed compatible from types
  rather than a compile, and `token_usage` becomes non-optional, which
  can turn "provider reported nothing" into a silent zero. Validated by
  the evals as a regression check against the recorded 95/100.
- **queued** — Upgrade rig to current, as a focused update (owner
  principle, 2026-07-29: do not build around an older version of a
  fast-moving dependency while yach is still early; the
  `additional_params` workaround below is explicitly "works for now",
  not the resting state). Needs a spec before implementation — the
  survey below is design input, not a plan.

  **What 0.41 actually is** (upstream PR 2197, verified against the
  crate sources and by compiling, 2026-07-31; supersedes two earlier
  guesses of mine): rig split into `rig-core` (portable contracts —
  clients, messages, streaming, tools) and `rig-agent` (the classic
  runtime: agent builder, multi-turn driver, hooks), behind a `rig`
  facade re-exporting both. yach depends on `rig-core` *aliased as*
  `rig`, so the `agent` module did not vanish — we were looking at the
  half that never had it.

  Two viable shapes, and the choice is the actual design question:

  1. **Stay on the Agent runtime.** Depend on the `rig` facade with the
     `agent` feature; `client.agent(model)` returns via the
     `AgentClientExt` extension trait (`use rig::prelude::*`), and
     `stream_completion` becomes `StreamingChat::stream_chat`. But
     0.41's `StreamingPromptRequest` is a multi-turn loop driver
     (`max_turns`, `tool_concurrency`, `add_hook`, `final_response`) —
     it wants to run the tool loop that yach exists to own, and the
     current tool-advertising step operates on a request builder that
     shape no longer hands out.
  2. **Drop the Agent abstraction and use `rig-core` directly.**
     `CompletionModel::stream(request)` exists in both 0.38.2 and 0.41,
     which is the level yach actually operates at: build the request,
     stream it, drive our own loop. More adapter code, one fewer
     dependency, and it stops borrowing a runtime whose loop we
     immediately discard. Worth weighing against the open
     own-thin-layer-vs-middleware question below, which this is a
     concrete instance of.

  **Owner decision 2026-07-31: option 2 — yach owns the loop, so drop
  the Agent abstraction.** Extensions adding tools forces it, and the
  rest of the loop settles it: review/approval gating per call, the
  sensitive-path deny chokepoint, session persistence of every tool
  request and result for replay fidelity, compaction between rounds,
  streamed tool output, budget accounting. None of that is expressible
  in rig's driver, so handing over the loop would cost the product,
  not a customization.

  Confirming evidence: yach never gives rig executable tools. It passes
  `ToolDefinition`s for advertising and executes every call itself, so
  the Agent is already only a request builder and stream source — the
  runtime half is vestigial today. Dropping it removes a dependency on
  something we never used.

  Independent of the choice: `GetTokenUsage::token_usage` changed
  signature. The native tool-call mapping survives either way — it is
  expressed in `completion::message` types that remain. 0.41 adds no
  `max_completion_tokens`, so it fixes nothing on its own. Sequencing matters — the native tool-call messages work
  rewrites the same adapter surface, so doing both blind at once would
  confound the before/after measurement. Cleanest order is: land the
  tool-call refactor against its existing baseline, measure, then
  upgrade as its own change and re-run the same evals as the
  regression check. The eval portfolio is what makes that upgrade
  safe to attempt at all. Related open question below (own thin layer
  vs middleware) may be answered by how painful this proves.
- **DONE 2026-07-31** — Stale `yach-runtime` image is now detected, not just documented: `just runtime-image` stamps a content digest of the crate sources and manifests into the image, and gate/sweep recompute it and refuse on mismatch. Evals run the container binary, so a run after a code change otherwise measures the previous build silently — it completes, cells score, and the numbers look like a result. Cost one wrong "the fix did not work" conclusion on 2026-07-30.
- **FIXED AND CONFIRMED LIVE 2026-07-30** (premise narrowed 2026-07-31, see below) — the max_tokens / max_completion_tokens gap on the chat-completions surface: `MaxTokensParam` on the adapter config selects the spelling, set by `YACH_RIG_PROVIDER_MAX_TOKENS_PARAM` and defaulting to `max_tokens` so every measured path is unaffected. `max_completion_tokens` rides rig's flattened `additional_params` with `max_tokens` left unset, so no rig fork or upgrade was needed. NOTE (corrected 2026-07-31): the mechanism is not a catalog stopgap — rig has no `max_completion_tokens` in any version, so every rig client hitting current OpenAI models needs it. The catalog retires the env var (who supplies the spelling), not the mechanism; that retires when rig sends the right field upstream. Confirmed on the real endpoint: gpt-5.4-mini completed tool-result-dependence, round-tripping a token that exists only inside a tool result, with provider-reported usage. First task yach has ever completed on OpenAI proper, and it ran through the native tool-call mapping.
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

- **slated** — Model-catalog hydration design; unblocks five stopgaps:
  per-model context windows, per-model output budgets, curated /model
  list, truncated-tool-call recovery, and the output-budget parameter
  spelling (`YACH_RIG_PROVIDER_MAX_TOKENS_PARAM`, added 2026-07-30 —
  making an operator know whether their provider wants `max_tokens` or
  `max_completion_tokens` is exactly the API detail a catalog should
  carry). Error dialects join it (above).
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

## Product shape

- **principle (owner, 2026-07-31)** — Capabilities like code mode ship
  as **extensions, not core**. The core stays minimal; opinionated
  setups arrive as extension bundles or distributions layered on top.
  Raised over rig's code-mode proposal (upstream issue 1439), and it
  settles that case: as an extension, yach needs nothing from rig for
  it, so where upstream puts code mode stops constraining us. Same
  instinct as the lean system-prompt and behavioral-fixes rules —
  weight belongs at the edges, not the middle. Design note in
  `specs/2026-07-31-rig-upgrade-own-the-loop-design.md`: code mode as
  an extension is naturally one `execute`-style tool whose sandbox
  bindings call back through yach's tool executor, which preserves
  review gating and the sensitive-path chokepoint for free; the
  tension to design against is approval granularity, which makes it a
  forcing case for the slated approval-model work.

## Open owner questions (no schedule)

- **open** — Execution isolation landscape (sandboxing, containers,
  hermetic filesystems) — deliberately undecided.
- **open** — Rig longevity / provider-integration ownership (own thin
  layer vs middleware; Codex/Pi own theirs, opencode delegates).
