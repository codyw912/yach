# Auto-Review Evaluation Record

Date: 2026-09-22
Reviewer: fixture (scripted assessments)
Corpus: `evals/auto-review/corpus/` (40 cases), `evals/auto-review/held-out/` (12 cases, disjoint)

## Method

`yach-bench eval-review --corpus evals/auto-review/corpus --reviewer fixture --out report.json`

The fixture reviewer returns scripted `ReviewAssessment` values derived from each
case's `expected_route` label. This validates the code-owned routing arithmetic
(`route_assessment`, `bind_review_request`, `ReviewAssessment::validate_against`)
but does not exercise a real model — the fixture is tautological by construction.

## Results

- 40/40 corpus cases routed correctly
- 0 automatic executions on labeled hold/fail cases
- 100% routine execution rate (10/10 routine cases)
- Held-out set: 12 cases, disjoint IDs, same schema

## Gate Status

`AUTO_REVIEW_EXECUTION_ENABLED` remains `false`. The plan requires a live
`--reviewer jev` run with `TYPESAFE_API_KEY` via SecretSpec before flipping the
gate. The fixture run proves the routing pipeline works; it does not prove the
reviewer model makes correct assessments. Per the spec, "gate fails →
auto-execution stays disabled" is a valid outcome.

## Limitations

- Fixture reviewer is deterministic; no model variance, latency, or cost data.
- No live Jev run performed — requires `TYPESAFE_API_KEY` via SecretSpec.
- Perf workload `review/route/*` registered but no baseline measurement yet
  (release build timed out on first attempt; thresholds marked inconclusive).
