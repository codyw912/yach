# Full-Access Approval Mode Design

Status: accepted and implemented 2026-08-24

## Problem

`accept-edits` removes review friction from Yach-owned hash-checked edit
transactions, but every non-allowlisted `bash` call still pauses for user
review. That is the dominant remaining approval cost in autonomous coding
turns: routine inspection, builds, tests, and version-control commands vary
enough that a narrow static allowlist does not produce uninterrupted work.

Yach has no process or filesystem sandbox. A command run by the host executor
can access paths, processes, credentials, and networks outside the project.
Therefore the next slice cannot honestly offer "workspace-safe automation."
It can offer an explicit, temporary host-access posture for users who accept
that risk.

## Decision

Add `full-access` as a session-only approval mode. In this mode:

- read-only tools run automatically;
- Yach-owned hash-checked edit transactions apply automatically;
- `bash` commands run without ordinary review;
- existing command preparation still validates the working directory, clamps
  timeouts, strips secret-shaped environment variables unless user-authorized,
  uses a separate process group, supports cancellation, and bounds captured
  output;
- structured edit hard denials remain in force;
- no claim is made that bash is confined to the workspace.

`full-access` is the smallest slice that directly removes the observed
autonomous-work blocker. Scoped grants reduce repeated prompts but do not cover
the varied commands in a long autonomous turn. Auto-review adds latency, cost,
and a new false-allow surface before Yach has an evaluation corpus. A sandbox
would change the safety model, but it is a separate, substantially larger
project.

## Authority And Lifetime

`full-access` is ephemeral authority:

- It is never written to `~/.yach/permissions/<project-key>.json`.
- Startup never restores it. A new session starts from the stored non-dangerous
  preference or `review`.
- Leaving and re-entering `full-access` requires a fresh confirmation.
- Repository configuration cannot select or recommend it.
- A provider cannot select it; only a negotiated client event representing an
  explicit user or caller choice can.
- The active mode remains in the backend-owned per-session mode cell. Changes
  affect future tool requests, including later rounds in the current turn, and
  never alter a review already waiting for a decision.
- Selecting another transcript/session deactivates `full-access` before that
  session can submit a prompt. The backend restores the project's stored
  non-dangerous preference, publishes the reset, and records it in the selected
  session. Future new/fork flows must keep the same invariant; a long-lived TUI
  process is not itself the authority lifetime.

The user-state preference schema continues to accept only `review` and
`accept-edits`. Treat a stored `full-access` value as invalid and fall back to
`review` with a warning.

## Honest Safety Boundary

The confirmation must state the actual boundary:

> Commands run directly on this host and may access files outside the project,
> credentials, network services, and other processes. Enable full access for
> this session only?

The choices are `Enable for this session` and `Cancel`; no typed phrase is
required. The status bar and `/status` render `full-access` with the danger
style for the rest of the activation.

Yach path and sensitive-file policy still protects Yach-owned read/edit tools.
It does not constrain an arbitrary shell command. Environment stripping,
project-root default cwd, and timeout/cancellation behavior are mitigations,
not isolation.

## Policy Resolution

Move bash approval routing out of the runner branch into one deterministic
mode-aware decision function. Its result is one of:

- `Allow(UserAllowlist)` when the existing parse-aware user allowlist matches;
- `Allow(FullAccess)` for other commands while the mode is `full-access`;
- `Ask(User)` for other commands in `review` or `accept-edits`;
- `Deny(...)` for an earlier hard policy failure.

The allowlist is evaluated before `full-access` so evidence preserves the most
specific reason. The runner consumes this result; it must not duplicate mode
logic.

Every bash request records durable permission evidence containing the active
mode, policy outcome, and reason before execution or review. Existing tool
request/result evidence remains authoritative for the command and its effects.
A `full-access` allow must not be represented as user-reviewed or allowlisted.

## Protocol And Clients

Extend the existing `ApprovalMode` enum with `FullAccess`; do not add a second
autonomy toggle.

### TUI

- `/approval` includes `full-access` after the two persistent modes.
- Selecting it opens the explicit confirmation.
- `/approval full-access` opens the same confirmation; it must never submit
  `FullAccess` directly or bypass the warning. Direct `review` and
  `accept-edits` commands may keep their current immediate behavior.
- Confirmation sends the existing correlated mode-selection event. Before
  confirmation, no `FullAccess` client event is sent.
- Cancellation leaves the previous mode unchanged.
- A correlated persistence path is not used for `full-access`; the backend
  acknowledges only after validating that the request is session-scoped.
- Switching back to `review` or `accept-edits` follows the existing picker flow
  and persists that non-dangerous preference.

### Headless

Unify `yach run --full-auto` with backend `full-access` instead of having the
headless client auto-approve each review request. The command-line flag is the
caller's explicit authorization for that invocation. The client selects
`full-access` before submitting the first prompt and fails if the backend does
not acknowledge it.

Without `--full-auto`, unresolved reviews continue to fail closed.

### RPC

A negotiated RPC client may select `full-access` explicitly. The selection is
session-only and correlated like other mode changes. Clients that do not
negotiate approval modes cannot enable it.

## Failure Behavior

- A failed or unsupported selection leaves the current mode unchanged.
- A stale or malformed persisted mode never activates `full-access`.
- If permission evidence cannot be persisted, do not start the command.
- Headless mode must not fall back to client-side auto-approval when
  `full-access` selection fails.
- Cancellation while the confirmation is visible leaves the previous mode
  active.

## Scope

This implementation slice includes:

1. `ApprovalMode::FullAccess` protocol compatibility and correlated selection.
2. Backend session-only mode lifetime and persistence rejection.
3. A single mode-aware bash policy decision boundary.
4. Durable bash permission-decision evidence with accurate allow reasons.
5. TUI picker confirmation and danger-state status treatment.
6. Headless `--full-auto` convergence on the same backend mode.
7. RPC, backend, UI, and headless contract tests plus an actual-TUI smoke.

## Non-Goals

- `plan` mode.
- Once/session/persistent command-pattern grants.
- Auto-review or a reviewer model.
- Process, filesystem, or network sandboxing.
- New shell parsing or broader allowlist syntax.
- Persisting `full-access` across sessions.
- Changing environment-variable authority.

These remain separate slices. The recommended order after this work is scoped
session grants, project-keyed user grants, then `plan`; auto-review follows only
after evidence and evaluation gates exist.

## Verification

Required behavior:

1. A new session never starts in `full-access`.
2. Switching transcripts deactivates `full-access` before the selected session
   can submit a prompt and restores the stored non-dangerous preference.
3. Picker selection requires explicit confirmation before activation.
4. Direct `/approval full-access` opens the same confirmation and emits no mode
   selection event before confirmation.
5. Cancelling either entry path leaves the prior mode active.
6. A non-allowlisted bash command asks in `review` and `accept-edits`.
7. The same command runs without review in `full-access`.
8. A pending command review is unchanged by a mid-turn mode switch; later
   commands use the new mode.
9. Leaving and re-entering `full-access` requires confirmation again.
10. Restarting restores only the stored non-dangerous mode.
11. Durable evidence distinguishes allowlist, full-access, user approval,
    rejection, and hard denial.
12. Secret-shaped environment stripping, cwd validation, timeout clamping,
    cancellation, and output bounds still apply in `full-access`.
13. Yach-owned edit hard denials still apply in `full-access`.
14. `yach run --full-auto` selects backend `full-access` before the prompt and
    does not auto-click review events.
15. RPC correlation and capability-negotiation invariants cover the new mode.
16. An actual TUI smoke enters the warning through `/approval full-access`,
    cancels without a mode change, then confirms through the picker and observes
    the visible danger status plus an uninterrupted non-allowlisted fixture bash
    call.

Implementation evidence: protocol, backend, permission-evidence, TUI, headless,
and RPC contract tests cover the required state transitions. An actual TUI
smoke entered the direct warning and cancelled without changing mode, confirmed
through the picker, then used a local OpenAI-compatible provider fixture to
issue `bash` after activation. No review appeared; the command completed and
wrote the expected project file.
