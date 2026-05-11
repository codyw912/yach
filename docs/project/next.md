# Next Work

Last updated: 2026-05-11

## Recommended Next Move

Recommended next move: continue Native MVP implementation with provider tool advertising for `project_path_info` behind explicit native-provider opt-in.

Why: the native-provider runner can now handle a completed safe read-only tool call and one continuation round, but real provider dogfooding still needs the initial provider request to advertise only the metadata-safe `project_path_info` schema so models can request it intentionally.

Relevant sources:

- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/plans/2026-05-10-native-session-store-resume-metrics.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-tool-loop.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-provider-continuation-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-provider-continuation.md`
- `docs/superpowers/specs/2026-05-11-native-provider-one-round-tools-design.md`
- `docs/superpowers/plans/2026-05-11-native-provider-one-round-tools.md`
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
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
