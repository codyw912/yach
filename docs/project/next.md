# Next Work

Last updated: 2026-05-22

## Recommended Next Move

Recommended next move: implement Task 3 from the first conservative extension
runtime plan: persistent metadata tool host invocation.

Why: package-root manifest indexing and post-first-paint manifest scanning are
now implemented without putting extension work on the first-render path or
spawning hosts during discovery. The next durable step is to keep that startup
property while proving a safe metadata `toy_tool` can run through a persistent
process-host protocol under yach-owned validation, evidence, and result shaping.

Relevant sources:

- `docs/superpowers/plans/2026-05-21-extension-runtime-first-slice.md`
- `docs/superpowers/specs/2026-05-20-extension-runtime-tool-replacement-design.md`
- `docs/superpowers/specs/2026-05-12-extension-tool-registration-design.md`
- `docs/superpowers/plans/2026-05-12-extension-tool-registration.md`
- `docs/superpowers/specs/2026-05-18-native-provider-multi-round-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-18-native-provider-multi-round-tool-loop.md`
- `docs/benchmarks/extension-startup-profile-2026-05-12.md`
- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/specs/2026-05-13-native-static-context-design.md`
- `docs/superpowers/plans/2026-05-13-native-static-context.md`

## Near-Term Alternative

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
- Extension runtime behavior beyond the accepted manifest-first,
  process-hosted, metadata-first runtime/replacement design.
- A working auto-review reviewer/subagent runtime.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
