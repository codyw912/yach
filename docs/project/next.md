# Next Work

Last updated: 2026-05-12

## Recommended Next Move

Recommended next move: write a focused design for the next extension runtime slice, likely `static_context_provider` or extension install UX, before implementing broader extension behavior. Real-provider smoke validation for explicit native-provider `project_path_info` tool-call emission remains a near-term alternative.

Why: the first extension-owned tool registration path is implemented for safe read-only metadata tools, including manifest/catalog parsing, host registration, process boundary, native workflow routing, provider advertising, and inactive-extension startup evidence. The next extension work is outside that accepted implementation plan and should decide whether yach first needs a context-provider contribution model or an install/loading UX; either way, extension discovery and activation must remain off the default first-frame path.

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
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
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
- Extension runtime implementation beyond safe metadata tool registration.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
