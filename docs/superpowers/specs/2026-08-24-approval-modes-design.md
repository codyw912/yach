# Approval Modes Design

Status: accepted 2026-08-24

## Motivation

Reviewing every non-readonly action spends attention on routine work, trains
habitual approval, and weakens the prompts that should signal a real escalation.
Cohort research is recorded in
`docs/project/records/2026-08-24-approval-modes-cohort-research.md`.

Yach will separate three concerns:

1. **Mode**: the session's baseline posture.
2. **Policy**: deterministic hard denials, restrictions, and user grants.
3. **Reviewer**: user initially; optional auto-review later for unresolved asks.

This slice implements authority provenance plus `review` and `accept-edits`.
`plan`, session-only `full-access`, scoped grants, auto-review, and sandboxing
remain later slices in that order.

## Hard Authority Boundary

Repository content is untrusted input. It may restrict execution, but it cannot
grant execution authority.

- User `~/.yach/config.json` remains authoritative for shell `allow` and
  `env_allow`.
- Project `.yach/config.json` may set restrictive shell runtime bounds, but its
  `allow` and `env_allow` entries are ignored and produce no auto-execution or
  environment-disclosure authority.
- Project mode preference lives in
  `~/.yach/permissions/<project-key>.json`, keyed by the same canonical raw-path
  identity used for user-state sessions.
- `.yach/config.json` and `.yach/permissions*` are protected metadata paths for
  provider edits. A provider cannot rewrite future-session policy.
- Sensitive paths, outside-project writes, extension self-approval, validation,
  and resource-root checks remain earlier hard denials in every mode.

## Modes In This Slice

### `review` (default)

- Read-only tools run automatically.
- Hash-checked workspace edit transactions ask the user.
- Parse-aware user allowlisted commands run; other host commands ask.

### `accept-edits`

- Read-only tools run automatically.
- Hash-checked workspace edit transactions apply without user review.
- Host commands retain the same allowlist/ask behavior as `review`.

`accept-edits` covers Yach edit transactions only. It does not classify shell
filesystem commands as edits.

## Protocol And State

`ApprovalMode` is a protocol enum. The client selects a mode with a correlated
client event; the backend publishes the active mode with a server event. The
capability is negotiated explicitly.

The backend owns the active mode. Each provider tool round receives an immutable
snapshot, so a mode change affects future rounds and never retroactively changes
a pending review. Successful selection persists the non-dangerous mode to the
project-keyed user store. Startup loads that store or defaults to `review`.

Session evidence records mode changes. Tool permission evidence continues to
record the concrete decision and reason.

## TUI

- `/approval` opens a selector containing `review` and `accept-edits`.
- `/approval <mode>` may select directly if the command parser already carries
  arguments cleanly; otherwise the selector is the single UI path.
- The active mode is always visible in the status surface and `/status`.
- Mode changes are disabled while another modal surface owns input.
- `accept-edits` is visually distinct but not presented as sandboxed.

## Headless And RPC

Existing `--full-auto` remains an explicit headless client behavior until the
later full-access slice unifies it with protocol modes. Unresolved interactive
review in headless mode still fails closed.

RPC clients negotiate the approval-mode capability and may select `review` or
`accept-edits`. Clients without the capability retain `review` behavior.

## Failure Behavior

- Missing or malformed user mode state: warn and use `review`.
- User-state persistence failure: do not change the active mode; return a
  correlated failure/status.
- Invalid/unsupported mode: reject without changing state.
- Project config attempting `allow` or `env_allow`: ignored for authority. The
  initial slice records this in tests; a user-facing warning may be added if it
  can be surfaced without startup noise.

## Verification

Required behavior:

1. A cloned project cannot auto-run a command through project `shell.allow`.
2. A project cannot expose environment variables through project `env_allow`.
3. Provider edits cannot modify `.yach/config.json`.
4. A new project starts in `review`.
5. `review` produces an edit review and does not write early.
6. `accept-edits` applies the same edit without a review event.
7. Non-allowlisted bash still asks in both modes.
8. Mode preference is isolated by canonical project key and stored privately.
9. The TUI shows and switches the active mode.
10. RPC invariant coverage proves selection correlation and behavior.
