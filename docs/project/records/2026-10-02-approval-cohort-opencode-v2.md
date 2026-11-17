# OpenCode v2 approval system: cohort evidence (2026-10-02)

Sources and versions:

- **Source**: `anomalyco/opencode` branch `beta`, commit `e5ecb571` (2026-09-17), `packages/cli` version 2.0.6. Cloned to `/tmp/cohort/opencode-v2`. All `path:line` references are relative to that clone.
- **v2 docs**: <https://dev.opencode.ai/v2/docs/permissions/>, `/agents/`, `/migrate-v1/`, read 2026-10-02.
- **v1 docs (baseline)**: <https://opencode.ai/docs/permissions/>.
- **Branches**: the `2.0` branch is stale (April 2026, no v2 `packages/core`). v2 lives on `beta`, which is ahead of the latest `v2.0.x` tag.
- **CLI binary**: the v2 package is `@opencode-ai/cli@next` (beta, no standalone binaries).

## 1. Default posture

**Result: v2 keeps the v1 permissive default, and the baseline is confirmed.** `build` is the default agent, with no mode concept above the rule list.

- **Default rules.** `Agent.Info.default` is `{*,*,allow}`, then `external_directory * → ask`, `read *.env → ask`, `read *.env.* → ask`, `read *.env.example → allow`. (`packages/schema/src/agent.ts:46-52`)
- **Evaluator fallback.** If no rule matches, the result is `ask`. (`packages/core/src/permission.ts:87-97`)
- **`build` agent.** It adds `question → allow`. (`packages/core/src/plugin/agent.ts:87-92`)
- **Net effect for `build`.** Reads, edits, shell, web and subagents all auto-run. Only external directories and `.env` reads prompt.
- **Shell is not classified as dangerous.** No command, including `rm -rf` or `git push`, prompts by default.

## 2. Postures

There are no named modes. A posture is one of three things.

**Agent**
- `build`: the permissive default above.
- `plan`: adds `edit * → deny` and then `edit ~/.opencode/plan/* → allow`. (`packages/core/src/plugin/plan.ts:34-43`)
  - Shell stays allowed. The v2 agents doc says so: "Shell commands are not generally denied."
  - Plan mode is enforced partly by prompt text. The reminder says "Do not modify any other files or ask a subagent to do so."
  - `plan_enter` and `plan_exit` appear in the permissions doc as reserved actions, but I found no use in the source. [INFERENCE] They are vestigial or future.
- `general` (subagent): `build` rules, with question and subagent denied.
- `explore` (subagent): `* deny`, then allows `read`, `grep`, `glob`, `webfetch` and `websearch`. The block also re-asks for `.env` reads and external directories. (`plugin/agent.ts:105-131`)
- `compaction`, `title` and `summary` agents are hidden. `title` and `summary` deny all; `compaction` pushes no rules, so it gets the base default. (`plugin/agent.ts:133-153`)

**Ordered rules**
- Rules are `{action, resource, effect}`, evaluated last-match-wins.
- Order: agent defaults, then configured agent rules, then session rules, then saved approvals, then a plugin hook. (`permission.ts:158-189`)
- Session rules are new since 2026-09-10: commit `45a2ed9a`, "per-session permission rules". They are API/plugin-settable. Children inherit them at creation.

**Auto-accept toggle (client-side, not an evaluator mode)**
- TUI config `session.permissions: "prompt" | "autoaccept"`. (`packages/tui/src/config/index.tsx:169`)
- The command-palette toggle was removed on 2026-09-11 (`312651f6`). It is now a config or settings-dialog value, and it is a persistent default rather than session-only.
- With `--auto` or `autoaccept`, the TUI hides the prompt and replies `once` to every `permission.asked` event. (`packages/tui/src/routes/session/index.tsx:243-259`)
- The prompt bar shows a muted `auto` label. (`packages/tui/src/component/prompt/metadata.tsx:45`)
- A separate desktop-app auto-approver is app-level and applies to all sessions and servers. (`packages/app/src/session/requests/auto-approve.ts`, commit `46d1f1fe`)

## 3. What auto-runs, prompts or is denied

| Posture | Auto-runs | Prompts | Denied |
| --- | --- | --- | --- |
| `build` default | Nearly everything, including shell and edits | External directory, `.env` reads | Nothing by default |
| `plan` | Reads, shell, web | External directory, `.env` | Edits outside the plan directory |
| `explore` | read, grep, glob, web | External directory, `.env` | Everything else |
| `--auto` / `autoaccept` | Everything that would ask | Nothing | Explicit `deny` rules only |

- **Explicit deny under auto.** It stays enforced because a denied request is never published as a prompt. The evaluator returns `deny` before any ask can be created. (`permission.ts:174-175`)
- **Deny beats everything.** A configured deny beats saved approvals, and the plugin hook never sees it. (`permission.ts:174-175`, plugin docs `services/www/src/docs/content/build/plugins/index.mdx:1585-1591`)
- **Multi-resource calls.** The call is denied if any resource is denied; otherwise it asks if any resource asks. (`permission.ts:165-178`)

## 4. Shell command handling

This answers the `git status *` question.

- **The shell tool scans the command into every command position.** Each position becomes its own resource, and a permission request carries the whole list. (`packages/core/src/tool/plugin/shell.ts:125-141`)
- **Per-resource rule evaluation.** Each command is matched against the rules separately. Any deny wins, then any ask, then allow.
- **Smoke run.** I ran the scanner (`packages/core/src/shell/scan.ts`) plus `Wildcard.match` with rules `shell * ask` and `shell "git status *" allow`:

| Command | Resources and effect |
| --- | --- |
| `git status && rm -rf x` | `git status` allow, `rm -rf x` **ask** |
| `git status; rm -rf x` | same |
| `git status $(rm -rf x)` | `git status $(rm -rf x)` allow, `rm -rf x` ask |
| `git status` followed by a newline and `rm -rf x` | `git status` allow, `rm -rf x` ask |
| `git status \| tee out.txt` | `git status` allow, `tee out.txt` ask |
| `bash -c 'rm -rf x'` | one resource, `bash -c 'rm -rf x'`, ask |
| `eval '...'`, `xargs rm`, `env rm -rf x` | judged at the wrapper (`eval`, `xargs`, `env`), not the inner command |
| `git status && $CMD` | `$CMD` becomes a resource and asks |
| `git status > ~/.bashrc` | one resource, `git status > ~/.bashrc`, **allow** |

- **Wildcards.** `*` matches `/`, and a trailing `" *"` also matches the bare command. (`packages/core/src/util/wildcard.ts:11`)
- **Hole: redirect targets.** A write by redirect is not checked against `edit` rules or `external_directory`. Only the working directory and `cd`/`pushd` targets feed `external_directory`. (`shell.ts:126-132`, `parse.ts:14,190`) The scanner test corpus also treats `find -exec`, `python -c` and `bash -lc` as opaque at the wrapper boundary. (`packages/core/test/shell-scan/adversarial.test.ts:14-26`) Commands wrapped by an unlisted launcher are not decomposed.
- **Scanner default.** `experimental.portable_shell_scanner` defaults to `false`. (`packages/schema/src/config/experimental.ts:8-10`, `shell.ts:124`) The commit title "make portable scanner authoritative" (`74a0b2d0`, 2026-08-26) refers to its parity work, not to a default flip. [INFERENCE]
  - The default path is the older tree-sitter walk that enumerates every `command` node. (`packages/core/src/shell/parse.ts:173-205`) I did not run that path; it needs the wasm and node_modules, which are not installed.
  - The portable scanner is a hand-written Bash/PowerShell tokenizer with `opaque` results. On opaque input the shell tool fails closed with "cannot analyze command", not an ask. (`parse.ts:214-215`)
  - Opaque reasons: `command-substitution` (when unparseable), `compound-command` (unsupported structure), `heredoc`, `dynamic-command-name`, `invalid-*`, and unterminated quotes or escapes. (`scan.ts:3-12`)
- **Saved-pattern suggestion.** The "always" pattern is a command prefix. `ARITY` is a table of about 160 entries, for example `git`=2, `npm run`=3, `rm`=1, so the saved grant is `git status *`. (`parse.ts:22-160,345-351`)
  - Docs say shell proposes "the exact command text". The code says otherwise: it saves `<arity-prefix> *`. [INFERENCE] The doc line is stale or loose.
- **Background commands.** They go through the same `prepare` permission check before spawn. (`shell.ts:199-211`)

## 5. Sandbox

**There is none.**

- No OS-level sandbox, interpreter or container backs auto-execution. A search for bwrap, landlock, seccomp, seatbelt and sandbox found only project "sandboxes" (worktree metadata) and workspace-driver remoting. (`packages/core/src/workspace.ts`, `project/sql.ts`)
- `packages/containers` contains build images, not an execution sandbox.
- `specs/v2/provider-policy.md:204`: "Provider policy is not a full sandbox for executable plugins."
- Containment is by path rules only:
  - Relative mutation paths cannot escape the location.
  - Symlink escapes are rejected.
  - `external_directory` gates external paths.
  - Shell redirects and subprocess behaviour are not contained.
- OpenCode also allows its own tool-output and temp directories without a prompt. (`packages/core/src/agent.ts:59-64`)

## 6. Reviewers and classifiers

**There is no built-in model reviewer.** There is a plugin extension point instead.

- `ctx.permission.hook("evaluate", …)` was added on 2026-08-25 (`0fd71906`, #45003).
- It runs after configured rules, for `allow` and `ask` results only. It may rewrite `effect` to `allow`, `ask` or `deny`, with a `message`.
- The plugin docs example is an LLM safety reviewer: it builds a prompt from the session context plus the action and resources, then sets `event.effect` from the model's answer. (`services/www/src/docs/content/build/plugins/index.mdx:1566-1590`)
- It is documented as plugin-authored. No shipped reviewer exists. [INFERENCE: none ship in this tree]
- A hook can both escalate and loosen. A bug or injection in the reviewer can turn `ask` into `allow`. The only floor is a configured `deny`.

## 7. Prompt UI

- **Header.** "Permission required", with the action title and body. (`packages/tui/src/routes/session/permission.tsx:200-215`)
  - Shell: `$ command`.
  - Edit: diff.
  - External directory: the pattern list.
  - Other tools: a generic tool line.
- **Options.** `Allow once`, `Always allow` (only when the request carries a `save` list), and `Reject`.
  - Escape means reject.
  - Reject inside a subagent session opens a feedback text box: "Tell OpenCode what to do differently". The text becomes a `CorrectedError`. (`permission.tsx:249-252`, `272-330`; `permission.ts:276-280`)
- **`Always` screen.** It lists the patterns that will be saved: "This will always allow the following patterns for this project." (`packages/tui/src/util/permission.ts:154-160`)
- **Reject cascade.** Rejecting also rejects every other pending request in that session. (`permission.ts:282-290`)
- **`always` scope.** `always` writes to the DB, scoped to the project: `PermissionTable` keyed by `project_id`. (`packages/core/src/permission/saved.ts`, `permission.ts:295-301`)
  - After a grant, other pending requests that now evaluate to `allow` are auto-released. (`permission.ts:306-318`)
  - Saved allows can never override a configured deny.

## 8. Headless

- **`opencode2 run`, default.** It auto-rejects any permission request, prints `permission requested: <action> (<resources>); auto-rejecting`, replies `reject` and interrupts the session. (`packages/cli/src/run/noninteractive.ts:135-153`)
  - Fail-closed, matching the Yach constraint.
- **`--auto`.** It replies `once`. It never saves a grant, and denies still hold. Aliases `--yolo` and `--dangerously-skip-permissions` are hidden. (`packages/cli/src/commands/commands.ts:27-35`)
- **Other surfaces.** The ACP bridge offers once, always and reject. (`packages/cli/src/acp/permission.ts:11-15`) Form requests are cancelled headlessly.

## 9. v1 versus v2 changes (August baseline to now)

| Item | v1 docs | v2 beta |
| --- | --- | --- |
| Config shape | per-tool maps, `permission` | one ordered `permissions` array, `{action, resource, effect}` |
| Action names | `bash`, `task`, `write`, `patch` | `shell`, `subagent`; write and patch fold into `edit` |
| Default for `.env` reads | deny | **ask** |
| `external_directory` default | ask | ask |
| `doom_loop` | action, default ask, fires on 3 identical calls | **removed**. Docs: "`doom_loop` and `lsp` are not current V2 Core permission actions." |
| `always` scope | "rest of the current OpenCode session" | **durable, per-project, saved to DB** |
| Shell parsing | tree-sitter, per command | still per command; hand-written portable scanner is opt-in and fails closed |
| Reviewer | none | plugin `permission.evaluate` hook |
| Auto toggle | `--auto` plus palette toggle | `--auto` plus persisted `session.permissions` config |
| Session-level rules | none | new (2026-09-10) |

- The v1 docs still document `doom_loop`, and a v1 `doom_loop` check exists in `dev` at `packages/opencode/src/session/processor.ts` (threshold 3). The v2 TUI keeps a presentation string for it (`packages/tui/src/util/permission.ts:131`) and a demo, but no v2 core code emits it. Loop handling instead lives in `steps` (a per-agent step cap that removes tools on the final step) and in restart limits (`packages/core/src/session/execution/restart.ts:20`).
- The v1 `migrate.ts` maps rules to the new array. (`packages/core/src/v1/config/migrate.ts:96-122`)
- v1 `opencode --auto` and `opencode run --auto` were documented in August. v2 keeps both flags.

## 10. Takeaways for Yach (the cohesive approval design)

1. **OpenCode proves the "rarely interrupts" default is shippable, but with no sandbox.** It rests on a simple rule: allow everything, except deny and ask for a short list of exceptions. It prompts for external directories and `.env` reads, not for commands. The cost is that shell is unconstrained: redirects and wrapper commands bypass the rule list.
2. **Compound-command decomposition is the design to copy for Yach's allowlist.** An allowed prefix never covers a chained second command, and unanalyzable input fails closed. The scanner has a large corpus of adversarial tests.
3. **Durable `always` grants are the legacy pattern the owner distrusts, and v2 went further toward it.** v1 scoped them to the session; v2 persists them per project. Yach's constraint to keep grants out of repo files holds: OpenCode stores them in its DB, not the repo.
4. **Auto mode is just "answer once to every ask" on the client.** The server has one posture, with no mode state. Deny survives because it never becomes an ask.
5. **The reviewer is a hook, not a product.** It can loosen decisions. If Yach adds an auto-review reviewer, it should be allowed only to allow, deny or escalate within the hard-deny floor.
6. **Doom-loop prompting was dropped, not moved.** Nothing replaces it except a step cap.
