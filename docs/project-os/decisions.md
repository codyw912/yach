# Decision Log

Use this log for product and architecture decisions that should outlive a chat session or commit message. Keep entries concise. If a decision becomes large or controversial, add a dedicated ADR-style doc later and link it here.

## Statuses

- `proposed` — suggested but not accepted.
- `accepted` — current decision.
- `superseded` — replaced by a later decision.
- `revisit` — accepted for now, but known to need re-evaluation.

## Decision template

```md
### DYYYYMMDD-NN — <Decision title>

- **Status:** proposed | accepted | superseded | revisit
- **Date:** YYYY-MM-DD
- **Context:** <What forced the choice?>
- **Decision:** <What are we doing?>
- **Rationale:** <Why this option?>
- **Consequences:** <What becomes easier/harder?>
- **Related docs:** <Links>
- **Follow-up:** <Optional>
```

## Current decisions

### D20260503-01 — Use Rig as first provider-library adapter spike candidate

- **Status:** accepted
- **Date:** 2026-05-03
- **Context:** U5 provider-library evaluation needs a serious first dependency spike candidate below yach's provider seam. Siumai has appealing lower agent-framework gravity, but its maturity/adoption signal is too weak for a core provider dependency at this stage. GenAI remains plausible but is better as a fallback/control candidate.
- **Decision:** Use Rig as the approved first provider-library adapter spike candidate. Drop Siumai from serious contention for now. Keep GenAI as the serious fallback/control candidate and direct SDKs as the escape hatch.
- **Rationale:** Rig appears more mature and more widely used/supported. Trying it first is worthwhile unless evidence shows yach cannot retain ownership of the loop, tools, sessions, transcript persistence, and protocol events.
- **Consequences:** The next implementation may add a minimal Rig dependency spike behind existing yach-owned provider seam types. The spike must stop before credentials, network calls, or native provider dogfood, and must switch to GenAI/direct SDK evaluation if Rig leaks agent/tool/session ownership or loses stream/tool/error fidelity.
- **Related docs:** `../spikes/2026-04-28-rig-provider-evaluation.md`, `../plans/2026-04-27-004-feat-native-backend-path-plan.md`
- **Follow-up:** Implement a thin Rig adapter feasibility spike with fixture-backed mapping tests and no provider credentials/network path.

### D20260426-01 — Keep project OS repo-first

- **Status:** accepted
- **Date:** 2026-04-26
- **Context:** Work selection was too dependent on the active agent, and the project owner wanted durable planning context before more implementation accumulated.
- **Decision:** Canonical planning context starts as Markdown in the repo under `docs/project-os/`.
- **Rationale:** Repo docs are available to agents, humans, worktrees, and future contributors without depending on external tracker state.
- **Consequences:** GitHub issues/projects can still be derived later, but they are not the first source of truth.
- **Related docs:** `../brainstorms/2026-04-26-project-os-requirements.md`, `../plans/2026-04-26-001-feat-project-os-skeleton-plan.md`

### D20260426-02 — Start with a single decision log, not ADR files

- **Status:** accepted
- **Date:** 2026-04-26
- **Context:** Yach needs a durable decision surface, but the first pass should avoid process overhead.
- **Decision:** Use this single decision log for now; introduce separate ADR files only when a decision needs more depth.
- **Rationale:** One log is easier for agents to keep current and discover.
- **Consequences:** Older decisions embedded in PRD/plans can be linked first and extracted later only when needed.
- **Related docs:** `../plans/2026-04-26-001-feat-project-os-skeleton-plan.md`

### D20260426-03 — Seed project OS from existing docs only

- **Status:** accepted
- **Date:** 2026-04-26
- **Context:** The project OS should be useful immediately, but the origin requirements explicitly avoid a full audit in the first pass.
- **Decision:** Seed roadmap, next work, compatibility, and performance surfaces from existing source docs with freshness/provenance labels.
- **Rationale:** This creates useful structure without laundering unverified claims into canonical status.
- **Consequences:** The next useful project task is a current M2/TUI checkpoint that can update these docs with fresh evidence.
- **Related docs:** `../status/m0-m1-checkpoint.md`, `roadmap.md`, `next-work.md`

### D20260427-02 — Prefer existing Rust LLM/provider crates below yach's provider seam

- **Status:** accepted
- **Date:** 2026-04-27
- **Context:** Native backend planning needs provider integrations quickly, but owning every provider API directly would create churn and slow the product path.
- **Decision:** Prefer using an existing Rust LLM/provider crate, with Rig as the leading evaluation candidate, below a yach-owned provider seam. Direct provider integrations remain possible as additional adapters if existing crates cannot preserve yach's required event fidelity, control, security, or minimal design constraints.
- **Rationale:** This lets yach hit the ground running without giving provider frameworks ownership of sessions, tools, resources, or protocol semantics. The replaceable seam keeps a long-term direct-owned path available if library abstraction costs become too high.
- **Consequences:** Provider spike work should optimize for keeping a provider-library adapter viable before defaulting to direct SDK ownership. Review criteria should focus on abstraction leakage, stream/tool/error fidelity, dependency cost, credential/debug-data handling, and ability to preserve yach-owned state.
- **Related docs:** `../plans/2026-04-27-004-feat-native-backend-path-plan.md`, `architecture-invariants.md`
- **Follow-up:** Use the provider spike to decide whether Rig is good enough, whether another crate is better, or whether a direct adapter is necessary for selected providers.

### D20260427-01 — Stop chasing exhaustive Pi-backend parity before native backend work

- **Status:** accepted
- **Date:** 2026-04-27
- **Context:** The stock Pi RPC adapter was always a temporary compatibility bridge. Recent M3 work proved enough session/fork/message surfaces to inform yach's UI/protocol shape, but continuing to reimplement every Pi backend feature through the temporary adapter would slow the native backend path.
- **Decision:** Treat the Pi backend path as a compatibility/reference layer, not a feature-complete target. Do not prioritize small Pi-backend-only parity gaps unless they unblock dogfooding, migration evidence, or native backend design. Start planning native Rust backend work sooner than the original strict Phase 1 gate implied.
- **Rationale:** yach's durable value is the Rust shell plus native backend architecture, not exhaustive temporary adapter parity. Compatibility work should harvest lessons and preserve migration paths, while native work should own future feature semantics.
- **Consequences:** M3 compatibility remains useful but becomes selective. Some Pi features, including compaction details, may stay backend-owned/opaque in the Pi adapter until the native backend models them explicitly. The Phase 2 gate shifts from "finish broad Phase 1 parity" to "enough evidence to design native backend without regressing the core dogfood loop."
- **Related docs:** `next-work.md`, `compatibility.md`, `architecture-invariants.md`, `../../PRD-v0.1.md`
- **Follow-up:** Reframe next work around native backend architecture planning and identify only the minimum remaining Pi evidence needed for migration/reference.

## Linked prior decisions not yet extracted

These decisions are important but remain in their source docs until extraction is useful:

- Stock Pi RPC first, SDK sidecar later: `../../PRD-v0.1.md`
- UI talks through `yach-proto`, never Pi RPC directly: `../../PRD-v0.1.md`, `../protocol/yach-proto-v0.md`
- Tokio from the start for TUI alpha: `../plans/2026-04-21-m2-tui-alpha-design.md`
- MCP is a separate lane, not the only extension model: `../../PRD-v0.1.md`
