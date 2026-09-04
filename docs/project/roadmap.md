# Yach Roadmap

Product direction for Yach. This file is a read-only mirror: detailed
milestone gates, status, and sequencing are decided in an external planning
system and reflected here by the maintainer. Do not edit it directly, and do
not recreate in-repo planning files (status boards, next-work queues)
alongside it.

## Vision

Yach is a minimal, extensible Rust-native coding harness for sustained local
software work. It combines an effective agent loop, truthful durable state,
calm terminal interaction, and explicit user-owned authority while keeping
canonical state, policy, execution brokerage, context accounting, and protocol
semantics in a small Yach-owned kernel.

The foundation is strong, but Yach is not yet close to the usability bar for
daily work. The roadmap advances through observed outcomes: structured
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

1. **Evidence-driven dogfood loop** — Representative real coding sessions
   produce trustworthy, reproducible evidence about effectiveness, resilience,
   interaction quality, autonomy, and coverage. Priorities come from observed
   blockers rather than feature comparison.
2. **Long-session correctness** — Yach preserves useful, truthful state
   through long turns, context pressure, provider and tool failures,
   interruption, and restart.
3. **Effective end-to-end coding** — The agent can complete substantial
   repository changes inside Yach: understand the codebase, plan, edit, run
   commands, inspect failures, verify behavior, and deliver a grounded result.
4. **Calm, governed daily operation** — Daily interaction is understandable
   and low-friction while authority remains explicit, auditable, and
   user-owned. Review-first stays the default; an advanced unsandboxed
   autonomy tier is easy to enter and leave and honestly labeled.
5. **Usage-ready extension platform** — Yach's extension-first posture is
   usable by someone outside the core codebase while the kernel retains
   policy, state, evidence, and execution invariants.
6. **Independent usage readiness** — Yach is clean and dependable enough for
   actual use beyond its author: sustained owner adoption, fresh-user success,
   and one external extension, without turning release mechanics into the
   product goal.

## Principles

- Automated tests, protocol matrices, deterministic evals, and scripted
  provider scenarios are the default proof for behavior they can observe.
  Manual evidence is reserved for claims that require a person.
- Review-first and advanced unsandboxed autonomy are explicit tiers. Review
  fatigue weakens practical safety, but Yach must not imply an OS isolation
  guarantee it does not provide.
- Automation stays architecturally viable: TUI and non-TUI clients share
  protocol and runtime semantics, without forcing daemon or hosted-product
  scope.
- Autonomy persistence and precedence are decided by a broader settings
  design, not ad hoc; until then, session-only full-access reset is
  authoritative.

## Design and history

- `docs/superpowers/specs/` — accepted design documents.
- `docs/superpowers/plans/` — implementation plans that executed those specs.
- `docs/project/records/` — dated research, measurements, and decisions.
