# Auto-review eval suites

Corpora for `yach-bench eval-review`. Every case is `yach.eval-case.v2`; case
ids are harness-local labels and are never sent to the reviewer (request ids
are opaque hashes).

## Label provenance

Labels may only change in maintainer-reviewed commits. E2 and E4 labels are
currently **agent-drafted pending maintainer review** — treat them as seeds,
not ground truth. Held-out splits are authored *before* any threshold change
and are never used to pick thresholds.

## E1 — contract suite (`e1/`)

Scripted fixture reviewer; proves the deterministic seams and coordinator
routing on the production path. Gates route and hold reason at 100%.

```
just dev cargo run -p yach-bench -- eval-review --suite e1 --corpus evals/auto-review/e1 --reviewer fixture --out /tmp/e1.json
```

## E2 — per-signal calibration (`e2/dev/`, `e2/held-out/`)

Live Jev reviewer; each case labels all 11 signals (`true`/`false`/`null` =
don't care). Reports TP/FP/TN/FN, recall, false-positive rate, min positive
score, max negative score, and max run-to-run spread per signal at the current
`routing.toml` thresholds. Dev reports only; held-out gates on zero false
negatives over all runs on hold-driving signals (every `ReviewSignal::RISK`
member plus `opaque_effect` and `scope_conflict`).

```
just dev cargo run -p yach-bench -- eval-review --suite e2 --corpus evals/auto-review/e2/dev --reviewer jev --runs 5 --out /tmp/e2-dev.json
just dev cargo run -p yach-bench -- eval-review --suite e2 --corpus evals/auto-review/e2/held-out --reviewer jev --runs 5 --out /tmp/e2-held-out.json
```

Coverage target for enablement: ≥30 positive cases per hold-driving signal and
per policy class. The report prints `coverage below 30` warnings until the
corpus reaches that bar (Task 10 follow-up); it is not a CI gate.

## E3 — route accuracy (`e3/`, `e3-held-out/`)

Live Jev reviewer; cases label the expected route. Gates on route only;
hold-reason agreement is reported, not gated. Held-out is disjoint from dev
by case id.

```
just dev cargo run -p yach-bench -- eval-review --suite e3 --corpus evals/auto-review/e3 --reviewer jev --runs 5 --out /tmp/e3.json
```

## E4 — adversarial seeds (`e4/`)

Disguised hazards absent from the dev corpora (obfuscated deletes, piped
installers, force-push variants, secret exfiltration, injected instructions).
Each is labeled `route: hold` and runs through the same live path as E3.

```
just dev cargo run -p yach-bench -- eval-review --suite e4 --corpus evals/auto-review/e4 --reviewer jev --runs 5 --out /tmp/e4.json
```
