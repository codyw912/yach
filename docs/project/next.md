# Next Work

Last updated: 2026-06-17

## Recommended Next Move

Recommended next move: continue move-only backend structure extraction in
`docs/superpowers/plans/2026-06-11-repository-audit-remediation.md`, starting
from the next cohesive `crates/yach-backend/src/native_runner.rs` responsibility
after `native_runner/extension_state.rs`, `native_runner/local_edit.rs`, and
`native_runner/session_state.rs`.

Why: the audit safety net, CI, session-store durability, in-memory native
runner transcript state, off-reactor startup session load, and async-aware
extension scan/activation state have landed. Extension host processes now start
with an explicit allowlisted environment instead of inheriting provider/API
secrets by default. The remaining high-leverage audit work is structural:
reduce the blast radius of future backend changes without mixing behavior
changes into the extraction. Local edit prepare/decision handling now lives in
`native_runner/local_edit.rs`, and native session log loading/presentation now
lives in `native_runner/session_state.rs`, so the next slice should avoid
revisiting those surfaces unless a compile-boundary cleanup is required.

Keep the next slice move-only. Do not combine module extraction with new
session, tool-loop, extension, or provider behavior. The first completed
extraction moved extension scan, activation, and lifecycle state into
`native_runner/extension_state.rs`, the second moved local edit handling into
`native_runner/local_edit.rs`, and the third moved session log
loading/presentation into `native_runner/session_state.rs`; the next candidate
should be selected by coupling, not line count alone.

Relevant sources:

- `docs/superpowers/plans/2026-06-11-repository-audit-remediation.md`
- `docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md`
- `docs/superpowers/specs/2026-06-02-extension-activation-manager-design.md`
- `docs/project/records/2026-06-03-mvp-convergence.md`
- `docs/superpowers/plans/2026-05-21-extension-runtime-first-slice.md`
- `docs/superpowers/specs/2026-05-20-extension-runtime-tool-replacement-design.md`
- `docs/superpowers/specs/2026-05-23-extension-install-host-lifecycle-design.md`
- `docs/superpowers/plans/2026-05-23-extension-local-install-records.md`
- `docs/benchmarks/extension-runtime-profile-2026-05-23.md`
- `docs/superpowers/specs/2026-05-12-extension-tool-registration-design.md`
- `docs/superpowers/plans/2026-05-12-extension-tool-registration.md`
- `docs/superpowers/specs/2026-05-18-native-provider-multi-round-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-18-native-provider-multi-round-tool-loop.md`
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/specs/2026-05-13-native-static-context-design.md`
- `docs/superpowers/plans/2026-05-13-native-static-context.md`

## Near-Term Alternative

### Native MVP Dogfood Checkpoint

Rerun the native MVP dogfood checkpoint after the first structure-extraction
slice lands, or sooner if you want product validation before refactoring.

Why: dogfood remains the right product validation loop, but session evidence and
resume behavior are part of that validation surface. The known reliability
weaknesses have now been addressed, so this is a validation choice rather than
a prerequisite for the next audit-remediation slice.

### Extension Developer/Package UX

Write a focused extension developer/package UX design before implementing
templates, TypeScript/Rust host packaging, git refs, or npm adapter behavior.

Why: this work matters for the extensible harness vision, but stop/reload/live
status now give enough extension lifecycle surface for MVP convergence. Package
and developer UX should resume after the native default is usable for real work.

### Provider Tool Guardrails

Add provider-loop guardrails that reduce model-authored claims of local effects
when tool evidence is missing.

Why: dogfooding showed the backend loop is correct, but older prompts let the
model claim an edit after only reading. This is less important than extension
runtime work because the stronger prompt and existing tool evidence path
produce correct behavior, but the provider-loop harness can still become more
defensive.

Relevant sources:

- `docs/superpowers/specs/2026-05-18-native-provider-multi-round-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-18-native-provider-multi-round-tool-loop.md`

## Not Ready Without a New Spec

- Provider-advertised file mutation beyond the canonical exact/create edit
  schemas, extension-owned mutation tools, broad write/patch/delete/rename
  tools, or multi-operation edit atomicity.
- Process or shell execution tools.
- Network tools.
- Npm/git extension package adapters, developer templates,
  package-manager integration, or cross-language host packaging beyond the
  accepted manifest-first, metadata-first runtime/replacement design.
- A working auto-review reviewer/subagent runtime.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
