# Approval Modes Cohort Research

Date: 2026-08-24

## Question

What approval postures have current coding harnesses converged on, especially
explicit execution modes and automated approval/review, and which distinctions
must Yach preserve before designing its own replacement for review-everything?

## Yach Baseline

Yach has more policy structure than its current UI exposes:

- `PermissionMode` already models `allow`, `ask`, `deny`, and an unimplemented
  `auto_review` fallback.
- Permission evidence records actor, capability, risk, target, reviewer,
  decision, reason, and user override.
- Workspace edits are hard-wired to `PermissionPolicy::default_local_edit()`,
  whose mode is `ask`.
- Bash commands auto-run only when the parse-aware project/user shell allowlist
  matches; every other command prompts.
- Sensitive paths and outside-project edit paths fail before review.
- Host commands are not sandboxed. Restricting their cwd and environment is not
  filesystem or process isolation.

This produces a single effective interactive posture: reads proceed, while
ordinary edits and non-allowlisted commands stop for user review.

## Cohort

| Harness | Explicit modes | Rules and grants | Automated review | Isolation relationship |
| --- | --- | --- | --- | --- |
| Claude Code | `default`, `dontAsk`, `acceptEdits`, `bypassPermissions`, `plan`, `auto` | Ordered deny, ask, mode, and allow evaluation; scoped path/command rules; mode can change during a session | `auto` uses a model classifier for permission prompts | `acceptEdits` stays workspace-scoped; bypass is explicitly full-system risk |
| Codex | `untrusted`, `on-request`, deprecated `on-failure`, `granular`, `never`; project trust selects the default | Exec-policy safe-command/rule decisions plus permission profiles and turn/session permission grants | `approvals_reviewer = guardian_subagent` routes eligible prompts to a risk-review subagent | Approval policy is evaluated together with read-only/workspace-write/full-access permission profiles |
| Gemini CLI | `default`, `autoEdit`, `plan`, `yolo` | Priority-ordered `allow`, `deny`, and `ask_user` rules by tool, arguments, environment, and active mode | No separate reviewer found; automation is mode/rule based | Mode hierarchy is explicit; plan is read-only and yolo is all-tools |
| OpenCode | ordinary permission policy plus session `--auto` toggle | Per-tool and input-pattern `allow`, `ask`, `deny`; prompts offer `once`, session `always`, or reject | Auto mode converts otherwise-ask decisions to allow; no model reviewer found | External-directory access remains a separate permission; explicit denies survive auto mode |
| OMP | `always-ask`, `write`, `yolo` | Tool-declared read/write/exec tier, argument-sensitive policy, user overrides, critical-pattern safety rules | No model reviewer in the documented path | Write mode auto-approves workspace/session mutations but still prompts for exec; yolo defaults to all tiers |
| Pi | No approval mode in the core agent | Enabled tools execute directly; users can allowlist/disable tool names | None | Sandboxing is an optional extension, not a prerequisite for direct execution |

## Source-Verified Details

### Claude Code

Claude separates mode from rule evaluation. Hooks and hard deny/ask rules run
before the mode; allow rules run after it. `acceptEdits` auto-approves file and
filesystem operations inside the working directory while leaving other tools on
normal policy. `auto` sends permission prompts to a model classifier. Modes can
change during an active streaming session, and `bypassPermissions` still cannot
override earlier deny/ask rules and critical-path safeguards.

Source: <https://code.claude.com/docs/en/agent-sdk/permissions>

### Codex

The current source defines `UnlessTrusted`, `OnRequest`, `Granular`, `Never`,
and deprecated `OnFailure`. Trusted projects default to `OnRequest`; untrusted
projects use `UnlessTrusted`, which only auto-approves known-safe read commands.
Codex treats approval policy and sandbox/permission profile as separate axes.
Its `guardian_subagent` approvals reviewer gathers context and applies a
risk-based framework to prompts that would otherwise reach the user.

Sources:

- <https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs>
- <https://github.com/openai/codex/blob/main/codex-rs/protocol/src/config_types.rs>
- <https://github.com/openai/codex/blob/main/codex-rs/core/src/config/mod.rs>
- <https://github.com/openai/codex/blob/main/codex-rs/core/src/guardian/review.rs>

### Gemini CLI

Gemini's policy engine has four ordered modes: `plan`, `default`, `autoEdit`,
and `yolo`. Rules produce `allow`, `deny`, or `ask_user`, with explicit priority
tiers. Persisted approvals are mode-aware so trust granted in a restrictive
mode flows only to that mode and more permissive modes.

Source: <https://geminicli.com/docs/reference/policy-engine/>

### OpenCode

OpenCode defaults most tools to allow, while sensitive reads, external
directories, and repeated identical calls have stricter defaults. Permission
rules are tool/input-pattern `allow`, `ask`, or `deny`. A prompt offers approve
once, approve matching suggested patterns for the current session, or reject.
`--auto` and a TUI command-palette toggle auto-approve requests that would
otherwise ask; explicit denies remain authoritative and the prompt displays an
`auto` indicator.

Source: <https://opencode.ai/docs/permissions/>

### OMP

OMP exposes three tier-based modes: `always-ask` auto-approves reads, `write`
also auto-approves workspace/session mutations, and `yolo` auto-approves
read/write/exec. Argument-sensitive tool policy and user overrides can still
force prompt or deny. The documented default is yolo; critical bash patterns
and provider computer-use safety checks add separate safeguards.

Source: `omp://approval-mode.md`

### Pi

Pi sessions execute enabled read, edit, write, and bash tools without a core
approval prompt. Users can enable/disable or allowlist tool names, and sandbox
execution is available as an extension rather than a core mode.

Sources:

- <https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/README.md>
- <https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/usage.md>
- <https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/extensions.md>

## Convergence

1. **Modes are explicit and user-visible.** The common shape is a restrictive
   planning/read mode, a prompting mode, an edit/write auto-approval mode, and
   an all-tools mode.
2. **Mode and policy are separate axes.** Hard denies and argument/path rules
   remain authoritative when a more permissive mode is active.
3. **Edit auto-approval is a first-class middle posture.** Claude `acceptEdits`,
   Gemini `autoEdit`, and OMP `write` all distinguish bounded mutations from
   arbitrary execution.
4. **Modes change at session runtime.** Claude supports dynamic mode changes;
   Gemini cycles modes; OpenCode toggles auto mode in the TUI; OMP and Codex
   expose session/runtime overrides.
5. **The active posture is visible.** OpenCode explicitly adds an `auto`
   indicator; other systems expose mode in session configuration or UI.
6. **Repeated prompts produce scoped grants.** OpenCode's once/session-always
   result and Codex turn/session permission grants avoid asking the same
   question repeatedly.
7. **Automated review is emerging as a separate dimension.** Claude `auto` and
   Codex's guardian reviewer classify prompts, but neither eliminates the base
   mode/rule system.
8. **Sandboxing changes what auto-execution means.** Codex couples modes to
   permission profiles. Claude warns that bypass is full-system access. OMP and
   Pi permit direct execution, but that is a product posture rather than proof
   that unsandboxed commands are safe.

## Non-Convergence

- Defaults range from conservative/trust-sensitive (Claude, Gemini, Codex) to
  permissive (OpenCode, OMP, Pi).
- Some systems model permissions by tool tier, others by named tool/input rule,
  and Codex additionally models requested filesystem/network permission sets.
- Auto-review classifiers are new and not universal. Their false-allow,
  false-deny, latency, model-cost, audit, and recursion properties require
  separate evaluation.
- `full-auto` can mean sandboxed workspace execution, host-wide bypass, or just
  "do not prompt". The label alone carries no portable safety guarantee.

## Constraints For Yach's Later Design

These are research constraints, not a selected design:

- Preserve a conservative default, per owner direction.
- Make modes explicit, switchable, and always visible.
- Keep hard resource/sensitive-path denials outside mode overrides.
- Treat edit/write and arbitrary process execution as different tiers.
- Do not claim a workspace-safe command mode until a real process/filesystem
  isolation boundary exists.
- Authority provenance must be explicit. Repository-controlled configuration
  may restrict behavior but must not grant auto-execution authority.
- Persistent project grants belong in user state keyed to the canonical
  project, never in provider-editable repository files. Permission-policy files
  themselves must be denied to provider edits or require an unavoidable
  explicit user review.
- Support scoped grants without weakening the durable audit trail.
- Model auto-review as an optional reviewer for prompts produced by policy, not
  as a replacement for modes and rules.
- Define headless behavior explicitly: unresolved prompts should deny/fail, not
  silently inherit an interactive default.

## Recommended Yach Direction

This direction follows the owner-selected conservative default while leaving a
first-class path to automated review.

### Base modes

1. `plan`: reads only; edit and process requests are denied rather than queued.
2. `review` (default): reads run; bounded edits and commands ask unless an
   explicit user grant resolves them.
3. `accept-edits`: hash-checked project edit transactions run automatically;
   host commands continue through command policy and review.
4. `full-access`: edits and host commands run without ordinary prompts, but
   core hard denials remain. Because Yach has no sandbox, this mode must be
   session-only, visibly dangerous, and described as host access rather than
   workspace-safe execution.

The selected mode is always visible and switchable during a session. A
conservative default applies to new projects; a user's preferred non-dangerous
mode may be remembered in user state for that canonical project.

### Reviewer axis

Reviewer selection is independent of mode:

- `user` is the initial/default reviewer for policy outcomes that ask.
- `auto-review` is a later optional reviewer that returns allow, deny, or
  escalate-to-user for the same structured request.

Auto-review cannot override hard denials or create authority. It runs with no
mutation/process tools, treats tool arguments and repository text as untrusted
data, escalates high/critical or uncertain cases, and falls back to the user on
timeout or failure. Evidence records reviewer model/version, structured risk,
decision, and bounded rationale. A labeled corpus of real Yach approval
decisions must measure false allows, false denies, latency, and cost before it
can become a recommended posture.

### Authority and rules

- Core hard denials remain first: sensitive paths, outside-project writes,
  extension self-approval, and protected permission-policy files.
- User-state global and project-keyed policy is the only source that can grant
  persistent auto-execution authority.
- Repository `.yach/config.json` may supply non-authoritative defaults and
  restrictions, but cannot add shell allow entries, environment exposure, or
  future-session grants by itself.
- Interactive prompts support approve once, grant for this session, and where a
  safe pattern exists, persist a project grant in user state. Every grant is
  shown before storage and remains auditable.
- Headless unresolved asks fail closed unless an explicit mode/policy was
  supplied by the caller.

### Sequencing

0. Close the existing authority-provenance gap: stop unioning repository
   `shell.allow`/`env_allow` into executable authority and protect permission
   configuration from provider edits.
1. Unify edit and shell decisions behind one mode-aware policy result; add
   protocol/session/UI mode state and ship `review` plus `accept-edits`.
2. Add `plan`, explicit session-only `full-access`, and scoped once/session/user
   project grants.
3. Add optional auto-review with structured evidence and evaluation gates.
4. Integrate a real sandbox later; only then consider a mode that promises
   workspace-confined process execution.
