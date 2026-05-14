# Next Work

Last updated: 2026-05-14

## Recommended Next Move

Recommended next move: review and then execute the native edit evidence and local harness implementation plan.

Why: native edit preview and guarded apply are now merged, but mutation still has no durable local-effect evidence or runtime-shaped harness. The next slice should add explicit redacted edit session events, JSONL round-trip coverage, and a backend-local preview/apply wrapper that remains outside `NativeToolRegistry` and provider advertising. Benchmarks, CLI/TUI access, hidden built-in tools, approval UX, extension mutation, and provider-visible edit tools should follow only after that evidence boundary is stable.

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
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`

## Near-Term Alternative

### Performance evidence follow-up

Continue performance evidence only when it informs a Native MVP implementation choice.

Why: yach's thesis depends on measured responsiveness, but performance work should now stay tied to concrete Native MVP decisions such as edit transactions, startup activation boundaries, or provider/tool-loop latency.

Relevant sources:

- `crates/yach-bench/benches/native_session.rs`
- `docs/benchmarks/current-baseline-2026-05-05.md`
- `docs/project-os/performance-evidence.md`

## Not Ready Without a New Spec

- Provider-advertised file mutation tools, extension-owned mutation tools, CLI/TUI edit commands, hidden built-in edit tools, edit benchmarks, delete/rename, or multi-operation edit atomicity.
- Process or shell execution tools.
- Network tools.
- Extension runtime implementation beyond safe metadata tool registration and manifest-only static context metadata.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
