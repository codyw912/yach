# Next Work

Last updated: 2026-05-15

## Recommended Next Move

Recommended next move: review the native edit benchmark and profiling design,
then write and execute its implementation plan.

Why: native edit preview, guarded apply, redacted evidence, and a backend-local
harness are now merged. Before exposing edits through CLI/TUI commands, hidden
built-ins, provider-visible tools, or extension-owned mutation, yach should be
able to measure preview, apply, evidence conversion, session append, and
end-to-end harness cost with enough granularity to explain bottlenecks.

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
- `docs/superpowers/specs/2026-05-12-extension-tool-registration-design.md`
- `docs/superpowers/plans/2026-05-12-extension-tool-registration.md`
- `docs/superpowers/specs/2026-05-13-native-static-context-design.md`
- `docs/superpowers/plans/2026-05-13-native-static-context.md`
- `docs/superpowers/specs/2026-05-13-native-edit-transactions-design.md`
- `docs/superpowers/plans/2026-05-13-native-edit-transactions-preview.md`
- `docs/superpowers/plans/2026-05-14-native-edit-transactions-apply.md`
- `docs/superpowers/specs/2026-05-14-native-edit-evidence-harness-design.md`
- `docs/superpowers/plans/2026-05-14-native-edit-evidence-harness.md`
- `docs/superpowers/specs/2026-05-15-native-edit-benchmark-trace-design.md`
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`

## Near-Term Alternative

### CLI/TUI edit access design

Design a local user-facing edit entry point after native edit profiling is in
place.

Why: the backend now has the mutation primitive and evidence boundary, but the
product surface still needs approval semantics, result display, cancellation,
and user-visible error behavior. Profiling first keeps that design grounded in
measured costs.

Relevant sources:

- `crates/yach-bench/benches/native_session.rs`
- `docs/benchmarks/current-baseline-2026-05-05.md`
- `docs/project-os/performance-evidence.md`

## Not Ready Without a New Spec

- Provider-advertised file mutation tools, extension-owned mutation tools,
  CLI/TUI edit commands, hidden built-in edit tools, delete/rename, or
  multi-operation edit atomicity.
- Process or shell execution tools.
- Network tools.
- Extension runtime implementation beyond safe metadata tool registration and manifest-only static context metadata.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
