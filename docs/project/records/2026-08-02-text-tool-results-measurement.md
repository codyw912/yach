# Text Tool-Results Measurement (2026-08-02)

Re-measurement after converting every built-in tool result from
undeclared JSON to plain text
(`specs/2026-08-01-text-tool-results-design.md`): byte-exact content,
bracketed exception-only notices, errors as a verdict line plus
guidance prose. Same method as the prior measurements: 125 cells — 5
tasks x 5 profiles x 5 repeats, runtime image rebuilt from the
measured commit and guard-verified, credentials resolved once per
profile. Reference: the 2026-08-01 rig-0.41 baseline (99/100 launched
cells + 24/25 patch; both misses zen-nemotron noise — one provider
rate limit, one did-not-finish).

## Rates (passes / runs)

| task | anthropic-haiku | zen-qwen | zen-nemotron | zen-deepseek | openai |
|---|---|---|---|---|---|
| tool-call-economy | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| tool-result-dependence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| multi-round-sequence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| compaction-continuation | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| notes-tally-fix | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |

**125/125.** Zero launch failures. Gate before the sweep: 7/7 tasks
plus all three driver checks. Spot checks confirm real runs:
provider-reported usage (`"reported": true`) across shapes, bare
codewords in answer files.

## The directional check

The one falsifiable prediction was that the envelope-echo class stays
at zero: no response or answer file reproducing tool-result structure.
A scan of every workspace artifact for `"outcome"` / `"exit_code"` /
`"byte_count"` fragments found nothing. The class was zero before this
change (the structure was already slimmed); it is still zero now that
the structure does not exist at all.

## What this measurement can and cannot say

- It says the conversion caused no regression on any measured shape,
  and `tool-result-dependence` — the task that round-trips a token
  living only inside a tool result — passes 25/25 with the token now
  arriving as byte-exact file text rather than a JSON field.
- It cannot show improvement: the suite has scored at ceiling since
  the payload slim, so it discriminates regressions only. The
  motivation for this change was cohort convergence and removing
  undeclared structure for unmeasured models, not a measured failure
  on these cells.
- zen-nemotron passed 25/25 this run after dropping cells in three of
  the four prior sweeps — consistent with its failures being model
  noise, not any yach change.

## Coverage

chatgpt-subscription remains unmeasured (needs a token directory the
cell runner cannot deliver) — a standing gap, not a result. Its code
path compiles through the same shared helpers as the measured shapes.

## Deviation recorded

The bash truncation notice reads `[truncated: kept X of Y output
bytes]` rather than the spec's `kept first A and last B of C bytes`;
the split position is carried by the inline `... [N bytes omitted]
...` seam marker the bounded capture already inserts. Accepted at
final review; the spec's wording is amended to match.
