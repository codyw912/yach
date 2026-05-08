# Project Decisions

This cockpit decision log is for lightweight local continuity. Durable product/architecture decisions should also be recorded in `docs/project-os/decisions.md`.

## Decisions

### 2026-05-04 — Explicit native-provider dogfood is allowed behind opt-in boundary

- **Status:** accepted
- **Context:** The Rig-first spike produced enough fixture and real-provider evidence to try constrained native-provider dogfood. Anthropic API-key and ChatGPT/Codex subscription OAuth paths work below yach-owned `ProviderRequest` / `ProviderStreamEvent` seams.
- **Decision:** Allow explicit non-default `yach tui --backend native-provider` dogfood for Anthropic and ChatGPT/Codex subscription paths using explicit env/token-dir configuration.
- **Rationale:** This converts provider-seam evidence into useful native dogfood without making native the default or surrendering yach-owned sessions/protocol/tool boundaries.
- **Consequences:** Provider network/credential use is allowed only through explicit opt-in configuration. Pi remains default. No credential persistence, raw payload persistence, retry loop, provider tools/resources, broad provider settings UI, or default-backend change is implied. The earlier “stop before credentials/network/native dogfood” constraint on the initial Rig spike is superseded for this explicit dogfood path only.
- **Related docs:** `docs/spikes/2026-04-28-rig-provider-evaluation.md`, `docs/plans/2026-05-04-001-feat-native-provider-error-ux-plan.md`, `.project/phases/04-minimal-real-native-dogfood-path.md`

### 2026-05-03 — Rig-first provider adapter spike

- **Status:** accepted
- **Context:** U5 provider-library evaluation needs a real first dependency spike candidate. Siumai has attractive lower-agent-framework gravity, but its maturity/adoption signal is too weak for this dependency tier. GenAI remains plausible but less mature/widely supported than Rig.
- **Decision:** Drop Siumai from serious contention for now. Use Rig as the approved first provider-library adapter spike candidate, with GenAI as the serious fallback/control candidate and direct SDKs as the escape hatch.
- **Rationale:** Rig appears more mature and more widely used/supported. It is acceptable to try first unless evidence shows Yach cannot keep ownership of the loop, tools, sessions, transcript persistence, and protocol events.
- **Consequences:** Next implementation may add a minimal Rig dependency spike behind existing yach-owned provider seam types. Stop before credentials/network/native dogfood. Switch to GenAI/direct SDK evaluation if Rig leaks agent/tool/session ownership or loses stream/tool/error fidelity.
- **Related docs:** `docs/spikes/2026-04-28-rig-provider-evaluation.md`, `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`
