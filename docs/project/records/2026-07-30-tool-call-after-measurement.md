# Tool-Call After-Measurement (2026-07-30)

Re-measurement of the slice-1 baseline
(`2026-07-28-tool-call-baseline.md`) against the native tool-call
messages change. Same tasks, same profiles, same five repeats per cell.

## Rates (passes / runs)

| task | anthropic before | anthropic after | zen before | zen after |
|---|---|---|---|---|
| tool-call-economy | **0/5** | **5/5** | 15/15 | 15/15 |
| tool-result-dependence | 5/5 | 5/5 | 14/15 | 15/15 |
| multi-round-sequence | 5/5 | 5/5 | 11/15 | 13/15 |
| compaction-continuation | 5/5 | 5/5 | 15/15 | **13/15** |
| notes-tally-fix | 5/5 | 5/5 | **7/15** | **14/15** |

Totals: 82/100 before, 95/100 after. Zen columns aggregate qwen,
nemotron and deepseek. No cell failed to launch in either run.

## The predictions, answered

1. **`tool-call-economy` on haiku moves off 0/5** — yes, to 5/5. This
   was the disconfirming case: had it stayed at 0/5, the root-cause
   claim was wrong and the detect-and-nudge fallback came back off the
   shelf. It moved.
2. **`notes-tally-fix` improves and no response contains the echo
   format** — yes on both. 7/15 to 14/15, and the format count went
   from **38 responses to 0** across 100 outcome documents. Models were
   reproducing a format we showed them; the format is gone, so the
   behavior is gone.
3. **`compaction-continuation` stays 20/20** — no. 20/20 to 18/20, both
   losses on nemotron. See below.
4. **`tool-result-dependence` holds** — yes, 19/20 to 20/20. Native
   `tool_result` blocks reach the model at least as well as the
   flattened text did, so the mapping is not lossy.

## The one that moved the wrong way

`compaction-continuation` on nemotron went 5/5 to 3/5. In both failing
runs compaction fired correctly and every turn completed; the model
read `codeword.txt` and then never called a write tool, so `answer.txt`
was missing. One of the two spent its final turn on
`list_project_paths` / `read_text_file` / `search_project` instead.

Read as: the context-rebuild path works (compaction fired, turns
completed, later turns still called tools), and the failure is the
same did-not-finish-the-work class that improved sharply on
`notes-tally-fix` for this same model. Two failures at n=5 on the
flakiest model in the set is weak evidence either way.

Not treated as settled. What would settle it: repeat the nemotron
cell at higher n, and compare against a nemotron-only baseline at the
same n. A drop caused by the structural change should reproduce; model
variance should not.

## Note on the comparison's integrity

The first after-run lost 100 of 125 cells to a guard added hours
earlier: it flagged any `scheme://` value as an unresolved secret
reference, and `YACH_RIG_*_BASE_URL=https://...` is a legitimate
endpoint. Every profile carrying a base URL was refused; anthropic,
the only one without, ran clean. The guard now inspects only
secret-named variables.

Worth recording because the guard existed to prevent misleading
zeros and instead produced a hundred of them. The failure was visible
only because launch failures are recorded as `reward=error` rather
than folded into the rate — without that, this document would report
a catastrophic regression that never happened.

## Coverage

Unchanged from the baseline: chatgpt-subscription is not measured (it
needs a token directory the cell runner cannot deliver), and OpenAI
proper is still blocked on the `max_tokens` / `max_completion_tokens`
gap. Both are gaps in the sweep, not results.
