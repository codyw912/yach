# Masking slice 2 measurement

Date: 2026-08-11
Spec: docs/project/specs/2026-08-09-compaction-masking-design.md
Plan: docs/project/plans/2026-08-09-compaction-masking.md
Stack: main..e8dc0217 (22 commits; final whole-branch review v2 merge-ready)

## Method

Owner-run live gate (`op run --env-file anthropic-haiku.env -- just
eval-gate`, claude-haiku-4-5, yach-runtime image rebuilt at the stack HEAD).
The new `masking-reclaim` task resumes a synthetic seeded session (8 terminal
turns, ~48KB of tool-result bodies, chapter-1 codeword) with the fixture's
compaction config (reserve 1000, keep_recent 500, threshold 10%, usable
35,000 → trigger ~3,500 tokens, masking floor 8,192 net tokens).

## Gate result

8/8 tasks passed on the first attempt, all five driver checks passed, no
retries or fallbacks. Live portion: 81 seconds of task time
(compaction-continuation 21s, masking-reclaim 10s, rest 4-16s each) —
inside the two-minute target.

## masking-reclaim evidence (evals/.gate/masking-reclaim/primary-attempt-1/)

- Pre-turn threshold check fired the masking pass: **7 results masked,
  44,735 bytes, 11,060 net tokens reclaimed** — the exact figure predicted
  by the candidate-selection arithmetic at review time (chapters 1-7
  eligible, chapter 8 protected by the 500-token keep-recent window).
- Status sequence observed: `context masked (11060 tokens reclaimed)` →
  `compacting context...` → `context compacted (summary): ~3K -> ~2K`.
- The live turn then answered the codeword prompt correctly:
  `answer.txt` = `juniper-4417-ember`, content that existed only in the
  masked chapter-1 body. The model re-read the file after seeing the
  elision marker — post-mask continuation works.
- Outcome accounting: turn reports `compactions: 1`,
  `masked_results: 7`, `masked_bytes: 44735`; usage reported
  (10,411 input / 194 output tokens for the resume turn).
- Session log: 7 `tool_result_masked` events, no checkpoint for the mask
  itself; the later checkpoint is the mid-turn summary.

## Interpretation

The run exercised both tiers in the intended order. Masking reclaimed the
stale chapter bodies up front (no summary call for them); when the codeword
re-read refilled context past the 3.5k threshold mid-turn, the summary
compactor ran on already-masked input (~3K observed, versus an estimated
~12.5K pre-mask context from the seed arithmetic — no control run was
measured). Masking deferred and shrank the summarization — the hybrid
behavior the design cited from the cohort evidence (mask first, summarize
later).

Note the shape this implies for real sessions: masking does not prevent
summaries, it makes them cheaper and later. Sessions whose growth is
dominated by one-shot tool output (file reads, search hits, command output)
get the largest benefit.

## Verification history (cumulative)

- Per-task reviews on Tasks 1-5 and 7, six fix rounds total.
- Final whole-branch review v2 at e8dc0217: merge-ready, no findings.
- `just test` exit 0 (15 suites, pipefail-verified), lint/fmt clean,
  eval-validate 8/8, compaction 63/63, runner 220/220.
- This gate run: 8/8 tasks + 5/5 checks, first attempt, 81s live.

## Follow-ups

- Board status: slice 2 MEASURED.
- Deferred by design: pinning/useless flags (extension-tool contract),
  TUI reveal affordance, argument masking.
