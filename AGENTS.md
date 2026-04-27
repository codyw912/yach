## Project OS

Before choosing nontrivial implementation work, read:

1. `docs/project-os/README.md`
2. `docs/project-os/next-work.md`
3. Relevant roadmap, invariant, decision, compatibility, or performance docs linked from the project OS

After material work, update the relevant project OS surface when priority, status, decisions, compatibility evidence, performance evidence, or architecture invariants changed. Use `docs/project-os/agent-handoff.md` for the update gate.

<!-- dev-init:rust:start -->
## Rust Environment

- Use `just` recipes for routine project commands so humans and agents go through the same environment entry point.
- For ad hoc commands that still need the Rust dev shell, use `just dev <cmd...>`.
- If you need shell syntax like pipes, redirects, or `&&`, run it through `just dev-shell '<cmd>'`.
- Avoid running bare `cargo ...` unless you are already inside the project's devenv shell via `direnv`, `direnv exec`, or `nix develop`.
<!-- dev-init:rust:end -->
