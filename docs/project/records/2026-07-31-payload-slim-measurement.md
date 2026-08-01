# Payload-Slim Re-Measurement (2026-07-31)

Re-measurement after the tool-result payload slim (#210), which was
deliberately deferred out of the native tool-call change so each was
measured alone. Same five tasks, same five repeats as the 2026-07-28
baseline and the 2026-07-30 after-measurement; the openai profile
joins for its first full sweep, so the matrix is 125 cells — 5 tasks
x 5 profiles x 5 repeats. Runtime image rebuilt from the measured
commit and verified by the stale-image guard before any cell ran.

## Rates (passes / runs)

| task | anthropic-haiku | zen-qwen | zen-nemotron | zen-deepseek | openai |
|---|---|---|---|---|---|
| tool-call-economy | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| tool-result-dependence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| multi-round-sequence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| compaction-continuation | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| notes-tally-fix | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |

**125/125.** On the 100 cells comparable to the previous measurement
(the four pre-existing profiles), 100/100 against 95/100. No cell
failed to launch. Spot checks confirmed the cells are real: outcome
documents carry provider-reported usage (`"reported": true`) for the
expected models, and answer files hold the expected content.

## The failure this change targeted is gone

The motivating case was `compaction-continuation` on the OpenAI shape
in the slice-1a sweep (2026-07-31, pre-slim): 2/5, with all three
failing runs writing the entire tool-result JSON blob into
`answer.txt` where other shapes extracted the `text` field. This run:
5/5, and all five answer files contain the bare codeword. The envelope
those models were echoing no longer exists to echo.

That 2/5 predates the recorded 2026-07-30 table (the OpenAI cell was
unblocked after those numbers were taken), so the openai column here
is both its first clean full-sweep baseline and the direct
before/after for this change on the one documented failure.

## The nemotron question does not reproduce

The 2026-07-30 measurement left `compaction-continuation` on nemotron
unsettled at 3/5 (both failures did-not-finish, wanted higher n), and
the slice-1a sweep repeated it at 3/5. Here it is 5/5. Strictly this
is a different condition — the payload changed — so it does not
retro-settle whether the native tool-call change caused a real drop;
but under the current system the concern does not reproduce, and
there is no longer a pre-slim configuration worth spending cells on.
Treated as closed unless it recurs.

## Caveats

- n=5 per cell remains small; a perfect score at this n bounds
  per-cell failure rates only loosely. The comparison that matters —
  no regression from 95/100, and the two known failure modes gone —
  does not depend on tight bounds.
- The suite now scores at ceiling for every measured shape. It can no
  longer discriminate improvements, only regressions. Fine for a
  regression gate; the verifier-awareness and cross-harness tracks
  are where new discriminating tasks come from.
- chatgpt-subscription is still unmeasured (needs a token directory
  the cell runner cannot deliver). A coverage gap, not a result.

## What this validates beyond #210

This is also the first sweep with the slice-1a seam swap (#208) and
the payload slim together on every provider shape, including OpenAI
proper — jointly clean at 125/125. The slice-1a sweep it follows had
7 launch-failure cells (excluded from rates by design) alongside its
9 task failures; this run had zero of either.
