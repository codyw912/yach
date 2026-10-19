"""Judge `Perf (latency)` from its `perf ab` documents, not exit codes.

Usage: latency_verdicts.py ROW=EXIT [ROW=EXIT ...]

Each argument names one gated row id and the exit status of its
`yach-bench perf ab --filter ROW --out ab-<slug>.json` run, where slug is the
row id with `/` replaced by `_`. `perf ab` exits 1 for both `regressed` and
`error`, so the exit status alone cannot tell a noisy timing from a broken
harness; this reads the verdicts instead.

Latency verdicts are advisory. `regressed` and `inconclusive` become
`::warning::` annotations and summary lines; they never fail the job.

The job fails (exit 1) on a harness failure:
- a `perf ab` exit status other than 0, 1 or 2, or an exit 1 that no row in
  the document explains (no row of any class regressed or errored);
- a missing, empty, unreadable or unparsable document, or one with no
  `verdicts` list;
- no latency verdict for the requested row;
- a latency verdict of `error`, or one this script does not know.

Count rows (`class` = `count`: `#alloc_count`, `#alloc_bytes`) are ignored
entirely: `Perf (deterministic)` owns them.

The step summary (`$GITHUB_STEP_SUMMARY`, or stdout) gets one table row per
latency verdict.
"""

import json
import os
import sys

OK = {"unchanged", "improved"}
ADVISORY = {"regressed", "inconclusive"}


def slug(row: str) -> str:
    return row.replace("/", "_")


def fmt(value: object) -> str:
    if not isinstance(value, (int, float)):
        return "-"
    ns = float(value)
    if ns >= 1e6:
        return f"{ns / 1e6:.2f} ms"
    if ns >= 1e3:
        return f"{ns / 1e3:.2f} µs"
    return f"{ns:.0f} ns"


def judge(row: str, exit_status: str, directory: str = ".") -> tuple[list[str], list[str], list[str]]:
    """Return (summary rows, warnings, failures) for one gated row."""
    summary: list[str] = []
    warnings: list[str] = []
    failures: list[str] = []
    path = os.path.join(directory, f"ab-{slug(row)}.json")
    if exit_status not in {"0", "1", "2"}:
        failures.append(f"{row}: perf ab exited {exit_status} (harness failure)")
    try:
        with open(path, encoding="utf-8") as f:
            doc = json.load(f)
        verdicts = doc["verdicts"]
        if not isinstance(verdicts, list):
            raise TypeError("verdicts is not a list")
    except (OSError, ValueError, KeyError, TypeError) as error:
        failures.append(f"{row}: no readable comparison document {path} ({error})")
        return summary, warnings, failures

    latency = [v for v in verdicts if isinstance(v, dict) and v.get("class") == "latency"]
    mine = [v for v in latency if v.get("id") == row]
    if not mine:
        failures.append(f"{row}: document has no latency verdict for this row")
    for v in latency:
        verdict = v.get("verdict")
        detail = v.get("detail") or {}
        delta = detail.get("median_delta_pct")
        budget = (v.get("budget") or {}).get("latency_pct")
        delta_cell = f"{delta:+.2f}%" if isinstance(delta, (int, float)) else "-"
        budget_cell = f"{budget:g}%" if isinstance(budget, (int, float)) else "-"
        summary.append(
            f"| `{v.get('id')}` | {verdict} | {fmt(v.get('base_summary'))} | "
            f"{fmt(v.get('current_summary'))} | {delta_cell} | {budget_cell} |"
        )
        if verdict in ADVISORY:
            warnings.append(
                f"{v.get('id')}: latency {verdict} ({delta_cell} against {budget_cell}); "
                "advisory on shared runners, not a merge gate"
            )
        elif verdict == "error":
            failures.append(f"{v.get('id')}: latency verdict error (harness failure)")
        elif verdict not in OK:
            failures.append(f"{v.get('id')}: unexpected latency verdict {verdict!r}")

    # `perf ab` exits 1 when any row of any class regressed or errored.
    # Count rows are ignored here, so a count-row verdict that explains the
    # exit must not fail the job; an exit 1 that no row explains is a
    # harness failure. An errored latency row already failed above.
    explained = any(
        isinstance(v, dict) and v.get("verdict") in {"regressed", "error"} for v in verdicts
    )
    if exit_status == "1" and not explained:
        failures.append(f"{row}: perf ab exited 1 but no row regressed or errored (harness failure)")
    return summary, warnings, failures


def main(argv: list[str]) -> int:
    if not argv:
        print("::error::latency_verdicts.py: no rows given")
        return 1
    summary = [
        "### Perf (latency), advisory",
        "",
        "| row | verdict | base p95 | current p95 | median delta | budget |",
        "|---|---|---:|---:|---:|---:|",
    ]
    warnings: list[str] = []
    failures: list[str] = []
    for arg in argv:
        row, sep, exit_status = arg.rpartition("=")
        if not sep or not row:
            failures.append(f"bad argument {arg!r}, expected ROW=EXIT")
            continue
        s, w, f = judge(row, exit_status)
        summary += s
        warnings += w
        failures += f
    summary.append("")
    summary += [f"- warning: {w}" for w in warnings]
    summary += [f"- **failure:** {f}" for f in failures]
    if not warnings and not failures:
        summary.append("No latency row regressed or was inconclusive.")
    out = os.environ.get("GITHUB_STEP_SUMMARY")
    text = "\n".join(summary) + "\n"
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write(text)
    else:
        sys.stdout.write(text)
    for w in warnings:
        print(f"::warning title=Perf (latency)::{w}")
    for f in failures:
        print(f"::error title=Perf (latency)::{f}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
