# Project Roadmap

Last updated: 2026-05-04

## Objective / north star

Yach is a Rust shell and eventual native backend for a fast, hackable coding-agent TUI. The durable product boundary is `yach-proto`: `yach-ui` talks protocol events, not Pi RPC, provider SDKs, or native backend internals. Pi RPC remains a compatibility/reference adapter while native sessions, tools, resources, and provider behavior become yach-owned.

## Current state

- M0/M1/M2 are effectively verified: workspace/protocol seed, Pi RPC-backed prompt loop, and TUI alpha dogfood loop exist.
- M3 compatibility work has produced useful Pi evidence and session/fork groundwork, but exhaustive Pi parity is no longer the durable priority.
- Native backend path is active via `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`.
- Current branch `feat/provider-seam-spike` has landed native backend groundwork, session log skeleton, provider seam fixtures, explicit native TUI selection, fixture streaming, failure/cancel persistence, and typed prompt lifecycle events.
- Provider-library choice remains undecided; no provider SDK dependency should be added without explicit approval.

## Target architecture / end-state

```text
yach-ui
  <-> yach-proto ClientEvent / BackendEvent / ServerEvent
    <-> backend runner selection
      -> yach-adapter-pi-rpc       # compatibility/reference path
      -> yach-backend native       # durable path
           -> yach-owned session/event log
           -> yach-owned resource/config surfaces
           -> yach-owned tool lifecycle and permissions
           -> yach-owned provider seam
                -> provider-library adapter(s) or direct SDK adapter(s)
```

Provider libraries may translate requests and streams, but they must not own canonical sessions, tool execution, resources, permission policy, or UI-facing event semantics.

## Guiding constraints and non-goals

### Constraints

- `yach-proto` is the UI/backend seam.
- `yach-ui` must remain independent from Pi RPC, provider SDKs, and native backend internals.
- Pi RPC compatibility is measured/reference behavior, not a feature-complete implementation target.
- Native state should be inspectable, file-first, and provisional until fixture pressure proves the shape.
- Provider-specific options belong behind adapter-owned validation/extension maps, not in common core types by default.
- Performance and compatibility claims require evidence.

### Non-goals for the current roadmap horizon

- Exhaustive Pi backend parity before native progress.
- Provider-hosted sessions as canonical yach sessions.
- Hardwiring Rig, Siumai, GenAI, OpenAI, Anthropic, or Gemini types into `yach-proto` or `yach-ui`.
- Stable public native session file format before dogfood evidence.
- Broad plugin/resource/tool system design before the native runner proves cancellation, errors, persistence, and backpressure.

## Phase sequence

### Phase 1 — Native seams foundation

**Outcome:** Yach has native backend boundaries that can evolve without leaking Pi or provider concerns into the UI.

Status: mostly complete for first pass.

Key outcomes:
- `yach-backend` crate exists.
- Backend session launch is centralized enough for Pi/native selection.
- Native session/event log skeleton exists with append/reload tests.
- Provider request/event/error seam exists with fixture coverage.

Validation:
- Backend/CLI clippy and tests pass.
- No provider SDK dependencies required.
- UI remains protocol-only.

### Phase 2 — Fixture native dogfood runner

**Outcome:** A constrained native mode can be launched explicitly through the existing TUI and exercise lifecycle behavior without network/provider credentials.

Status: in progress.

Key outcomes:
- `yach tui --backend native` is explicit and reversible; Pi remains default.
- Native mode advertises limitations and backend status.
- Fixture prompt streaming works through existing protocol events.
- Failed and cancelled turns are persisted in `.yach/native-sessions/default.jsonl`.
- Prompt lifecycle has typed finish/cancel events.
- Bounded queue/backpressure semantics are specified and tested before claiming native stream robustness.

Validation:
- `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings`
- `just dev cargo test -p yach-backend -p yach-cli`
- Protocol-level changes also validate `yach-proto`, `yach-adapter-pi-rpc`, `yach-ui`, and `yach-cli` as needed.

### Phase 3 — Provider adapter evidence and dependency decision

**Outcome:** Yach selects or rejects an initial provider-library adapter based on fixture-backed event fidelity, not preference or convenience.

Status: partially started via Rig evaluation report and seam fixtures.

Key outcomes:
- Compare Rig, Siumai, GenAI, and/or direct SDK paths against the same yach-owned provider fixtures.
- Preserve text deltas, completion/failure/cancel, usage/finish metadata, provider response IDs as metadata, and tool-call placeholders.
- Decide whether one minimal real adapter spike is worth adding as a dependency.
- Keep provider credentials, network calls, and dependency additions approval-gated.

Validation:
- Adapter spike compiles behind existing seam.
- Fixture tests prove no provider types leak into protocol/UI/native session records.
- Human approval recorded before adding durable provider dependencies.

### Phase 4 — Minimal real native dogfood path

Plan: `phases/04-minimal-real-native-dogfood-path.md`

**Outcome:** A developer can run a constrained native backend through the TUI with one real provider path while preserving yach-owned sessions and inspectable state.

Entry criteria:
- Phase 2 lifecycle/backpressure behavior is tested.
- Phase 3 provider dependency decision is approved.
- Credential and debug redaction policy is defined for the minimal path.

Key outcomes:
- One real prompt stream through native backend.
- One structured provider error surfaced with actionable user-facing copy.
- Cancellation returns UI to idle without stale events corrupting the next turn.
- Session event append/reload remains inspectable and yach-owned.
- No provider framework types leak into `yach-ui`, `yach-proto`, or canonical session records.

Validation:
- Automated clippy/tests for backend/provider adapter/protocol/UI affected crates.
- Manual or scripted TUI smoke for success, provider error, and cancellation.
- Evidence doc links exact commands and limitations.

### Phase 5 — Native-owned tools, resources, and session model hardening

**Outcome:** Native backend grows beyond chat streaming while preserving Pi’s file-first customization spirit and explicit trust boundaries.

Entry criteria:
- Minimal real native prompt loop is dogfoodable.
- Provider stream/error/session metadata assumptions have held under real use.

Key outcomes:
- Native resource/config roots and file-reading policy are specified.
- Tool call lifecycle is yach-owned: schema validation, permission policy, execution, result redaction/size limits, and transcript persistence.
- Native session tree/fork/branch semantics are yach-owned and not provider-thread-owned.
- Raw debug payload capture is opt-in, redacted, and retention-aware.

Validation:
- Fixture and integration tests for resource policy, tool lifecycle, and session branching.
- Compatibility references to Pi behavior where useful, without blocking on exhaustive parity.

### Phase 6 — Evidence gate and product consolidation

**Outcome:** Decide whether native backend becomes the primary product path, remains experimental, or needs architectural revision.

Key outcomes:
- Compatibility evidence distinguishes required Pi-compatible behavior, native-only behavior, and intentionally abandoned Pi parity.
- Performance evidence compares user-visible latency and resilience, not just microbenchmarks.
- Project docs reflect the current product route and deprecate stale milestone assumptions.
- Default backend selection policy is revisited only with evidence.

Validation:
- Updated `docs/project-os/compatibility.md`, `performance-evidence.md`, `roadmap.md`, and `.project/now.md` as needed.
- Owner decision recorded for any default-backend or compatibility-scope shift.

## Cross-phase risks

- **Provider abstraction leakage:** high-level frameworks may hide event boundaries, tool calls, retry/cache IDs, or error details. Mitigation: fixture tests and adapter-owned extension validation.
- **Session format premature stability:** early JSONL shape may become hard to migrate. Mitigation: mark native records provisional and keep migrations explicit later.
- **Backpressure blind spot:** unbounded channels can mask slow-consumer failure until real providers stream quickly. Mitigation: bounded internal queue policy and tests before real dogfood claims.
- **Security/trust drift:** credentials, local files, debug payloads, and tool results can leak if treated as implementation details. Mitigation: define minimum controls before provider/tool dogfood.
- **Compatibility overcorrection:** chasing Pi parity can delay durable native design. Mitigation: use Pi as reference/evidence, not target-complete backend.
- **Performance assumption risk:** Rust/native does not guarantee better UX. Mitigation: keep latency and resilience evidence current.

## Validation / success measures

- `yach-ui` remains protocol-only across all phases.
- Native backend can be selected explicitly and reversed without changing user config.
- Every active native turn has explicit started/delta/finished/failed/cancelled semantics.
- Native state has an inspectable file-first artifact.
- Provider dependency decisions are backed by fixture evidence and human approval.
- Manual dogfood smokes cover success, failure, cancellation, and stale/late stream behavior.
- Docs distinguish provisional implementation details from stable product commitments.

## Open questions

- Which provider-library candidate, if any, should be tried first behind the yach-owned seam?
- What is the minimum acceptable credential source and redaction policy for first real native dogfood?
- How much of the native session tree/fork model should be designed before real provider dogfood versus after fixture pressure?
- When should native mode become default, if ever, and what evidence is required for that owner decision?

## Next planning/deepening candidates

- Use `phases/04-minimal-real-native-dogfood-path.md` for the next native-provider dogfood chunks.
- Deepen Phase 3 only if the project revisits provider-library selection beyond the currently working Rig Anthropic and ChatGPT/Codex subscription paths.
- After the next validated implementation slice, run a factual docs checkpoint to reconcile `docs/project-os/roadmap.md`, `docs/project-os/next-work.md`, and this cockpit roadmap.
