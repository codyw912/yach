# Model-Catalog Slice 2 Measurement (2026-08-03)

Verification for catalog hydration slice 2
(`specs/2026-08-02-model-catalog-hydration-design.md`, fetched-layer
contract; owner rulings 2026-08-03: status line + session start; cap
only what's dangerous): the shared models.dev transform, the
`Fetched` resolution rung, and the background refresher with ETag
caching. Runtime image guard-verified; gate 7/7 plus driver checks.

## Live wire checks (host binary, before the sweep)

The full refresh lifecycle demonstrated on the real endpoint:

- First run: `status: catalog refreshed (232 models, models.dev
  2026-08-03)`; `~/.yach/catalog/models-dev.json` written with ETag;
  that run's own config still read `baked:2026-08-02` — resolution
  never waits, the cache feeds the next session, exactly as designed.
- Second run: `status: catalog up to date` (the 304 revalidation
  path), and provenance flipped: `"context_window": {"source":
  "fetched:2026-08-03", "value": 200000}`,
  `"rates_source": "fetched:2026-08-03"` — same values, honestly
  relabeled to the layer that now serves them.
- The fetch-failed path is covered by unit tests (exact fallback
  strings); forcing it live requires killing the network and was
  skipped deliberately.

## Rates (passes / runs)

| task | anthropic-haiku | zen-qwen | zen-nemotron | zen-deepseek | openai |
|---|---|---|---|---|---|
| tool-call-economy | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| tool-result-dependence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| multi-round-sequence | 5/5 | 4/5 | 5/5 | 5/5 | 5/5 |
| compaction-continuation | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| notes-tally-fix* | 5/5 | 5/5 | 5/5 | 5/5 | 4/5 |

**123/125 of launched cells** (main run 99/100; the
`notes-tally-fix` block lost to the credential lapse and re-run as a
patch, 24/25 — *that column). Both misses behavioral:

- zen-qwen `multi-round-sequence` r5: its script printed 0 instead
  of 42 — did-not-verify class; qwen's first drop since 2026-07-30.
- gpt-5.4-mini `notes-tally-fix` r5: stopped short and OFFERED to
  finish ("If you want, I can keep going and fix the script")
  instead of finishing. Second occurrence of this exact
  ask-instead-of-act pattern on this task (slice-1 measurement had
  the sibling), which promotes it from noise to a quirk-corpus
  observation: gpt-5.4-mini, notes-tally-fix, 2 of its last 10 runs.

Container cells confirmed the in-container fetch is harmless: each
ephemeral cell fetched fresh (200 path), resolved from baked (the
refresh lands after resolution), and rewards were unaffected.

## Security posture (final-review driven, owner-ruled twice)

Fetched data is community-published and unreviewed, so the shared
transform bounds what a wrong or hostile payload can do — refined
after an audit showed blanket clamps would distort real data:
context_window caps at 2M (the one number with unbounded blast
radius via compaction accounting; no floor — real 77-token embedding
windows pass), output_ceiling unclamped (`min(ceiling, 32k)` bounds
its effect; real 512k ceilings survive), cost rates cap 1000/M,
display names sanitized. Cache writes are atomic (temp + rename);
one catalog generation per session start; `--backend fixture` makes
no network calls. A hand-edited cache is indistinguishable from
fetched (same label) — accepted: `~/.yach` is the user's own trust
domain, and the higher-precedence override file is theirs by design.

## Process finding: the sweep credential lapse is systematic

Three consecutive sweeps have lost the same trailing
`notes-tally-fix` block to the authorization TTL (~50 minutes in,
always the last task). Queued fix: per-task-block credential
re-resolution in the sweep driver, so a lapse costs a delay, not a
block.

## Coverage

chatgpt-subscription unmeasured (standing). Slice 3 (provider
discovery / key-truthful picker) remains queued; the refresh
throttle (pi's checkedAt mechanic) rides with it.
