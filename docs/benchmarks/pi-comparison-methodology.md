# Pi Comparison Methodology

Yach performance work should prefer reliable evidence over flattering numbers. Same-machine Pi comparisons are valuable only when the workload, timing boundary, and environment are explicit enough that the result would still be meaningful if Pi wins.

## Goal

Use Pi comparisons to answer specific engineering questions:

- Where is yach already responsive enough?
- Where does yach have measurable headroom or regressions?
- Which workloads justify optimization work?
- Are yach's product-performance claims grounded in fair same-machine evidence?

Do **not** use comparison benchmarks as marketing unless the methodology is strong enough to survive scrutiny.

## Target hierarchy

Performance targets have two layers:

1. **Product SLOs** — user-facing minimum bars from the PRD. These should remain stable and environment-tolerant.
2. **Engineering regression guards** — stricter internal thresholds based on what yach can currently achieve on controlled workloads. These can evolve as architecture and fixtures improve.

Current interpretation:

| Surface | Product SLO | Current evidence | Suggested engineering guard stance |
|---|---:|---|---|
| Startup after backend ready | `<250 ms` | Live synthetic terminal p95 ~2.1 ms, p99 ~2.5 ms | Product SLO is loose for the measured path; use stricter internal guard once live methodology stabilizes. |
| Idle keypress-to-paint | p95 `<16 ms` | Live synthetic keypress-handler-to-draw p95 ~0.9 ms, p99 ~1.5 ms | Aim materially below 16 ms for idle; 16 ms is a minimum UX bar, not the quality target. |
| Active stream keypress | p95 `<32 ms` | Headless sequential replay only | Needs live/backlog-aware methodology before setting stricter guard. |
| Heavy tool output | p99 `<50 ms` | Headless compact-output replay only | Needs live/tail methodology before setting stricter guard. |
| Huge transcript viewport | avoid full-buffer behavior | Headless 10k-entry viewport p95 ~17.7 ms | Current risk signal; set scale-aware guard after live scroll/resize characterization. |

## Fair Pi baseline invocation

The user's Pi installation has extensions configured. Pi comparisons should start from a clean Pi invocation so extension/package overhead does not contaminate the baseline.

Use clean flags unless a report explicitly justifies otherwise:

```sh
pi \
  --no-extensions \
  --no-skills \
  --no-prompt-templates \
  --no-themes \
  --no-context-files \
  --offline
```

For RPC-mode comparisons:

```sh
pi \
  --mode rpc \
  --no-extensions \
  --no-skills \
  --no-prompt-templates \
  --no-themes \
  --no-context-files \
  --offline
```

Notes:

- `--no-extensions` disables extension discovery. Do not pass explicit `-e` flags in clean comparisons.
- `--no-skills` disables skill discovery/loading.
- `--no-prompt-templates` disables prompt-template discovery/loading.
- `--no-themes` disables theme discovery/loading.
- `--no-context-files` avoids `AGENTS.md` / `CLAUDE.md` discovery.
- `--offline` avoids startup network operations; use it for local UI/render benchmarks. If a workload intentionally measures real startup behavior including network/provider checks, omit `--offline` and document why.

Record `pi --version` and the full `pi --help` relevant flags when methodology changes.

## Measurement classes

Use these labels consistently in reports:

| Class | Use | Claim strength |
|---|---|---|
| `headless proxy` | Deterministic yach app/event/render into test backend | Regression/component evidence only. |
| `live terminal` | yach or Pi running against a real terminal backend/TTY | Candidate user-perceived evidence if timing boundary is clear. |
| `PTY harness` | Controlled pseudo-terminal automation for yach/Pi | Good for same-machine comparison if first-frame/input boundaries are robust. |
| `RPC/process` | Process startup, RPC readiness, protocol event handling | Backend/adapter evidence, not TUI paint evidence. |
| `manual/no-data` | Human-observed or blocked methodology | Useful for notes, not performance claims. |

## Fairness rules

A Pi comparison is not valid unless the report records:

- yach command and Pi command, exactly as run;
- yach commit SHA and Pi version/package path;
- machine/OS/terminal/PTY environment;
- viewport size;
- sample count;
- warmup/cooldown policy;
- timing start and stop boundaries;
- fixture size and shape;
- excluded phases;
- whether the workload is equivalent, approximate, or intentionally asymmetric;
- raw output or artifact path.

A comparison should be designed so either system can win. If the harness uses an internal yach-only shortcut, label it as yach-only evidence, not Pi comparison evidence.

## Anti-gaming rules

Avoid benchmarks that:

- measure yach internals against Pi end-to-end behavior;
- include the user's Pi extensions unless the comparison is explicitly "configured Pi";
- use different terminal sizes or fixture sizes;
- count yach headless render time against Pi live terminal time;
- rely on human stopwatch timing;
- discard outliers without a predeclared rule;
- compare model/provider/network latency when the claim is about TUI responsiveness;
- optimize for a synthetic fixture before showing that fixture represents real dogfood pain.

If exact equivalence is impossible, publish the limitations and use cautious wording.

## Candidate first comparisons

### 1. Startup / first interactive frame

Question: after process invocation, how quickly does each UI reach a usable prompt under clean local startup settings?

Pros:

- Easy to understand.
- User-visible.

Risks:

- First-frame detection in terminal output can be brittle.
- Pi may do startup work yach does not yet do, or vice versa.
- Provider/model setup and session discovery may dominate unless carefully excluded.

Recommended status: methodology prototype only until first-frame detection is robust.

### 2. Idle input-to-draw in a PTY

Question: once ready, how quickly does the UI react to typed characters?

Pros:

- Directly maps to perceived editor responsiveness.
- Current yach live-terminal evidence already suggests strong performance.

Risks:

- Need reliable event injection and draw/flush detection for both systems.
- Terminal output diffing can be noisy.

Recommended status: good candidate if PTY harness can observe redraw completion consistently.

### 3. Large transcript scroll/render

Question: how does each UI behave with large session history and viewport changes?

Pros:

- Yach has an early risk signal here.
- Likely meaningful for real long coding sessions.
- Avoids model/provider/network latency if using fixture sessions.

Risks:

- Need equivalent session/transcript fixtures for Pi and yach.
- Session file formats differ.
- UI representations may not render identical content.

Recommended status: best engineering comparison once fixture generation is solved.

### 4. Heavy tool output

Question: how do tails behave when a large tool result arrives or is expanded/collapsed?

Pros:

- Real coding-agent workload.
- Tail latency matters.

Risks:

- Pi and yach may summarize/collapse tool output differently.
- Expansion state and rendering semantics must be documented.

Recommended status: second wave after transcript/input methodology.

## Claim wording

Use precise language.

Acceptable:

> In a clean same-machine PTY startup harness with extensions disabled, yach measured p95 X and Pi measured p95 Y for the specified timing boundary. The workload excludes provider/model latency.

Acceptable:

> This result is approximate because Pi and yach expose different readiness signals. It should guide methodology, not product claims.

Avoid:

> Yach is faster than Pi.

Avoid:

> Rust proves yach is faster.

Avoid:

> Pi is slow.

## Report template additions

Pi comparison reports should include all standard benchmark report fields plus:

- Pi clean flags used.
- Pi version and package path.
- Whether extensions/skills/templates/themes/context files were disabled.
- Equivalence assessment: `equivalent`, `approximate`, `asymmetric`, or `unsupported`.
- Fairness risk notes.
- Claim wording that the evidence supports.

## Current decision

For now, P6 should continue with methodology-first comparisons. Do not publish a broad yach-vs-Pi claim until at least one comparison uses a clean Pi invocation, a documented timing boundary, stable fixture generation, and raw artifacts.
