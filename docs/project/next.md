# Next Work

Last updated: 2026-05-11

## Recommended Next Move

Recommended next move: run real-provider smoke validation for explicit native-provider `project_path_info` tool-call emission, or begin the extension-owned tool registration design that can populate the typed provider-advertising representation after yach policy approval.

Why: the native-provider runner now advertises only the metadata-safe `project_path_info` schema on initial explicit native-provider requests, maps that advertising into Rig schema-only tool definitions, and strips advertising from continuation requests. The next risk is whether real providers emit the intended tool call shape in practice, while the next architecture step is letting extension-owned tools register into the same yach-owned policy and advertising pipeline without giving adapters execution ownership.

Relevant sources:

- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/plans/2026-05-10-native-session-store-resume-metrics.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-tool-loop.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-provider-continuation-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-provider-continuation.md`
- `docs/superpowers/specs/2026-05-11-native-provider-one-round-tools-design.md`
- `docs/superpowers/plans/2026-05-11-native-provider-one-round-tools.md`
- `docs/superpowers/specs/2026-05-11-native-provider-tool-advertising-design.md`
- `docs/superpowers/plans/2026-05-11-native-provider-tool-advertising.md`
- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`

## Near-Term Alternative

### Performance evidence follow-up

Continue performance evidence only when it informs a Native MVP implementation choice.

Why: yach's thesis depends on measured responsiveness, but the next product move is autonomous safe read-only tool integration unless a performance question blocks it.

Relevant sources:

- `crates/yach-bench/benches/native_session.rs`
- `docs/benchmarks/current-baseline-2026-05-05.md`
- `docs/project-os/performance-evidence.md`

## Not Ready Without a New Spec

- Defaulting to the native backend.
- File mutation tools.
- Process or shell execution tools.
- Network tools.
- Extension-owned tool registration.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
