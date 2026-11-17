# Approval Model Design

**Outcome:** Cohesive approval design

**Status:** approved 2026-11-18, including the execution seam and the
auto-review UI changes; direction approved 2026-10-15

Amends the `review` host-command rule in
[Approval Modes](2026-08-24-approval-modes-design.md) and the deterministic-bypass
ban in [Auto-Review Signals, Routing, and Evaluation](2026-09-24-auto-review-signals-routing-design.md).
Does not amend [Full-Access Approval](2026-08-24-full-access-approval-design.md)
except to state that `full-access` stays session-only and is unchanged.

Citations are line numbers on `main` at commit `451c65dc` (2026-11-17) unless a
URL is given. `[INFERENCE]` marks anything not read in code, docs, or a cited
page. The last section lists what changed between the 2026-10-15 draft and this
revision.

## Problem

A normal coding session in the default mode stops for work the user does not
need to see. The outcome's dogfood note records a 2026-10-02 ylog session that
raised 14 prompts, 7 edits and 7 shell, almost all routine `uv run` / `python`
inspection. The session log itself is not in the repo, and a search of local
`~/.yach/sessions` did not recover a session with that 7/7 split (the only
ylog session with review rows has 13 edit reviews and 23 shell reviews). The
7 shell command strings are therefore not available. This spec uses the note's
characterization, and uses the recoverable session only as a shape catalog for
the corpus, not as the acceptance baseline.

Owner decision 2026-10-15 re-scopes that outcome. Those 7 shell prompts were
runners and interpreters. This slice does not classify them, so it does not
promise that the 14-prompt baseline drops. Done-when is the read-only
classifier and the auto-review UI, plus a repeat dogfood record of the
measured prompt drop for inspection commands the table names. The prompt-drop
target for runners moves to a future sandbox/runner outcome. Runner and
interpreter prompts are explicitly out of scope here.

Today that prompting is the mode working as designed, not a bug:

- Four modes, one enum (`crates/yach-proto/src/lib.rs:273-278`): `review`
  (default), `accept-edits`, `auto-review`, `full-access`. A new session loads
  the stored project preference or `review`
  (`crates/yach-backend/src/permission.rs:30-31`); a stored dangerous mode is
  ignored with a warning (`permission.rs:66-67`). `full-access` and
  `auto-review` are refused on persist (`permission.rs:70-76`).
- In `review`, every hash-checked workspace edit asks
  (`permission.rs:447-454`, mapped at `runner.rs:8358-8364`) and every host
  command asks unless the user allowlist matches (`permission.rs:516-526`).
  `accept-edits` skips the edit prompt and still asks for every
  non-allowlisted command (`runner.rs:8361`, `permission.rs:517`).
- `decide_shell` (`permission.rs:477-547`) runs before any reviewer, in this
  order: standing user restrictions (`permission.rs:574-593`, precedence at
  `permission.rs:596-606`, a match holds under `full-access` and an allowlist,
  test at `permission.rs:1302`), the parse-aware user allowlist
  (`shell.rs:128-160`, `shell.rs:163-232`; project config cannot grant,
  `shell.rs:94-97` and `107-118`; the default list is empty, `shell.rs:52`),
  the session grant (byte-identical command plus resolved cwd, memory only,
  `runner.rs:5035-5063`, granted only from "Approve for this session" at
  `runner.rs:9378-9380`, `9486-9488`, `9594-9596`), then the mode. The runner
  passes the allowlist and grant results in as booleans
  (`runner.rs:9190-9198`).
- `auto-review` returns `route_to_reviewer` (`permission.rs:539-545`). The
  runner builds a coordinator only when the effective mode is `auto-review`
  (`runner.rs:5760-5781`) and calls it only for that decision
  (`runner.rs:9212-9224`). A model `Execute` is rewritten to
  `ReviewFailure::Disabled` because `AUTO_REVIEW_EXECUTION_ENABLED` is `false`
  (`review/coordinator.rs:39-41`, `407-412`, `421-434`). Holds still reach the
  user. Enablement stays gated on E1-E4 plus an E5 record
  (`docs/project/specs/2026-09-24-auto-review-signals-routing-design.md:234-237`).
- The status bar renders `approval:<mode>` and, when a reviewer id is known,
  `auto:<id>` (`status_bar.rs:149-159`). Both segments use the muted style
  except `approval:full-access`, which is red and bold
  (`status_bar.rs:346-351`; the composer styles spec moved the reviewer
  segment from accent to muted, `2026-10-17-composer-styles-design.md:164-172`).
  Nothing in that string says execution is compiled off. The UI learns the
  reviewer id from `ReviewerStatusChanged` (`yach-proto/src/lib.rs:1048-1055`,
  consumed at `crates/yach-ui/src/app.rs:1266-1280`), which the backend emits
  on reconnect and on reviewer lifecycle changes (`runner.rs:1885-1890`,
  `runner/extension_state.rs:216`, `230`, `432`, `567`). Nothing on the wire
  carries the gate state, and the constant is `pub(crate)` to the backend; the
  UI crate does not depend on `yach-backend`, only on `yach-proto` and
  `yach-trace` (`crates/yach-ui/Cargo.toml:22-23`).
- A pending command row offers Approve once / Approve for this session /
  Reject. An edit row offers Approve / Reject only (`transcript.rs:739-758`).
  `ReviewOrigin` is on the wire (`yach-proto/src/lib.rs:722-726`, carried by
  `CommandReviewSummary` at `744-752` and `LocalEditPreviewSummary` at
  `729-740`). The runner sets it for commands only: `Risk` on a reviewer hold
  (`runner.rs:9011-9018`, `9278-9283`, `9301`), `ReviewerError` on a reviewer
  failure (`runner.rs:9409`), `None` on a plain ask (`runner.rs:9513`). The
  edit path always sends `None` (`runner/local_edit.rs:235-251`) and discards
  the reviewer's route when it falls through to the manual row
  (`runner.rs:8633-8636`). The hold reason strings exist in the runner
  (`runner.rs:9272-9277`, `9383-9391`) but are not on the wire. A search of
  `crates/yach-ui` finds `review_origin` only in test fixtures, so a held
  prompt, a reviewer-error prompt, and a gate-disabled prompt all look the
  same.
- `full-access` keeps its confirmation and danger styling
  (`approval_selector.rs:56-75`, `status_bar.rs:346-348`).

There is no sandbox. Host commands run on the host through `bash -c` with a
null stdin, a cleared and filtered environment, and their own process group
(`shell.rs:337-348`). Working-directory confinement (`runner.rs:9139-9163`),
timeout clamps (`shell.rs:120-126`), secret-shaped env stripping
(`shell.rs:242-260`), and process-group kill are mitigations, not isolation
(full-access spec, "Honest Safety Boundary"). The coordinator is told
`SandboxState::None` (`runner.rs:5774`). Hard denials and integrity checks are
not mode decisions: invalid workdir (`runner.rs:9139-9163`), unknown executor
(`runner.rs:9165-9177`), wrong capability or risk (`permission.rs:488-492`),
edit hash or precondition failure, extension self-approval, and resource-root
validation. A mode cannot make an invalid action valid.

The accepted signals spec forbids the bypass this outcome needs: "A
deterministic bypass may be keyed only to explicit user grants or an enforced
sandbox capability" and "may never be keyed to command-name patterns"
(`2026-09-24-auto-review-signals-routing-design.md:180-193`). Approving this
design amends that paragraph for one closed classifier. It does not amend the
execution gate, the signal set, or the enablement suites.

`.github/risk.toml` marks `permission.rs`, `shell.rs`, and `review*` high
tier (`.github/risk.toml:27-48`). A classification change is a high-tier diff
and is reviewed as one; `AGENTS.md:78-91` requires a test that the unsafe case
is refused, not only that the allowed case works.

## Decision

Keep the four modes. Change what `review` does to a host command. After
restrictions, the user allowlist, and the session grant, a closed, fail-closed
classifier may allow a command whose every segment is a known read-only form.
Unknown, composite-but-unparseable, redirecting, substituting, or
argument-unsafe commands still ask. Hard denials and restrictions stay first
and are not classifier decisions.

This is approach A from the brainstorm. Approach B (a fifth default mode,
leaving stored `review` strict) was rejected: four modes already read as
separate mechanisms, and the outcome is a better default, not a new posture.
Approach C (honest auto-review UI only, no classifier) was rejected because
disclosure alone leaves the prompts in place. The re-scoped done-when is both
the classifier and the auto-review UI; disclosure alone still does not meet it.

Edits stay prompted in `review`. `accept-edits` remains the way to skip them.
The signals spec kept edit review because `scope_conflict` caught
test-weakening diffs
(`2026-09-24-auto-review-signals-routing-design.md:186-189`); that finding
stands.

`auto-review` stays a separate, session-only, opt-in axis for the residue
`review` still asks about. It is not a second classifier, and the classifier
does not call the reviewer. Its UI must say, per request, what it did, and
the status bar must say execution is off.

`full-access` is unchanged: session-only, never persisted, confirmation and
danger styling kept, and it still allows non-allowlisted commands with reason
`approval_mode_full_access`.

Session grants are unchanged: exact command string plus resolved cwd, memory
only, never a prefix or a family.

## Authority order

`decide_shell` stays the single decision function. The runner consumes its
result and does not grow a second mode branch. The runner computes the
classifier verdict the way it already computes the allowlist and grant
results, and passes it in as one more boolean (`runner.rs:9190-9198`). The new
step is one arm inside the existing `Review | AcceptEdits` match
(`permission.rs:516-526`), evaluated only after the steps that already return:

1. Hard denials that already fail before `decide_shell` (unknown executor,
   invalid workdir, and the other integrity checks). Unchanged.
2. Standing user restrictions (`check_restrictions`). A match holds even when
   the command would classify as read-only, including under `full-access` and
   an allowlist. Unchanged.
3. User allowlist (`shell_user_allowlist`). Unchanged. An allowlisted command
   is allowed for that reason, not reclassified.
4. Session grant (`shell_session_grant`). Unchanged.
5. **Read-only classifier**, only when the mode is `review` or
   `accept-edits`. An allow records reason `read_only_classifier`, reviewer
   `None`, mode `Allow`. Anything the classifier does not accept falls
   through.
6. Mode, unchanged for the residue: `review` and `accept-edits` ask with
   `approval_mode_requires_review`; `full-access` allows with
   `approval_mode_full_access`; `auto-review` returns `route_to_reviewer`.
7. **Execution seam**, only for a `read_only_classifier` allow. It resolves
   the program from a fixed system `PATH`, rewrites the command into its
   helper-disabled form, and builds a fixed environment. It cannot widen the
   decision. If it refuses, the runner records a second
   `PermissionDecisionRecorded` with reason `read_only_seam_refused`,
   outcome `needs_user_review`, and the command goes down the ordinary
   `review` / `accept-edits` ask path, as if step 5 had fallen through.
   Allowlist and session-grant allows do not pass through the seam: those are
   the user's own authority and run the command as written, as today.

`accept-edits` gets the same classifier as `review`. Its only difference
remains edit transactions. The approval-modes spec said the two modes share
host-command behavior; this keeps that, by moving both.

The classifier never runs for `auto-review`. A command `review` would have
classified is still sent to the reviewer when that mode is selected, so the
reviewer and the gate stay the authority for that session. The classifier
never runs under `full-access`, which already allows.

## Classifier

One function, pure over
`(command, project_root, cwd, &SensitivePathPolicy) -> Allow | Ask`. `cwd`
is the resolved working directory the runner already confined inside the
project root (`runner.rs:9139-9163`); the command's relative paths are
relative to it, not to the root. The policy is the effective sensitive-path
policy the runner already resolves for the project (`sensitive_paths.rs:174-189`,
loaded at `runner.rs:10589-10590`), passed in as data. The function does not
consult the mode, the reviewer, the network, the environment, or the
filesystem except to canonicalize path arguments as specified below. `Ask`
carries no partial allow: a command is allowed only when every segment is
allowed.

The classifier is the pure half of a two-part decision. Everything that
depends on the host, on `PATH`, on the process environment, or on repository
configuration is not in the classifier; it belongs to the execution seam
below, which runs after `decide_shell` and before spawn, and which is the
only place that consults any of those. A spec reader can therefore check the
classifier against the corpus alone, and the seam against its own tests.

The function lives in `shell.rs` beside `auto_run_eligible`, which owns the
parse gate it reuses; `decide_shell` consumes the verdict. Both files are high
tier.

### Parse gate

Reuse the conservative segment split (`shell.rs:163-232`). The command is
`Ask` immediately when that split returns `None`, or when any segment fails
`shell_words` splitting, is empty, or begins with a word containing `=`.

That already covers, and this spec keeps:

- command substitution `$(...)` and process substitution, anywhere outside
  single quotes;
- backticks;
- redirects `>` and `<`, including `cat x > y` and `>>`;
- background `&`;
- unterminated quotes;
- an env-assignment prefix (`FOO=bar cmd`) on any segment.

`&&`, `||`, `;`, `|`, and newlines split into segments. A pipeline is allowed
only when every segment classifies as read-only. A list is allowed only on
the same rule. One unrecognized segment asks for the whole command.

Wrappers are not stripped. A segment whose first word is `env`, `xargs`,
`sudo`, `nice`, `nohup`, `time`, `stdbuf`, `watch`, or any other prefix the
table does not name, asks. The table matches the first word only, after
`shell_words` splitting. That word
must be a bare program name: a word containing `/` asks, whether absolute
(`/usr/bin/cat`) or relative (`./cat`, `bin/git`). The executor runs the
command through `bash -c` in the project directory (`shell.rs:337-348`), so a
relative first word is a project-local file the project controls, not the
program the table named; a bare name is resolved by the execution seam, not
by the user's `PATH`. A bare name not in the table asks.

### Never allowed

These programs ask in every form, including with no arguments and inside a
pipeline whose other segments are read-only:

- interpreters and runners: `python`, `python3`, `uv`, `pytest`, `cargo`,
  `npm`, `npx`, `pnpm`, `yarn`, `node`, `deno`, `bun`, `ruby`, `perl`, `php`,
  `make`, `just`, `rake`, `go`, `rustc`, `pip`, `pipx`;
- shells: `bash`, `sh`, `zsh`, `dash`, `fish`, `ksh`, and `busybox`;
- evaluators and launchers: `eval`, `exec`, `xargs`, `env`, `sudo`, `doas`,
  `nice`, `nohup`, `time`, `stdbuf`, `watch`, `script`, `unshare`, `nsenter`,
  `ssh`, `scp`, `rsync`, `curl`, `wget`, `nc`, `ncat`, `socat`.

`sed` is excluded. In-place `-i`, `w` write commands, and `e` execute
commands are not reliably separable from a read invocation by a flag scan, so
no `sed` form is read-only. `awk` is excluded for the same reason (`system`,
redirection inside the program).

`git` and `jj` write, publish, and config subcommands are not in the table.
Absence is the denial; the table does not try to list every dangerous
subcommand.

### Path rule

A path argument is allowed only when it is relative, contains no `..`
component, resolves inside the canonical project root, and is not a
sensitive path.

- Resolution uses the same confinement idea as
  `ResourceRoot::resolve_existing` (`resource.rs:627-661`): join to the
  resolved `cwd`, canonicalize, and require the result to be inside the
  canonical project root (`starts_with` the canonical root, and not the
  root's parent). A symlink whose canonical target leaves the root is `Ask`,
  as `SymlinkEscapesRoot` already is for resources.
- A path that does not exist yet is `Ask`. Canonicalize cannot prove where a
  not-yet-created path, or a dangling symlink, would land, and a read-only
  command has no need to name one.
- An absolute path is `Ask`, even when it points inside the root. The rule is
  relative-only so the classifier does not grow a second way to spell "here".
- A glob metacharacter (`*`, `?`, `[`) asks when it appears outside single
  quotes, in any segment. The shell expands those before the program runs,
  and the expansion can name a symlink the classifier did not see. Inside
  single quotes the shell does not expand, so `rg -n 'decide_[a-z]+' src`
  keeps its pattern. Double quotes and backslash escapes do not get that
  exception: the classifier does not re-implement shell expansion, and an
  unquoted or double-quoted metacharacter asks.
- `--` ends flags for programs whose rule says it does. Tokens before `--`
  are flags; tokens after are operands. A flag-like operand after `--` is an
  operand, still subject to the path rule when the program's operands are
  paths.
- **Sensitive paths.** The canonical path, made relative to the project
  root with `/` separators (the same normalization as
  `ResourceRoot::normalized_relative_path`, `resource.rs:663-672`), is
  checked with `SensitivePathPolicy::denies` (`sensitive_paths.rs:192-196`).
  A match is `Ask`. This is the same chokepoint Yach's own read, search, and
  list tools consult (`resource.rs:308-313`, `337`, `487`, `548`), applied
  with the same effective policy, so `cat .env` asks in `review` exactly as
  `read_text_file(".env")` is refused. The sensitive-file spec left the
  shell tool out of its scope and said a shell design "must address this
  boundary itself" (`2026-07-14-sensitive-file-deny-design.md:39-41`); this
  rule is that boundary for classified commands. A control-plane path
  (`.git`, `.jj`, `.yach`, `target`, `.hg`, `.svn` as any component,
  `resource.rs:278-285`) is also `Ask`. A directory operand is checked by
  its own relative path (so `cat secrets/` asks) and by the subtree rule
  below; a directory that passes both may still contain sensitive files,
  and the read-set rule below decides whether the program may walk it.
- **Subtree rule.** `denies` on a directory's own path is not enough:
  `secrets/**` matches `secrets/private.txt` but not `secrets`, and a
  walker's exclusions filter only the entries it finds, not the directory
  it was asked to start from (probe, GNU coreutils 9.11: `ls -A
  --ignore=secrets secrets` printed `private.txt`, and so did `ls` from
  `cwd = <root>/secrets`). So for every deny pattern that ends in `/**`,
  the classifier takes the part before `/**` as a directory pattern,
  compiled the same way as the policy compiles the original (a pattern
  with `/` stays anchored at the root): `secrets/**` gives the root-level
  `secrets`, and `**/.ssh/**` gives `**/.ssh`, any depth. It asks when that
  directory pattern matches the directory or any of its ancestors inside
  the root. The directory is each directory operand, and, for a filtered
  walker with no path operand, the resolved `cwd` it would walk. So `ls
  secrets`, `ls .ssh`, `ls secrets/sub`, `rg
  TOKEN secrets`, and a bare `ls` or `rg TOKEN` with `cwd` inside `secrets`
  all ask. Trailing-`*` patterns need no extra step: `id_rsa*` already
  matches the directory `id_rsa.d` itself, so `denies` asks for it.

`pwd` has no path argument. `.` as an operand is the resolved `cwd`, which
is inside the root; it is allowed when it passes the subtree rule, so `ls .`
from `cwd = <root>/secrets` asks. An empty operand is `Ask`.

The subtree check is pure: it reads only the policy's patterns and the
canonical path the path rule already computed.

### Read-set rule

Checking the operands is enough only when the operands are the whole set of
files the program reads. Every program in the table is in one of three
classes, named in its row:

- **Explicit.** The program reads exactly its path operands: `cat`, `head`,
  `tail`, `wc`, `grep`, `diff`, `stat`, `file`, `basename`, `dirname`,
  `git blame`, and `du -s`, which stats a tree but opens no file and prints
  only its operands. The path rule on each operand is the whole check. A
  directory operand asks for the file readers (`cat dir` fails anyway, and
  `grep pattern dir` or `diff -r a b` would make the set implicit); `du -s`
  and `stat` accept a directory operand because they print nothing from
  inside it.
- **Filtered.** The program walks a tree, and the execution seam appends an
  exclusion for every deny pattern of the effective policy so the walk never
  opens or names a sensitive file: `rg`, `ls`, and the `git` read
  subcommands. The classifier allows the walk only because the seam will
  filter it; the "Execution seam" section states the exact flags per program
  and the tests that prove the filter holds. A walk whose start operand is
  itself inside a denied or control-plane directory asks under the path rule
  above, so the filter never has to work from inside the set it excludes.
- **Unfiltered, so not in the table.** `find` and `tree` walk a tree and
  have no exclusion the seam can trust: `find` would need the user's
  expression rewritten around `-prune`, and `tree -I` matches basenames
  with its own wildcard rules. Both are removed from the table; every form
  asks.

Allow patterns in the policy (`DEFAULT_SENSITIVE_ALLOW_PATTERNS`,
`sensitive_paths.rs:40-45`, and user `files.allow`) are honored by the
operand check because `denies` applies them. The seam's walk exclusions do
not re-implement the allow carve-outs: a filtered walk simply does not show
`.env.example` either. That is a loss of recall, not of safety, and an
explicit `cat .env.example` still works.

### Program table

Flags not listed in a program's allow rule are denied. Repeated allowed flags
are allowed. `--` is allowed only where the rule says so. A combined short
flag (`-la`) is allowed only when every letter in it is an allowed short flag
for that program; otherwise the segment asks. "None (cwd)" below means the
program defaults to the resolved `cwd`, which the runner already confined
inside the root. "Read set" names the class from the read-set rule; the
seam column is what the execution seam adds to the command before spawn.

| Program | Read set | Allowed | Denied, in addition to anything unlisted | Seam adds |
| --- | --- | --- | --- | --- |
| `ls` | Filtered | No arguments, or path operands under the path rule. Short flags `-l`, `-a`, `-A`, `-h`, `-1`, `-C`, `-F`, `-d`, `-R`, `-t`, `-S`, `-r`, `-G`, combined freely. Long flags `--all`, `--almost-all`, `--human-readable`, `--classify`, `--directory`, `--recursive` | Any flag that takes an argument, including `-w`, `--quoting-style`, `-I`, `--ignore`, `--hide` | `--ignore=<pattern>` per deny pattern, see seam |
| `cat` | Explicit | `-n`, `--number`, `-b`, `--number-nonblank`. One or more file operands. `--` | Zero operands (stdin), `-` as an operand, a directory operand | nothing |
| `head`, `tail` | Explicit | `-n <N>`, `-c <N>`, `--lines=<N>` or `--lines <N>`, `--bytes=<N>` or `--bytes <N>`. `<N>` is a decimal integer with an optional leading `+` or `-`. One or more file operands. `--` | `-f`, `--follow`, `--retry`, `-F`, any flag that names a pid, a descriptor, or an output file. Zero operands, a directory operand | nothing |
| `wc` | Explicit | `-l`, `-w`, `-c`, `-m`, `--lines`, `--words`, `--bytes`, `--chars`. One or more file operands. `--` | `--files0-from` and any flag that names a file by option. Zero operands, a directory operand | nothing |
| `grep` | Explicit | `-n`, `-H`, `-h`, `-i`, `-E`, `-F`, `-w`, `-x`, `-c`, `-o`, `-v`, `-a`, and the long forms `--line-number`, `--with-filename`, `--no-filename`, `--ignore-case`, `--extended-regexp`, `--fixed-strings`, `--word-regexp`, `--line-regexp`, `--count`, `--only-matching`, `--invert-match`, `--text`. Pattern is the first operand, or one or more `-e <pattern>` / `--regexp=<pattern>`. Every later operand is a file. `--` | `-r`, `-R`, `--recursive`, `-f`, `--file`, any exclude or include flag. Zero path operands, a directory operand | nothing |
| `rg` | Filtered | `-n`, `-i`, `-F`, `-w`, `-c`, `-l`, `-H`, `-U`, `--multiline`, `--json`, `--no-heading`, and the long forms `--line-number`, `--ignore-case`, `--fixed-strings`, `--word-regexp`, `--count`, `--files-with-matches`. Pattern is the first operand, or one or more `-e` / `--regexp`. File or directory operands under the path rule, or none (cwd). `--` | `--pre`, `--pre-glob`, `--hostname-bin`, `-g`, `--glob`, `--iglob`, `-.`, `--hidden`, `--no-ignore`, `-u`, `-uu`, `-uuu`, `--no-config`, and any other flag that takes a command or a path by option. A file operand that matches the policy asks under the path rule | `--no-config`, then `-g '!<pattern>'` per deny pattern, see seam |
| `git` | Filtered (`status`, `diff`, `log`, `show`), Explicit (`blame`, `rev-parse`, `branch --list`) | The git rule below. No row here adds a flag | `-c`, `-C`, `--git-dir`, `--work-tree`, `--namespace`, `--config-env`, `--exec-path`, `-p`, `--paginate`, `--no-pager`, `--attr-source`, and any global option | the git launch form, see seam |
| `diff` | Explicit | One or two file operands. `-u`, `-q`, `-N`, `-y`. `--` | `-r`, `--recursive`, any flag that names an output file. Zero operands, a directory operand | nothing |
| `stat` | Explicit | One or more path operands, files or directories. `-t`, `--terse`. `--` | `-c`, `--format`, `--printf`, `-f`, `--file-system`. Format strings are not scanned, so no format flag is allowed | nothing |
| `file` | Explicit | One or more file operands. `-b`, `--brief`, `-i`, `--mime`. `--` | `-f`, `-m`, `-M`, and any flag that names a magic file. A directory operand | nothing |
| `pwd` | none | No arguments, or exactly one of `-L`, `-P` | Any operand, any other flag | nothing |
| `du` | Explicit | `-s` or `--summarize` is required. `-h`, `--human-readable`. Path operands, files or directories, or none (cwd). `--` | `-d`, `--max-depth`, `-a`, `--all`, `--exclude`, and any other flag. Without `-s` | nothing |
| `which` | none | One or more operands that contain no `/` | Any operand containing `/`, any flag | nothing |
| `basename`, `dirname` | Explicit | Exactly one operand: a relative path under the path rule, or a name containing no `/` | A second operand, any flag | nothing |
| `echo` | none | Literal operands and no flag. Zero operands is allowed | Any flag, including `-e` and `-n` | nothing |
| `true`, `false` | none | No arguments | Any argument | nothing |

Not in the table, so every form asks: `find`, `tree`, `realpath`, `printf`,
`cd`, `sed`, `awk`, `cut`, `sort`, `uniq`, `tr`, `nl`, `od`, `hexdump`,
`strings`, `column`, `date`, and every program in "Never allowed". `find`
and `tree` are out under the read-set rule: they walk, and the seam has no
exclusion it can trust for either. `printf` is out because `%n` is not
scanned. `realpath` is out because it prints paths outside the root. `cd` is
out because the executor runs one command and a `cd` segment does not change
that command's cwd. `sed` and `awk` are out because a flag scan cannot
separate `-i`, `w`, and `e` (and `awk`'s `system`) from a read.

Shipping code must not treat an unlisted program or flag as harmless. Adding one is a spec edit.

### Git rule

The segment is `git` plus a read subcommand, with no global option before the
subcommand. The classifier checks the submitted words only; the execution
seam rewrites the segment into the launch form below before it runs, because
a plain `git diff`, `git log -p`, `git show`, or even `git status` runs
programs named by repository configuration that no flag on the submitted
command can see. Allowed subcommands and their flags:

- `status`: no flag, or any of `-s`, `--short`, `-b`, `--branch`,
  `--porcelain`. `--untracked-files=no` and `--untracked-files=normal` are
  allowed; any other value asks. Pathspecs, if present, use the path rule.
- `diff`: no flag, or any of `--stat`, `--name-only`, `--name-status`,
  `--numstat`, `--color-moved`, `-U<N>` / `--unified=<N>` with a decimal
  `<N>`, `-- <pathspec>`. Pathspecs use the path rule. `--output`,
  `--ext-diff`, and `--textconv` are denied.
- `log`: `-n <N>` or `-<N>` with a decimal `<N>`, `--oneline`, `--stat`,
  `--name-only`, `-p`, `-- <pathspec>`. Pathspecs use the path rule.
  `--pretty` and `--format` are denied, so format placeholders are not
  scanned. `--output` is denied.
- `show`: an optional revision matching `^[A-Za-z0-9_./^~@-]+$` and
  containing no `:`, then the same flags as `log`. A `:` in the revision
  can name a path in another tree and is denied. `--output` is denied. The
  seam appends `^{commit}` to the revision so `show` cannot print a blob or
  a tree by hash, which would bypass the pathspec filter.
- `blame`: `-L <range>` with `<range>` matching `^[0-9]+,[0-9]+$` or
  `^[0-9]+$`, and one file operand under the path rule. No other flag.
- `branch --list`: `--list` is required. An optional pattern operand is
  allowed only when it contains no `/`. `-d`, `-D`, `-m`, `-M`,
  `--set-upstream-to`, and any flag other than `--list` are denied.
- `rev-parse`: any of `--verify`, `--quiet`, `--short`, `--abbrev-ref`, and
  one revision operand matching the `show` revision pattern. `--git-path`
  and `--sq-quote` are denied, as is any flag not listed here.

A pathspec operand is a plain relative path under the path rule. A pathspec
magic prefix (`:(`, `:/`, `:!`, `:^`) asks, so the user cannot write an
`:(exclude)` or `:(top)` that competes with the seam's own exclusions, and
a pathspec that names a sensitive path asks under the path rule.

Denied on every git segment, wherever they appear: `-c`, `--config-env`,
`-C`, `--git-dir`, `--work-tree`, `-p`, `--paginate`, `--no-pager`,
`--attr-source`, `--output`, `--ext-diff`, and any subcommand not listed. An
env-assignment prefix such as `GIT_PAGER=less` is already an ask from the
parse gate. `git -c core.pager=less status` asks because `-c` is present,
not because the classifier inspects the pager value.

What repository configuration can run during the allowed subcommands, from
the git documentation and from a probe repository with every helper set to
a marker script (git 2.55.0, 2026-11-17):

| Helper | Set by | Runs during | Neutralized by |
| --- | --- | --- | --- |
| external diff driver | `diff.external`, `diff.<driver>.command` + attributes, `GIT_EXTERNAL_DIFF` | `diff`, `log -p`, `show` | `--no-ext-diff` |
| textconv filter | `diff.<driver>.textconv` + attributes | `diff`, `log -p`, `show`, `blame` | `--no-textconv` |
| clean filter | `filter.<driver>.clean` / `.process` + attributes | `status`, `diff`, `blame` (worktree refresh) | `--attr-source=<empty tree>` plus `-c core.attributesFile=/dev/null`; see the residual below |
| fsmonitor daemon or hook | `core.fsmonitor` | `status`, `diff`, `blame` | `-c core.fsmonitor=false` |
| `post-index-change` hook | `core.hooksPath` | `status` | `-c core.hooksPath=<nonexistent>` |
| pager | `core.pager`, `pager.<cmd>`, `GIT_PAGER`, `PAGER` | every subcommand | `--no-pager` and no `PAGER` in the fixed environment |
| signature verification | `log.showSignature` + `gpg.program` | `log`, `show` | `--no-show-signature` and `-c log.showSignature=false` |
| submodule recursion | `.gitmodules`, `submodule.recurse`, `status.submoduleSummary` | `status`, `diff` spawn a child `git` whose own config applies | `-c diff.ignoreSubmodules=all -c status.submoduleSummary=false -c submodule.recurse=false`; the child's `core.fsmonitor` was still invoked with the first two alone, and was not with all three |

In the probe, the submitted commands ran the external diff, textconv, clean
filter, fsmonitor, and index hook; the launch form below ran none of them.
One residual stays: `$GIT_DIR/info/attributes` outranks every attributes
source and is not covered by `--attr-source` or `core.attributesFile`, so a
clean filter attached there still ran on `diff`. The seam therefore
preflights that file (see the seam) and refuses when it exists.

`rev-parse` and `branch --list` ran nothing in the probe and have no helper
surface in the documentation; the seam still launches them in the same
fixed environment.

### Jj rule

`jj` is not in the allow table. Nearly every `jj` command snapshots the
working copy and writes a new operation before it does anything else
([Working copy](https://docs.jj-vcs.dev/latest/working-copy/), "Most `jj`
commands you run will commit the working-copy changes if they have changed",
read 2026-11-17). `jj status`, `jj log`, `jj diff`, `jj show`, and `jj op log`
are reads in intent and writes in effect. Allowing them would make the
classifier a mutation bypass.

`--ignore-working-copy` skips that snapshot
([CLI reference](https://docs.jj-vcs.dev/latest/cli-reference/), "If you want
to avoid snapshotting the working copy and instead see a possibly stale
working-copy commit, you can use `--ignore-working-copy`", read 2026-11-17).
This spec does not special-case that flag. A user who wants unattended `jj`
reads allowlists the exact command. The classifier will not grow a jj
exception in this slice.

### Execution seam

A command the classifier allowed does not run as the user typed it in the
user's environment. It runs through a fixed launch path built by the runner
between `decide_shell` and `PreparedCommand` (`runner.rs:9179-9184`), using
the same `HostCommandExecutor` (`shell.rs:329-348`) with a different
`command` string and `env`. The seam is the only place that consults the
host: `PATH`, the process environment, the repository's git state. It cannot
allow anything; it can only run what the classifier allowed in a narrower
form, or refuse. Allowlist, session-grant, and `full-access` runs do not
pass through it and are unchanged.

**Program resolution.** The first word of every segment is looked up, as a
bare name, in a fixed system `PATH`: a constant list in `shell.rs`, by
default `/usr/local/bin:/usr/bin:/bin`, overridable only by the user's
`~/.yach/config.json` (`shell.system_path`, user scope only, the same
authority rule as `shell.allow`, `shell.rs:94-97`). The seam rejects any
configured entry that canonicalizes inside the project root. The user's own
`PATH` is never read. The first regular, executable file named `<dir>/<word>`
wins; if none exists, or the winner canonicalizes inside the project root,
the seam refuses. The segment's first word is replaced by that absolute path
in the launch form, so `bash -c` does not search again. A project cannot
shadow `cat` by adding `./node_modules/.bin` or a direnv `bin/` to `PATH`,
because neither is in the fixed list. On this host that list is
`/run/current-system/sw/bin` and `/etc/profiles/per-user/<user>/bin` (NixOS
has no `/usr/bin/cat`); the default is wrong there, which is why the config
override exists, and the seam refusing with `read_only_seam_refused` is the
honest result until it is set.

**Environment.** The launch environment is built from nothing, not from
the inherited one. It holds exactly:

- `PATH` set to the fixed system `PATH`;
- `LANG=C.UTF-8` and `TERM=dumb`;
- `GIT_CONFIG_NOSYSTEM=1` and `GIT_CONFIG_GLOBAL=/dev/null`, so git reads
  only the repository's own config, which the launch form's `-c` pairs
  override where it matters;
- `GIT_TRACE2=0`, `GIT_TRACE2_EVENT=0`, `GIT_TRACE2_PERF=0`, and
  `GIT_NO_LAZY_FETCH=1`.

`HOME`, `TMPDIR`, the user's locale, and every other inherited variable are
absent. Every git invocation the seam makes, preflight included, gets this
environment.

An earlier revision removed a named list from `build_command_env`
(`shell.rs:251-260`), and each review found a variable the list missed:
`GIT_LITERAL_PATHSPECS=1` turned the seam's `:(exclude,glob,top)`
pathspecs into literal names, so `git diff -- .` printed
`credentials.json`; `GIT_TRACE=<absolute path>` made `git status` write
that file, outside edit review; and a list naming `LD_PRELOAD` still let
`LD_DEBUG_OUTPUT` through (review probe, git 2.55.0 and glibc:
`LD_DEBUG=all LD_DEBUG_OUTPUT=<path>` wrote `<path>.<pid>`), and `LD_AUDIT`,
which `ld.so(8)` documents as loading a library into every program (not
probed here: the host has no C compiler to build one).
`GIT_EXEC_PATH`, `GIT_SSH_COMMAND`, `GIT_CONFIG_GLOBAL`, `BASH_ENV`, the
`BASH_FUNC_*` exports, `RIPGREP_CONFIG_PATH`, and `GREP_OPTIONS` are more of
the same class. An allowlist closes the class instead of naming members.
`GIT_CONFIG_GLOBAL=/dev/null` also covers Trace2 targets set in user
config, which `-c trace2.eventTarget=` did not turn off in the probe; the
three `GIT_TRACE2*=0` variables cover the system config too.

In the probe, with `GIT_LITERAL_PATHSPECS`, `GIT_TRACE`,
`GIT_TRACE_SETUP`, a `GIT_CONFIG_GLOBAL` naming a Trace2 target,
`GIT_EXEC_PATH` and `GIT_EXTERNAL_DIFF` pointing at marker scripts,
`LD_DEBUG=all` with `LD_DEBUG_OUTPUT`, and `BASH_ENV` all in the parent
environment, the allowlisted launch printed only `a.txt` from `git diff`
and wrote no file; the inherited environment wrote three trace files.
`git rev-parse`, `git log`, `rg`, `ls`, and `bash -c` all ran normally with
no `HOME`.

**Per-program rewrite.** The seam rewrites the segment's words and joins
them with `shell_words::join` (`shell-words-1.1.1/src/lib.rs:334`), so the
launch string is a quoted argv, not the user's text. Only the segments below
change; every other table program runs with its absolute path and its own
words.

**Translatable patterns.** The policy compiles its patterns with `globset`
(`sensitive_paths.rs:198-213`), where `*` also matches `/` and `{a,b}`,
`[...]`, `?` and `\` have globset's meaning. ripgrep, git `glob`
pathspecs, and `ls --ignore` each support a different subset: in the
review probe (git 2.55.0, ripgrep 15.1.0, GNU coreutils 9.11),
`:(exclude,glob,top)**/{private,secret}.txt` excluded neither file,
`--ignore='{private,secret}.txt'` hid neither, and
`:(exclude,glob,top)**/id_rsa*` did not exclude `id_rsa.d/n.txt`, which the
policy denies because its `*` crosses `/`. The seam therefore translates
only patterns in one subset, where each walker's match is the policy's
match:

- characters other than `*`, `?`, `[`, `]`, `{`, `}`, and `\` are literal;
- `**` appears only as a whole segment;
- `*` appears only as the last character of the last segment (`.env.*`,
  `id_rsa*`), or as its first character when the pattern has no `/` or the
  segment before it is `**` (`*.pem`, `**/*.pem`, `*key*`).

A leading `*` is safe only there because the `**` already matches any
depth; in `secrets/*.pem` globset's `*` would also match
`secrets/a/b.pem`, which no walker's `*` does, so that shape refuses. A
trailing `*` gets a second exclusion, `<pattern>/**`, so a directory whose
name the star matches is excluded with everything under it, as globset
excludes it (probe: adding `**/id_rsa*/**` and `**/.env.*/**` excluded
`id_rsa.d/n.txt` and `.env.d/x` in both git and `rg`). Every default
pattern is in the subset. A user `files.deny` pattern that is not makes
the seam refuse every filtered walk (`rg`, `ls`, and all git reads) with
`read_only_seam_refused`; explicit-operand programs keep working, because
the classifier checks their operands with `denies` itself. Allow patterns
are not translated at all (see the read-set rule), so their syntax does
not matter here.

- `rg`: the seam launches `rg` from the project root, not from `cwd`, so
  the policy's root-relative patterns can be anchored. It rewrites each
  path operand to its root-relative form without a `./` prefix (the
  classifier already canonicalized it under the path rule). When the
  submitted command has no path operand, the seam adds the cwd's
  root-relative path (`crates/x` for `cwd = <root>/crates/x`), and adds
  nothing when `cwd` is the root, so the walk covers exactly what the user
  asked for. It inserts `--no-config` immediately after the program, then
  exclusions for every deny pattern of the effective policy:
  - a pattern without `/` gets the policy's own any-depth expansion
    (`sensitive_paths.rs:198-213`), one `-g '!**/<pattern>'`;
  - a pattern with `/` gets two root-anchored forms, `-g '!/<pattern>'`
    and `-g '!./<pattern>'`. ripgrep matches `-g` globs like `.gitignore`
    globs, against the path as the walk built it: a walk started from a
    named operand yields `sub/private.txt`, a walk with no operand or `.`
    yields `./sub/private.txt`, and each anchored form matches only one of
    them. In the review probe (rg 15.1.0) from the root, `/secrets/**`
    alone excluded `secrets/s.txt` for operand `secrets` but not for no
    operand or `.`; `./secrets/**` alone did the reverse; the pair excluded
    it for no operand, `.`, `./`, `secrets`, and `sub`, and left
    `sub/secrets/s.txt` alone, which `denies` also allows;
  - a pattern whose last segment ends in `*` also gets the `/**` forms
    from "Translatable patterns" above.

  Because the launch cwd is the root, output paths are root-relative, not
  cwd-relative; the review row and the transcript show the launch form so
  that is visible. `--no-config` stops `RIPGREP_CONFIG_PATH` from adding
  `--pre`; a configured `--pre <script>` ran the script for every searched
  file in the probe, and `--no-config` on the same invocation ran nothing.
  Negative globs apply to the walk, not to explicit file operands, which is
  why the classifier checks file operands itself. An exclusion later on the
  command line outranks an earlier inclusion, and `-g` is denied on the
  submitted command, so the user cannot re-include.
- `ls`: `ls --ignore` matches entry names, not paths, so the seam can
  express a deny pattern only when its last segment is a usable name
  pattern. For each pattern:
  - no `/` (`.env`, `*.pem`, `id_rsa*`): `--ignore=<pattern>`;
  - ends in `/**` (`secrets/**`, `**/.ssh/**`, `**/.azure/**`): ignore the
    directory by name, `--ignore=<last directory segment>` (`secrets`,
    `.ssh`, `.azure`);
  - otherwise, with a `/` (`**/.aws/credentials`): `--ignore=<last
    segment>` (`credentials`).

  GNU `ls` matches `--ignore` with a leading-period rule: a leading `*`
  does not match a leading `.`, so `--ignore='*.pem'` still lists `.x.pem`,
  and `--ignore='*.env'` still lists `.prod.env`, which the policy denies
  (probe, GNU coreutils 9.11). For every translated name that begins with
  `*` the seam therefore emits three entries: the name, the name with `.`
  prepended (`.*.pem`), and the name with the leading `*` removed (`.pem`,
  which `.*.pem` does not match). A translated name that is empty, is `*`
  or `**`, or would match every entry makes the seam refuse rather than
  emit it. Naming by basename is broader than the policy (`secrets/**`
  also hides a nested `src/secrets`), which loses recall, not safety. The
  seam also appends `--ignore=.git --ignore=.jj --ignore=.yach`. GNU `ls`
  applies `--ignore` to every directory it lists, including under `-R`; a
  file named directly as an operand is still shown, which the classifier's
  operand check already covers. This is GNU `ls`; the seam's `ls` resolves
  through the fixed `PATH`, and a host whose `ls` rejects `--ignore` fails
  the command, not the boundary.
- `git`: the launch form is

  ```text
  <git> --no-pager --no-optional-locks --no-lazy-fetch
    --attr-source=<empty-tree>
    -c core.fsmonitor=false -c core.hooksPath=<nonexistent>
    -c core.attributesFile=/dev/null -c log.showSignature=false
    -c status.submoduleSummary=false -c diff.ignoreSubmodules=all
    -c submodule.recurse=false
    <subcommand> <subcommand options> <user words>
    -- <user pathspecs> <exclude pathspecs>
  ```

  where the subcommand options are `--no-ext-diff --no-textconv` for `diff`,
  `log`, and `show`, `--no-show-signature` for `log` and `show`, and
  `--no-textconv` for `blame`; the revision of `show` gets `^{commit}`; and
  the exclude pathspecs are `:(exclude,glob,top)<pattern>` for every deny
  pattern in the translatable subset, with the same any-depth expansion and
  trailing-`*` `/**` form as `rg` (`top` anchors to the root, so one form
  suffices), appended to `status`, `diff`, `log`, and `show`. `-c` values
  propagate to child `git` processes
  through `GIT_CONFIG_PARAMETERS` (probe: an alias that echoes the variable
  prints every `-c` pair), so the submodule override reaches the child.
  `<empty-tree>` is the hash of the empty tree for the repository's
  object format (`git hash-object -t tree /dev/null`, run by the seam in
  the same fixed environment; `4b825dc6…` for SHA-1). Before the launch
  the seam runs, from `cwd` in the fixed environment,
  `<git> --no-pager --no-lazy-fetch rev-parse --path-format=absolute
  --show-toplevel --git-path info/attributes`, which prints the working
  tree root and then the absolute attributes path. `--path-format=absolute`
  (git 2.31 and later) is required: without it `--git-path` prints a path
  relative to `cwd` (probe: from `<root>/sub2` it printed
  `../.git/info/attributes`), and in a linked worktree the file lives in
  the main repository's `.git`, not under the worktree (probe: a worktree
  at `/w/w` printed `<main>/.git/info/attributes`). A git that rejects the
  option fails `rev-parse`, which refuses. It refuses in three cases:
  - `rev-parse` fails, which covers "not a repository" with a recorded
    reason instead of a prompt-free error;
  - the canonicalized working tree root is not the canonicalized project
    root. `top` in the exclude pathspecs means the root of git's working
    tree, not yach's project root, so the policy's root-relative patterns
    are only in the right coordinates when the two coincide. A nested
    repository under the project (probe: with project root `/p`, a
    repository at `/p/vendor`, and `files.deny = ["vendor/private.txt"]`,
    `git diff` from `/p/vendor` with `:(exclude,glob,top)vendor/private.txt`
    printed `private.txt`) and a project that is a subdirectory of a
    larger repository both refuse. Rebasing the patterns into the nested
    repository's coordinates was not taken: one equality check is easier
    to test than a translation, and the common case, a project whose root
    is its repository root, is unaffected;
  - the attributes file at the printed absolute path exists, because it is
    the one attributes source nothing else neutralizes.

  The preflight ran no helper in the probe.

  `--no-lazy-fetch` (`git(1)`: "Do not fetch missing objects from the
  promisor remote on demand"; equivalent to `GIT_NO_LAZY_FETCH=1`, which the
  seam also sets) closes the partial-clone path: in a partial clone, a read
  that meets a missing object would otherwise start a fetch, which runs
  repository-configured programs such as `remote.<name>.uploadpack`, a
  transport helper, or a credential helper, and writes objects. With it,
  the read fails on the missing object instead. A classified git read in a
  partial clone can therefore fail where the same command run by hand would
  fetch; that is a loss of function, not of safety, and the user can
  approve the command as typed.

**Refusal.** When resolution fails, a configured entry is inside the root,
or the git preflight refuses, the runner records
`PermissionDecisionRecorded` with reason `read_only_seam_refused` and
`outcome: needs_user_review`, then continues down the ordinary ask path for
the mode. The user sees the ordinary review row for the command as typed;
if approved, it runs as typed, because that is the user's own decision. The
seam never rewrites a user-approved command.

**What the seam does not do.** It does not sandbox, confine the network, or
limit what an approved program may read beyond the filters above. It does
not protect against a `cat` that is itself trojaned in the system `PATH`.
Those are the sandbox outcome's. It is not a pure function, and the spec
does not pretend it is: it is the single, tested place where the host is
consulted, and its tests build real directories, real `PATH` entries, and a
real repository.

### What the dogfood shapes do

The note's 7 shell prompts were routine `uv run` / `python` inspection. The
command strings were not recovered. Under this table every such prompt still
asks: `uv`, `python`, and `pytest` are in the never-allowed list, in every
form, including `uv run pytest -q` and `python -c 'print(1)'`.

The recoverable ylog session
(`session-2580678-1790971084868581101`, local, not in the repo, 2026-10-02)
has 23 shell reviews, all approved (33 `approve` and 3 `approve_for_session`
decisions across its 36 review rows). None of them pass the classifier. Each
is a runner, an interpreter, a heredoc, a write, or a compound that includes
one of those. Representative rows, all `Ask`:

| Command shape | Verdict | Why |
| --- | --- | --- |
| `python - <<'PY' …` | Ask | `python` is never allowed, and `<` is a redirect the parse gate already rejects |
| `uv init --package … && uv add …` | Ask | `uv` is never allowed, and `uv add` writes |
| `uv run ruff format . && uv run pytest && git status --short` | Ask | `uv` on the leading segments; the `git status --short` segment would pass alone |
| `command -v jj; …; jj status; yach sessions show --help` | Ask | `command`, `jj`, and `yach` are not in the table |
| `jj describe -m '…' && jj new -m '…'` | Ask | `jj` writes; not in the table |
| `git diff --check` as its own segment | Ask | `--check` is not in the git diff allow list |
| `git status --short` as its own segment | Allow | listed flag, no pathspec; runs in the git launch form with the policy's exclude pathspecs |

So the honest expectation, and the owner re-scope, is: the 7 shell prompts in
the cited session do not drop, because they were runners and interpreters.
Those prompts are out of scope for this slice. Their prompt-drop target moves
to a future sandbox/runner outcome. The drop this slice records is inspection
commands the table names (`ls`, `cat`, `rg`, `git status`, `git diff`, `git
log`), which the cited session did not use as standalone prompts. Acceptance
below records that measured drop from a repeat session. It does not promise
that the 7 runner prompts disappear, and a remaining runner prompt is not a
miss.

Commands that do pass, and are the corpus's allow rows: `ls -la`,
`cat src/main.rs`, `head -n 40 Cargo.toml`,
`rg -n decide_shell crates/yach-backend/src`, `git status --short`,
`git diff --stat`, `git log -n 5 --oneline`, `pwd`, `du -sh .`. `rg`
patterns are single-quoted in the corpus where they contain a metacharacter.
`du -sh` is allowed because `-s` is required and `-h` is an allowed short
flag. `find . -name '*.rs' -type f`, an allow row in the 2026-10-15 draft,
is now an ask row: `find` walks, and nothing filters its walk. In this
repository that is the one inspection shape the re-scope gives back.

Shapes the sensitive-path rule turns into asks, none of which the draft
named: `cat .env`, `head -n 5 secrets/prod.env`, `grep -n KEY credentials.json`,
`rg TOKEN .env` (explicit operand), `ls .ssh`, `cat .git/config`. The same
shapes against `read_text_file` are already refused today; the classifier
now agrees with that boundary instead of opening a second door around it.
Shapes that pass but run filtered: `rg -n TOKEN .` (never opens `.env`),
`ls -la` (does not list `.env`), `git diff` (does not show a `credentials.json`
hunk), `git status --short` (does not list an untracked `.env`).

## Auto-review UI while execution is off

`AUTO_REVIEW_EXECUTION_ENABLED` stays `false` until the signals spec's E1-E4
and E5 record pass. This slice does not flip it and does not weaken those
suites.

### Status bar

`ReviewerStatusChanged` (`yach-proto/src/lib.rs:1048-1055`) gains
`execution_enabled: bool` with `#[serde(default)]`. Every emitter
(`runner.rs:1885-1890`, `runner/extension_state.rs:216`, `230`, `432`, `567`)
sets it from `AUTO_REVIEW_EXECUTION_ENABLED`. A missing field decodes as
`false`, which is true of every backend before this change. The UI has no
other way to learn the gate: the constant is `pub(crate)` in the backend and
`yach-ui` does not depend on `yach-backend`.

While the gate is off and a reviewer id is configured, the status bar
replaces `auto:<id>` with `auto:<id> (off)`. The `(off)` is part of the
segment text, not a color change; the reviewer segment keeps its current
muted style (`status_bar.rs:349-351`) and its priority (85,
`status_bar.rs:21`), so a narrow bar still drops it whole before it drops
`approval:<mode>`. When the gate is on, the segment returns to `auto:<id>`
with no suffix. `approval:auto-review` stays. `full-access` danger styling is
unchanged. This is a TUI-visible change and needs the before/after grids
`AGENTS.md` asks for.

### Per-request state

Each request that reaches the reviewer path under `auto-review`, command or
edit, shows exactly one of these states in its row, not only in the status
bar:

| State | When | What the user sees |
| --- | --- | --- |
| Not sent | No reviewer was invoked: restriction hold, allowlist allow, session-grant allow, or a failed tool result (hard denial; `reviewer_unavailable` when `auto-review` is selected with no reviewer host, `runner.rs:9217-9224`) | Whatever the ordinary path already shows. No reviewer line is added |
| Held | The route was `Hold`: a reviewer hold that survived the gate (holds pass through, `coordinator.rs:425-434`), or the evidence-budget hold the coordinator or runner raises before invoking the reviewer (`coordinator.rs:256-262`, `runner.rs:9232-9235`) | `held: <reason>` using the hold-reason label the runner already derives (`evidence_over_budget`, `reviewer_hold_risk`, `reviewer_hold_evidence`, `restriction_ask_first`; `runner.rs:9272-9277`) |
| Would have allowed | The reviewer returned `Execute` and `gate_route` rewrote it to `ReviewFailure::Disabled` (`coordinator.rs:425-434`) | `would-have-allowed (execution off)` and the ordinary ask. The user is asked because execution is off, and the row says that |

To carry that to the client:

- `ReviewOrigin` gains one variant, `ExecutionOff` (wire value
  `execution_off`), set only for the would-have-allowed row. Existing `Risk`,
  `ReviewerError`, and `HumanPerforms` are unchanged.
- `CommandReviewSummary` and `LocalEditPreviewSummary` gain
  `review_reason: Option<String>` (`#[serde(default)]`, omitted when `None`),
  carrying the label the runner already derives for the route: the hold
  reasons above for `Risk`, the failure reasons at `runner.rs:9383-9391`
  (`reviewer_disabled`, `reviewer_stale`, `reviewer_malformed`,
  `reviewer_timed_out`, `reviewer_unavailable`, `reviewer_evidence_failed`,
  `reviewer_oversized`) for `ReviewerError` and `ExecutionOff`. The hold and
  failure reasons are not on the wire today; `review_origin` alone cannot
  render `held: <reason>`.
- The edit path sets both fields from the route instead of discarding it
  (`runner.rs:8633-8636`, `runner/local_edit.rs:235-251`). Today every edit
  preview reaches the client with `review_origin: None`, and the pre-review
  status text reads "auto-review unavailable; user approval required"
  (`crates/yach-ui/src/app.rs:351-360`) whether or not the reviewer ran.
- `docs/protocol/yach-proto-v0.md:110-123` documents `review_origin` values
  and `ReviewerStatusChanged`; both paragraphs are updated in the same
  change.

The transcript prints the state from the row's recorded fields; it must not
infer "would have allowed" from the absence of a hold. A `ReviewerError`
origin is labeled `reviewer error: <reason>`, never `would-have-allowed`. A
`None` origin adds nothing, as today.

Headless `yach run` without `--full-auto` still fails closed on an unresolved
review (`headless.rs:903-920`). A would-have-allowed command is unresolved:
the run fails, and the failure reason names the tool and says execution is
off, read from the payload's `review_origin`, instead of today's "reviewer
held or unavailable" (`headless.rs:909-912`). Headless does not auto-approve
it.

## Decision recording

`PermissionDecisionRecorded` already stores the reason
(`permission.rs:265-288`, rendered by `yach sessions show` as
`permission <capability> <outcome> <reason>` at `sessions.rs:416-429` and
`641-643`). The classifier reuses that field. No new event.

| Authority | `reason` | `outcome` | `reviewer` |
| --- | --- | --- | --- |
| Read-only classifier | `read_only_classifier` | `allowed` | `none` |
| Execution seam refused (follows a classifier allow) | `read_only_seam_refused` | `needs_user_review` | `user` |
| User allowlist | `shell_user_allowlist` | `allowed` | `none` |
| Session grant | `shell_session_grant` | `allowed` | `user` |
| User approved once | `user_approved` | `allowed` | `user` |
| User approved for session | `user_approved_for_session` | `allowed` | `user` |
| User rejected | `user_rejected` | `denied` | `user` |
| Full access | `approval_mode_full_access` | `allowed` | `none` |
| Asked | `approval_mode_requires_review` | `needs_user_review` | `user` |
| Routed to reviewer | `route_to_reviewer` | `needs_user_review` | `auto_review` |

`yach sessions show` prints the reason it already prints. A reader can tell
`read_only_classifier` from `user_approved` and from `shell_session_grant`
without a new subcommand, and a seam refusal reads as two permission lines
on the same request, the allow and then the refusal. The permission line
does not name the command: the summary's target is the operation (`bash`)
and the workdir (`runner.rs:9082-9094`, sanitized at `permission.rs:770-810`),
and the bash tool line's argument summary is `tool payload redacted`
(`tools.rs:2627-2635`). The reader correlates by order within the turn, as
today. JSON output already carries the full summary; it gains no field. The
launch string the seam ran is not recorded: it is a function of the
submitted command, the policy, and the fixed `PATH`, each of which is
recorded or configured, and it would repeat the deny patterns into every
log line.

A would-have-allowed ask leaves these facts in the session log: the
permission decision with reason `route_to_reviewer`; the
`ReviewAssessmentRecorded` event, whose route reason folds every gated
`Execute` into `reviewer_error` (`coordinator.rs:437-471`, the fold at `469`)
and which `sessions show` does not render (`sessions.rs:783-786`); and the
persisted `ToolReviewRequested` payload with `review_origin` and
`review_reason` (`runner.rs:9304-9313`). `sessions show` already prints a
`review <tool_request_id> <decision>` line from `ToolReviewDecisionRecorded`
(`sessions.rs:431-444`, `644-650`); it appends `origin=<review_origin>
reason=<review_reason>` from the matching `ToolReviewRequested` in the same
turn when the payload carried them, so the would-have-allowed row reads
`origin=execution_off reason=reviewer_disabled`. The assessment record's
`reviewer_error` fold is left alone; the review line is the disclosure.

## Headless

The classifier runs in `decide_shell` and the seam in the same runner path,
so `yach run` uses both with no separate code. A classified-read-only command
does not raise a review, and the run continues. A command the classifier
does not accept, or one the seam refuses, still raises a review, and headless
still fails closed unless `--full-auto` selected `full-access`.

`--full-auto` is unchanged: it selects backend `full-access`
(`headless.rs:648-666`) and does not consult the classifier. `--auto-review`
is unchanged: it selects `auto-review` (`headless.rs:668-672`), the
classifier does not run, and an unresolved review fails the run. The two
flags stay mutually exclusive (`headless.rs:162-166`).

## Signals-spec amendment

Replace the second bullet of "Deterministic layer" in
`docs/project/specs/2026-09-24-auto-review-signals-routing-design.md` (the
bullet at lines 190-193) with:

> A deterministic bypass may be keyed only to an explicit user grant, an
> enforced sandbox capability reported in `SandboxState`, or the closed
> read-only command classifier specified in
> `docs/project/specs/2026-10-15-approval-model-design.md`. That classifier
> is a fixed program table with per-program argument rules, evaluated only
> after integrity checks, standing user restrictions, and the user allowlist,
> and only in `review` and `accept-edits`. It fails closed: an unrecognized
> program, a program named by path, an unlisted flag, a path that does not
> resolve inside the project root or that the sensitive-path policy denies, a
> redirect, a substitution, an env-assignment prefix, or a composite command
> it cannot split is an ask, never an allow. A command it allows runs only
> through that spec's execution seam: a fixed system `PATH`, a fixed
> environment, and per-program flags that disable configured helpers and
> exclude sensitive paths from walks. The ban on command-name bypasses
> stands for every other bypass, including writes, interpreters, runners,
> and shells. No sandbox exists yet. This slice adds no bypass other than
> that classifier.

The first bullet of that section (in-workspace edits stay reviewed in
automatic-review mode) is not amended. The execution gate, the signal set,
the routing function, and E1-E5 are not amended.

The approval-modes spec's `review` bullet "Parse-aware user allowlisted
commands run; other host commands ask"
(`2026-08-24-approval-modes-design.md:47`) becomes "Parse-aware user
allowlisted commands run; commands the read-only classifier accepts run; other
host commands ask." The `accept-edits` host-command bullet (`:53`) gains the
same clause. `README.md:36-39` and `:94-98` describe the allowlist as the only
auto-run path and are updated in the same change. All of these land in the
same change as the classifier, not before it.

## Migration

Stored `review` preferences change behavior with no prompt and no schema
change. A project whose `~/.yach/permissions/<project-key>.json`
(`permission.rs:109-121`) says `review` starts allowing classified read-only
commands on the next session. That is the intended default change. Users who
want the old posture have no fifth mode to switch to; they get it back only
by not running those commands, or by a restriction, which still holds. The
notice is a `CHANGELOG.md` entry under Changed, plus the PR description and
the `README.md` edit above; each says this in one sentence, names the program
table, and points at this spec.

No persisted data is rewritten. `full-access` and `auto-review` remain
rejected on persist. Old logs that lack `read_only_classifier` stay valid.
Old review payloads without `review_reason` decode as `None`.

## Evaluation

Two deterministic test sets, no model, own this slice: a corpus for the
pure classifier and a seam suite for the launch path. Neither is a live
suite and neither enters the auto-review enablement gate. The ask rows and
the seam refusals are the refusal tests `AGENTS.md:88-91` requires for a
high-tier change.

### Classifier corpus

The corpus calls the classifier function directly with a temp project root,
a cwd inside it, and the default `SensitivePathPolicy`. A separate
`decide_shell` test asserts the reason and the ordering (classifier verdict
`true` under `review` records `read_only_classifier`; the same verdict under
`auto-review` still records `route_to_reviewer`; under `full-access` still
`approval_mode_full_access`).

Allow rows are the passing commands listed under the dogfood shapes, plus one
pipeline of two passing segments (`git status --short | head -n 5`), one
list (`pwd && git status`), `stat src` (a directory operand to a program
that prints nothing from inside it), and `du -sh src`. The path operands in
those rows exist in the temp project; the test creates them.

Ask rows, each one a separate case:

- `find . -name '*.rs'` (not in the table)
- `find . -delete`
- `tree -L 2`
- `git -c core.pager=less status`
- `git --paginate diff`
- `git --attr-source=HEAD diff`
- `git diff --output=/tmp/x`
- `git diff -- ':(exclude)src'` (pathspec magic)
- `git show HEAD:Cargo.toml` (a `:` in the revision)
- `cat x > y`
- `cat $(hostname)`
- a backtick substitution
- `ls && python -c 'print(1)'`
- `pwd; rm -rf /tmp/x` (the `rm` segment is unrecognized as read-only)
- `FOO=bar ls`
- `bash -c ls`
- `uv run pytest`
- `sed -n '1p' Cargo.toml`
- `xargs ls`
- `./cat Cargo.toml` (relative first word)
- `bin/git status` (relative first word)
- `/usr/bin/cat Cargo.toml` (absolute first word)
- a single-quoted path operand that contains `..` (`cat 'src/../Cargo.toml'`)
- a relative path operand whose symlink canonicalizes outside the project root
- an unquoted glob operand (`ls *.rs`)
- a double-quoted glob operand (`ls "*.rs"`)
- an absolute path inside the root (`cat /absolute/project/Cargo.toml`)
- a not-yet-existing relative path
- a relative path that is inside the root from the root but escapes it from
  the cwd (cwd `sub/`, operand `../../outside`, with `outside` created beside
  the temp root)
- `cat .env`, `head -n 5 .env.local`, `wc -l secrets/prod.env`,
  `grep -n KEY credentials.json`, `rg TOKEN .env`, `stat id_rsa`,
  `file .npmrc`, `git blame .env` (each operand matches the default deny
  list; the files exist in the temp project)
- `cat .env.example` is an **allow** row beside them (the default allow
  carve-out, `sensitive_paths.rs:40-45`)
- `cat sub/.env` and `cat deep/er/.ssh/id_ed25519` (any-depth match)
- `cat .git/config`, `ls .jj`, `cat .yach/config.json` (control plane)
- `ls .ssh`, `ls secrets`, `ls secrets/sub`, `rg TOKEN secrets`,
  `git diff -- secrets` (walk rooted inside a denied subtree; `secrets/**`
  does not match `secrets` itself, so these exercise the subtree rule), and
  a bare `ls`, `ls .`, `rg TOKEN`, and `rg TOKEN .` with
  `cwd = <root>/secrets`
- `cat src` (directory operand to an explicit-set program)
- `grep -r KEY .`, `diff -r a b` (recursive flags on explicit-set programs)
- `rg -g '*.json' TOKEN`, `rg --hidden TOKEN`, `rg -uu TOKEN`,
  `rg --no-config TOKEN` (walk-widening or seam-owned flags on the submitted
  command)
- `ls --ignore=x`, `ls -I x` (seam-owned flag on the submitted command)
- `du -d1 .`, `du .` (without `-s`)
- a user-configured `files.deny` pattern (`*.secret`) with `cat a.secret`,
  asserting the classifier takes the policy it is given and not the default

The symlink case builds a symlink in a temp project and asserts `Ask`. The
glob cases assert `Ask` without creating matching files, because the denial
is the metacharacter outside single quotes, not the expansion. A
single-quoted ordinary path (`cat 'src/main.rs'`) is an allow row, so
single quotes themselves are not a denial. A double-quoted ordinary path
with no metacharacter (`cat "src/main.rs"`) is also an allow row. A cwd case
asserts that `cat Cargo.toml` with cwd `sub/` is `Ask` when only
`<root>/Cargo.toml` exists and `Allow` when `<root>/sub/Cargo.toml` exists:
paths resolve against the cwd, not the root.

A restriction case asserts that a `HumanPerforms` match on `ls` still asks
when the classifier would allow it. An allowlist case asserts the reason
stays `shell_user_allowlist` rather than `read_only_classifier`.

### Seam suite

The seam suite builds a temp project, a temp system `PATH` directory holding
real `cat`, `ls`, `rg`, and `git` (copied or symlinked from the test host; the
test is skipped with a named reason when a program is absent), and where
needed a temp git repository. Each case passes a classifier-allowed command
through the seam and asserts either the launch form and the observed
behavior, or a `read_only_seam_refused` decision in the session log followed
by a `ToolReviewRequested`.

- **PATH shadowing.** A project-local `bin/cat` that writes a marker, with a
  user `shell.system_path` that lists `<root>/bin` first: the seam rejects
  the entry and resolves `cat` from the temp system dir; the marker is not
  written. The same with `<root>/node_modules/.bin` and with a directory
  symlinked into the root. A `shell.system_path` with no `cat` at all:
  refusal, reason recorded, review row shown.
- **BASH_ENV and functions.** `BASH_ENV` pointing at a file that writes a
  marker, and an exported `BASH_FUNC_cat%%` that writes a marker: the seam's
  launch runs `cat Cargo.toml` with neither marker written. The control
  (`shell.allow` containing `cat`, so the seam is bypassed) writes both,
  which is the pre-existing allowlist behavior and is asserted as such.
- **Launch environment.** The parent environment carries
  `GIT_LITERAL_PATHSPECS=1` (alone among the pathspec variables: git
  refuses it combined with `GIT_ICASE_PATHSPECS`, which would make the
  control fail instead of leak), `GIT_TRACE=<abs path>`,
  `GIT_TRACE_SETUP=<abs path>`, `GIT_CONFIG_GLOBAL`
  naming a file that sets `trace2.eventTarget` and `trace2.normalTarget`
  to absolute paths, `GIT_EXEC_PATH` and `GIT_EXTERNAL_DIFF` and
  `GIT_SSH_COMMAND` pointing at marker scripts, `LD_DEBUG=all` with
  `LD_DEBUG_OUTPUT=<abs path>`, and `BASH_ENV` naming a marker script.
  `git diff -- .`, `git status --short`, `rg TOKEN`, and `ls` through the
  seam do not print `credentials.json` and create none of the trace,
  debug, or marker files, for the preflight and the launch alike. The
  test also asserts the launch environment's keys equal the allowlist
  exactly, so a new inherited variable cannot slip in. The control (the
  same commands via `shell.allow`) prints the file and creates the trace
  files.
- **rg config.** `RIPGREP_CONFIG_PATH` naming a file with `--pre <marker
  script>`: `rg TOKEN .` through the seam writes no marker and the launch
  string contains `--no-config`. The same run against a tree with `.env`,
  `sub/credentials.json`, and `secrets/prod.env` containing `TOKEN` matches
  none of them and matches `src/ok.txt`.
- **rg from a subdirectory.** A policy with `files.deny = ["sub/private.txt"]`
  and the default patterns, a tree with `ok.txt`, `sub/private.txt`,
  `sub/ok.txt`, `secrets/s.txt`, and `sub/secrets/s.txt`, each containing
  `TOKEN`. With `cwd = <root>/sub`, both `rg TOKEN .` and `rg TOKEN` match
  exactly `sub/ok.txt` and `sub/secrets/s.txt` (the policy's `secrets/**`
  is root-anchored), never `sub/private.txt` and never a file outside
  `sub`. With `cwd = <root>`, `rg TOKEN`, `rg TOKEN .`, and `rg TOKEN ./`
  each match exactly `ok.txt`, `sub/ok.txt`, and `sub/secrets/s.txt`: the
  root search is the case a single anchored glob form misses. The golden
  launch strings show the root launch directory, the rewritten operands,
  and both anchored forms.
- **Untranslatable patterns.** A user `files.deny` of `{private,secret}.txt`,
  of `secrets/*.pem`, and of `[ab].key`, one at a time: `rg TOKEN`, `ls`,
  and `git diff` through the seam each refuse with `read_only_seam_refused`,
  and `cat ok.txt` still runs. A user `files.deny` of `id_rsa*` against a
  tree with `id_rsa.d/n.txt`: `rg`, `ls`, and `git diff` show nothing under
  `id_rsa.d`.
- **ls filter.** `ls -la` and `ls -R .` through the seam against that tree
  list neither `.env`, `credentials.json`, `secrets`, `.ssh`, `.x.pem`,
  `.pem`, `.prod.env`, nor `.git`, and do list `src` and `README`: a
  filter that hides every ordinary entry fails this case.
- **git helpers.** A temp repository with `diff.external`,
  `diff.<d>.textconv`, `diff.<d>.command`, `filter.<f>.clean`,
  `core.fsmonitor`, `core.hooksPath` with a `post-index-change` hook,
  `gpg.program` with `log.showSignature`, a `.gitattributes` binding the
  drivers, and a submodule whose own `core.fsmonitor` is set, every helper a
  marker script. `git status --short`, `git diff`, `git diff --stat`,
  `git log -n 2 -p`, `git show HEAD`, and `git blame <file>` through the
  seam write no marker. The control (the same commands via `shell.allow`)
  writes markers, asserting the repository really would have run them.
- **git info/attributes.** The same repository with `.git/info/attributes`
  binding the clean filter: the seam refuses every git subcommand with
  `read_only_seam_refused`, both from `cwd = <root>` and from
  `cwd = <root>/src`; the review row is shown. A linked worktree of that
  repository opened as the project: the seam refuses, because the file
  lives in the main repository's `.git`.
- **git pathspec filter.** A commit touching `credentials.json` and
  `secrets/prod.env` beside `a.txt`, with worktree edits to all three:
  `git diff`, `git diff --stat`, `git status --short`, `git log -n 1 -p`,
  and `git show HEAD` through the seam print no line containing
  `credentials.json`, `secrets/`, or the secret content, and do print
  `a.txt`. `git show <blob-hash>` through the seam fails (the `^{commit}`
  peel), so a blob cannot be printed by hash.
- **git outside a repository.** `git status` in a temp project with no
  `.git`: refusal with `read_only_seam_refused`, not a bare error.
- **git root differs from project root.** A project at `<root>` with a
  repository at `<root>/vendor`, `files.deny = ["vendor/private.txt"]`, and
  a worktree edit to `vendor/private.txt`: `git diff` and `git status` from
  `cwd = <root>/vendor` refuse with `read_only_seam_refused` and print
  nothing. A project at `<repo>/app` inside a larger repository: every git
  read refuses. The control (the same project with root at its repository
  root) runs.
- **git partial clone.** A `--filter=blob:none` clone whose
  `remote.origin.uploadpack` is a marker script that then runs
  `git-upload-pack`, with blobs missing locally. `git show HEAD` and
  `git log -n 1 -p` through the seam write no marker and fail on the
  missing object. The control (the same command via `shell.allow`) writes
  the marker. Probed with git 2.55.0 on 2026-11-18: `git show HEAD:a.txt`
  ran the configured script; with `--no-lazy-fetch` it printed `bad
  object` and ran nothing.
- **Launch string.** For each rewritten program, the launch string is
  asserted exactly against a golden value built from the policy's default
  patterns, so a change to the deny list or to the rewrite is a visible
  diff.

The gated-path UI is tested without a model. Inside `yach-backend`,
`cfg!(test)` opens the execution gate (`coordinator.rs:407-412`), so a unit
test that drives the real coordinator cannot observe `ReviewFailure::Disabled`;
the existing `gate_route(false, Execute)` test (`coordinator.rs:1103-1137`)
covers the rewrite, and the runner test feeds a `ReviewFailed { Disabled }`
route into the shell and edit review paths and asserts the persisted
`ToolReviewRequested` payload carries `execution_off` and `reviewer_disabled`.
The `yach-ui` tests render a command row and an edit row for each of the
three states and assert the exact labels. The status bar test renders
`auto:<id> (off)` with `execution_enabled: false` and `auto:<id>` with `true`,
and asserts the segment is still dropped before `approval:<mode>` at a narrow
width. The headless test follows
`review_request_without_full_auto_cancels_and_reports_approval_required`
(`headless.rs:1670`) with an `execution_off` payload and asserts the failure
reason says execution is off.

## Acceptance

1. The signals-spec paragraph, the approval-modes bullets, the README
   sentences, and the protocol doc paragraphs are amended as written above,
   in the same change as the classifier.
2. In `review`, every corpus allow command runs without a `ToolReviewRequested`
   event, and the session log records a `PermissionDecisionRecorded` with
   reason `read_only_classifier`.
3. In `review`, every corpus ask command still raises a review. `accept-edits`
   does not change that.
4. In `review`, a hash-checked workspace edit still asks. `accept-edits`
   still applies the same edit without a review.
5. Every seam-suite case passes: the shadowing, `BASH_ENV`, `rg` config,
   `ls` filter, git helper, git `info/attributes`, git pathspec filter, and
   no-repository cases, each with its control where the suite names one.
   A seam refusal records `read_only_seam_refused` and then raises the
   ordinary review for the command as typed.
6. In `review`, with the default sensitive-path policy, `cat .env`,
   `rg TOKEN .env`, and `ls .ssh` raise a review, and `cat .env.example`
   does not. With a project `files.deny` adding `*.secret`, `cat a.secret`
   raises a review. A classified `rg TOKEN .`, `ls -la`, `git diff`, and
   `git status --short` produce output that names no denied path and no
   denied content, asserted against a temp tree that contains them.
7. A standing restriction holds a command the classifier would allow, under
   `review` and under `full-access`.
8. A session grant still matches only the exact command string plus resolved
   cwd, and is still granted only from "Approve for this session". A
   session-granted or allowlisted command runs as typed, not through the
   seam.
9. With auto-review selected and the gate off, the status bar shows
   `auto:<id> (off)`; a held command or edit row shows `held: <reason>`; a
   reviewer `Execute` shows `would-have-allowed (execution off)` on the ask
   row; a reviewer failure shows `reviewer error: <reason>`. Each is asserted
   by a rendering test, and the status bar by the before/after grids.
10. `yach sessions show` for a classified command prints
    `permission shell_command allowed read_only_classifier`; for a seam
    refusal it prints that line followed by
    `permission shell_command needs_user_review read_only_seam_refused`.
    The same command approved by the user prints `user_approved`. A
    would-have-allowed row prints `origin=execution_off reason=reviewer_disabled`
    on its review line.
11. Headless without `--full-auto` runs a classified command and fails closed
    on a command the classifier does not accept and on a seam refusal.
12. A repeat dogfood session, recorded in `docs/project/records/`, lists each
    shell request from its session log with the recorded permission reason,
    and reports the count with `read_only_classifier` over the total as the
    measured drop for inspection commands. It names the 14-prompt baseline
    from the outcome's dogfood note as context, not as a target: the
    baseline's command strings are unrecoverable, so no before/after count
    against it is possible. `uv` / `python` / `pytest` prompts remain and are
    out of scope; the record says so rather than treating a remaining runner
    or interpreter prompt as a miss. The prompt-drop target for those runners
    is a future sandbox/runner outcome, not this slice.

## Non-goals

- Persistent or prefix command rules saved from prompts. A prefix grant is a
  chaining bypass. Session grants stay exact.
- OS, process, filesystem, or network sandboxing.
- Classifying interpreters or runners, including a "read-only `uv run`"
  exception, and any prompt-drop target for those commands. Explicitly out of
  scope: the runner executes project code the classifier cannot see. That
  drop belongs to a future sandbox/runner outcome.
- Auto-applying edits in `review`.
- Flipping `AUTO_REVIEW_EXECUTION_ENABLED`, editing the signal set, or
  lowering the E1-E5 gate.
- A fifth approval mode, or a strict-`review` compatibility flag.
- A `jj` exception, with or without `--ignore-working-copy`.
- Teaching the classifier `sed`, `awk`, `printf`, `realpath`, `find`, or
  `tree`, and filtering any walker the seam does not already filter.
- Sending the classifier's verdict to the reviewer, or skipping the reviewer
  because the classifier would have allowed.
- Changing the `ReviewAssessmentRecorded` route reason or rendering that
  event in `sessions show`.
- Sandboxing the seam's launch: the fixed `PATH` and environment narrow
  what runs, they do not confine it.

## Risks

- **Stored `review` changes under users.** There is no migration prompt. The
  README sentence and the PR description are the notice. A user who depended
  on `ls` asking will not be asked.
- **The table is a bypass keyed to names.** The signals spec is amended
  because of that, not in spite of it. The amendment is the whole bound:
  closed table, after restrictions, fail closed. Adding a program is a spec
  edit, not a code convenience.
- **Sensitive paths are enforced lexically, before the run.** The
  classifier refuses an operand the policy denies, and the seam filters the
  walkers it allows; neither inspects file contents, and the policy is a
  pattern list, so a secret in `notes.txt` is as visible to `cat` as it is
  to `read_text_file` today. The boundary is the same list at the same
  strength for both tools, which is the sensitive-file spec's own claim
  (`2026-07-14-sensitive-file-deny-design.md:48-62`), extended to the one
  tool it excluded. Two gaps remain and are named rather than papered over:
  a `git log -p` or `git show` of a commit that moved a secret into a
  non-denied path shows it, because the filter is by current pathspec; and
  a filtered walk cannot apply the policy's allow carve-outs, so
  `.env.example` is hidden from `rg` and `ls` but readable by `cat`.
- **The seam is host-dependent code in a security position.** Its program
  resolution, environment, and git launch form are facts about GNU `ls`,
  ripgrep, bash, and git 2.55 on 2026-11-17, each checked here by a probe,
  not by reading. A different `ls` that rejects `--ignore`, or a future git
  that adds a helper hook to `status`, changes the boundary. The seam suite
  runs the real programs so such a drift fails a test instead of passing
  silently; the launch-string golden makes the rewrite reviewable in a
  diff. The default system `PATH` is wrong on NixOS hosts, including the
  one this spec was written on, and the seam refuses there until
  `shell.system_path` is set: a prompt, never a wrong allow.
- **Symlinks and globs.** An unquoted or double-quoted glob asks; the
  classifier does not expand it. A single-quoted pattern is data, and is
  allowed only when the program's rule allows that argument. A symlink
  whose canonical target stays inside the root is allowed. A symlink out
  of the root asks. That is the intended cut.
- **A new wire enum value.** `ReviewOrigin` has no unknown-value fallback
  (`yach-proto/src/lib.rs:720-726`), so a client older than this change fails
  to decode a review payload carrying `execution_off`. The TUI and backend
  ship in one binary; the stdio rpc transport is the cross-version surface,
  and it checks `PROTOCOL_VERSION` for equality (`crates/yach-cli/src/rpc.rs:190-200`).
  The implementation plan states how an older peer is handled (a tolerant
  decoder or a protocol version bump) and tests that choice. The two new
  optional fields are additive and need no such handling.
- **`jj` exclusion will keep prompting in this repo.** yach dogfood uses `jj`
  constantly. Those prompts remain until the user allowlists exact commands
  or switches to `full-access`. That is accepted.
- **The measured drop may be small.** The cited 7 shell prompts were runners
  and interpreters and will still ask. That is the re-scope, not a miss.
  Acceptance requires an honest record of the inspection-command drop, not a
  target count against the 14-prompt baseline. If a repeat session is also
  all runners, the record says the default did not drop for those commands,
  and this outcome is still met by the classifier and the auto-review UI.
- **High-tier review.** `permission.rs`, `shell.rs`, and `review/**` are high
  tier. The classifier lives in `shell.rs`, `decide_shell` consumes it, and
  the diff is reviewed as high tier with the refusal corpus as its test.

## Changes since the 2026-10-15 draft

Code on `main` moved about a month between the draft and this revision. Every
citation was re-read; the line numbers above are current. These are the
changes that affected the design, followed by draft claims the re-read showed
did not hold.

Design-affecting code changes:

- **Status bar restyle** (composer styles, `2026-10-17-composer-styles-design.md:164-172`,
  `status_bar.rs:10-21`, `346-351`). The reviewer segment is muted, not
  accent, and approval moved to priority 99 above context and status. The
  draft said `(off)` "keeps the accent color"; it now keeps the muted style,
  and the spec notes the segment is still dropped whole on a narrow bar.
- **Risk policy moved** from `.sjujperpowers/risk.toml` to
  `.github/risk.toml:27-48`, and `AGENTS.md:78-91` now states review rules
  for high-tier paths, including a required refusal test. The evaluation
  section names the ask corpus as that test.
- **`ReviewerStatusChanged` is emitted** on reconnect and on reviewer
  lifecycle (`runner.rs:1885-1890`, `runner/extension_state.rs`); at the
  draft's base only the reconnect emitter existed. This confirms the status
  bar always has a reviewer id to decorate under `auto-review`.
  `docs/protocol/yach-proto-v0.md:118-123` still says the backend does not
  emit it; that sentence is outside this spec and is corrected when the
  paragraph gains `execution_enabled`.

Draft claims corrected by the re-read (none of these is new code; the draft
was wrong or silent about code that already behaved this way):

- **The UI cannot see the gate.** `AUTO_REVIEW_EXECUTION_ENABLED` is
  `pub(crate)` in the backend and nothing on the wire carries it. The spec
  adds `execution_enabled` to `ReviewerStatusChanged`.
- **Hold reasons are not on the wire.** `CommandReviewSummary` carries only
  `review_origin`; `held: <reason>` needs the reason. The spec adds
  `review_reason` to both review summaries. The draft's "plus the evidence
  refs already recorded" is dropped: evidence refs are not persisted
  anywhere the client or `sessions show` can read.
- **Edit rows discard the route.** Under `auto-review` the edit path falls
  through to the manual row with `review_origin: None`
  (`runner.rs:8633-8636`, `runner/local_edit.rs:249`). The draft's table
  said "each auto-review request row" without covering edits; the spec now
  states that edit rows carry the same three states.
- **`sessions show` does not name the command.** The permission line is
  capability, outcome, reason; the bash tool line's argument summary is
  `tool payload redacted`. The draft said the line names the command.
- **`reviewer_disabled` is not in the session log.** The assessment record
  folds a gated `Execute` into `reviewer_error` (`coordinator.rs:469`) and
  `sessions show` renders neither that event nor `ToolReviewRequested`. The
  draft said `sessions show` prints `reviewer_disabled` on the review line.
  The spec now has the review line read `origin`/`reason` from the persisted
  `ToolReviewRequested` payload.
- **Paths resolve against the cwd, not the root.** The runner runs the
  command in a resolved `workdir` inside the root (`runner.rs:9139-9163`);
  the draft joined path operands to the project root. The classifier now
  takes the cwd, and the corpus gained two cwd cases.
- **Unit tests open the gate.** `gate_execution` passes
  `cfg!(test) || fixture_reviewer` (`coordinator.rs:407-412`), so an in-crate
  test of the real coordinator never sees `Disabled`. The evaluation section
  says how the gated path is tested.
- **"Not sent" listed a classifier allow under `auto-review`**, where the
  classifier never runs. Removed. The no-reviewer-host case is a failed tool
  result (`reviewer_unavailable`), not a row.
- **"Assessed and held" renamed "Held".** The evidence-budget hold is raised
  before the reviewer is invoked (`coordinator.rs:256-262`,
  `runner.rs:9232-9235`), so not every hold is an assessment.
- **A new `ReviewOrigin` value is not transparently additive.** The enum has
  no unknown-value fallback; listed under Risks with a requirement on the
  plan.
- **`--ignore-working-copy` quote re-sourced.** The draft's man-page wording
  is not what the current page says; the CLI reference is quoted instead,
  read 2026-11-17.
- **Acceptance 10 tightened.** The 14-prompt baseline's command strings are
  unrecoverable, so a drop "against" it cannot be measured; the record now
  reports the classifier-allowed share of the repeat session's shell
  requests from its own log.
- **Changelog.** The draft had no changelog to use; the notice now includes a
  `CHANGELOG.md` entry alongside the README sentence and the PR description.

### Changes from the first review of this revision

An automated review of the pull request carrying this revision raised four
findings. All four held against the code, and each changed the design rather
than the risk list:

1. **Program names were matched by basename.** `./cat` or `bin/git` passed
   the argument rules and ran as a project-local script under `bash -c`.
   The first word must now be a bare name (any `/` asks), and the bare name
   is resolved by the execution seam from a fixed system `PATH` that may not
   contain a directory inside the project root; the user's `PATH` is never
   consulted. Corpus rows `./cat Cargo.toml`, `bin/git status`,
   `/usr/bin/cat Cargo.toml`; seam cases for a project `bin/cat`,
   `node_modules/.bin`, and a symlinked directory.
2. **Git read subcommands ran configured helpers.** A probe repository with
   every helper set to a marker script showed `git status` running the
   clean filter, fsmonitor, and the index hook, `git diff` running the
   external diff as well, and `git log -p` / `show` / `blame` running
   textconv. Denying flags on the submitted command could not see any of
   that. The seam now launches every git read in a fixed form
   (`--no-pager --no-optional-locks --attr-source=<empty tree>`, the
   `-c` overrides, `--no-ext-diff --no-textconv --no-show-signature` where
   they apply, `^{commit}` on `show`), preflights `$GIT_DIR/info/attributes`
   because it is the one source nothing else neutralizes, and the helper
   table in the git rule records what each flag disables and what remains.
   The alternative, dropping `diff`, `log -p`, and `show`, was not taken
   because `git diff` and `git log` are the dogfood shapes this slice
   exists to stop prompting for; the seam is the cost of keeping them.
3. **Ripgrep read a config file from the environment.** `RIPGREP_CONFIG_PATH`
   survives `build_command_env`, and a configured `--pre` ran a script per
   searched file in the probe. The seam inserts `--no-config`, removes the
   variable, and the seam suite has the configured-`--pre` case. The other
   table programs were checked the same way: `GREP_OPTIONS` has not affected
   GNU grep since 3.6 (2020) and is removed anyway; `LS_COLORS` and
   `QUOTING_STYLE` change `ls` output only and are removed; no table program
   pages, so `LESSOPEN` is moot; `BASH_ENV`, `ENV`, and exported functions
   (`BASH_FUNC_*`) alter `bash -c` itself and are removed; `POSIXLY_CORRECT`
   changes option parsing and is removed; `TREE_COLORS` is moot because
   `tree` left the table.
4. **Classified commands bypassed the sensitive-file boundary.** `cat .env`
   would have run without a prompt in every default `review` session while
   `read_text_file(".env")` is refused. The classifier now takes the
   effective `SensitivePathPolicy` as an argument and asks for any operand
   it denies, plus control-plane paths; programs are classed by read set,
   explicit operands are checked one by one, the walkers the seam can filter
   (`rg` with `-g '!…'`, `ls` with `--ignore`, git with `:(exclude,glob,top)`
   pathspecs) run filtered, and the walkers it cannot (`find`, `tree`,
   `du` without `-s`, recursive `grep` and `diff`) left the table or ask.
   The old risk bullet that called this "a known hole" is replaced by one
   that names the two residual gaps (history that moved a secret, and allow
   carve-outs not applied to filtered walks).

The split between the two is stated at the top of the classifier section:
the classifier stays pure over `(command, project_root, cwd, &policy)`, and
every check that needs the host lives in the seam, which runs after the
decision and can only narrow or refuse. The alternative split, dropping
`rg` and the helper-running git subcommands so no seam is needed, was
considered and not taken: it would leave `cat`, `ls`, and `grep` and give
back most of the inspection shapes the outcome is for, and the PATH and
`BASH_ENV` findings need a fixed launch environment even for those.

Each probe in this subsection was run on 2026-11-17 with git 2.55.0,
ripgrep 15.1.0, GNU coreutils 9.11, and bash 5.3.15; the seam suite exists
so the claims are re-run on every build instead of trusted from here.

### Changes from the second review of this revision

Three findings against the execution seam, each reproduced before the
change:

1. **`rg` globs were cwd-relative.** ripgrep matches `-g` globs relative to
   the directory it runs in, while the policy is root-relative, so from a
   subdirectory a root-anchored deny such as `sub/private.txt` stopped
   applying (probe: `cwd = <root>/sub` printed `private.txt`). The seam now
   launches `rg` from the project root with root-relative operands, adds
   the cwd's root-relative path when the user gave no operand, and emits
   each pattern that contains `/` in two anchored forms, `/<pattern>` and
   `./<pattern>`: probing the fix showed that a root search with no
   operand or `.` builds `./`-prefixed paths that only the second form
   matches. Suite case: "rg from a subdirectory".
2. **`ls --ignore` translation hid ordinary entries.** Reducing
   `secrets/**` or `**/.ssh/**` to their last segment produced
   `--ignore=**`, which hides every entry not starting with `.` (probe:
   `ls -A --ignore='**'` listed only dotfiles). Subtree patterns now become
   their directory name. The "ls filter" case now also asserts that
   ordinary entries stay visible. Review of the fix found a second gap:
   GNU `ls` does not let a leading `*` match a leading `.`, so `*.pem`
   missed `.x.pem`; each `*` name is now also emitted as `.*<rest>` and
   `<rest>`.
3. **Partial clones fetched lazily.** A read that meets a missing object in
   a partial clone starts a fetch that runs repository-configured programs
   (probe: `remote.origin.uploadpack` ran on `git show HEAD:a.txt`). The
   launch form and the preflight now pass `--no-lazy-fetch` and set
   `GIT_NO_LAZY_FETCH=1`. Suite case: "git partial clone".

The finding that configured textconv drivers run under plain `git diff`
was already answered by the launch form in the first revision.

Later reviews of this revision added four more:

4. **Policy globs are not walker globs.** The policy compiles patterns with
   `globset`, which supports `{a,b}` alternatives and lets `*` cross `/`.
   git pathspecs support neither (probe: `:(exclude,glob,top)**/{private,secret}.txt`
   excluded nothing, and `**/id_rsa*` did not exclude `id_rsa.d/n.txt`),
   and `ls --ignore` has no alternatives either. The seam now translates
   only a stated subset where every walker matches what the policy
   matches, adds `<pattern>/**` for a trailing `*`, and refuses every
   filtered walk when a user deny pattern falls outside the subset. Suite
   case: "Untranslatable patterns".
5. **git's root is not always the project root.** `top` in an exclude
   pathspec anchors at git's working tree root, so in a repository nested
   under the project the policy's root-relative patterns excluded nothing
   (probe: `git diff` from `/p/vendor` with
   `:(exclude,glob,top)vendor/private.txt` printed `private.txt`). The git
   preflight now prints `--show-toplevel` and refuses unless it equals the
   project root. The runner already takes the nearest ancestor with a
   `.git` entry as the project root (`runner.rs:10593-10599`), so the usual
   project is unaffected. Suite case: "git root differs from project
   root". Reviewing this fix found that `--git-path` prints a path
   relative to `cwd`, so from a subdirectory the attributes check looked
   in the wrong place; the preflight now passes `--path-format=absolute`,
   and the "git info/attributes" case runs from a subdirectory and from a
   linked worktree.
6. **A walk can start inside a denied subtree.** `secrets/**` does not
   match the directory `secrets`, and `ls --ignore=secrets secrets` still
   lists its contents, because exclusions filter entries, not the walk's
   starting directory (probe: `ls -A --ignore=secrets secrets` printed
   `private.txt`; the same from `cwd = <root>/secrets`). The path rule now
   has a subtree rule: for each deny pattern ending in `/**`, the part
   before it is matched against every directory operand, the walked `cwd`,
   and their ancestors, and a match asks. Corpus cases: `ls secrets`, `ls
   secrets/sub`, and bare `ls` and `rg` with `cwd` inside `secrets`.
7. **Programs read their environment.** The seam removed a named list of
   variables, and each review found one it missed: `GIT_LITERAL_PATHSPECS`
   turns the seam's exclusion pathspecs into literal names (probe: `git
   diff -- .` printed `credentials.json`), `GIT_TRACE` made `git status`
   write an arbitrary file, and `LD_DEBUG_OUTPUT` made the dynamic loader
   write one. The seam now builds the launch environment from an
   allowlist (`PATH`, `LANG`, `TERM`, and fixed git settings including
   `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, and
   `GIT_TRACE2*=0`) instead of removing names from the inherited one.
   Suite case: "Launch environment".

## Sources

- `crates/yach-proto/src/lib.rs:273-278`, `281-286`, `720-726`, `729-740`,
  `744-752`, `966-970`, `1048-1055`
- `crates/yach-backend/src/permission.rs:30-31`, `66-67`, `70-76`, `109-121`,
  `171-176`, `265-288`, `447-454`, `477-547`, `574-593`, `596-606`,
  `736-750`, `770-810`, `1302`
- `crates/yach-backend/src/shell.rs:52`, `94-97`, `107-118`, `120-126`,
  `128-160`, `163-232`, `242-260`, `337-348`
- `crates/yach-backend/src/runner.rs:1885-1890`, `5035-5063`, `5760-5781`,
  `8358-8364`, `8480-8552`, `8633-8636`, `8941-8958`, `8960-8992`,
  `9003-9018`, `9082-9094`, `9139-9177`, `9190-9224`, `9232-9235`,
  `9270-9283`, `9301-9313`, `9378-9391`, `9409`, `9486-9488`, `9513`,
  `9594-9596`
- `crates/yach-backend/src/runner/extension_state.rs:216`, `230`, `432`, `567`
- `crates/yach-backend/src/runner/local_edit.rs:235-251`
- `crates/yach-backend/src/review/coordinator.rs:39-41`, `256-262`,
  `379-388`, `407-412`, `421-434`, `437-471`, `1103-1145`
- `crates/yach-backend/src/resource.rs:278-285`, `308-313`, `337`, `487`,
  `548`, `627-661`, `663-672`
- `crates/yach-backend/src/sensitive_paths.rs:16-45`, `174-196`, `198-213`
- `crates/yach-backend/src/tools.rs:2627-2635`
- `crates/yach-ui/Cargo.toml:22`
- `crates/yach-ui/src/status_bar.rs:10-21`, `112-113`, `149-159`, `346-351`
- `crates/yach-ui/src/transcript.rs:61-66`, `719-758`
- `crates/yach-ui/src/approval_selector.rs:56-75`
- `crates/yach-ui/src/app.rs:351-360`, `1266-1280`, `4348-4352`
- `crates/yach-cli/src/sessions.rs:416-444`, `641-650`, `761-786`
- `crates/yach-cli/src/headless.rs:162-166`, `648-672`, `903-920`, `1670`
- `crates/yach-cli/src/rpc.rs:190-200`
- `.github/risk.toml:27-48`; `AGENTS.md:78-91`
- `README.md:36-39`, `94-98`
- `docs/protocol/yach-proto-v0.md:110-123`
- `docs/project/specs/2026-09-24-auto-review-signals-routing-design.md:180-193`,
  `234-237`
- `docs/project/specs/2026-08-24-approval-modes-design.md:43-56`
- `docs/project/specs/2026-10-17-composer-styles-design.md:164-172`
- `docs/project/records/2026-10-02-approval-cohort-refresh.md`
- `docs/project/specs/2026-07-14-sensitive-file-deny-design.md:39-41`, `48-62`
- Local session `session-2580678-1790971084868581101.jsonl` (2026-10-02, not
  in the repo): 13 edit reviews, 23 command reviews, 36 approve decisions
- Jujutsu working copy, https://docs.jj-vcs.dev/latest/working-copy/, read
  2026-11-17
- Jujutsu CLI reference, https://docs.jj-vcs.dev/latest/cli-reference/, read
  2026-11-17
- git 2.55.0 documentation: `git(1)` for `--attr-source`, `--no-optional-locks`,
  and `-c`; `git-config(1)` for `GIT_CONFIG_COUNT`; `gitattributes(5)` for
  the precedence of `$GIT_DIR/info/attributes`; read 2026-11-17
- GNU grep NEWS, release 3.6 (2020-11-08): `GREP_OPTIONS` no longer affects
  behavior; read 2026-11-17
- Probe runs on 2026-11-17 (git 2.55.0, ripgrep 15.1.0, GNU coreutils 9.11,
  bash 5.3.15): helper markers per git subcommand with and without the
  launch form; `RIPGREP_CONFIG_PATH` `--pre` with and without `--no-config`;
  `rg -g '!…'` and `ls --ignore` against a tree containing denied names;
  `BASH_ENV` and `BASH_FUNC_cat%%` under `bash -c`; `git show <blob>^{commit}`
