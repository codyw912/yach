# Next Work

Last updated: 2026-05-15

## Recommended Next Move

Recommended next move: execute Task 6 of the native edit local access
implementation plan: cross-crate verification and cleanup.

Why: native edit preview, guarded apply, redacted evidence, a backend-local
harness, local profiling, the local access design, and the implementation plan
are now merged, and the first implementation slice has established the shared
permission/reviewer vocabulary plus redacted permission decision evidence. The
backend-owned edit access facade now owns pending prepared transactions behind
preview IDs, and the protocol now exposes the local edit prepare, preview,
decision, and finish lifecycle. The native runner now connects those events to
the backend facade and persisted evidence, and the TUI now has a temporary
`/debug-edit` manual harness that sends local prepare requests, correlates
preview and finish responses, and submits apply/reject decisions. This is not
the product edit surface; the intended edit surface remains agent-selected
tools once mutation tools are explicitly designed and exposed. The next slice
should run the full cross-crate verification pass from the plan, fix any
compile or lint cleanup surfaced by strict workspace checks, and confirm
provider replay and protocol JSONL compatibility still behave as expected.

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
- `docs/benchmarks/native-edit-profile-2026-05-15.md`
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`

## Near-Term Alternative

### Production edit tracing design

Design production edit tracing for local edit operations after the CLI/TUI
access surface is specified.

Why: durable trace IDs and production observability should follow the concrete
local UX boundaries so they record the right user-visible states and approval
events.

Relevant sources:

- `docs/superpowers/specs/2026-05-15-native-edit-benchmark-trace-design.md`
- `docs/benchmarks/native-edit-profile-2026-05-15.md`

## Not Ready Without a New Spec

- Provider-advertised file mutation tools, extension-owned mutation tools,
  hidden built-in edit tools, delete/rename, or multi-operation edit atomicity.
- Process or shell execution tools.
- Network tools.
- Extension runtime implementation beyond safe metadata tool registration and manifest-only static context metadata.
- A working auto-review reviewer/subagent runtime.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
