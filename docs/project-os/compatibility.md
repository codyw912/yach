# Compatibility Tracker

This tracker makes Pi parity explicit. It is an index of status and evidence, not a replacement for detailed plans, checkpoints, or test reports.

Last updated: 2026-04-27

## Status rules

- Keep implementation status separate from evidence status when they differ.
- Link evidence before using `verified`.
- Use `unknown` instead of guessing.
- Use `blocked` only with a named blocker.

## Phase 1 compatibility matrix

| Area | PRD ref | Category / tier | Adapter path | Implementation status | Evidence status / link | Blocker / unknown | Next action |
|---|---|---|---|---|---|---|---|
| Prompt streaming | §6.4 | Tier A stock RPC | RPC | `verified` at adapter and M2 TUI basic loop | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md` | Stream/local-cancel hardening is implemented and manual smoke passed; broader dogfood/perf evidence remains | Track user-perceived latency under performance evidence. |
| Dialogs: select/confirm/input/editor | §6.4 | Tier A stock RPC | RPC | `verified` at adapter and M2 TUI basic loop | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md` | Confirm/input/select/editor passed manual `tui-dialog-smoke`; organic backend dialog coverage remains limited | Keep broader extension/dialog compatibility in M3 evidence pass. |
| Notifications/status/widgets/title | §6.4 | Tier A stock RPC | RPC | `verified` for mapped dispatch actions; `implemented-unverified` for richer TUI presentation | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md` | Rich component-backed UI still Tier B; `/help` now has a readable overlay instead of status-only text | Keep tracker split between stock RPC and rich UI. |
| Editor text updates | §6.4 | Tier A stock RPC | RPC | `planned` | `../status/m0-m1-checkpoint.md` marks omitted | Needs protocol/UI surface | Plan with protocol updates when prioritized. |
| Model/session switch/fork/stats/export | §6.4 | Tier A stock RPC | RPC | `partial` overall; model selection `verified` for M2 TUI basic loop | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md` | Model list uses stock RPC `get_available_models`, `set_model` provider/modelId shape, backend-confirmed status updates, and manual TUI smoke. Ctrl+F maps current-branch duplication to stock RPC `clone`; picker/session tree still incomplete | Track broader session list/tree/fork compatibility under M3. |
| Settings/resources/packages | §6.1 | Resource compatibility | RPC / SDK sidecar TBD | `planned` | No evidence yet | Needs ResourceLoader/setting parity work | Track under M3 plan. |
| Existing Pi session files/tree | §6.2 | Session compatibility | RPC / SDK sidecar TBD | `planned` | No evidence yet | Needs session-file compatibility design | Track under M3 plan. |
| Logic suite examples | §6.5 | Canonical logic suite | RPC/SDK depending on surface | `planned` | No evidence yet | Suite harness not created | Define suite during M3. |
| Rich UI suite examples | §6.5 | Tier B rich UI | SDK sidecar | `deferred` | No evidence yet | SDK sidecar/rich protocol not built | Track under M4. |

## Entry template

Use `templates/compatibility-entry-template.md` when adding rows that need more detail than the matrix can hold.

Required fields:

- Area
- PRD reference
- Category/tier
- Adapter path
- Implementation status
- Evidence status/link
- Blocker/unknown
- Next action
