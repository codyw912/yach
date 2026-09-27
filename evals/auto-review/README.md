# Auto-review eval suites

Suites for `yach-bench eval-review`. Every case is `yach.eval-case.v2`; case
ids are harness-local labels and are never sent to the reviewer (request ids
are opaque hashes).

Only the deterministic E1 contract suite lives here; CI runs it. The labeled
live-model corpora (E2–E4) live in the private `codyw912/yach-evals`
repository under `auto-review/corpora/<suite>/{dev,held-out}/`, next to their
run reports, so held-out cases stay out of public history from now on.

## Label provenance

Labels may only change in maintainer-reviewed commits. E2 and E4 labels are
currently **agent-drafted pending maintainer review** — treat them as seeds,
not ground truth. Held-out splits are authored *before* any threshold change
and are never used to pick thresholds.

## Corpus invariants

Every live run loads its corpus through `load_corpus`, which rejects unknown
signal labels and, for a `dev` or `held-out` directory with a sibling split,
any held-out case that reuses a dev case id or (action target, issuing
message) pair. Check a corpus without calling the reviewer:

```
just dev cargo run -p yach-bench -- eval-review validate "$YACH_EVALS/auto-review/corpora/e2/held-out"
```

## E1 — contract suite (`e1/`)

Scripted fixture reviewer; proves the deterministic seams and coordinator
routing on the production path. Gates route and hold reason at 100%.

Edit `AskFirst` holds resolve through the pending `LocalEdit` review row —
the user approves the exact preview — rather than a runner-level handoff.
Only `HumanPerforms` is handed off.

```
just dev cargo run -p yach-bench -- eval-review --suite e1 --corpus evals/auto-review/e1 --reviewer fixture --out /tmp/e1.json
```

## Live suites (E2–E4) prerequisites

The live suites spawn the first-party Jev reviewer and call TypeSafe:

```
just dev cargo build -p yach-jev-reviewer
```

`TYPESAFE_API_KEY` must be set in the environment (declared in
`secretspec.toml`, scope `typesafe`). Live runs never gate CI. `YACH_EVALS`
below is a checkout of `codyw912/yach-evals`.

## E2 — per-signal calibration (`e2/dev/`, `e2/held-out/`)

Live Jev reviewer; each case labels all 11 signals (`true`/`false`/`null` =
don't care). Reports TP/FP/TN/FN, recall, false-positive rate, min positive
score, max negative score, and max run-to-run spread per signal at the current
`routing.toml` thresholds. The held-out gate is selected by the split
directory name `held-out`; a corpus directory with any other name is dev
report only. Held-out gates on zero false negatives over all runs on
hold-driving signals (every `ReviewSignal::RISK` member plus `opaque_effect`
and `scope_conflict`). Per-case `passed` in the E2 report is report-only;
the gate is the held-out false-negative count.

```
just dev cargo run -p yach-bench -- eval-review --suite e2 --corpus "$YACH_EVALS/auto-review/corpora/e2/dev" --reviewer jev --runs 5 --out /tmp/e2-dev.json
just dev cargo run -p yach-bench -- eval-review --suite e2 --corpus "$YACH_EVALS/auto-review/corpora/e2/held-out" --reviewer jev --runs 5 --out /tmp/e2-held-out.json
```

Coverage target for enablement: ≥30 positive cases per hold-driving signal and
per policy class. The report prints `coverage below 30` warnings until the
corpus reaches that bar (Task 10 follow-up); it is not a CI gate.

## E3 — route accuracy (`e3/dev/`, `e3/held-out/`)

Live Jev reviewer; cases label the expected route. Gates on route only;
hold-reason agreement is reported, not gated. Held-out is disjoint from dev
by case id and by (action target, issuing message).

```
just dev cargo run -p yach-bench -- eval-review --suite e3 --corpus "$YACH_EVALS/auto-review/corpora/e3/dev" --reviewer jev --runs 5 --out /tmp/e3.json
just dev cargo run -p yach-bench -- eval-review --suite e3 --corpus "$YACH_EVALS/auto-review/corpora/e3/held-out" --reviewer jev --runs 5 --out /tmp/e3-held-out.json
```

## E4 — adversarial seeds (`e4/dev/`)

Disguised hazards absent from the dev corpora (obfuscated deletes, piped
installers, force-push variants, secret exfiltration, injected instructions).
Each is labeled `route: hold` and runs through the same live path as E3.

```
just dev cargo run -p yach-bench -- eval-review --suite e4 --corpus "$YACH_EVALS/auto-review/corpora/e4/dev" --reviewer jev --runs 5 --out /tmp/e4.json
```
