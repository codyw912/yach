# Next Work

Last updated: 2026-05-09

## Recommended Next Move

Complete the planning-flow cutover.

Why: the project is intentionally retiring both cockpit and Project OS as active workflows. Before more native-backend work accumulates, humans and agents need a short active planning surface that shows current state and next work without maintaining duplicate systems.

Done when:

- `docs/project/README.md`, `state.md`, `next.md`, and `records/` exist.
- `AGENTS.md` points to `docs/project/README.md`.
- `docs/project-os/` and `docs/archive/project-cockpit/` are marked reference-only.
- A dated cutover record exists.
- The old Project OS fast path is no longer described as active.

## Ready After Cutover

### Native backend tool/resource/session hardening

Recommended first slice: backend-only `project_path_info` or provider tool-result continuation primitives, depending on owner preference at implementation time.

Why: native backend dogfood is the durable product path, but local data exposure and provider continuation need small yach-owned slices before broader tools/resources work.

Relevant sources:

- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`
- `docs/superpowers/specs/2026-05-09-planning-flow-cutover-design.md`

### Performance evidence follow-up

Keep performance work scoped to claims that affect product direction or native-backend decisions.

Why: yach's thesis depends on measured responsiveness, but the next product move is native-backend hardening unless a performance question blocks it.

Relevant sources:

- `docs/benchmarks/current-baseline-2026-05-05.md`
- `docs/project-os/performance-evidence.md`

## Not Ready Without a New Spec

- Defaulting to the native backend.
- Sending local file contents to a provider.
- File mutation tools.
- Process or shell execution tools.
- Network tools.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
