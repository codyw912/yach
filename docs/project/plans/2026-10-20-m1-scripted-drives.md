# M1 Scripted Drives Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the four Layer S drives the M1 dogfood portfolio requires
before the first live run: a retryable and a non-retryable scripted
provider failure, a scripted cancellation, and a two-file edit followed by
a verifier.

**Architecture:**
- Each drive is a bash script in `.agents/skills/verify-yach/scripts/` with
  a feature file in `features/`, following the existing drives.
- All four run headless through `yach_cli run`, the same way
  `drive-headless` does. They check the outcome JSON, the session
  inspector (`yach sessions show latest --json`), and the workspace on disk.
  None needs a TUI.
- `lib/common.sh` gains four reply-script builders: `failed_round`,
  `cancelled_round`, `tool_round` and `file_tag`. No Rust changes. The
  scripted provider already deserializes `Failed`, `Cancelled` and
  multi-call tool rounds.

**Tech Stack:** bash, jq, tmux-free `yach run` on the scripted provider
(`--features bench`), shellcheck from the devenv shell.

**Spec:** `docs/project/records/2026-10-15-dogfood-portfolio.md`

**Source:** linear:CEW-147

## Global Constraints

- The pass signals are the portfolio's "To build before the first live run"
  table (`docs/project/records/2026-10-15-dogfood-portfolio.md`, Layer S).
  A drive asserts on artifacts: outcome JSON, session inspector JSON, the
  session log, and files on disk. Reply text is never the pass signal.
- Credless and offline: `YACH_RIG_PROVIDER=scripted`, a private HOME,
  session directory and workspace under
  `$XDG_STATE_HOME/sjujperpowers/verify-yach/run/`, created by `reset_run`.
  Never touch `~/.yach`, the ylog checkout, or a real provider.
- Each drive prints its evidence directory as its last line and exits
  non-zero, with a message on stderr, when an observable result is missing
  (`.agents/skills/verify-yach/SKILL.md:29`). The drive's own checks print
  `drive-<name>: ...`; a check made through the shared `expect` helper
  prints `verify-yach: expected /<pattern>/ in <file>`, as the existing
  drives' checks do.
- `scripts/lib/tmux-tty.sh` is a vendored driver; do not edit it
  (`SKILL.md:50`).
- `.agents/skills/verify-*/scripts/**`, `features/**` and `SKILL.md` are
  protected paths (`.github/scripts/changes.py:25-41`). CI does not run the
  drives, so each task's own run plus its mutation check is the proof.
- `just dev shellcheck -x -P SCRIPTDIR -s bash -S warning <files>`, run from
  the repo root, must be clean for every script this plan adds or edits.
  `-P SCRIPTDIR` resolves each drive's `lib/common.sh` source against the
  drive's directory; without it, shellcheck run from the root cannot follow
  the source and warns on every variable it sets. The current scripts are
  clean at that level.
- Do not change yach code to make a drive pass. If an observed behavior
  differs from this plan, stop and report it.

## Facts this plan relies on

Observed 2026-10-20 on main `4b95c442` with the bench build, using the same
helpers:

- **Provider failure.** A round `[Started, Failed{kind}]` ends the turn
  failed even with a success round queued behind it:
  - `yach run` exits 1 with `outcome: "failed"` and `response: ""`;
  - the inspector shows one `provider_attempt` item with
    `outcome: "failed"`, `error_kind: <kind>` and `retry_index: 0`.

  A provider-emitted `Failed` is terminal by design
  (`docs/project/specs/2026-08-28-provider-attempt-reliability-design.md:77-81`).
  So the queued success round proves "not retried into success" for both
  kinds. It does not exercise the live retry ladder.
- **Cancellation.** `[Started, Cancelled]`:
  - `yach run` exits 1 and its outcome JSON says `failed`; it has no
    cancelled value (`crates/yach-cli/src/headless.rs:931-934`);
  - the inspector turn `outcome` is `"cancelled"`, with one attempt whose
    `outcome` is `"cancelled"` and no `error_kind`.

  The inspector is the signal.
- **The default preset replaces the edit tool.** A fresh HOME gets the
  `full` preset (`crates/yach-cli/src/bundled.rs:295-316`), which swaps
  `read_text_file`/`edit_text_file` for hashline
  (`crates/yach-hashline-extension/yach.extension.json:27-40`):
  - `edit_text_file` takes `{"input": "<patch>"}`, where the patch starts
    with `[path#TAG]` and `TAG` is the first 16 uppercase hex digits of the
    SHA-256 of the file's LF-normalized text
    (`crates/yach-hashline-extension/src/main.rs:696-699`);
  - the tag must have been minted by a `read_text_file` of that file in
    the same session, so reads come first;
  - `{"path","find","replace"}` fails validation with
    `missing_required_field: input`;
  - `--preset minimal` drops both edit tools (`unknown_tool`), so the drive
    uses the default preset.
- **Tool results.** The inspector redacts tool arguments and results. The
  session log's `tool_execution_finished.result_content` keeps them:
  - an applied hashline edit starts with the line `[applied]`;
  - a bash command with no output and exit 0 renders as
    `[no output; exit code 0]` (`crates/yach-backend/src/runner.rs:9667-9677`).

  A bash tool with a non-zero exit still has tool `outcome: "completed"`,
  so the exit code has to be read from the result text.
- **Approvals.** `yach run` without `--full-auto` denies edits (read-only
  safe). The two-file drive passes `--full-auto`. Review gating is
  `drive-tool-review`'s contract, not this one's.

---

### Task 1: Scripted provider failure drive

**Files:**
- Modify: `.agents/skills/verify-yach/scripts/lib/common.sh` (insert after
  `bash_round`, line 49)
- Create: `.agents/skills/verify-yach/scripts/drive-provider-failure`
- Create: `.agents/skills/verify-yach/features/provider-failure.md`
- Modify: `.agents/skills/verify-yach/SKILL.md` (full-pass command, line
  14; drive table, lines 31-36)

**Interfaces:**
- Produces, in `lib/common.sh`: `failed_round <kind>`, which prints one
  round as compact JSON: `[Started, {type: "failed", turn_id: "turn-1",
  error: {kind, message: "scripted <kind>", redacted_debug: null}}]`.
  `<kind>` is a serialized `ProviderErrorKind` (`rate_limited`,
  `authentication`, ...).

- [ ] **Step 1: Write the drive (the failing check)**

Create `.agents/skills/verify-yach/scripts/drive-provider-failure` with mode
755:

```bash
#!/usr/bin/env bash
# A scripted provider failure ends the turn failed, with no retry into the
# success round queued behind it. Covers one retryable kind (rate_limited)
# and one non-retryable kind (authentication). A Failed event inside a
# provider round is terminal by design (the attempt layer retries request
# errors, not provider-emitted failures), so this proves the terminal path
# and its session evidence, not the live retry ladder.
set -euo pipefail
. "$(cd "$(dirname "$0")" && pwd)/lib/common.sh"

ev=$("$tty" evidence-dir yach)
"$tty" stop "$session"

n=0
for kind in rate_limited authentication; do
  n=$((n + 1))
  reset_run
  write_script "$(failed_round "$kind")" "$(text_round 'retried into success')"

  rc=0
  yach_cli run --prompt "provider failure $kind" > "$ev/0$n-$kind-outcome.json" 2> "$ev/0$n-$kind-stderr.log" || rc=$?
  echo "exit: $rc" > "$ev/0$n-$kind-exit.txt"
  [[ $rc -eq 1 ]] || { echo "drive-provider-failure: $kind: yach run exited $rc, expected 1 (failed)" >&2; cat "$ev/0$n-$kind-stderr.log" >&2; exit 1; }
  jq -e '.outcome == "failed" and .response == ""' "$ev/0$n-$kind-outcome.json" > /dev/null \
    || { echo "drive-provider-failure: $kind: unexpected outcome document" >&2; cat "$ev/0$n-$kind-outcome.json" >&2; exit 1; }

  yach_cli sessions show latest --json > "$ev/0$n-$kind-session.json"
  jq -e --arg k "$kind" '
      (.turns | length) == 1
      and .turns[0].outcome == "failed"
      and ([.turns[0].items[] | select(.kind == "provider_attempt") | .attempt]
           | length == 1
             and .[0].outcome == "failed"
             and .[0].error_kind == $k
             and .[0].retry_index == 0)' "$ev/0$n-$kind-session.json" > /dev/null \
    || { echo "drive-provider-failure: $kind: session does not show one failed $kind attempt" >&2; jq . "$ev/0$n-$kind-session.json" >&2; exit 1; }
done
echo "$ev"
```

- [ ] **Step 2: Run it to see it fail**

Run: `.agents/skills/verify-yach/scripts/launch && .agents/skills/verify-yach/scripts/drive-provider-failure`

Expected: non-zero exit with `failed_round: command not found` (the
builder does not exist yet). `launch` records the bench build the drives
use.

- [ ] **Step 3: Add `failed_round` to `lib/common.sh`**

Insert after `bash_round` (after line 49):

```bash

# One provider round that fails with a ProviderErrorKind, given in its
# serialized snake_case form (rate_limited, authentication, ...).
failed_round() {
  jq -cn --arg k "$1" --argjson s "$started" \
    '[$s, {type: "failed", turn_id: "turn-1",
           error: {kind: $k, message: ("scripted " + $k), redacted_debug: null}}]'
}
```

- [ ] **Step 4: Run it to see it pass**

Run: `.agents/skills/verify-yach/scripts/drive-provider-failure`

Expected: exit 0. The last line is the evidence directory, which holds
`01-rate_limited-*` and `02-authentication-*` outcome, stderr, exit and
session files.

- [ ] **Step 5: Mutation check**

Prove the "not retried into success" assertion bites. Run a copy with the
failure round removed, so the success round answers instead:

```bash
s=.agents/skills/verify-yach/scripts
sed 's|write_script "$(failed_round "$kind")" |write_script |' "$s/drive-provider-failure" > "$s/mut-provider-failure"
chmod +x "$s/mut-provider-failure"; rc=0; "$s/mut-provider-failure" || rc=$?; rm -f "$s/mut-provider-failure"
echo "mutant exit: $rc"
```

Expected: `drive-provider-failure: rate_limited: yach run exited 0, expected 1 (failed)`
and `mutant exit: 1`. Delete the mutant; it is never committed.

- [ ] **Step 6: Feature file and skill index**

Create `.agents/skills/verify-yach/features/provider-failure.md`:

```markdown
# Scripted provider failure

**Entry point:** `yach run --prompt "..."`, `yach sessions show latest --json`; `scripts/drive-provider-failure`.

**Preconditions:** scripted provider. Round 1 is `[Started, Failed]` with the error kind under test; round 2 is a text reply that must never be used. No TTY needed.

**Steps:**

1. `failed_round rate_limited`, then `yach run --prompt "provider failure rate_limited"`. → Exit 1. Outcome JSON `outcome: "failed"`, `response: ""`.
2. `yach sessions show latest --json`. → One turn, `outcome: "failed"`, exactly one `provider_attempt` item: `outcome: "failed"`, `error_kind: "rate_limited"`, `retry_index: 0`.
3. Fresh run with `failed_round authentication`. → The same shape with `error_kind: "authentication"`.

**Gotchas:**

- A `Failed` event inside a provider round is terminal: the attempt layer retries request errors, not provider-emitted failures (`docs/project/specs/2026-08-28-provider-attempt-reliability-design.md:77-81`). The queued success round proves the turn was not retried into success. It does not exercise the live retry ladder (1 s, then 2 s, at most three attempts).
- The inspector nests the session's `ProviderAttemptFinished` event as `items[] | select(.kind == "provider_attempt") | .attempt`. There is no literal `ProviderAttemptFinished` key in its JSON.
```

In `.agents/skills/verify-yach/SKILL.md`, replace line 14 with:

```bash
$s/doctor && $s/launch && $s/drive-prompt-reply && $s/drive-tool-review && $s/drive-resume && $s/drive-headless && $s/drive-provider-failure; rc=$?
```

and add a row after the `drive-headless` row (line 36):

```markdown
| `scripts/drive-provider-failure` | [Provider failure](features/provider-failure.md): a scripted `rate_limited` and `authentication` failure each end the turn failed, one attempt, no retry |
```

Replace line 38's first sentence ("Reply scripts are built by
`scripts/lib/common.sh` (`text_round`, `bash_round`, `write_script`): ...")
with:

```markdown
Reply scripts are built by `scripts/lib/common.sh` (`text_round`, `bash_round`, `failed_round`, `write_script`): a JSON array of provider rounds, each an array of provider stream events.
```

- [ ] **Step 7: Lint**

Run: `just dev shellcheck -x -P SCRIPTDIR -s bash -S warning .agents/skills/verify-yach/scripts/drive-provider-failure .agents/skills/verify-yach/scripts/lib/common.sh`

Expected: no output, exit 0.

- [ ] **Step 8: Commit**

Check that the change holds only the four files in this task's **Files**
list, then checkpoint as `AGENTS.md` requires:

```bash
jj diff --stat
jj describe -m "Add verify-yach scripted provider failure drive"
jj new
```

### Task 2: Scripted cancellation drive

**Files:**
- Modify: `.agents/skills/verify-yach/scripts/lib/common.sh` (after
  `failed_round`)
- Create: `.agents/skills/verify-yach/scripts/drive-cancellation`
- Create: `.agents/skills/verify-yach/features/cancellation.md`
- Modify: `.agents/skills/verify-yach/SKILL.md` (full-pass command, drive
  table, builder list)

**Interfaces:**
- Consumes: `failed_round` (Task 1), used only by this task's mutation
  check.
- Produces, in `lib/common.sh`: `cancelled_round`, which prints
  `[Started, {type: "cancelled", turn_id: "turn-1", reason: "scripted cancellation"}]`.

- [ ] **Step 1: Write the drive (the failing check)**

Create `.agents/skills/verify-yach/scripts/drive-cancellation` with mode 755:

```bash
#!/usr/bin/env bash
# A provider round that ends Cancelled leaves the turn cancelled, not failed.
# `yach run`'s outcome document has no cancelled value (it reports the turn
# as failed), so the session inspector is the signal.
set -euo pipefail
. "$(cd "$(dirname "$0")" && pwd)/lib/common.sh"

ev=$("$tty" evidence-dir yach)
"$tty" stop "$session"
reset_run
write_script "$(cancelled_round)" "$(text_round 'retried into success')"

rc=0
yach_cli run --prompt "cancelled turn" > "$ev/01-outcome.json" 2> "$ev/01-stderr.log" || rc=$?
echo "exit: $rc" > "$ev/01-exit.txt"
[[ $rc -eq 1 ]] || { echo "drive-cancellation: yach run exited $rc, expected 1" >&2; cat "$ev/01-stderr.log" >&2; exit 1; }
jq -e '.response == ""' "$ev/01-outcome.json" > /dev/null \
  || { echo "drive-cancellation: the queued success round was used" >&2; cat "$ev/01-outcome.json" >&2; exit 1; }

yach_cli sessions show latest --json > "$ev/02-session.json"
jq -e '
    (.turns | length) == 1
    and .turns[0].outcome == "cancelled"
    and ([.turns[0].items[] | select(.kind == "provider_attempt") | .attempt]
         | length == 1
           and .[0].outcome == "cancelled"
           and .[0].error_kind == null)' "$ev/02-session.json" > /dev/null \
  || { echo "drive-cancellation: session does not show one cancelled turn and attempt" >&2; jq . "$ev/02-session.json" >&2; exit 1; }
echo "$ev"
```

- [ ] **Step 2: Run it to see it fail**

Run: `.agents/skills/verify-yach/scripts/drive-cancellation`

Expected: non-zero exit with `cancelled_round: command not found`.

- [ ] **Step 3: Add `cancelled_round` to `lib/common.sh`**

Insert after `failed_round`:

```bash

# One provider round that the provider ends as cancelled.
cancelled_round() {
  jq -cn --argjson s "$started" \
    '[$s, {type: "cancelled", turn_id: "turn-1", reason: "scripted cancellation"}]'
}
```

- [ ] **Step 4: Run it to see it pass**

Run: `.agents/skills/verify-yach/scripts/drive-cancellation`

Expected: exit 0, with the evidence directory as the last line.

- [ ] **Step 5: Mutation check**

Prove the drive tells cancelled from failed. Replace the cancelled round
with a failed one:

```bash
s=.agents/skills/verify-yach/scripts
sed 's|write_script "$(cancelled_round)"|write_script "$(failed_round cancelled)"|' "$s/drive-cancellation" > "$s/mut-cancellation"
chmod +x "$s/mut-cancellation"; rc=0; "$s/mut-cancellation" || rc=$?; rm -f "$s/mut-cancellation"
echo "mutant exit: $rc"
```

Expected: `drive-cancellation: session does not show one cancelled turn and attempt`
and `mutant exit: 1`.

- [ ] **Step 6: Feature file and skill index**

Create `.agents/skills/verify-yach/features/cancellation.md`:

```markdown
# Scripted cancellation

**Entry point:** `yach run --prompt "..."`, `yach sessions show latest --json`; `scripts/drive-cancellation`.

**Preconditions:** scripted provider. Round 1 is `[Started, Cancelled]`; round 2 is a text reply that must never be used. No TTY needed.

**Steps:**

1. `yach run --prompt "cancelled turn"`. → Exit 1, `response: ""`.
2. `yach sessions show latest --json`. → One turn with `outcome: "cancelled"` and exactly one `provider_attempt` item with `outcome: "cancelled"` and no `error_kind`.

**Gotchas:**

- `yach run`'s outcome JSON has no cancelled value: a provider cancellation is reported as `outcome: "failed"` (`crates/yach-cli/src/headless.rs:931-934`). Assert cancellation on the session inspector, never on the run outcome.
- This is a provider-ended cancellation. A user cancel (Ctrl-C in the TUI, or `CancelTurn` over rpc) is a different path, and the RPC matrix covers it (`crates/yach-cli/tests/rpc_matrix.rs:910`).
```

In `SKILL.md`, append `&& $s/drive-cancellation` to the full-pass command
after `$s/drive-provider-failure`. Add this row after the
`drive-provider-failure` row:

```markdown
| `scripts/drive-cancellation` | [Cancellation](features/cancellation.md): a scripted provider cancellation leaves the turn and attempt cancelled, not failed |
```

In the builder list, change `` `failed_round`, `write_script` `` to
`` `failed_round`, `cancelled_round`, `write_script` ``.

- [ ] **Step 7: Lint**

Run: `just dev shellcheck -x -P SCRIPTDIR -s bash -S warning .agents/skills/verify-yach/scripts/drive-cancellation .agents/skills/verify-yach/scripts/lib/common.sh`

Expected: no output, exit 0.

- [ ] **Step 8: Commit**

Check that the change holds only this task's files, then checkpoint:

```bash
jj diff --stat
jj describe -m "Add verify-yach scripted cancellation drive"
jj new
```

### Task 3: Two-file edit and verifier drive

**Files:**
- Modify: `.agents/skills/verify-yach/scripts/lib/common.sh` (after
  `cancelled_round`)
- Create: `.agents/skills/verify-yach/scripts/drive-multi-file-edit`
- Create: `.agents/skills/verify-yach/features/multi-file-edit.md`
- Modify: `.agents/skills/verify-yach/SKILL.md` (full-pass command, drive
  table, builder list)
- Modify: `docs/project/records/2026-10-15-dogfood-portfolio.md` (Step 9)

**Interfaces:**
- Produces, in `lib/common.sh`:
  - `tool_round <calls-json>` takes a JSON array of
    `{call_id, name, arguments_json}` objects and prints one round:
    `[Started, ToolCallCompleted per call..., Completed]`;
  - `file_tag <path>` prints the hashline snapshot tag of a file with LF
    line endings: 16 uppercase hex digits.

- [ ] **Step 1: Write the drive (the failing check)**

Create `.agents/skills/verify-yach/scripts/drive-multi-file-edit` with mode
755:

```bash
#!/usr/bin/env bash
# Scripted tool rounds read two seeded files, edit both, then run the
# workspace verifier. The proof is on disk and in the session log: the
# verifier fails on the seed, both files differ from the seed afterwards,
# the verifier exits 0 when the drive runs it again, and the session
# recorded both edits and the verifier's exit code.
# The reply text is not the signal.
#
# The default (full) preset replaces edit_text_file with hashline: reads
# return [path#TAG] headers and edits are line-anchored patches whose tag
# must match a read in the same session. The drive computes the tags from
# the seed, so the reads must come first.
set -euo pipefail
. "$(cd "$(dirname "$0")" && pwd)/lib/common.sh"

ev=$("$tty" evidence-dir yach)
"$tty" stop "$session"
reset_run
ws="$state/run/workspace"
printf 'alpha\n' > "$ws/a.txt"
printf 'beta\n' > "$ws/b.txt"
printf '#!/bin/sh\ngrep -qx ALPHA a.txt && grep -qx BETA b.txt\n' > "$ws/verify.sh"
chmod +x "$ws/verify.sh"
cp "$ws/a.txt" "$ev/seed-a.txt"
cp "$ws/b.txt" "$ev/seed-b.txt"
if (cd "$ws" && ./verify.sh); then
  echo "drive-multi-file-edit: ./verify.sh already passes on the seed" >&2; exit 1
fi

patch_a=$(printf '[a.txt#%s]\nPUT 1.=1:\n+ALPHA\n' "$(file_tag "$ws/a.txt")")
patch_b=$(printf '[b.txt#%s]\nPUT 1.=1:\n+BETA\n' "$(file_tag "$ws/b.txt")")
write_script \
  "$(tool_round '[{"call_id":"call-1","name":"read_text_file","arguments_json":{"path":"a.txt"}},
                  {"call_id":"call-2","name":"read_text_file","arguments_json":{"path":"b.txt"}}]')" \
  "$(tool_round "$(jq -cn --arg a "$patch_a" --arg b "$patch_b" \
      '[{call_id: "call-3", name: "edit_text_file", arguments_json: {input: $a}},
        {call_id: "call-4", name: "edit_text_file", arguments_json: {input: $b}}]')")" \
  "$(tool_round '[{"call_id":"call-5","name":"bash","arguments_json":{"command":"./verify.sh"}}]')" \
  "$(text_round 'Both files edited; the verifier passed.')"

# --full-auto: this drive proves the edit and verifier path. Review gating
# is drive-tool-review's job.
rc=0
yach_cli run --prompt "edit a.txt and b.txt, then run ./verify.sh" --full-auto \
  > "$ev/01-outcome.json" 2> "$ev/01-stderr.log" || rc=$?
echo "exit: $rc" > "$ev/01-exit.txt"
[[ $rc -eq 0 ]] || { echo "drive-multi-file-edit: yach run exited $rc" >&2; cat "$ev/01-stderr.log" >&2; exit 1; }

changed=0
cmp -s "$ws/a.txt" "$ev/seed-a.txt" || changed=$((changed + 1))
cmp -s "$ws/b.txt" "$ev/seed-b.txt" || changed=$((changed + 1))
cp "$ws/a.txt" "$ws/b.txt" "$ev/"
[[ $changed -eq 2 ]] || { echo "drive-multi-file-edit: $changed of 2 files differ from the seed" >&2; exit 1; }

vrc=0
(cd "$ws" && ./verify.sh) || vrc=$?
echo "verify.sh exit: $vrc" > "$ev/02-verifier.txt"
[[ $vrc -eq 0 ]] || { echo "drive-multi-file-edit: ./verify.sh exited $vrc after the turn" >&2; exit 1; }

yach_cli sessions show latest --json > "$ev/03-session.json"
jq -e '
    .turns[0].outcome == "completed"
    and ([.turns[0].items[] | select(.kind == "tool" and .outcome == "completed")] | length == 5)' \
  "$ev/03-session.json" > /dev/null \
  || { echo "drive-multi-file-edit: session does not show a completed turn with 5 completed tools" >&2; jq . "$ev/03-session.json" >&2; exit 1; }

# The inspector redacts tool output; the session log keeps it. The
# verifier's recorded result must report exit code 0.
log=$(find "$state/run/sessions" -name '*.jsonl')
jq -r 'select(.type == "tool_execution_finished") | .result_content' "$log" > "$ev/04-tool-results.txt"
expect "$ev/04-tool-results.txt" '^\[applied\]$'
[[ $(grep -c '^\[applied\]$' "$ev/04-tool-results.txt") -eq 2 ]] \
  || { echo "drive-multi-file-edit: expected 2 applied edits" >&2; cat "$ev/04-tool-results.txt" >&2; exit 1; }
expect "$ev/04-tool-results.txt" '^\[no output; exit code 0\]$'
echo "$ev"
```

- [ ] **Step 2: Run it to see it fail**

Run: `.agents/skills/verify-yach/scripts/drive-multi-file-edit`

Expected: non-zero exit with `file_tag: command not found`.

- [ ] **Step 3: Add `tool_round` and `file_tag` to `lib/common.sh`**

Insert after `cancelled_round`:

```bash

# One provider round that requests every tool call in a JSON array of
# {call_id, name, arguments_json} objects.
tool_round() {
  jq -cn --argjson calls "$1" --argjson s "$started" --argjson c "$completed" \
    '[$s] + [$calls[] | {type: "tool_call_completed", turn_id: "turn-1", tool_call: .}] + [$c]'
}

# The snapshot tag hashline_read mints for a file ([path#TAG]): the first 16
# hex digits, uppercase, of the SHA-256 of its LF-normalized text. Seeds
# written with LF need no normalization. macOS has shasum, not sha256sum.
file_tag() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi \
    | cut -c1-16 | tr a-f A-F
}
```

- [ ] **Step 4: Run it to see it pass**

Run: `.agents/skills/verify-yach/scripts/drive-multi-file-edit`

Expected: exit 0. The evidence directory holds the seeds, the edited
`a.txt` (`ALPHA`) and `b.txt` (`BETA`), `02-verifier.txt` with
`verify.sh exit: 0`, the session JSON, and `04-tool-results.txt` with two
`[applied]` blocks and `[no output; exit code 0]`.

- [ ] **Step 5: Mutation checks**

Two mutants. The first patches `a.txt` twice and never edits `b.txt`. The
second makes the verifier require a value nothing writes:

```bash
s=.agents/skills/verify-yach/scripts
sed 's|--arg b "$patch_b"|--arg b "$patch_a"|' "$s/drive-multi-file-edit" > "$s/mut-one-edit"
sed 's|grep -qx BETA b.txt|grep -qx GAMMA b.txt|' "$s/drive-multi-file-edit" > "$s/mut-verifier"
chmod +x "$s/mut-one-edit" "$s/mut-verifier"
for m in mut-one-edit mut-verifier; do rc=0; "$s/$m" || rc=$?; echo "$m exit: $rc"; done
rm -f "$s/mut-one-edit" "$s/mut-verifier"
```

Expected:
- `drive-multi-file-edit: 1 of 2 files differ from the seed`, then
  `mut-one-edit exit: 1`;
- `drive-multi-file-edit: ./verify.sh exited 1 after the turn`, then
  `mut-verifier exit: 1`.

- [ ] **Step 6: Feature file and skill index**

Create `.agents/skills/verify-yach/features/multi-file-edit.md`:

```markdown
# Two-file edit and verifier

**Entry point:** `yach run --prompt "..." --full-auto`, `yach sessions show latest --json`, the session log; `scripts/drive-multi-file-edit`.

**Preconditions:** scripted provider, default (full) preset. The workspace is seeded with `a.txt` (`alpha`), `b.txt` (`beta`), and the verifier `verify.sh`, which exits 0 only when the files read `ALPHA` and `BETA`. No TTY needed.

**Steps:**

1. Round 1 reads both files (`read_text_file` ×2). Round 2 sends two `edit_text_file` hashline patches whose `[path#TAG]` headers come from `file_tag` on the seeds. Round 3 runs `./verify.sh` with `bash`. Round 4 is the reply. → `yach run` exits 0.
2. Compare the files with the seeds. → Both differ: `ALPHA`, `BETA`.
3. Run `./verify.sh` again from the workspace. → Exit 0.
4. `yach sessions show latest --json`. → The turn is `completed`, with five completed `tool` items.
5. Read the session log's `tool_execution_finished.result_content`. → Two `[applied]` edits and `[no output; exit code 0]` from the verifier.

**Gotchas:**

- The full preset replaces `edit_text_file` with hashline (`crates/yach-hashline-extension/yach.extension.json:27-40`). Its argument is `{"input": "<patch>"}`, not `{"path","find","replace"}`. A patch's tag must match a `read_text_file` of that file earlier in the same session, so reads come first. `--preset minimal` has no edit tool at all.
- A bash tool that exits non-zero still has tool `outcome: "completed"`. The exit code is only in the result text, and the inspector redacts it, which is why the drive reads the session log.
- `--full-auto`: headless runs are read-only unless told otherwise. Review gating of edits and commands is `drive-tool-review`'s job.
- This is the harness contract in the scratch workspace. The live multi-tool scenario runs on ylog with `uv run pytest`; see the portfolio record.
```

In `SKILL.md`, append `&& $s/drive-multi-file-edit` to the full-pass
command after `$s/drive-cancellation`. Add this row after the
`drive-cancellation` row:

```markdown
| `scripts/drive-multi-file-edit` | [Two-file edit](features/multi-file-edit.md): scripted reads, two hashline edits and a verifier run; both files change and the verifier exits 0 |
```

In the builder list, change
`` `failed_round`, `cancelled_round`, `write_script` `` to
`` `failed_round`, `cancelled_round`, `tool_round`, `file_tag`, `write_script` ``.

- [ ] **Step 7: Lint**

Run: `just dev shellcheck -x -P SCRIPTDIR -s bash -S warning .agents/skills/verify-yach/scripts/drive-multi-file-edit .agents/skills/verify-yach/scripts/lib/common.sh`

Expected: no output, exit 0.

- [ ] **Step 8: Full pack**

Run the full pass from `SKILL.md` (lines 12-17), which now includes the
three new drives.

Expected: `exit status: 0`.

- [ ] **Step 9: Update the portfolio record**

In `docs/project/records/2026-10-15-dogfood-portfolio.md`:
- move the four rows of "To build before the first live run" into the
  "Exists today" table, naming each drive script and feature file in the
  format of the existing rows;
- change the full-pass sentence after that table to the new command;
- in "Re-run gate" item 1 and "Follow-up, not this record", say that the
  drives exist, citing this plan.

Do not change the pass signals.

- [ ] **Step 10: Commit**

Check that the change holds only this task's skill files and the portfolio
record, then checkpoint:

```bash
jj diff --stat
jj describe -m "Add verify-yach two-file edit drive; record the Layer S drives as built"
jj new
```

## Coverage against the spec

| Portfolio row (To build) | Task | Asserted by |
| --- | --- | --- |
| Provider failure, retryable (`RateLimited`) | 1 | exit 1, `outcome: failed`, one attempt with `error_kind: rate_limited` and `retry_index: 0`, the queued success round unused |
| Provider failure, non-retryable (`Authentication`) | 1 | the same, with `error_kind: authentication`. "No retry storm" is the single attempt. |
| Cancellation | 2 | inspector turn `outcome: cancelled`, one attempt `outcome: cancelled`, no `error_kind` |
| Two-file edit + verifier | 3 | `./verify.sh` fails on the seed, both files differ from the seed, `./verify.sh` exits 0 when rerun, two `[applied]` results and the verifier's `exit code 0` in the session log |
