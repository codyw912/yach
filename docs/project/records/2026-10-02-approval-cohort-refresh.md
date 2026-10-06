# Approval Cohort Refresh

Date: 2026-10-02. Outcome: plane:YACH-18.

Refreshes `2026-08-24-approval-modes-cohort-research.md` for the YACH-18
design: what posture each harness gives a new user, what runs without a
prompt, and what makes that safe (or doesn't). Adds the two rewrites in
progress, opencode v2 (beta) and omp² (`omp2` branch). Evidence, not a
selected design.

## Sources

| Harness | Source read | Version / date |
| --- | --- | --- |
| Claude Code | anthropics/claude-code CHANGELOG `1c229fc`; code.claude.com permission-modes, permissions, sandboxing docs | 2.1.288, 2026-10-02 |
| Codex CLI | openai/codex `44dd77b` (`codex-rs/`) | rust-v0.160.0, 2026-10-01 |
| Gemini CLI | google-gemini/gemini-cli `fb972b2` | stable v0.61.0 (2026-09-23), nightly 0.64.0 |
| Pi | badlogic/pi-mono `69f0be6` | coding-agent 1.0.0, 2026-10-01 |
| opencode v2 | anomalyco/opencode branch `beta` `e5ecb571`; default ruleset re-checked on `dev` `108b988` (2026-10-02, `packages/core/src/plugin/agent.ts:102-118`); dev.opencode.ai/v2/docs | CLI 2.0.6, 2026-09-17 |
| omp² | can1357/oh-my-pi branch `omp2` `2f92f3b`; ADRs, `docs/py`; Harness Playbook (stencil.so) | 2026-09-04 |
| yach | this repo, `main@origin` `ee68eaff` | 2026-10-02 |

Per-harness notes with `path:line` citations were collected under
`.sjujperpowers/research/2026-10-02-cohort-{established,opencode-v2,omp2}.md`
(local, not versioned); the claims below are the ones that matter for
YACH-18.

## Corrections to the 2026-08-24 record

1. Claude Code's built-in default became `auto` (model classifier) in
   2.1.283/284 (2026-09-25). `default` is now labelled "Manual".
2. Codex retired `untrusted` and deleted its known-safe command allowlist
   on 2026-08-20 (#39630). Commands that match no rule now run inside the
   sandbox without a prompt.
3. Codex's reviewer default is `user`; `auto_review` ("Approve for me") is
   opt-in. `guardian_subagent` is a legacy alias.
4. Gemini ships an LLM checker (Conseca), off by default.
5. OMP's default is `yolo` in both v1 and omp²; the rewrite did not change
   it.

## Default posture for a new user

| Harness | Default | Edits | Shell | What bounds it |
| --- | --- | --- | --- | --- |
| Claude Code | `auto` | run (in cwd) | read-only set runs; everything else goes to a classifier | model classifier with a built-in block list; sandbox exists, off by default |
| Codex | `on-request` + `workspace-write` (trusted project); read-only until trust | run (in workspace) | runs inside the sandbox; prompts only for a dangerous match or a sandbox escape | OS sandbox (bubblewrap on Linux, Seatbelt on macOS); network off |
| Gemini CLI | `default` | prompt | prompt, except a short known-safe read allowlist | prompts; sandbox opt-in |
| Pi | no approvals | run | run | nothing in core; isolation is a deployment choice |
| opencode v2 | `build` agent: allow `*` | run | run, all of it | nothing; asks only for external directories and `.env` reads |
| omp² | `yolo` | run | run | nothing by default; 7-backend sandbox crate exists, off by default |
| yach | `review` | prompt | prompt unless on the user allowlist | prompts |

Only Gemini and yach still default to prompting for routine work. Of the
harnesses that don't, two bound the default with a mechanism (Codex: OS
sandbox; Claude: classifier) and three bound it with nothing (Pi, opencode
v2, omp²).

## Mechanisms

### Command-text classification

- **Claude** keeps a fixed read-only bash set (`ls cat echo pwd head tail
  grep find wc which diff stat du cd`, read-only `git`), not configurable.
  Compound commands are split and every part must match; documented as
  "not a security boundary".
- **Gemini** upgrades ask→allow for a strict known-safe allowlist with
  per-tool deep checks (`find`, `rg`, `git`, `sed`); compound commands are
  split per segment; any redirection downgrades allow→ask.
- **Codex removed its allowlist.** Classification now only finds
  *dangerous* commands (`rm -f`, through `sudo`/`env`/`bash -lc`,
  fail-closed at depth 8); everything else is left to the sandbox.
- **opencode v2** checks every command position separately (`git status &&
  rm -rf x` → allow + ask). Gaps: redirect targets are not checked
  (`git status > ~/.bashrc` is allowed), and wrappers (`bash -c`, `eval`,
  `xargs`, `env`) are judged at the wrapper.
- **omp²** parses bash into an IR (paths read/written, cwd, dynamism) in
  the same interpreter that executes it; the bash tool itself declares no
  effects and never prompts. Before-the-fact capability prompts
  (`git push`, `ln`, network) are designed (ADR 0028), not built.

### Sandboxes

- **Codex** is the only harness whose default rests on a sandbox.
  `workspace-write`: read `/`, write project + `/tmp` + `$TMPDIR`, with
  `.git`, `.codex`, `.agents`, `.aws` re-bound read-only; network off.
  Linux backend is bubblewrap with `no_new_privs` and a seccomp network
  filter; Landlock-only is rejected for filesystem policies.
- **Claude** has an OS sandbox (Seatbelt; bubblewrap+socat) off by
  default. When on, its "auto-allow" mode runs sandboxed commands without a
  prompt regardless of permission mode; network goes through a proxy with
  an empty allow list.
- **Gemini** sandbox is opt-in (Seatbelt, Docker/Podman, gVisor, LXC).
- **omp²** has a real `omp-sandbox` crate (Seatbelt, bubblewrap,
  Landlock+seccomp, gVisor, Docker, AppContainer), off by default. Its one
  implemented prompt in the shell path is *after* a sandbox denial: classify
  the denied path or host, ask once, rerun once with only that opened.
- **opencode v2, Pi:** none.

### Automated reviewers

- **Claude `auto`** is now the default: rules first, then reads and in-cwd
  edits auto-approved, then a server-side classifier for the rest. Built-in
  block list (`curl|bash`, force push, prod deploys, secret exfiltration);
  boundaries stated in the conversation count as block signals. After 3
  consecutive or 20 total blocks it pauses and falls back to prompting.
- **Codex `auto_review`** reviews only requests that would have reached the
  user (sandbox escapes, network, MCP), 90 s timeout, fails closed; denials
  are listed in the TUI. The TUI recommends it when a user picks Full
  Access.
- **opencode v2** has a plugin hook that can rewrite allow/ask (and so can
  loosen); no shipped reviewer.
- **omp²** reserves a REVIEW hook phase for paid classifiers (deny-only);
  none ships.

### Prompt choices and saved rules

| Harness | Prompt options | Persistence |
| --- | --- | --- |
| Claude | Yes / don't ask again for `<prefix>` / switch to auto / No (+comment) | saved to `.claude/settings.local.json` |
| Codex | proceed / always for prefix / this session / no / no + tell Codex | prefix written as an exec-policy rule |
| Gemini | once / session / all future sessions | mode-aware user-tier rule |
| opencode v2 | once / always / reject (+feedback in subagents) | durable per project, in its DB (v1: session only) |
| omp² | once / session | none in the TUI |
| yach | once / session (identical command + cwd) / reject | none |

Saved rules remain central in Claude, Codex, Gemini and opencode v2;
opencode v2 made them *more* durable.

### Repository config and headless

- Claude ignores `auto`/`bypassPermissions` as `defaultMode` from project
  settings; Gemini disables workspace policy files and forces `default` in
  untrusted folders; Codex keys trust in user config. All keep
  repository-supplied config from granting autonomy (yach already does).
- Headless: Claude `dontAsk`/deny, Gemini deny, opencode v2 reject unless
  `--auto`, omp² deny unless `yolo`, Codex `never` (sandbox bounds it,
  dangerous commands forbidden). None silently allows.

## yach host feasibility

Two Linux mechanisms are available on the dev host (kernel 6.18.47),
recorded separately:

- **Bubblewrap:** unprivileged bubblewrap works (0.12.0 from nixpkgs;
  user namespaces enabled). With `--ro-bind / / --bind /tmp /tmp
  --unshare-net`, writes to `/etc` fail, `/tmp` writes succeed, and
  network is blocked. `bwrap` is not on the default PATH; it is a packaging
  dependency (devenv/nix or bundled), not a host limit.
- **Landlock:** enabled in the active LSM list
  (`capability,landlock,yama,bpf,ima`); `landlock_create_ruleset(NULL, 0,
  LANDLOCK_CREATE_RULESET_VERSION)` returns ABI 7. Needs no binary. Since
  ABI 4 (Linux 6.7) it can also restrict TCP bind/connect, but not UDP or
  unix sockets, so a full network cutoff still needs a network namespace
  (bubblewrap `--unshare-net`) or seccomp.

## What this means for YACH-18

Evidence, not decisions:

1. The field has moved from "prompt for routine work" to "don't prompt,
   bound it with something". The something is a sandbox (Codex), a
   classifier (Claude), or nothing (Pi, opencode v2, omp²).
2. Read-only command allowlists survive only as a small, path-checked
   layer (Claude, Gemini); Codex removed its own once the sandbox was the
   gate. A classifier alone removes some prompts but does not change the
   default's character.
3. The two mechanisms are complementary: Codex runs everything in the
   sandbox and sends only escapes to a reviewer (user or `auto_review`);
   Claude classifies everything and keeps the sandbox optional.
4. yach's auto-review cannot be the default bound yet: execution is
   compiled off and its gates are unmet (E2 held-out recall .76). A sandbox
   has no such gate.
5. Prompt-after-denial (omp²) and prompt-for-escape (Codex) are the two
   patterns that turn a sandbox into a low-interruption default: prompts
   occur only when work needs more than the workspace.
6. Composite-command handling is the known weak point of text rules
   (opencode's redirect and wrapper gaps); any yach classification must
   split segments, check redirect targets, and fail closed on what it
   can't parse.
