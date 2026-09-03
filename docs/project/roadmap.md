# Yach Roadmap

## Vision

Yach is a minimal, extensible Rust-native coding harness for sustained local
software work. It should combine an effective agent loop, truthful durable
state, calm terminal interaction, and explicit user-owned authority while
keeping canonical state, policy, execution brokerage, context accounting, and
protocol semantics in a small Yach-owned kernel.

The foundation is strong, but Yach is not yet close to the usability bar for
daily work. This roadmap advances through observed outcomes: structured
dogfood first, then long-session correctness, end-to-end effectiveness,
governed daily operation, usage-ready extensibility, and independent use.

## Non-goals

- Treating the historical native MVP declaration as evidence of current
  daily-work readiness.
- Matching every feature of another harness or supporting every provider and
  tool-call dialect.
- Making a remote daemon, hosted service, or multi-client operation part of
  this roadmap arc.
- Claiming OS sandboxing or making an OS sandbox guarantee a readiness gate.
- Building an extension marketplace, package registry, or arbitrary
  client-side UI injection system.
- Requiring built-in multi-agent orchestration, model roles, or plan mode;
  public contracts should leave these feasible as later product or extension
  work.
- Treating crates.io publication, a version number, or release ceremony as the
  product outcome.
- Treating manual use as the only proof when deterministic automation or evals
  can establish the contract more reliably.

## Milestones

### M1: Evidence-driven dogfood loop

**Outcome:** Representative real coding sessions produce trustworthy,
reproducible evidence about effectiveness, resilience, interaction quality,
autonomy, and coverage. Priorities come from observed blockers rather than
feature comparison.

**Done when:** An approved, versioned dogfood portfolio record defines the
repositories, task classes, provider families, failure injections,
automation/manual split, evidence format, failure classes, blocker severity,
and re-run gate. It covers fresh start, multi-tool work that changes more than
one file and runs verification commands, review and autonomous modes, context
pressure, provider failure, cancellation, restart and resume, and at least two
provider families. At least one recorded run classifies every failure as
reproducible, intermittent-valid with a recorded repeat policy, or
provider-invalid, and every blocker in the first recorded severity cohort is
resolved and re-run against the same portfolio.

**Status:** active
**Specs:** —
**Plans:** —

### M2: Long-session correctness

**Outcome:** Yach preserves useful, truthful state through long turns, context
pressure, provider and tool failures, interruption, and restart.

**Done when:** Sustained-session scenarios prove oversized-turn handling,
mid-turn overflow recovery, re-compaction continuity, honest usage and meter
state, graceful tool-budget exhaustion, cancellation, crash and restart, and
resumed provider/transcript parity without manual log repair or silent evidence
loss.

**Status:** planned
**Specs:** —
**Plans:** —

### M3: Effective end-to-end coding

**Outcome:** The agent can complete substantial repository changes inside Yach:
understand the codebase, plan, edit, run commands, inspect failures, verify
behavior, and deliver a grounded result.

**Done when:** A representative cross-language task portfolio completes
entirely through Yach on multiple real repositories and provider families. It
covers symbol-aware navigation where available, multi-file changes,
test/debug loops, recoverable malformed or stale tool calls, and verifiable
final outcomes without switching harnesses to finish the task.

**Status:** planned
**Specs:** —
**Plans:** —

### M4: Calm, governed daily operation

**Outcome:** Daily interaction is understandable and low-friction while
authority remains explicit, auditable, and user-owned.

**Done when:** Named scripted TUI scenarios and recorded manual checks pass for
progress, active state, reviews, failures, tool evidence, context, models,
sessions, and recovery with explicit pass conditions. Review-first remains the
default. A recorded scenario switches into and out of the advanced unsandboxed
autonomy tier without editing files, and its label, status, and evidence state
that Yach provides no OS isolation. The approved settings design is implemented
and exercised: recorded scenarios show global, project, and session values
with explicit ownership, precedence, persistence, migration, and revocation,
and repository content cannot grant host authority.

**Status:** planned
**Specs:** —
**Plans:** —

### M5: Usage-ready extension platform

**Outcome:** Yach's extension-first posture is usable by someone outside the
core codebase while the kernel retains policy, state, evidence, and execution
invariants.

**Done when:** A developer can author, install, configure, diagnose, upgrade,
disable, and remove a non-bundled local extension using documented versioned
contracts and conformance tests. First-party extensions use the same public
contracts as external extensions for every surface the external-extension
proof exercises; other exceptions require the accepted posture spec's named
reasons: bootstrap dependency, security authority, canonical-state ownership,
transport semantics, or measured performance. Project trust and failure
isolation are explicit, and TUI and stdio RPC observe equivalent extension
semantics. This does not require a marketplace, package registry, arbitrary UI
injection, or OS-sandbox claim.

**Status:** planned
**Specs:** —
**Plans:** —

### M6: Independent usage readiness

**Outcome:** Yach is clean and dependable enough for actual use beyond its
author without turning release mechanics into the product goal.

**Done when:** All three proofs hold with recorded evidence:

1. Sustained owner adoption: a recorded observation window defined in the M1
   portfolio shows substantial coding tasks completed through Yach, with no
   task abandoned to another harness because of a Yach defect.
2. Fresh-user success: a technical user with no prior Yach involvement
   installs Yach, configures a supported provider, completes and resumes a
   portfolio task, and diagnoses one injected failure using only shipped
   documentation and Yach output.
3. External extension success: a developer outside the core codebase completes
   the M5 extension path and records the contracts, tools, and documentation
   used.

Distribution builds from publishable dependencies or an explicitly owned,
published provider layer, and the normal deterministic plus pinned-live release
gate passes. Automation remains architecturally supported through the protocol
and invariant matrix but is not a first-class product requirement in this arc.

**Status:** planned
**Specs:** —
**Plans:** —

## Evidence policy

Automated tests, protocol matrices, deterministic evals, and scripted provider
scenarios are the default proof for behavior they can observe. Manual evidence
is reserved for claims that require a person: interaction quality, sustained
owner adoption, fresh-user independence, and external extension usability.
Live provider failures that cannot vote on product correctness must be
classified explicitly rather than silently retried into a passing result.

## Decisions

- 2026-09-03 — Optimize first for an owner-grade daily harness, then independent
  actual usage — the architecture is credible, but the product remains far
  below the daily-work usability bar.
- 2026-09-03 — Use an outcome-gated, dogfood-first sequence — repeated real work
  should determine capability and usability priorities instead of speculative
  feature parity.
- 2026-09-03 — Make a usage-ready extensible local harness the destination —
  the repository is already public, so readiness is demonstrated use rather
  than a formal public-launch milestone.
- 2026-09-03 — Keep automation architecturally viable but not a first-class
  feature requirement — TUI and non-TUI clients must share protocol and runtime
  semantics without forcing daemon or hosted-product scope.
- 2026-09-03 — Keep review-first and advanced unsandboxed autonomy as explicit
  tiers — review fatigue weakens practical safety, but Yach must not imply an OS
  isolation guarantee it does not provide.
- 2026-09-03 — Defer autonomy persistence and precedence decisions to a broader
  settings design — current session-only full-access reset remains authoritative
  until configuration ownership, scope, migration, and revocation are designed
  together.
- 2026-09-03 — Require automated and human completion proof — deterministic
  contracts should not wait on manual use, while automation cannot substitute
  for sustained adoption, fresh-user independence, or external extension use.

## Open questions

- What representative repositories, task classes, provider families, and
  failure injections form the M1 portfolio?
- Which measured M1 blocker should become the first focused design and plan?
- What configuration model should govern global, project-specific user, and
  session values, especially security-sensitive authority preferences?
- Which symbol-aware, debugging, planning, and verification capabilities prove
  necessary for M3 rather than merely matching another harness?
- Which extension contribution surfaces are required for one useful external
  extension, and which remain intentionally deferred?
- Does removing the vendored Rig release block require upstream convergence or
  an owned published provider layer?
