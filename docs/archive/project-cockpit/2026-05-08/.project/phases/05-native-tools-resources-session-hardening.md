# Phase 5 — Native Tools, Resources, and Session Model Hardening

Last updated: 2026-05-05

## Goal

Grow the native backend beyond constrained chat streaming by defining and then implementing yach-owned tools, resources, and session semantics without surrendering trust, file-first customization, or protocol ownership to Pi RPC, provider SDKs, or provider-hosted sessions.

This phase is successful when native backend work can safely execute the first yach-owned tool/resource/session workflows through explicit native dogfood while keeping `yach-ui` protocol-only, native records inspectable/provisional, and provider/tool/resource data redacted and policy-bound.

## Why it matters

Phase 4 proved that a constrained native-provider prompt loop can work below a yach-owned provider seam. The next product value comes from what makes coding agents useful: local files/resources, tool calls, permissions, and durable session structure. These are also the highest-trust parts of the backend. Planning them before implementation prevents accidental provider-framework ownership, raw local-data leakage, unstable session-format commitments, and Pi-RPC compatibility overcorrection.

## Dependencies / entry criteria

- Phase 1 native seams are checkpointed: `yach-backend` exists, runner launch is centralized, native session/event records exist, and provider seam types exist.
- Phase 2 fixture native runner is checkpointed: explicit `--backend native`, lifecycle events, failure/cancel persistence, malformed/backpressure fixture coverage.
- Phase 3 Rig-first provider adapter evidence is checkpointed for Anthropic and ChatGPT/Codex subscription paths.
- Phase 4 explicit `--backend native-provider` dogfood is checkpointed for success, cancellation, invalid-model failure classification, provider metadata persistence, and status-only error UX.
- Pi remains default; native/native-provider remain explicit and reversible.
- `yach-proto` remains the UI/backend seam; `yach-ui` must not import native backend, provider, or Pi RPC internals.
- Native session JSONL remains backend-internal/provisional.

## Expected end-state

By the end of this phase:

- Native resource/config roots are specified, canonicalized, and tested enough for first safe file/resource reads.
- Tool call lifecycle is yach-owned: tool definitions, schema validation, permission policy, execution boundary, result redaction/size limits, and transcript/session persistence.
- Provider-emitted tool-call requests remain below the yach provider seam and are translated into yach-owned tool requests; provider libraries do not execute tools or own the loop.
- Native session tree/fork/branch semantics are sketched and exercised enough to avoid provider-thread/session ownership.
- Sensitive local data handling is explicit: credentials, resource contents, tool arguments/results, provider metadata, and raw debug payloads have redaction/retention rules.
- A minimal explicit native dogfood workflow can demonstrate one safe resource/tool/session behavior without making native default.
- Compatibility references to Pi behavior are used for migration/product insight, not as exhaustive parity gates.

## Workstreams

### 1. Resource and configuration policy

- Define native resource/config roots for project-local, user-global, generated state, and optional compatibility imports.
- Canonicalize paths and address symlink/path traversal before reading resources.
- Distinguish trusted local config from model/provider-visible context.
- Decide reload/discovery semantics for the first resource surfaces.
- Keep file-first customization visible and inspectable.

### 2. Tool definition and permission model

- Define the yach-owned tool registry shape: tool name, description, input schema, output policy, trust level, and execution handler boundary.
- Treat model/provider tool calls as untrusted input; validate JSON arguments before execution.
- Require an explicit permission policy for file/network/process-affecting tools.
- Preserve user approval hooks or policy decisions before dangerous tool execution.
- Define tool-result size limits, redaction, summarization, and persistence rules.

### 3. Provider tool-call mapping

- Map `ProviderStreamEvent::ToolCall*` / provider tool-call placeholders into yach-owned pending tool requests.
- Preserve provider call ids only as metadata needed to send tool results back to the provider; do not make them canonical session ids.
- Keep provider libraries below the provider seam; they may surface tool-call requests but must not execute yach tools or mutate yach sessions directly.
- Add fixtures before real tool execution through providers.

### 4. Native session tree/fork/branch model

- Define enough native session semantics for turns, entries, parent links, branches, fork points, and provider metadata.
- Use existing Pi session/fork evidence as reference input, not a schema to copy blindly.
- Keep native JSONL provisional until branch/fork/tool/resource pressure proves it.
- Plan migration/import separately from native canonical model design.

### 5. Security, redaction, and debug data policy

- Forbid credential/raw provider payload persistence by default.
- Define explicit debug mode requirements before storing raw payloads or tool/provider traces.
- Redact authorization headers, API-key patterns, local secret-looking values, and oversized outputs where possible.
- Define file permissions/retention expectations for native session logs and generated state.
- Document which data may be sent to providers and under what policy.

### 6. Evidence and docs checkpoint

- Update `docs/project-os/compatibility.md` only for evidence-backed compatibility changes.
- Update `docs/protocol/yach-proto-v0.md` only for actual protocol behavior.
- Update `docs/project-os/roadmap.md`, `docs/project-os/next-work.md`, and `.project/now.md` after validated implementation slices.
- Keep performance claims tied to evidence if tools/resources affect latency or transcript size.

## Key decisions needed

1. **Resource roots:** Which project/user/generated paths are first-class native resource roots, and which Pi resource locations are compatibility imports only?
2. **Permission default:** Are native tools deny-by-default, prompt-for-approval, or policy-configured for the first dogfood slice?
3. **Tool execution boundary:** Do native tools run in-process initially, behind subprocess boundaries, or behind a trait that can later move out-of-process?
4. **Provider tool loop:** What is the minimum safe loop for provider tool-call request -> yach tool execution -> provider tool result continuation?
5. **Session branch shape:** What parent/branch/fork metadata is needed before tools/resources start writing richer records?
6. **Debug payload policy:** What explicit setting, redaction, and retention rules are required before raw provider/tool payload capture?
7. **Protocol surface:** Which tool/resource/session behaviors require `yach-proto` events now versus backend-internal records first?

## Risks and mitigations

- **Security/trust drift:** Local files, commands, tool args/results, and provider payloads can leak sensitive data. Mitigation: define policy before execution; default to no raw payload persistence; validate and redact.
- **Provider loop leakage:** Rig or another library may encourage owning tool execution/history. Mitigation: provider adapter only surfaces tool-call requests; yach owns execution/session mutation.
- **Session format ossification:** Tool/resource records may make provisional JSONL feel stable too early. Mitigation: keep explicit provisional labels and defer migration promises.
- **Protocol churn:** Tools/resources/session tree may tempt broad protocol additions. Mitigation: add only user-visible protocol events needed by UI; keep backend-internal evidence first.
- **Pi parity overcorrection:** Resource/session compatibility can become exhaustive Pi reimplementation. Mitigation: use Pi as reference for migration-critical workflows only.
- **Performance regressions:** Tool outputs/resources can inflate transcripts and slow rendering. Mitigation: size limits, summaries, and performance evidence for large outputs.
- **Permission UX scope creep:** Designing a full policy UI can swamp native backend progress. Mitigation: start with narrow explicit policy and document limitations.

## Validation strategy

Planning/docs chunks:

```bash
git diff --check
```

Backend-only resource/tool/session slices:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings
just dev cargo test -p yach-backend -p yach-cli
git diff --check
```

Protocol/UI-impacting slices:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-proto -p yach-ui -p yach-cli -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-proto -p yach-ui -p yach-cli -p yach-backend
git diff --check
```

Optional final confidence after meaningful implementation:

```bash
just dev cargo clippy --workspace --all-targets -- -D warnings
just dev cargo test --workspace
```

Manual evidence should be approval-gated when it uses real providers, credentials, local sensitive resources, or destructive tools.

## Acceptance criteria

- `yach-ui` remains protocol-only and does not import Pi RPC, provider SDK, or native backend internals.
- Native resources have explicit roots, path canonicalization, and first-pass tests before provider-visible reads.
- Native tools are defined/executed through yach-owned schemas, permission policy, execution boundary, and redacted/size-limited results.
- Provider tool-call requests are translated into yach-owned tool requests; provider libraries do not execute tools or own sessions.
- Native session records can represent tool/resource-influenced turns, parent links, branch/fork metadata, outcomes, and provider metadata without claiming stable public format.
- Credentials, raw provider payloads, resource contents, tool args/results, and debug data have explicit persistence/redaction rules.
- At least one constrained native dogfood workflow demonstrates the first safe tool/resource/session behavior or documents why implementation is deferred.
- Docs record evidence and limitations without declaring native backend default/production-ready.

## Candidate chunks

### Chunk 1 — Resource/config root policy plan

- **Why it matters:** Native tools/resources cannot safely read local files until yach defines roots, trust levels, path canonicalization, provider visibility, and reload/discovery semantics.
- **Expected files/areas:** `docs/plans/`, `.project/now.md`, possibly references to `docs/project-os/compatibility.md`, `docs/project-os/architecture-invariants.md`, and `docs/protocol/yach-proto-v0.md`.
- **Max scope:** Planning/design only. Define first resource root model and recommend first implementation slice. No code changes.
- **Validation command:** `git diff --check`.
- **Risk level:** Medium due security/data policy, but low implementation risk because planning-only.
- **Stop/ask condition:** Stop before approving provider-visible file reads, credential/config persistence, migration/import semantics, or broad resource UI.
- **Human approval needed:** No for planning; yes before implementation that exposes local files to providers.

### Chunk 2 — Native tool lifecycle and permission plan

- **Why it matters:** Provider tool calls and native tools are high-trust boundaries; yach needs an owned lifecycle before execution.
- **Expected files/areas:** `docs/plans/`, `.project/now.md`, references to provider seam docs and `docs/project-os/architecture-invariants.md`.
- **Max scope:** Planning/design only. Define tool registry shape, schema validation, permission defaults, execution boundary options, result redaction/size policy, and first safe tool candidate. No code changes.
- **Validation command:** `git diff --check`.
- **Risk level:** Medium-high due security implications, but planning-only.
- **Stop/ask condition:** Stop before committing to default permission behavior, executing tools, provider tool-result continuation, or process/network/file mutation policy.
- **Human approval needed:** No for planning; yes before implementing permission/security behavior.

### Chunk 3 — Native session branch/tool record shape plan

- **Why it matters:** Tool/resource work will add richer records. The native session model should represent parent links, branches, tool calls/results, provider metadata, and outcomes without copying provider/Pi-owned sessions.
- **Expected files/areas:** `docs/plans/`, `.project/now.md`, possibly `docs/protocol/yach-proto-v0.md` if UI-visible implications are documented.
- **Max scope:** Planning/design only. Propose provisional backend-internal record additions and migration cautions. No code changes, no stable format promise.
- **Validation command:** `git diff --check`.
- **Risk level:** Medium due session model coupling.
- **Stop/ask condition:** Stop before declaring native JSONL stable, adding migration tooling, or changing user-visible session tree policy.
- **Human approval needed:** No for planning; yes before stable format/migration decisions.

## Later candidate chunks

- Implement resource root/path canonicalization helpers and tests.
- Implement first read-only resource fixture flow with no provider submission.
- Implement tool registry skeleton and schema validation fixtures.
- Implement provider tool-call-to-yach-pending-tool-request mapping fixtures.
- Implement provisional native session records for tool calls/results.
- Add performance evidence for large tool output/resource transcript impact.

## Explicit non-goals

- Making native or native-provider backend default.
- Stabilizing native session JSONL as a public format.
- Importing/migrating all Pi settings/resources/packages/sessions.
- Implementing destructive tools, network tools, or process-mutating tools in the first slice.
- Persisting credentials or raw provider payloads by default.
- Adding broad provider settings/model UI.
- Implementing typed protocol error events unless separately approved.
- Reworking Pi RPC process IO or chasing Pi-only parity gaps that do not inform native design.
