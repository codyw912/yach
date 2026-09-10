# Harness Playbook Review (2026-09-08)

Review of Can Bölük's "The Harness Playbook" (2026-09-02,
https://stencil.so/blog/harness-playbook) against yach's current code and
accepted specs. Trigger: omp, Pi, OpenCode, and OpenClaw are all mid-rewrite
after first-generation learnings; omp pioneered many harness concepts and
was widely used, so the postmortem half of the article is evidence worth
holding yach's next milestones against. This record captures what the
article claims, what yach actually does, and the constraints future
features should be held to. It is evidence, not a plan: outcomes and
sequencing live in the external planner.

Method: two read-only mapping agents (article structure; yach direction
docs), then six read-only code audits, one per article chapter, each
citing `path:line`. Article text for the State chapter and Appendix A was
verified against the primary source; other chapters rely on the structural
map. Three findings were corrected during review after checking accepted
specs; the corrected versions appear here.

## The article in brief

Thesis: a harness is systems software, not a while loop around a fetch.
It owns an authoritative world, journals changes, runs untrusted actions,
replicates state to views, schedules actors, adapts incompatible
protocols, and renders in real time. Unavoidable complexity needs an owner
("embrace suffering"); first-generation harnesses pushed it onto
extensions and users.

Five consequences organize the playbook: one authoritative session; a
trusted control plane; bounded work; explicit compatibility; views are
projections. Four architecture tests (multiplexed workspace, remote
driver, spectator, "Factorio" autonomous fleet) are proposed as the design
envelope every subsystem must survive.

Strongest evidence: Appendix A audits 78 official Pi extension examples;
of 17 stateful ones, two were correct. Every failure has the same shape:
the journal's leaf pointer moves while some other authority (closure
counter, live registry, transient map, whole-file scan) resets or
survives arbitrarily.

Thin spots (assessment): omp² implementation status is unspecified, so the
proposed solutions (XML DOM, Director stack, job primitive, `dyn`) are
designs, not proven systems. The five-tool roster benchmark is one task.
Language arguments are experiential. Nothing below depends on trusting
omp²'s solutions; it depends on the failure evidence, which is concrete.

## Chapter-by-chapter status

Status vocabulary: matches / partial / absent / by-design-different
(yach's accepted spec deliberately chose otherwise).

### State: one authoritative session

| Article claim | yach | Evidence |
|---|---|---|
| State derivable from journal alone | partial | `session.rs:316-457` (18 event variants, append-only) |
| Tree/branch structure with leaf pointer | absent | `parent_entry_id` exists but user entries get `None` (`session.rs:730-748`); no leaf pointer |
| Fork / rewind | absent | fork UI wired to backend stub (`runner.rs:2487-2492`) |
| Controller/actor separation | matches | protocol-only UI seam; headless/RPC share semantics |
| Extension state journaled | absent | no event variant; `tool.invoke` carries id/name/args only (`extension.rs:283-302`) |

Inventory of state affecting the next request that is not rebuilt from
the journal on resume (full table in the audit): extension host state
(process memory, survives session switch in one process); static-context
bytes (summary only; files re-read per request, by accepted spec
`2026-05-13-native-static-context-design.md:131-136`); tool catalog and
shell policy (rebuilt per turn); approval mode (project file authoritative
by accepted spec `2026-08-24-approval-modes-design.md:33-35,64-71`;
journaled changes are evidence); native Responses replay suffix (memory;
only checkpoint artifact persists); in-flight turn, streamed text, pending
edit transaction, retry counters (memory; appended only on completion).

Corrected classification: yach has **zero verified two-authority bugs
today**. Approval mode and static context are specified behavior;
extension state has no session-state contract to violate; the rest are
crash-window losses from append-on-completion. What survives is
structural: the journal is authoritative for history, not for the next
request. Fork, rewind, subagents, and stateful extensions all require the
latter. Yach avoids Appendix A because those features do not exist yet.

### Runtime: trusted host, bounded work

| Article claim | yach | Evidence |
|---|---|---|
| Policy on host; sandbox is a bounded stub | partial | host-side permission engine; executor is `bash -c` with user privileges; `CommandExecutor` seam exists, only `host` accepted (`shell.rs:294-327`, `runner.rs:7790-7798`) |
| One lifecycle object per tool call | partial | provider `call_id` → `tool-request-N-M` → permission/review/preview/transaction IDs; streaming arg deltas ignored (`runner.rs:4172-4185`) |
| Edit preview/apply do not duplicate work | partial | `PreparedEditTransaction` carries after-content preview→apply (good); file read at normalize, preview, and twice at apply (`edit.rs:976-1048`; pre-publish recheck is spec-required TOCTOU defense) |
| Output bounded once, opt-out, spill to artifact | absent | 11 independent sites; no spill; no opt-out; edit diff bounded in engine and again in projection (`agent_edit_tools.rs:966-1014`) |
| One job primitive for long-running work | absent by spec | background/PTY are v1 non-goals (`shell-execution-design.md:82-93`) |
| Cancellation has a kill boundary | partial | bash: own process group, SIGKILL on drop (`shell.rs:339-356,425-453`); extensions: `invoke_tool` has no token, host killed only on drop/reload (`extension.rs:1775-1853`) |

### Control plane: values and behaviors

| Article claim | yach | Evidence |
|---|---|---|
| One typed declaration per setting (ConVar) | absent | ≥11 store families; shell/files/compaction each own struct+loader over `config.json`; thinking has three readers (`user_config.rs:12-25`, `shell.rs:93-116`, `thinking_config.rs:31-67`) |
| Inherit to children by default | n/a | no subagents |
| Binds/aliases/profiles in-band | absent | static slash table, hardcoded key match (`slash_commands.rs:22-158`, `app.rs:1961-2025`) |
| Director stack owns cross-turn behavior | absent | only the hardcoded provider loop (`runner.rs:5000-5353`); no plan/goal/todo/verify modes |
| Hooks vs Directors as extension surface | absent | protocol is invoke/resource/edit-proposal/result; posture spec defers hooks, commands, roles (`extension-first-product-posture-design.md:180-240`) |

Yach does not reproduce omp's "same mode check at six entry points"
because it has no modes. Busy guards duplicated between TUI and backend
are transport safety, not policy.

### Inference: compatibility as data

| Article claim | yach | Evidence |
|---|---|---|
| Quirks as data with precedence and `unknown` | partial (good) | zero model-name branches in backend call sites; one catalog rule (`catalog/lib.rs:650-656`); per-field provenance (`Sourced<T>`); capabilities `Option<bool>`, no explicit tri-state |
| Provider is more than `stream` | partial | discovery, usage, retries, OAuth, compaction as five separate seams; no token counting |
| Forced tool calls | absent | Rig supports `tool_choice`; yach never sets it (`rig_adapter.rs:860-905`) |
| Dialect/JSON repair, loop detection, leaked-dialect parsing | absent | reasoning/unknown stream content discarded (`rig_adapter.rs:1331-1334`); invalid args fail the turn (`runner.rs:8068-8131`); cohort record already flagged orphan healing gap |
| Compaction scheduled early from snapshot, branch-valid commit | by-design-different | blocking at 90%; staged-then-commit persistence; no branching (`context-compaction-design.md:34-46`) |
| Shake / Remote / Handoff | matches / matches / absent | masking (`compaction.rs:154-216`); Responses native compactor (`compaction.rs:798-843`) |
| Prompt as fold separate from UI | partial | `provider_messages_from_event_slice` is functionally a fold (`runner.rs:3670-3789`) |

### Tool surface: schema tax and deep builtins

| Article claim | yach | Evidence |
|---|---|---|
| Small permanent roster | matches | 7 builtins, ~3.0 KB / ~750 tokens per request; hashline replaces read+edit without growth |
| `i` intent arg, versioned contracts | absent | advertising is name/description/parameters (`tools.rs:434-446`) |
| Deep `Read` (ranges, dirs, summaries, archives, URLs, internal resources) | absent | project-relative UTF-8 file ≤32 KiB, fails above (`resource.rs:226-315`) |
| Policy-aware in-process Bash | partial | `shell_words` + conservative lexer, argv-prefix allowlist, always `bash -c`; no capability boundaries recognized (`shell.rs:79-167`) |
| Long tail via `dyn` / code surface | absent | no MCP, eval, or discovery |
| AutoQA feedback path | absent | |

### Interface: transcript as protocol

| Article claim | yach | Evidence |
|---|---|---|
| No ANSI strings; one-pass primitive | partial | ratatui `Line`/`Span`; `String`-backed entries; every delta bumps revision → full line-cache rebuild (`transcript.rs:92-105,759-778`) |
| External content sanitized | absent | tool output copied into transcript strings (`app.rs:1200-1209`) |
| Structured tool rows | partial | lifecycle events typed; `ToolResult.output` is `String` + metadata; Wave 2 row fields present, no shared live/resume reducer |
| Block protocol (active/finalized/committed; Mutable vs AppendOnly) | absent as protocol | inline viewport + `insert_before` archive matches V+S model (`app.rs:4116-4123`); no block states, no named resize policy |
| Presentation policy in renderer | partial | semantic color tokens; `Theme` threaded through every component; hardcoded glyphs; no stream pacing |
| Verification protocol | partial | `BenchmarkApp` + `TestBackend` drives the real renderer off-screen (`app.rs:3889-3993`); no machine-readable state export |

## Cross-cutting findings

1. **Yach is clean mostly by omission.** Every "matches" is either a
   deliberate spec decision (typed provider dispatch, host-side
   permission, append-only log, staged compaction commit, process-hosted
   extensions) or the absence of the feature that would expose the
   problem. This is a good position only if the next features are built
   on the right primitives. Milestones 2 (long-session correctness) and
   5 (extension platform) are where that gets decided.

2. **Phase-keyed instead of object-keyed** recurs four times: session
   state appended on completion; tool call correlated by five IDs across
   maps and events; transcript row mutated from one enum variant to
   another; settings reconstructing scope at eleven load sites. The
   article's deepest idea, one authoritative object whose lifecycle is the
   journal, addresses all four.

3. **Bounding at N sites instead of one projection policy.** Execution
   and transport bounds (shell capture, extension frame cap, read size,
   search traversal) are correct and must stay; they prevent host
   exhaustion before a result exists. What is duplicated is the
   model-visible projection: notice wording, visible window, and the edit
   diff bounded twice. Full output should reach an artifact by streaming,
   never by buffering unbounded.

4. **Extension runtime is state-blind and cancellation-blind.** Kill
   boundary exists but is not wired to turn cancellation; no state slot or
   restore hook; no hooks or commands. An external author's first stateful
   extension will reproduce Appendix A example 4 (`dynamic-tools.ts`).

## What not to adopt (yet)

- **XML DOM as representation.** `SessionEvent` + serde is the
  Rust-native form of the same idea. Adopt the fold, not the format.
- **Job primitive, Director stack, `dyn`, small local models, TLA+.** No
  second consumer exists for any of them. Record the constraint; build it
  with the first consumer.
- **Speculative compaction.** Requires branching, which requires the fold.
- **In-process Bash interpreter.** The lexer+allowlist is defensible. The
  article's real point (approve at `git push`, network, outside-workspace
  boundaries) is reachable by extending recognition, not writing a shell.
- **Roster tax.** Already ~750 tokens; not a problem to solve.

## Constraints for future features

The value of this review is in applying it as omp-like features arrive.
Each row is a test the feature's design should pass before implementation.

| When yach adds | Hold it to |
|---|---|
| **Fork / rewind** | Target state must be producible by folding the child's resolved immutable history: a copied prefix, or a lineage-resolved parent prefix plus the child's own events. Materialize inherited state into the child at fork creation, with no runtime dependency on mutable parent state; storage may be prefix copy or lineage plus an explicit `fork_inherited` event (`2026-08-26-model-defaults-session-state-design.md:243-246` already permits either). If any runtime object needs bespoke reset on fork, it is Appendix A. |
| **Stateful extensions** | Journaled state slot with a restore hook; "host may be killed at any turn boundary" enforced in tests; any state outside the slot is undefined across turns. |
| **Subagents** | Same host/sandbox boundary as bash; copy-on-write filesystem view returning a diff; settings seeded from parent live values with no second "inherit" setting; lifecycle through the same job primitive as background shell. |
| **Background shell / dev servers** | One stdio-shaped job (spawn/poll/message/kill/inspect); central blocking budget; output spills through the same artifact path as bounded results. Not a second lifecycle beside `CommandExecutor`. |
| **Plan mode / verify-before-yield / todo reminders** | One public agent-owned primitive that owns candidate yields, journaled as state. A private flag in `runner.rs` checked at N entry points is omp's mode-exclusivity problem. |
| **Hooks / interceptors** (posture spec's deferred plane) | One-turn observe/edit only. Anything retaining control across turns is the primitive above, not a hook. |
| **MCP / dynamic tools** | Behind a stable discovery surface, not permanent schemas; roster changes invalidate provider caches, so measure before adding. |
| **Remote client / spectator / web** | Consumes the journal patch stream. A second state plumbing path means controller and actor are not separated. |
| **New provider** | Zero model-name branches; capability as catalog data with explicit unknown; dialect repair in the adapter, not the loop. Yach already does this; hold the line. |
| **Any new tool result** | One projection policy for what the model sees; execution bound stays with the executor; full output reaches an artifact by streaming. |
| **Static context changes** | Current files apply at request construction (accepted spec). Journal the content hash per turn so evals can detect instruction drift; persist bytes only if exact historical replay becomes a required invariant. |

The pattern across every row: decide the authority and lifecycle object
before building the feature. Retrofitting is what forced omp into a
rewrite.

## Candidate work, in priority order

Not a plan; inputs for the external planner.

1. Define `SessionState = fold(events)` as the only source the runner
   reads when assembling a request. Add turn-lifecycle events (started /
   delta / settled), a static-context hash per turn, an extension-state
   slot, and an environment snapshot per turn. Prove with a property test:
   `fold(log) == state after resume` and `fold(prefix) == state after
   crash at any event boundary`. Prerequisite for fork, milestone 2, and
   honest resume.
2. One model-visible projection policy for tool results, with streaming
   artifact spill and explicit opt-out. Collapse the duplicated projection
   sites (edit diff, shell/search/list notices); keep execution bounds.
3. Wire extension cancellation to the existing process-group kill
   boundary; make host restart at turn boundaries the enforced contract.
4. Structured retryable tool errors instead of turn failure; plumb
   `tool_choice`. Both already flagged in the 2026-07-26 cohort record.
5. Settings consolidation: one typed declaration site for the
   `config.json` family, with scope/persistence declared alongside.
6. Read depth: ranges, directories, and `TooLarge` → bounded read with
   spill. Follows from item 2.
7. Transcript block states, when Wave 3 visual work resumes.

Items 1–3 determine whether milestones 2 and 5 are achievable as
specified. The rest are incremental.

Not prioritized: escape-sanitizing external content at the transcript
boundary. The article cites terminal-injection CVEs, but yach renders
through ratatui cells and does not parse ESC (`transcript.rs:1117-1232`),
so no injection behavior is verified here. This stays contract hardening
— worth an explicit sanitization contract if the renderer ever forwards
strings as terminal programs, or if an executable test demonstrates
control-sequence effects. Not a security fix on current evidence.

## Sources

- Article: https://stencil.so/blog/harness-playbook (Markdown at
  `/index.md`). State chapter `index.md:151-290`; Appendix A
  `index.md:1748-1877`.
- Audits (session-local agent outputs, not durable): `ArticleMapper`,
  `YachDirectionMapper`, `StateAuthorityAudit`, `RuntimeBoundaryAudit`,
  `ControlPlaneAudit`, `ToolSurfaceAudit`, `InferenceAudit`,
  `InterfaceAudit`. All `path:line` citations above were carried from
  those reports; code was not modified.
- Corrections applied during review: approval-mode startup authority is
  specified (`2026-08-24-approval-modes-design.md:64-71`); static-context
  discovery at request time is specified
  (`2026-05-13-native-static-context-design.md:131-136`); extension
  cross-session state is an API gap, not a verified bug; output bounding
  split into execution bounds (keep) and projection policy (centralize);
  fork storage left open per the model-default spec; transcript ESC
  sanitization demoted from prioritized work to unverified contract
  hardening (ratatui renders cells, ESC is not parsed).
