# Model-Catalog Slice 1 Measurement (2026-08-03)

Verification for catalog hydration slice 1
(`specs/2026-08-02-model-catalog-hydration-design.md`): the
`yach-catalog` crate with baked models.dev data, override layers,
per-field provenance, cost reporting, and the five stopgap consumers
rewired. This slice changes numbers in flight — per-model context
windows and output budgets now feed compaction accounting — so the
sweep was mandatory. Runtime image guard-verified; gate 7/7 plus
driver checks (including `outcome-schema` against documents carrying
the new `config`/`cost` blocks).

## Rates (passes / runs)

| task | anthropic-haiku | zen-qwen | zen-nemotron | zen-deepseek | openai |
|---|---|---|---|---|---|
| tool-call-economy | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| tool-result-dependence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| multi-round-sequence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| compaction-continuation | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| notes-tally-fix* | 5/5 | 5/5 | 5/5 | 5/5 | 4/5 |

**124/125 of launched cells.** The main sweep lost its entire
`notes-tally-fix` block (25 cells) to a credential-authorization
lapse — recorded as `reward=error`, excluded from rates, re-run as a
patch block (*that column). The one miss: gpt-5.4-mini wrote a tally
script whose grep pattern returned zero counts, said so in prose,
and stopped — behavioral did-not-finish, turn completed cleanly,
first behavioral drop for that model in four sweeps. Not a catalog
effect: its config block shows the correct 272k window and rates.

## Cost and provenance verified by hand

Two cells recomputed to the digit:

- openai `tool-result-dependence`: (2058 x 0.75 + 56 x 4.5) / 1M =
  **0.001796 usd** — matches `cost.usd` exactly;
  `rates_source: "baked:2026-08-02"`; window
  `{272000, baked:2026-08-02}` (the gpt-5.x standard-window pin);
  budget `{32000, default}` (the capped-case provenance fix labeling
  honestly).
- anthropic-haiku `compaction-continuation`: (27080 x 1 + 522 x 5) /
  1M = **0.02969 usd** — exact; and its window reads
  `{68000, env}` — the compaction fixture's env override flowing
  through the EnvOverride layer with visible provenance. The
  provenance system demonstrating itself on real evidence.

## Behavior changes shipped (all owner-ruled)

- Claude 5-family sessions get their native 1M windows (community
  data confirmed correct; my initial capability-gating concern was
  stale knowledge).
- OpenAI gpt-5.x pins to the 272k standard window at snapshot
  generation (extended context is explicit opt-in with a 2x price
  cliff past 272k input; codex and omp pin the same; retire with a
  deliberate extended-context option).
- `/model` lists the baked catalog (dated-snapshot aliases filtered);
  mid-session switches rehydrate window/budget/spelling from the
  supplied entries — closing the stale-window trap the whole-branch
  review caught before it ever shipped.
- Zero-valued catalog metadata (0 windows, 0/0 cost rates) is treated
  as absence: the floor serves, and no fabricated $0 can reach
  evidence.

## Catalog design findings worth keeping

- Published context figures are ambiguous between total and input,
  and between native and opt-in tiers; generation-time policy
  transforms (omp's pattern) are the right home for corrections —
  yach's first two: the gpt-5.x pin and the zero-as-absent rule.
- Pre-existing, now board-tracked: `sum_log_usage` sets
  `reported: true` if ANY entry carries usage, so partially-reported
  sessions read as fully computed with understated sums
  (headless.rs:355; confirmed during the Task 6 audit).

## Coverage

chatgpt-subscription remains unmeasured (standing gap). Slices 2
(fetched refresh) and 3 (provider discovery) remain queued; the
picker-hygiene minors from the final review (hyphen-dated openai
aliases, non-chat models listed) are ledgered against slice 3's
key-truthful discovery.
