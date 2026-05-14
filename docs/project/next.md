# Next Work

Last updated: 2026-05-14

## Recommended Next Move

Recommended next move: execute the native edit transactions apply implementation plan.

Why: the preview-only backend primitive is now merged. The next Native MVP gap is guarded apply for prepared transactions: `NativeEditEngine::apply`, one operation per transaction, create no-overwrite publish, modify hash/race guards, same-directory temporary files, ordinary permission preservation, no-write-on-validation-failure tests, and structured apply metadata. Session evidence, hidden/local tool harnessing, benchmarks, and provider-visible edit tools should follow as later slices after apply semantics are stable.

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

- Provider-advertised file mutation tools, extension-owned mutation tools, edit session evidence, hidden/local edit tool harnessing, edit benchmarks, delete/rename, or multi-operation edit atomicity.
- Process or shell execution tools.
- Network tools.
- Extension runtime implementation beyond safe metadata tool registration and manifest-only static context metadata.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
