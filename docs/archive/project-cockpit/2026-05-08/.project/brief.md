# Project Brief

## North star

Yach is a Rust shell and eventual native backend for a fast, hackable coding-agent TUI. The UI/backend boundary should stay yach-owned through `yach-proto`, while Pi RPC remains a temporary compatibility/reference adapter rather than the durable product backend.

## Current product direction

- Preserve Pi's file-first, inspectable, low-friction customization spirit.
- Keep `yach-ui` independent from Pi RPC, provider SDKs, and native backend internals.
- Build native backend seams incrementally: runner seam, native session/event log, provider seam, provider-library spike, then minimal native dogfood runner.
- Prefer existing Rust provider libraries below a replaceable yach-owned provider seam when they preserve event fidelity and control.
- Require evidence for compatibility and performance claims; do not assume Rust/native is faster without measurement.

## Canonical planning sources

- `docs/project-os/README.md`
- `docs/project-os/next-work.md`
- `docs/project-os/roadmap.md`
- `docs/project-os/architecture-invariants.md`
- `docs/project-os/decisions.md`
- Active plan: `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`

## Invariants to protect

- `yach-proto` is the UI/backend seam.
- `yach-ui` does not speak Pi RPC or provider APIs directly.
- Pi RPC adapter is compatibility/reference, not a feature-complete target.
- Native sessions, tools, resources, and protocol events are yach-owned.
- Project OS docs remain the canonical repo-level planning/evidence surface.
