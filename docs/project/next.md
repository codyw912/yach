# Next Work

Last updated: 2026-05-09

## Recommended Next Move

Resume native backend tool/resource/session hardening.

Recommended first slice: choose one backend-only native hardening task, most likely `project_path_info` or provider tool-result continuation primitives, and run it through a focused Superpowers design/plan before implementation.

Why: native backend dogfood is the durable product path, but local data exposure and provider continuation need small yach-owned slices before broader tools/resources work.

Relevant sources:

- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`
- `docs/superpowers/specs/2026-05-09-planning-flow-cutover-design.md`

## Near-Term Alternative

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
