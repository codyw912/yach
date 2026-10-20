## Where to start

- Direction: `docs/project/roadmap.md` (read-only; it mirrors decisions made elsewhere).
- Design: `docs/project/specs/`. Nontrivial features get a spec before implementation.
- Execution: `docs/project/plans/`. Each spec gets a numbered implementation plan.
- Evidence: `docs/project/records/` — dated research, measurements, decisions.
- Product docs index: `docs/README.md`.

Roadmap outcomes, milestone status, and work queues live in an external
planning system that is configured locally on the maintainer's machines, not
in this repository. If that configuration is present in your session, plan
through it. If it is absent, lifecycle mutation is unavailable: do not edit
`roadmap.md`, do not write new specs or plans, and do not create in-repo
substitutes (status snapshots, next-work queues, boards). Ask the maintainer
to run provider setup instead. Ordinary read/build/test/contribution work
is unaffected.

## Version Control workflow

- After opening a PR, make sure you include the full PR URL in your reply.
- A PR that changes what the TUI shows must include before/after screenshots:
  run `just tui-themes before` on the base and `just tui-themes after` on the
  change, then embed the affected grids in the PR description (see "TUI design
  review" in `README.md`). Host the images on the `pr-screenshots` branch,
  linked by commit ID, not in the change itself.

This repo uses Jujutsu (`jj`) for local development.

- Prefer `jj status`, `jj diff`, `jj log`, and `jj op log` over Git status/log commands.
- Do not run `git add` or `git commit`.
- Create checkpoints with `jj describe -m "<message>"` followed by `jj new`.
- Use clear checkpoint descriptions that describe completed intent, not vague progress.
- Before publishing or handing off, inspect the stack with `jj log -r 'main..@'`.
- Use `jj squash`, `jj split`, `jj rebase`, and `jj describe` to shape work into reviewable commits.
- Use bookmarks for publishable branches: `jj bookmark create <name> -r <rev>`.
- Push with `jj git push --bookmark <name> --remote origin`.
- Use `jj op log` and `jj undo` for recovery.
- Avoid mutating Git commands unless explicitly instructed.

## Stacked PRs

PRs are squash-merged. Open dependent PRs as a GitHub stack so they land
without manual retargeting:

- Give each PR its own bookmark, base each PR on the branch of the PR below
  it, push, then run `gh stack link <bottom> ... <top>` with PR numbers or
  branch names, bottom first. To grow a stack, run it again with the full list.
  `gh stack` is a separate extension (`github/gh-stack`), not part of `gh`.
- GitHub deletes merged branches (`delete_branch_on_merge`). After a merge,
  `jj git fetch` drops their bookmarks and abandons the commits nothing else
  points to; a local bookmark still on them keeps them. Rebase any remaining
  local work onto `main@origin`.
- If only the lower part of a stack merged, GitHub rebases the remaining PR
  branches itself. Fetch before touching them and build on the fetched
  bookmarks; do not push local copies over GitHub's rebase. This path has not
  been exercised in this repo yet.
- `jj-stack` is an alternative for stacks of single changes that are revised
  often: one change per PR, on generated `jj-stack/` branch names, published
  with `submit` and `sync`.

## Code Review Rules

Mechanical checks (rustfmt, clippy, `just lint`, tests) run elsewhere; do not
report formatting, naming, or lint-level style.

### Approach and design
- If the overall approach is wrong for the problem, or a materially simpler
  design fits the existing seams, report it as P1 and describe the
  alternative. Judge the change against the decisions and rejected
  alternatives stated in the PR description or its linked spec; if a
  non-obvious choice has no stated reason, say so rather than guessing.
- Specs in `docs/project/specs/` and plans in `docs/project/plans/` are
  reviewed for substance: wrong assumptions about the current code, missing
  failure modes, acceptance criteria that cannot be tested, and scope that
  does not match the stated outcome. A plan that builds something its spec
  does not ask for, or skips something it requires, is P1.

### Safety boundaries
Every local mutation goes through a hash-checked edit transaction with user
review, sensitive files are denied to tools by default, and secret-shaped
environment variables are stripped from subprocesses.
- Report as P0 any path that writes a file outside the edit transaction and
  its review, runs a command that was neither reviewed nor matched by the
  trusted-command allowlist, lets a tool read a sensitive path, or exposes a
  secret to a subprocess, log, or transcript. Safe path: go through the
  existing seams (`edit_access`, `review`, `permission`, `sensitive_paths`,
  `shell`).
- Changes to the high-tier paths in `.sjujperpowers/risk.toml` (permissions,
  sandboxing, credentials, extensions, vendored or containerised code) need a
  test that the unsafe case is refused, not only that the allowed case works.
  A missing refusal test is P1.

### Checks that guard other checks
- Loosening `clippy.toml`, `crates/yach-bench/perf-thresholds.toml`, CI
  workflows, `justfile` recipes, or toolchain pins, or deleting or ignoring
  tests, can weaken a check without touching the code it checks. Report it as
  P1 unless the PR description explains why.

### Evidence
- A change to what the TUI shows needs the before/after grids described in
  "Version Control workflow". Report their absence as P1.
- Performance claims must cite an in-repo benchmark result; an unsupported
  claim in a PR description or doc is P1.

## Screenshots in chat

- When the session runs under Paseo (`PASEO_AGENT_ID` is set), show a saved
  screenshot in the reply as `![label](/absolute/path.png)`. Paseo renders
  it inline, which is the quick way to compare TUI changes while iterating.
- Other clients may not render local images; there, give the absolute path.
- Chat screenshots do not replace the before/after grids a TUI-visible PR
  needs.
<!-- dev-init:rust:start -->
## Rust Environment

- Use `just` recipes for routine project commands so humans and agents go through the same environment entry point.
- For ad hoc commands that still need the Rust dev shell, use `just dev <cmd...>`.
- If you need shell syntax like pipes, redirects, or `&&`, run it through `just dev-shell '<cmd>'`.
- Avoid running bare `cargo ...` unless you are already inside the project's devenv shell via `direnv`, `direnv exec`, or `devenv shell`.

- The Rust version lives in `rust-toolchain.toml`. The devenv shell reads it
  via `languages.rust.toolchainFile`. CI does not read it: the
  `dtolnay/rust-toolchain` action selects from its own revision, so the
  workflows pin `@<version>` explicitly. `just toolchain-check`, which runs
  as part of `just lint`, fails when those disagree.
- Bumping requires three coordinated edits, listed at the top of
  `rust-toolchain.toml`: the channel, every workflow pin, and the
  `devenv.yaml` input revisions (which are exact commits — `devenv update`
  re-locks them but cannot advance them).
- This is alignment, not a guarantee. If you suspect a mismatch — a bump in
  flight, an unexpected lint, a CI failure that does not reproduce — run
  `just lint-ci`, which installs the version the workflows pin and lints
  with it outside the devenv shell. Do not reach for the floating `stable`
  toolchain for this: it tests a different compiler than CI.

## Cross Building

- Keep cross toolchains out of the default shell unless this project needs them regularly.
- For Nix package builds, use `nix build .#packages.aarch64-linux.default` or `nix build .#packages.x86_64-linux.default` with local or remote builders configured.
- For Zig-based Rust builds, add `zig`, `cargo-zigbuild`, and the required Rust targets in `devenv.local.nix` or a project-specific cross profile.
- Do not add cross targets, Zig, or remote builder recipes to the default template unless every new Rust project should pay that cost on shell entry.
<!-- dev-init:rust:end -->
