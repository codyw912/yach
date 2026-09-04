# Docs Retirement Implementation Plan

**Spec:** `docs/superpowers/specs/2026-09-03-docs-retirement-design.md`
**Source:** external (docs retirement, 2026-09-03)

One commit stacked on the roadmap commit; both publish as one PR against
`main`. No code behavior changes; a fresh-target `cargo check -p yach` is the
only build gate.

### Task 1: Delete retired planning trees

Remove `docs/project/{README,state,next,board}.md`, `docs/plans/`,
`docs/project-os/`, `docs/archive/`, `docs/status/`, `docs/spikes/`,
`docs/brainstorms/`. Verify the surviving `docs/` tree is exactly
`README.md` (added in Task 3), `benchmarks/`, `project/roadmap.md`,
`project/records/`, `protocol/`, `superpowers/`.

### Task 2: Shrink roadmap.md to public direction

Rewrite `docs/project/roadmap.md`: read-only-mirror header, vision, non-goals,
six milestones as title plus one-line outcome, principles, pointers to
specs/plans/records. Remove done-when gates and status fields.

### Task 3: Rewrite entry points

`AGENTS.md`: replace the planning section with a provider-neutral "Where to
start" block and the fail-closed rule. New `docs/README.md`: one-screen index
plus a retired-paths note. Neither may name a tracker, workspace, project, or
configuration path. Add the local planning-configuration directory to
`.gitignore` with a generic comment so the configuration never enters the
public tree.

### Task 4: Correct root README

Honest status line pointing at `roadmap.md`; `config.json` → `config.toml`
with the real `[thinking]` / `[model.default]` TOML shape verified against
`crates/yach-backend/src/user_config.rs`; `--backend native-fixture` →
`--backend fixture`; acknowledge `/connect` and `/model`; remove links to
deleted files; point Development at `docs/README.md`.

### Task 5: Fix surviving product-doc and code references

`docs/benchmarks/README.md` and `docs/protocol/yach-proto-v0.md`: replace
references to deleted files with surviving sources.
`crates/yach-cli/src/main.rs`: redirect the `board.md` comment to
`roadmap.md`.

### Task 6: Verify

Fresh-target `cargo check -p yach` passes. Grep confirms zero tracker names
in any added line of the diff (the `.gitignore` path entry is the sole
permitted match). No Markdown link in a surviving product doc targets a
deleted path, and no current-guidance reference to a deleted path remains in
product docs, `justfile`, `evals/`, `.github/`, or non-test code; the single
retired-paths note in `docs/README.md` is the deliberate exception.

### Task 7: Review and publish

Reviewer subagent over the range; fix findings; approve publication; push;
open PR; checkpoint. Close the execution issues only after the PR URL exists.
