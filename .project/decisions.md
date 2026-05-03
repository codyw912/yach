# Project Decisions

This cockpit decision log is for lightweight local continuity. Durable product/architecture decisions should also be recorded in `docs/project-os/decisions.md`.

## Decisions

### 2026-05-03 — Rig-first provider adapter spike

- **Status:** accepted
- **Context:** U5 provider-library evaluation needs a real first dependency spike candidate. Siumai has attractive lower-agent-framework gravity, but its maturity/adoption signal is too weak for this dependency tier. GenAI remains plausible but less mature/widely supported than Rig.
- **Decision:** Drop Siumai from serious contention for now. Use Rig as the approved first provider-library adapter spike candidate, with GenAI as the serious fallback/control candidate and direct SDKs as the escape hatch.
- **Rationale:** Rig appears more mature and more widely used/supported. It is acceptable to try first unless evidence shows Yach cannot keep ownership of the loop, tools, sessions, transcript persistence, and protocol events.
- **Consequences:** Next implementation may add a minimal Rig dependency spike behind existing yach-owned provider seam types. Stop before credentials/network/native dogfood. Switch to GenAI/direct SDK evaluation if Rig leaks agent/tool/session ownership or loses stream/tool/error fidelity.
- **Related docs:** `docs/spikes/2026-04-28-rig-provider-evaluation.md`, `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`
