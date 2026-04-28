# jcode Comparator Evaluation

Date: 2026-04-28  
Status: evidence review / no local benchmark run  
Source: `https://github.com/1jehuang/jcode`

## Summary

`jcode` is a relevant yach comparator, not a dependency candidate. It is a Rust coding-agent harness with a strong multi-session and performance/resource-efficiency pitch, explicit provider crates, desktop/mobile/client ambitions, memory architecture, and published startup/input/memory comparison numbers against Pi and other tools.

This pass did not force-clone the full repository or run/install jcode binaries. The repo is large enough that the fetch tool reported API-only content by default, and the plan requires human approval before force-cloning or executing third-party code. Findings below are therefore an evidence review and benchmark-design input, not measured yach-vs-jcode performance evidence.

## What was inspected

- GitHub repository API/readme view for `https://github.com/1jehuang/jcode`.
- Raw docs fetched from the repository:
  - `docs/TERMINAL_BENCH.md`
  - `docs/MEMORY_ARCHITECTURE.md`
  - `docs/PROVIDER_SESSION_SHARED_CONTRACT_AUDIT.md`
  - `docs/MULTI_SESSION_CLIENT_ARCHITECTURE.md`
- Yach comparator methodology:
  - `docs/benchmarks/README.md`
  - `docs/benchmarks/pi-comparison-methodology.md`
  - `docs/project-os/performance-evidence.md`
  - `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`

## jcode claims and surfaces worth tracking

### Published performance claims

The README presents jcode as materially faster and more memory-efficient than several CLI agents. Visible claims include:

| Surface | jcode claim from README | Comparator value to yach | Current confidence |
|---|---:|---|---|
| Time to first frame | ~14.0 ms | Directly relevant to startup/readiness methodology. | Published claim only; needs local reproduction before indexing as evidence. |
| Time to first input | ~48.7 ms | Relevant to yach startup-to-interactive and first usable prompt surfaces. | Published claim only; timing boundary needs review. |
| 1 active session memory | 27.8 MB with local embedding off; 167.1 MB with embedding on | Relevant to native backend/multi-session memory goals. | Published claim only; measurement method likely PSS but needs exact reproduction details. |
| 10 active sessions memory | 117.0 MB with local embedding off; 260.8 MB with embedding on | Highly relevant to multi-session scaling. | Published claim only. |
| Extra PSS per added session | ~9.9 MB embedding off; ~10.4 MB embedding on | Useful target shape for yach future multi-session work. | Published claim only. |

### Architecture surfaces

Visible jcode workspace structure includes provider and runtime crates:

- `crates/jcode-agent-runtime`
- `crates/jcode-provider-core`
- `crates/jcode-provider-gemini`
- `crates/jcode-provider-openrouter`
- `crates/jcode-provider-metadata`
- `crates/jcode-desktop`
- `crates/jcode-tui-workspace`
- `crates/jcode-mobile-core`

The provider/session boundary audit is especially relevant. It argues against prematurely extracting high-churn `Provider` / `EventStream` and full session/runtime modules, preferring small serde-only shared contracts and narrow provider identity/selection support crates first. This aligns with yach's current caution: keep `yach-backend` seams small until fixtures and dogfood paths prove the shape.

## Benchmark equivalence map

| jcode claim/workload | Closest yach surface today | Equivalence assessment | Notes |
|---|---|---|---|
| Time to first frame | `yach-tui-startup-report`, `yach-tui-ready-startup-report` in `crates/yach-bench` | Approximate / methodology prototype | Existing yach reports already warn first byte is not first stable frame/readiness. Need define whether jcode's first frame is first PTY output, first complete frame, or first stable prompt. |
| Time to first input | `yach-tui-ready-startup-report` plus possible input probe | Approximate / incomplete | Yach has startup reports and keypress reports, but first typed probe appearing on screen is a distinct boundary. Could add a PTY input-probe harness if warranted. |
| 1-session PSS | No dedicated yach memory harness in current benchmark docs | Unknown / new harness needed | Existing yach performance evidence is latency-heavy. Memory benchmarking needs process tree/PSS method and clear backend/provider settings. |
| 10-session PSS | No dedicated yach multi-session memory harness | Unknown / future native-session benchmark | Current yach is not yet a native multi-session backend; comparing now may be strategically useful but product-asymmetric. |
| Extra PSS per added session | No dedicated yach multi-session memory harness | Unknown / future native-session benchmark | More relevant after native runner supports multiple sessions without Pi sidecar dependence. |
| Terminal-Bench/Harbor task runs | No yach Terminal-Bench adapter | Out of current scope | jcode docs assume OpenAI OAuth file and model defaults. This is agent-capability benchmarking, not local UI responsiveness. |

## Methodology unknowns before local benchmark claims

Before yach records any jcode comparison in `docs/project-os/performance-evidence.md`, a local benchmark report should answer:

- Exact jcode version/commit and install/run method.
- Whether local embeddings are enabled or disabled.
- Whether startup is cold or warm.
- Exact command used to launch jcode.
- Terminal/PTY harness, viewport size, shell, OS, machine, and sample count.
- Timing start boundary: process spawn, PTY spawn, first byte, first full frame, or first stable prompt.
- Timing stop boundary for first input: key written to PTY, echo observed, full redraw observed, or prompt model updated.
- Whether provider/auth/network setup is disabled, mocked, cached, or live.
- Whether memory measurement uses RSS, PSS, process tree PSS, or another metric.
- Whether multi-session measurement starts independent processes, one workspace process with many sessions, desktop clients, or background services.
- Raw artifacts or logs sufficient to audit the result.

## Architecture observations for yach

### Useful lessons

- jcode's own boundary audit warns against extracting broad provider/session traits too early. This supports yach's current incremental approach: provider request/event/error types first, then fixture pressure, then provider-library choice.
- jcode's provider support crates appear to be leaf-like and support-oriented rather than owning the full runtime. This is consistent with yach's goal of keeping provider libraries/adapters below yach-owned session/tool/resource semantics.
- jcode's memory architecture is async/non-blocking and file-backed under `~/.jcode/memory/`. That is relevant as a future reference for yach's file-first resources and long-lived project/session knowledge, but not part of the current native dogfood runner scope.
- jcode's multi-client ambition (desktop/mobile/server/TUI) is a reminder to keep yach's protocol boundary explicit, but yach should avoid broadening scope into a superapp while native backend basics are still unproven.

### Anti-lessons / scope guards

- Do not adopt jcode's crate structure as a template wholesale. It is solving a broader desktop/mobile/server product shape than yach's current native dogfood milestone.
- Do not treat jcode's provider crates as proof that yach should create many provider crates now. Yach's plan says to split only when concrete adapter consumers justify it.
- Do not compare jcode's mature multi-session memory numbers to yach's transitional Pi-backed architecture as a product verdict. That would be asymmetric until yach has a native runner path.

## Recommended next actions

1. **Do not add any yach dependency on jcode.** Keep it as comparator/reference only.
2. **Ask before force-cloning or running jcode.** The repo is large, and local execution should be an explicit choice.
3. **If approved, start with no-credential local UI benchmarks:**
   - first output / first full frame if detectable;
   - first typed probe visible;
   - process tree PSS for 1 session;
   - process tree PSS for N sessions only if the launch mode is clear and safe.
4. **Create a benchmark report only after local measurements:** `docs/benchmarks/jcode-comparison-2026-04-28.md` or later date.
5. **Index `docs/project-os/performance-evidence.md` only for measured results.** This evidence-review spike should not become a performance claim.
6. **Feed architecture observations into the provider spike:** compare yach's P0 provider seam against jcode's documented warning that full provider/session extraction too early creates churn.

## Supported claim from this report

Supported:

> jcode is a relevant yach comparator for startup/readiness, memory scaling, multi-session architecture, and provider/session seam design. Current evidence is sufficient to justify a comparator benchmark plan, but not sufficient to make measured yach-vs-jcode performance claims.

Not supported:

> yach is faster/slower than jcode.

Not supported:

> yach should use jcode as a dependency or copy its architecture.
