## Project Cockpit

This project uses `.project/` as the canonical agent operating surface.

Before meaningful work, read:

1. `.project/brief.md`
2. `.project/now.md`
3. Only the handoff/spec/doc explicitly linked from `.project/now.md` for the selected chunk, if needed.

Do not read legacy `docs/project-os/` files by default. They are historical/reference material unless `.project/now.md` links to a specific file.

After meaningful work, update `.project/now.md` when current state, validation status, blockers, commit status, or next chunks changed. Append to `.project/decisions.md` only for durable decisions. Write `.project/handoffs/*.md` only when context would be expensive to reconstruct.

Validated implementation chunks should normally be committed before stopping or starting another chunk, unless there is a clear no-commit reason.

<!-- dev-init:rust:start -->
## Rust Environment

- Use `just` recipes for routine project commands so humans and agents go through the same environment entry point.
- For ad hoc commands that still need the Rust dev shell, use `just dev <cmd...>`.
- If you need shell syntax like pipes, redirects, or `&&`, run it through `just dev-shell '<cmd>'`.
- Avoid running bare `cargo ...` unless you are already inside the project's devenv shell via `direnv`, `direnv exec`, or `nix develop`.
<!-- dev-init:rust:end -->
