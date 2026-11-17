# Planning history

Until 2026-10-29, agents planned and executed yach work with the sjujperpowers
skill pack, which kept its state in a gitignored `.sjujperpowers/` directory.
That pack is retired: agents now use cstack (`verify`, `ship`, `land`) and
cairn (`roadmap`, `outcome`), whose planning lives outside the repository.
This directory keeps the parts of that local state that still explain past
decisions. Like the records, it is evidence, not instructions.

| Path | What it is |
| --- | --- |
| `sdd/<plan>.md` | The execution ledger for each plan run by subagent-driven development: Kata task refs, rulings, review rounds, verification results, and how the work was published. Named after the plan in `../plans/`. |
| `../records/2026-10-02-approval-cohort-{established,opencode-v2,omp2}.md` | The per-harness research notes behind `../records/2026-10-02-approval-cohort-refresh.md`, which cites them at their old `.sjujperpowers/research/` path. |
| `../../../.github/risk.toml` | The path risk policy, formerly `.sjujperpowers/risk.toml`. CI's `Changes` job and the review rules in `AGENTS.md` read it. |

Ledgers cite their old `.sjujperpowers/sdd/<plan>/` paths, `/tmp` evidence,
and `artifact://` handles as they were written; those locations are not kept.
Evidence that matters is in the records. Agent-host names, checkpoint refs,
and host operator commands were replaced with "Checkpoint recorded" or a plain
description before the ledgers were published. Later, references to the
maintainer's private issue tracker were replaced with the outcome's name where
they carried context; older ledgers still cite some private issue ids, which
resolve only for the maintainer. Nothing else was edited.

Not kept: the per-task briefs, reports, and review diffs beside each ledger
(the diffs are reproducible from commits still in the repository, and the
ledgers summarize the reports); the verdict ledger `verdicts.jsonl`, whose
rows grade changes with a retired tool and point at temporary evidence files
that no longer exist; and `config.json`, the retired pack's tracker
configuration, which cairn now takes from the maintainer's local planning
setup.
