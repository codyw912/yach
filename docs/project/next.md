# Next Work

Last updated: 2026-05-12

## Recommended Next Move

Recommended next move: execute the extension-owned tool registration implementation plan through the manifest/catalog, `toy_tool` registration, native workflow routing, policy-gated provider advertising, and inactive-extension startup evidence slices. Real-provider smoke validation for explicit native-provider `project_path_info` tool-call emission remains a near-term alternative.

Why: the extension-owned tool registration design is now accepted and the implementation plan decomposes it into safe, testable slices. Startup profiling still constrains the work: extension discovery and activation must stay off the default first-frame path, while extension-owned tools enter the same yach-owned policy and advertising pipeline without giving adapters execution ownership.

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
- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`

## Near-Term Alternative

### Performance evidence follow-up

Continue performance evidence only when it informs a Native MVP implementation choice.

Why: yach's thesis depends on measured responsiveness, but the next product move is extension-owned tool registration unless a performance question blocks it.

Relevant sources:

- `crates/yach-bench/benches/native_session.rs`
- `docs/benchmarks/current-baseline-2026-05-05.md`
- `docs/project-os/performance-evidence.md`

## Not Ready Without a New Spec

- Defaulting to the native backend.
- File mutation tools.
- Process or shell execution tools.
- Network tools.
- Extension runtime implementation beyond the accepted tool-registration plan.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
