# Next Work

Last updated: 2026-05-18

## Recommended Next Move

Recommended next move: dogfood the native-provider edit loop with
provider-visible read/search/list content tools enabled.

Why: exact/create edit tools now have the minimum content acquisition surface
needed for practical file edits. The next useful evidence is real-session
behavior: whether bounds are too small, whether result summaries are readable,
and whether the model reliably uses read/search/list before edit calls.

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
- `docs/superpowers/plans/2026-05-15-native-edit-benchmark-trace.md`
- `docs/superpowers/specs/2026-05-15-native-edit-local-access-design.md`
- `docs/superpowers/plans/2026-05-15-native-edit-local-access.md`
- `docs/superpowers/specs/2026-05-15-native-agent-edit-tool-surface-design.md`
- `docs/superpowers/plans/2026-05-15-native-agent-edit-tools.md`
- `docs/superpowers/specs/2026-05-17-production-edit-tracing-design.md`
- `docs/superpowers/plans/2026-05-17-production-edit-tracing.md`
- `docs/superpowers/specs/2026-05-18-provider-read-search-content-design.md`
- `docs/superpowers/plans/2026-05-18-provider-read-search-content.md`
- `docs/benchmarks/native-edit-profile-2026-05-15.md`
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`

## Near-Term Alternative

### Broader Mutation Surface

Design broader mutation tools only after read/search content is scoped.

Why: canonical exact/create edit tools assume the provider already has the
needed target text. Until provider-visible content acquisition is explicit,
broadening mutation would add risk without addressing the main usefulness gap.

Relevant sources:

- `docs/superpowers/plans/2026-05-11-native-read-search-context.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-tool-loop-design.md`
- `docs/superpowers/specs/2026-05-15-native-agent-edit-tool-surface-design.md`
- `docs/superpowers/plans/2026-05-15-native-agent-edit-tools.md`

## Not Ready Without a New Spec

- Provider-advertised file mutation beyond the canonical exact/create edit
  schemas, extension-owned mutation tools, broad write/patch/delete/rename
  tools, or multi-operation edit atomicity.
- Process or shell execution tools.
- Network tools.
- Extension runtime implementation beyond safe metadata tool registration and manifest-only static context metadata.
- A working auto-review reviewer/subagent runtime.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
