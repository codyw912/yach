# omp² (omp rewrite) approval/sandbox research

Source: `/tmp/cohort/omp2`, branch `omp2`, commit `2f92f3b` (2026-09-04, shallow clone). Paths are repo-relative. v1: local `omp://approval-mode.md`. Playbook: <https://stencil.so/blog/harness-playbook> (fetched 2026-10-02). "Impl" means verified in `crates/`; "Design" means ADR or `docs/py` only. I read code and ran no builds or tests.

## Bottom line

1. **Default is still `yolo`.** `sv_tools_approval_mode` defaults to `ApprovalMode::Yolo` (`crates/envd/src/tool_settings.rs:102-104`, enum `#[default] Yolo` at `crates/envd/src/admission.rs:49-57`). That is the same default v1 documents.
2. **The bash tool never trips a tier.** `crates/tools/src/shell.rs:495-498` sets `effects: Effects::empty()` with the comment "The shell string is not an approval capability". A test pins it (`bash_declares_no_whole_script_capability`, `shell.rs:1359`). Empty effects resolve to tier `read` (`admission.rs:111-133`), so `always-ask` and `write` both auto-allow `bash` at the tool gate. v1 treated bash as `exec` and prompted for it in those modes.
3. **ADR 0028's capability-boundary approval is mostly design.** It promises a prompt at the first out-of-workspace write, `ln`, `git push`, or network request. I found no per-builtin or `git push` prompt in `crates/`. The ADR's own status line says "Partial… distinct network-request approval… remain incomplete". The only just-in-time prompt in the shell path is the sandbox-denial retry below.
4. **A real OS sandbox exists but is off by default.** `sv_sandbox_mode` defaults to `off` (`crates/envd/src/exec_settings/sandbox.rs:26-35,147-155`).

## Modes and default posture

| Item | Detail | Status |
|---|---|---|
| Tiers | `Read` / `Write` / `Exec` derived from declared `Effects`: exec, inference, subagents, or desktop input → Exec; document write globs → Write; else Read (`admission.rs:100-134`). Design also lists a `PRIVILEGED` tier (`docs/py/06-policy.md:391-412`). | 3 in code |
| Modes | `always-ask` (auto Read), `write` (auto Read+Write), `yolo` (auto all, default). `resolve_approval`, `admission.rs:170-200`. | Impl |
| Per-tool override | `sv_tools_approval` (`bash=allow\|deny\|prompt`) beats the mode (`admission.rs:180-197`). | Impl |
| Flags | `--approval-mode`, `--yolo`/`--auto-approve`; explicit mode wins (`crates/app/src/cli.rs:1731-1735,1901-1907`). Override is connection-local (`server.rs:13484`). | Impl |
| Plan | A Director, not a mode; it is given `write,ask` tools (`crates/agent/src/directors/plan.rs:120`). `--plan-yolo` hands off to an implementing model on approval. | Impl |
| `sv_approval_mode` | Default `"on-request"` (`crates/agent/src/vars.rs:131-135`). Grep finds only the `/security` report reading it (`crates/chat/src/commands/misc.rs:172`). | Declared, no gate uses it [INFERENCE] |

v1's critical bash patterns, `bash.patterns`, and `allowCompoundCommands` have no counterpart in omp² code. I grepped `crates/` and found none.

## How policy decides

- **Native gate.** `AdmissionGate` (`envd/src/admission.rs:446-665`) resolves Allow, Deny, or Prompt.
  - A prompt waits for an answer until a deadline, then settles as a structured `PolicyDenied` (`:634-664,740`).
  - A transform patch can change args, and the BashIR is regenerated (`apply_admission_patch`, `:679-697`).
  - A tool's effect envelope can be narrowed but never widened (`effects_narrow_or_refuse`, `:668-677`).
  - The kernel builds the prompt in `SettingsAdmission` (`crates/driver/src/headless/kernel.rs:590-650`): scopes `once` and `session`, `require_human: true`, `default: false`, evidence "`<tier> tier under approval mode <mode>`".
- **Verdict types.**
  - Hook decisions are `Allow | Deny | Modify | Defer | RequireApproval`. Legal phase pairings are enforced in `crates/agent/src/hooks.rs:109-113`.
  - A denial settles as `CallOutcome::Aborted(POLICY_DENIED, PolicyDenied{reason, code, decision_id, rules})` (`docs/py/06-policy.md:159-173`).
- **Hook phases** (`docs/py/05-hooks.md:685-705,2060-2091`; `hooks.rs:47-61`): `PRECHECK ∥ → TRANSFORM (ordered) → REVIEW ∥ → APPROVAL → OBSERVE`.
  - PRECHECK is pure and deny-only.
  - REVIEW is the slot for paid-model classifiers: "Paid inference is allowed; externally visible effects are not". Any `Deny` denies.
  - That is omp²'s place for an auto-reviewer. It is Design; no built-in classifier ships. The doc's `pi-menshen` guardian port is an example only (`06-policy.md:1955`).
  - APPROVAL hooks return `RequireApproval(ApprovalSpec)`. Core merges them into one durable ticket per invocation, and a hook never suspends waiting for a human (`06-policy.md:1322-1382`).
  - The call runs only with zero denies among non-deferring hooks (`05-hooks.md:2083-2085`).
- **Fail-closed.** A crashed or quarantined policy extension stays registered as a synthetic `Deny` stub. Only an explicit user disable removes it (`06-policy.md:223-247`).
- **Who can grant.**
  - Extension-sourced decisions need the manifest capability `approvals.decide` (`envd/src/policy.rs:282-283`).
  - `require_human=true` forbids any non-human answer (`06-policy.md:1414-1418`). The sandbox-amendment and privileged-write prompts set it (`exec.rs:585-602`, `server.rs:2070-2087`).
  - Core renders the dialog, so an extension cannot supply or spoof it (`06-policy.md:1376-1378`).
  - Sandbox profiles compose as "denies union, allows intersect, mode max", so config can tighten and an extension can never loosen (`06-policy.md:988-994`).
- **Scopes (`ApprovalScope`).** The enum has `Once/Call/Turn/Session/Persist` (`crates/agent/src/approvals.rs:58-73`). The TUI emits only `once` and `session`. `Persist` needs a `policy.persist` capability (Design, `06-policy.md:1240-1242`).
- **Routes and unreachable (Design).**
  - Routes: `AUTO/LOCAL/PARENT/EXTERNAL/NONE`; subagents route to `PARENT` (`06-policy.md:1481-1490`).
  - `Unreachable`: `FAIL_CLOSED` (default), `ESCALATE_LOCAL`, `FAIL_OPEN_AUDITED`. The last is refused unless the IR is read-only (`06-policy.md:1492-1508`).

## What the in-process interpreter changes

- **Classification (Impl).** `analysis::analyze` in `omp_shell` produces `BashIr` (`admission.rs:719-738`), and Python `tool_call` hooks receive it (`envd/src/tools.rs:2206-2230`).
  - The IR gives per-command resolved cwd, inferred path reads/writes, and dynamism flags (`06-policy.md:266-270`).
  - The same code that parses a script executes it. That is the Design claim ("a policy does not approximate the command — it reads the parse", `06-policy.md:26-34`).
  - Words like `$D` or `$(...)` stay flagged dynamic (`:272-277`).
  - The only in-tree consumer I found is `github_mutation_targets` (`admission.rs:382-431`), which derives `gh` issue/PR mutations for cache invalidation, not approval.
- **Builtins never spawn, so kernel sandboxes do not reach them.** The design says so directly: "a `deny_write` on `.git` stops `/usr/bin/rm` and does not stop the in-process `rm`" (`06-policy.md:2591-2603`). It calls this "the single largest unresolved risk".
  - The implemented answer is an engine-level `PathPolicy` trait with `check_read`, `check_write`, and an atomic `open` after authorization (`crates/shell/src/interp.rs:97-106`).
  - `ExecSandboxAttempt` implements it and is installed with `set_path_policy` (`exec_sandbox.rs:467-479`, `exec.rs:2367-2370`).
  - Builtins call `check_write` through the host (`shell-builtins/src/host.rs:265-270`). Redirects and glob expansion hit it too (`shell/src/expansion.rs:847`).
  - The file policy resolves symlinks and `..`, and carves out `.git`, `.omp`, and `.agents` (`exec_sandbox.rs:30,1098`).
  - External children go through a `SpawnWrapper` that prefixes bwrap or seatbelt and filters env.
  - The doc itself concedes this check is "Rust-enforced but same-process, so a bug in the check is a bypass".
- **Env hygiene.** `env_inherit` defaults to `all` (`sandbox.rs:78-86`), so by default nothing is scrubbed.

## Sandbox kinds and status

- **`crates/sandbox` (`omp-sandbox`), Impl.** About 6.8k lines including tests.
  - Backends: Seatbelt, Bubblewrap, Landlock+seccomp, gVisor, Docker, Docker+runsc, Windows AppContainer (`crates/sandbox/src/capability.rs:22-50`).
  - Strict specs fail if the backend cannot enforce every requested capability (`crates/sandbox/README.md`).
  - Each backend has its own test file under `crates/sandbox/tests/`.
  - The root `README.md:97` still calls it a "Deferred isolation boundary", which is stale.
- **Knobs.**
  - `ExecSandboxMode`: `off` (default), `read-only`, `workspace-write`.
  - Read mode: `host` (default), `minimal`, `scoped`. Network: `disabled`, `open`, `scoped`.
  - Scoped network is a domain allow/deny broker with ports 80/443 by default (`sandbox.rs:114-139,147-320`, `envd/src/sandbox_proxy.rs`).
  - `unscoped_writes=overlay` redirects stray writes to an ephemeral layer and degrades to scoped writes if the backend cannot overlay (`exec_sandbox.rs:261-299`).
  - Note that the network default is `disabled`, but only takes effect once a mode other than `off` is active (`exec_sandbox.rs:202-237`).
- **Sandbox denial → one-shot approval (Impl).** `run_session_command` (`exec.rs:2416-2486`) does this:
  1. Classify the denial as `ReadPath`, `WritePath`, `Network{host,port}`, or `Unknown`.
  2. Freeze the path identity (dev/inode) before asking.
  3. Ask with kind `sandbox_amendment`, scope `once`, `require_human`, `default: false`, `fail_closed`, a 120s timeout, and `pattern` = the exact command (`exec.rs:571-615`).
  4. If approved, rerun that one command with only that scope opened. Cancellation never reruns (`tool_shell.rs:1021-1075`).
  - `Unknown` denials get no grant. With no approval route (headless), the answer is false and the denial stands (`exec.rs:577-579`).
  - This is prompt-after-failure, not predictive classification. With no sandbox active, `classify_sandbox_denial` returns `None` (`exec.rs:2691`), so the path is unreachable in the default config.
- **Host/sandbox split (ADR 0006), "Partial".** The host owns policy, approval, and journal. The "stub" is the `envd` daemon, and remote, container, and VM targets "still use the full envd binary rather than a separately minimized stub". Extension hosts at the `sandboxed` tier launch under `omp-sandbox` (`envd/src/exthost/spawn.rs:524-535`).
- **Python `SandboxProfile` design** (HARD/BROKERED/BEST_EFFORT/NONE enforcement receipts, `OFF/OBSERVE/ENFORCE`).
  - Types and a `SandboxPolicyRuntime` trait exist (`envd/src/policy.rs:300-690`).
  - The 2026-08-19 user rulings say v1 "ships nothing": no SOCKS filter, macOS, or Windows enforcement, deferred to VM-grade "vibevmm" plus "isobox" (`06-policy.md:2587-2646`).
  - The crate and `sv_sandbox_*` convars shipped afterwards and the doc was not updated, so docs and code disagree. [INFERENCE: later work supersedes the rulings.]

## Subagents

ADR 0007 is marked "Implemented". `sv_task_isolation_mode` selects a copy-on-write child workspace: `none` (default), `auto`, `apfs`, `btrfs`, `zfs`, `reflink`, `overlayfs`, `projfs`, `block-clone`, `rcopy`. `sv_task_isolation_merge` is `patch` (default) or `branch`, and `sv_task_isolation_apply` controls auto-apply (`crates/driver/src/subagent/settings.rs:84-135,316-372`).

I read only the settings, not `envd/src/workspace/operations.rs`, so "Implemented" is the ADR's own word. Isolation is off by default, so a child writes the parent tree unless the user enables it. `ApprovalRoute::Parent` is Design; I did not trace a forwarding path. v1 ran subagents headless in `yolo` and a `prompt` policy rejected the call (`approval-mode.md` "Subagents").

## Prompt UI and headless

- **TUI.** `ApprovalOverlay::decision` maps `y` → approve at the prompt's default scope, `a` → approve with `Session` scope, `n` → deny once (`crates/chat/src/overlays/mod.rs:527-545`). The overlay shows title, reason, scope, and an optional timeout. The prompt lives in the session DOM (`crates/agent/src/approvals.rs`). I found no persist-to-config button.
- **Headless print/JSON.** An approval-requiring call is denied immediately with source `Unavailable` and the message "use `--approval-mode yolo` or `tools.approval.<tool> allow`" (`crates/app/src/print_mode.rs:598-610`). That is fail-closed, but only reachable when the mode is not `yolo`.
- **ACP, RPC, collab.** Not traced beyond the mode being passed through.
- **Dynamic devices** (`dyn` builtin) are admitted per resolved device, tier from declared effects, `once` only (`admission.rs:255-324`).

## Harness Playbook

- **Trusted control plane.** ADR 0001 consequence 2: policy and session ownership stay on the host and sandboxes get bounded requests. ADR 0006 quotes "The sandbox should execute, not decide".
- **Approval theatre.** The bash chapter calls string-level approval "horrible: nobody will read it". It cites Anthropic's auto mode as beating humans "by quite far". It proposes asking when execution reaches `ln` and skipping the prompt when the user already allowed writes to that directory. The harness would become "a capability approver: 'May I use Git to push?'"
- **Code vs article.** The parser, the interpreter, and the `PathPolicy` seam exist. The capability prompts for push, `ln`, and generic network do not. REVIEW-phase classification is a design slot with no shipped classifier.

## Changes vs omp v1

| Topic | v1 | omp² @ `2f92f3b` |
|---|---|---|
| Modes | `always-ask`/`write`/`yolo`, default `yolo` | Same three, same default; `on-request` var declared, unused by gates [INFERENCE] |
| Bash gating | `exec` tier, critical-pattern prompts, `bash.patterns`, compound-chain rules | Tier `read` (empty effects); patterns gone; BashIR for Python hooks; sandbox-denial retry |
| Containment | None ("not process or filesystem containment") | Optional 7-backend `omp-sandbox` plus engine `PathPolicy`, off by default |
| Approval state | Per-turn prompt plus `tools.approval.<tool>` | Durable Core-owned tickets in the journal; structured `PolicyDenied`; extension decisions gated by a capability |
| Subagents | Headless `yolo` | Optional CoW workspace, patch/branch return; parent routing designed |
| Extensions | TS in-process | Python hooks; synthetic-deny stub on failure |

## Observations for yach (not recommendations)

- omp²'s "rarely interrupts" comes from a `yolo` default with no sandbox in the default config. It is a posture choice, not a safety mechanism.
- The sandbox-backed pattern omp² built is run, ask only if the sandbox blocked a specific path or host, then rerun once. It requires a sandbox and a human, and it does not run in the default config.
- ADR 0028's before-the-fact capability approval works only because the interpreter owns the builtins. Yach runs real host processes, so it would need a shell layer of its own or a sandbox to do the same.
