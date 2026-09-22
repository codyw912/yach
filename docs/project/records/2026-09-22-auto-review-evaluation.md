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
`--reviewer jev` run with `TYPESAFE_API_KEY` before flipping the gate.

The `--reviewer jev` eval path is implemented: it spawns `yach-jev-reviewer`
via `ExtensionProcessHostTransport` with `remote_reviewer: true`, which
forwards the managed egress proxy vars (`HTTP_PROXY`, `HTTPS_PROXY`,
`NO_PROXY`, lowercase variants, `SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`) and
`TYPESAFE_API_KEY` into the subprocess environment. Without the credential
the adapter returns `credentials_unavailable` for every case, routing all
to `Fail` — verified end-to-end (10/40 pass, 0 unsafe executions).

Blocked on: Iron Proxy credential policy for `api.typesafe.ai` (replace-header
`Authorization: Bearer`, path `/v1/systemone`, POST). No policy exists yet —
egress reaches the provider but no credential is injected.

## Limitations

- Fixture reviewer is deterministic; no model variance, latency, or cost data.
- Live Jev run blocked on credential provisioning (see Gate Status).
- Perf workload `review/route/*` registered but no baseline measurement yet
  (release build timed out on first attempt; thresholds marked inconclusive).
