# Planning history

Until 2026-10-29, agents planned and executed yach work with the sjujperpowers
skill pack, which kept its state in a gitignored `.sjujperpowers/` directory.
That pack is retired: agents now use cstack (`verify`, `ship`, `land`) and
cairn (`roadmap`, `outcome`) against the Linear project. This directory keeps
the parts of that local state that still explain past decisions. Like the
records, it is evidence, not instructions.

| Path | What it is |
| --- | --- |
| `sdd/<plan>.md` | The execution ledger for each plan run by subagent-driven development: Kata task refs, rulings, review rounds, verification results, and how the work was published. Named after the plan in `../plans/`. |
| `../records/2026-10-02-approval-cohort-{established,opencode-v2,omp2}.md` | The per-harness research notes behind `../records/2026-10-02-approval-cohort-refresh.md`, which cites them at their old `.sjujperpowers/research/` path. |
| `../../../.github/risk.toml` | The path risk policy, formerly `.sjujperpowers/risk.toml`. CI's `Changes` job and the review rules in `AGENTS.md` read it. |

Ledgers cite their old `.sjujperpowers/sdd/<plan>/` paths, `/tmp` evidence,
and `artifact://` handles as they were written; those locations are not kept.
Evidence that matters is in the records. Agent-host names, checkpoint refs,
and host operator commands were replaced with "Checkpoint recorded" or a plain
description before the ledgers were published; nothing else was edited.

Not kept: the per-task briefs, reports, and review diffs beside each ledger
(the diffs are reproducible from commits still in the repository, and the
ledgers summarize the reports); the verdict ledger `verdicts.jsonl`, whose
rows grade changes with a retired tool and point at temporary evidence files
that no longer exist; and `config.json`, the retired pack's tracker
configuration (Linear team `CEW`, project `Yach`, Kata project `yach`), which
cairn now takes from the maintainer's local planning setup.

## Plane to Linear

Plane was retired on 2026-10-13 and its issues migrated to Linear (team
`CEW`, project `Yach`). Merged specs and plans that carry `plane:YACH-<n>`
headers keep them; this table maps them. Each migrated Linear issue carries
the `plane:YACH` label. Issues created after the migration have no Plane
counterpart.

| Outcome | Linear | Plane |
| --- | --- | --- |
| M1 Evidence-driven dogfood loop | CEW-147 | YACH-1 |
| M2 Long-session correctness | CEW-148 | YACH-2 |
| M3 Effective end-to-end coding | CEW-149 | YACH-3 |
| M4 Calm, governed daily operation | CEW-150 | YACH-4 |
| M5 Usage-ready extension platform | CEW-151 | YACH-5 |
| M6 Independent usage readiness | CEW-152 | YACH-6 |
| Retire in-repo planning docs | CEW-153 | YACH-7 |
| Performance measurement framework | CEW-154 | YACH-8 |
| Contributor environment on native devenv | CEW-155 | YACH-9 |
| Prompt cache visibility | CEW-156 | YACH-16 |
| Extension capability contract | CEW-157 | YACH-10 |
| Intent-aware automatic approvals (Jev reviewer) | CEW-158 | YACH-11 |
| Distribution presets | CEW-159 | YACH-12 |
| Binary size gate | CEW-160 | YACH-13 |
| Skills (Agent Skills format) | CEW-161 | YACH-14 |
| Diagnosable dogfood sessions | CEW-162 | YACH-15 |
| Codex model listing follows Codex releases | CEW-163 | YACH-17 |
| Cohesive approval design | CEW-164 | YACH-18 |
| Daily-use TUI fixes (ylog session) | CEW-165 | YACH-19 |
| TUI theming engine | CEW-166 | YACH-20 |
| User-facing observer extension seam | CEW-167 | YACH-21 |
