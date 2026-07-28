Historical `.project/` cockpit artifacts, if present under `docs/archive/project-cockpit/`, are reference-only and not active workflow instructions.

## Project Planning

- Active project planning starts at `docs/project/README.md`.
- For nontrivial work, read `docs/project/state.md` and `docs/project/next.md` before choosing the next task.
- Fresh-session resume guidance lives in `docs/project/README.md`.
- `docs/project-os/` and `docs/archive/project-cockpit/` are reference-only, not active workflow instructions.

## Version Control workflow

- After opening a PR, make sure you include the full PR URL in your reply.

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
<!-- dev-init:rust:start -->
## Rust Environment

- Use `just` recipes for routine project commands so humans and agents go through the same environment entry point.
- For ad hoc commands that still need the Rust dev shell, use `just dev <cmd...>`.
- If you need shell syntax like pipes, redirects, or `&&`, run it through `just dev-shell '<cmd>'`.
- Avoid running bare `cargo ...` unless you are already inside the project's devenv shell via `direnv`, `direnv exec`, or `nix develop`.

## Cross Building

- Keep cross toolchains out of the default shell unless this project needs them regularly.
- For Nix package builds, use `nix build .#packages.aarch64-linux.default` or `nix build .#packages.x86_64-linux.default` with local or remote builders configured.
- For Zig-based Rust builds, add `zig`, `cargo-zigbuild`, and the required Rust targets in `devenv.local.nix` or a project-specific cross profile.
- Do not add cross targets, Zig, or remote builder recipes to the default template unless every new Rust project should pay that cost on shell entry.
<!-- dev-init:rust:end -->
