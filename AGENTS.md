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

### Codex review
Codex reviews a PR automatically when it is opened and posts as
`chatgpt-codex-connector[bot]`. It keeps one summary comment per PR whose
hidden `codex-review-state:v1` line records the commit it reviewed and whether
that review has finished. Do not write `@codex` in PR descriptions or titles:
any mention other than an exact `@codex review` comment starts a Codex cloud
task, which fails in this repo. Never edit, delete, or resolve Codex's comments.

These helpers print Codex's review state for the PR's current head (nothing if
it has not reviewed that commit) and its findings on that commit:
```sh
codex_state() {
  gh api --paginate repos/codyw912/yach/issues/<n>/comments \
    --jq '.[] | select(.user.login == "chatgpt-codex-connector[bot]") | .body
          | capture("codex-review-state:v1 (?<s>\\{.*\\}) -->").s | fromjson | .code_review' \
    | jq -c --arg h "$(gh pr view <n> --json headRefOid -q .headRefOid)" 'select(.commit_sha == $h)'
}
codex_findings() {
  gh api --paginate repos/codyw912/yach/pulls/<n>/comments \
    | jq -s --arg h "$(gh pr view <n> --json headRefOid -q .headRefOid)" \
      '[.[][] | select(.user.login == "chatgpt-codex-connector[bot]" and .original_commit_id == $h)
        | {url: .html_url, title: (.body | split("\n")[0])}]'
}
```

1. After opening a PR, do not request a review. Wait up to 5 minutes for the
   automatic one to start:
   `for i in $(seq 5); do codex_state | grep -q . && break; sleep 60; done`
   If `codex_state` still prints nothing, request a review (step 2) and say
   in your reply that the automatic review did not start.
2. After any later push, request a review with the REST API
   (`gh pr comment` is blocked by the proxy):
   `gh api repos/codyw912/yach/issues/<n>/comments -f body='@codex review'`
3. Wait for the review of the current head to finish. Run the wait as a
   background job if your harness wakes you when one finishes; otherwise run
   it in the foreground:
   `for i in $(seq 30); do codex_state | jq -e 'select(.status == "completed")' && break; sleep 60; done`
   If it prints nothing, it gave up after 30 minutes; say so in your reply
   instead of reporting the PR ready.
4. If the state shows `"has_any_findings": false`, the review is clean.
   Otherwise run `codex_findings`. Each title carries a P0, P1, or P2 badge.
   Fix each P0/P1 finding or explain why it is wrong; P2 and lower are
   optional. Replies inside review threads are not available yet: answer in
   one conversation comment that lists each finding and what you did. If you
   pushed fixes, go back to step 2.
5. Report the PR ready only when the latest Codex review of the current head
   has no P0/P1 findings. Give the review's URL in your reply.

## Stacked PRs

PRs are squash-merged. Open dependent PRs as a GitHub stack so they land
without manual retargeting:

- Give each PR its own bookmark, base each PR on the branch of the PR below
  it, push, then run `gh stack link <bottom> ... <top>` with PR numbers or
  branch names, bottom first. To grow a stack, run it again with the full list.
  `gh stack` is a separate extension (`github/gh-stack`), not part of `gh`.
- The maintainer merges. Merging a PR in a stack also merges every PR below
  it, as one squash commit each.
- GitHub deletes merged branches (`delete_branch_on_merge`). After a merge,
  `jj git fetch` drops their bookmarks and abandons the commits nothing else
  points to; a local bookmark still on them keeps them. Rebase any remaining
  local work onto `main@origin`, and confirm each merged PR's squash commit is
  an ancestor of `main@origin`.
- If only the lower part of a stack merged, GitHub rebases the remaining PR
  branches itself. Fetch before touching them and build on the fetched
  bookmarks; do not push local copies over GitHub's rebase. This path has not
  been exercised in this repo yet.
- `jj-stack` is an alternative for stacks of single changes that are revised
  often: one change per PR, on generated `jj-stack/` branch names. Use
  `submit` and `sync` only; `jj-stack merge` merges, which agents do not do.

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
