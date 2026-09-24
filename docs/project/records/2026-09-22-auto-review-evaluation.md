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

`AUTO_REVIEW_EXECUTION_ENABLED` remains `false`.

The `--reviewer jev` eval path is implemented: it spawns `yach-jev-reviewer`
via `ExtensionProcessHostTransport` with `remote_reviewer: true`, which
forwards the managed egress proxy vars (`HTTP_PROXY`, `HTTPS_PROXY`,
`NO_PROXY`, lowercase variants, `SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`) and
`TYPESAFE_API_KEY` into the subprocess environment.

### Live Jev run (2026-09-24)

Credential provisioned via Iron Proxy `replace-header` policy on
`api.typesafe.ai`. The eval ran end-to-end: 4/40 cases passed, 0 unsafe
executions.

The model returned `hold_clarify` for all 40 cases — every assessment
routed to `HoldReason::NeedsClarification`. Only the 4 cases expecting
`hold_clarify` passed. This is the model being conservative on minimal
evidence (each case has a single user message like "Run the focused review
tests"), not a transport or credential failure.

The eval gate (`passed != total`) is calibrated for the fixture reviewer's
scripted assessments, not a live model. For a live eval the meaningful
metrics are `automatic_executions_on_hold_or_fail` (0 — no unsafe
executions) and the route distribution. The gate needs adjustment before
it can validate a live reviewer.

## Limitations

- Fixture reviewer is deterministic; no model variance, latency, or cost data.
- Live Jev run shows the model is conservative on minimal evidence — the
  eval corpus may need richer evidence or the gate needs a live-model
  threshold.
- Perf workload `review/route/*` registered but no baseline measurement yet
  (release build timed out on first attempt; thresholds marked inconclusive).
