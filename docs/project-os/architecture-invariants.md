# Architecture Invariants

These are durable constraints that should survive individual implementation choices. If work needs to change one, record the decision in `decisions.md` and update impacted roadmap, compatibility, or performance docs.

## Invariant change protocol

Before changing or weakening an invariant:

1. State which invariant is affected.
2. Explain whether the change is deliberate architecture evolution or accidental drift.
3. Add or update an entry in `decisions.md`.
4. Update related trackers (`roadmap.md`, `compatibility.md`, `performance-evidence.md`) when the change affects status or evidence.
5. Note the impact in the implementation handoff.

## Current invariants

### I1. Yach UI does not speak Pi RPC directly

- **Source:** `../../PRD-v0.1.md`, `../status/m0-m1-checkpoint.md`
- **Rationale:** Keeps UI independent from Pi-specific transport details and makes adapter replacement possible.
- **Current status:** `verified` by M0/M1 checkpoint at the architecture level.
- **Would violate:** UI code importing or serializing Pi RPC commands directly instead of going through `yach-proto`/adapter boundaries.

### I2. `yach-proto` is the UI/adapter seam

- **Source:** `../../PRD-v0.1.md`, `../protocol/yach-proto-v0.md`
- **Rationale:** Yach owns its capability model, typed events, and future adapter compatibility.
- **Current status:** `implemented-unverified` as a v0 current contract; not a final stability promise.
- **Would violate:** Adding UI/backend features with no protocol representation when they cross the boundary.

### I3. Minimal core, maximal customization

- **Source:** `../../PRD-v0.1.md`
- **Rationale:** Yach should preserve Pi’s hackability while improving performance and architecture.
- **Current status:** `planned` principle; needs repeated validation as features accumulate.
- **Would violate:** Making Rust-native plugins the only customization path or forcing every extension into core.

### I4. Process boundaries are intentional

- **Source:** `../../PRD-v0.1.md`
- **Rationale:** Out-of-process adapters/plugins keep the core safer, restartable, and language-agnostic.
- **Current status:** `in-progress` via Pi RPC process boundary and planned SDK sidecar/native plugin paths.
- **Would violate:** Tight in-process coupling that makes adapters/plugins non-restartable without an explicit decision.

### I5. Pi compatibility is measured, not hand-waved

- **Source:** `../../PRD-v0.1.md`, `compatibility.md`
- **Rationale:** Phase 1 succeeds only if existing Pi setup/session/extension behavior is actually preserved enough to matter.
- **Current status:** `in-progress`; M0/M1 checkpoint covers some Tier A RPC surfaces, but resource/session/rich parity remain to prove.
- **Would violate:** Declaring compatibility complete without evidence links or explicit unknowns.

### I6. Tail latency evidence gates the Rust-shell thesis

- **Source:** `../../PRD-v0.1.md`, `performance-evidence.md`
- **Rationale:** The project only justifies Phase 2 if the Rust shell feels materially better or proves cleaner architecture with real performance wins.
- **Current status:** `planned`/partial; baseline protocol benchmarks exist, but full UI/Pi comparison SLOs remain.
- **Would violate:** Proceeding to native backend work based on “Rust should be faster” rather than evidence.

### I7. Native backend starts after enough Phase 1 evidence, not exhaustive parity

- **Source:** `../../PRD-v0.1.md`, `decisions.md#d20260427-01--stop-chasing-exhaustive-pi-backend-parity-before-native-backend-work`
- **Rationale:** The Pi-shaped shell should prove enough value and compatibility lessons to guide native work, but the stock Pi backend path is temporary and should not absorb exhaustive feature-parity effort.
- **Current status:** `accepted` architecture evolution.
- **Would violate:** Either building native provider/session/tool/plugin systems with no compatibility/performance evidence at all, or delaying native backend planning solely to chase minor Pi-backend-only parity gaps.

### I8. File-first configuration and resources stay first-class

- **Source:** `../../PRD-v0.1.md`
- **Rationale:** Pi’s settings, packages, skills, prompts, themes, and context files are central to the product identity.
- **Current status:** `planned`; not proven by the M0/M1 checkpoint.
- **Would violate:** Designing phase-1 behavior that ignores or replaces Pi’s file/resource surfaces without a compatibility decision.
