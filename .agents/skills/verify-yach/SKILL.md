---
name: verify-yach
description: Use when verifying yach behavior end to end (TUI prompts and replies, tool review, resume, headless runs) for verifying-by-risk recipe evidence or before handing off a user-facing change
---

# Verify yach

Drives a real `yach` binary in a private tmux server against the scripted provider (`--features bench`, `YACH_RIG_PROVIDER=scripted`). No credentials and no network for the turn itself. Each run uses its own HOME, session directory and workspace under `$XDG_STATE_HOME/sjujperpowers/verify-yach/run/`, so the operator's `~/.yach` is never read or written.

Run a full pass, keeping the first failure's exit status and always cleaning up:

```bash
s=.agents/skills/verify-yach/scripts
$s/doctor && $s/launch && $s/drive-prompt-reply && $s/drive-tool-review && $s/drive-resume && $s/drive-headless && $s/drive-provider-failure && $s/drive-cancellation; rc=$?
$s/cleanup; crc=$?; [ "$rc" -ne 0 ] || rc=$crc
echo "exit status: $rc"; [ "$rc" -eq 0 ]
```

## Launch

`scripts/launch` builds `yach` with `--features bench` through `just dev`, resets the run directory, and starts `yach tui` in tmux session `yach` with a one-reply script. Each drive script restarts the session with its own reply script, so `launch` is a smoke check that the binary starts; drives do not depend on its state.

## Doctor

`scripts/doctor` checks `tmux`, `jq`, `just`, `devenv`, that `just dev cargo` works, and that the bench build succeeds. Each failure prints the fix. Run it first, and whenever a drive fails before yach starts.

## Drive

Each script prints its evidence directory as its last line and exits non-zero when an observable result does not appear.

| Script | Feature |
|---|---|
| `scripts/drive-prompt-reply` | [Prompt and reply](features/prompt-reply.md): Markdown reply, `/status` counts |
| `scripts/drive-tool-review` | [Tool review](features/tool-review.md): approve runs the command; reject leaves the workspace untouched |
| `scripts/drive-resume` | [Resume](features/resume.md): `--resume` restores the transcript and continues the same session |
| `scripts/drive-headless` | [Headless](features/headless.md): `yach run` outcome JSON, `sessions list/show` |
| `scripts/drive-provider-failure` | [Provider failure](features/provider-failure.md): a scripted `rate_limited` and `authentication` failure each end the turn failed, one attempt, no retry |
| `scripts/drive-cancellation` | [Cancellation](features/cancellation.md): a scripted provider cancellation leaves the turn and attempt cancelled, not failed |

Reply scripts are built by `scripts/lib/common.sh` (`text_round`, `bash_round`, `failed_round`, `cancelled_round`, `write_script`): a JSON array of provider rounds, each an array of provider stream events. A tool-call round is followed by the round that answers after the tool result. To cover a new behavior, add a drive script and a feature file rather than improvising tmux commands.

## Evidence

Under `$XDG_STATE_HOME/sjujperpowers/evidence/yach/<UTC time>-<pid>/` (default `~/.local/state/...`), outside the repository. Screen captures are numbered text files (`01-...txt`); the highest number is the finished screen. Headless evidence is the outcome JSON, stderr log, exit status and `sessions` output.

## Cleanup

`scripts/cleanup` kills the private tmux server and removes the run directory. Safe to run twice; evidence is kept.

## Helpers

`scripts/lib/tmux-tty.sh` is sjujperpowers' driver, copied unchanged. Put yach quirks in `lib/common.sh` or the drive scripts, not in the driver.

| Command | Does |
|---|---|
| `start <name> <dir> <cmd...>` | start `cmd` in a detached 200x50 pane on a private tmux server |
| `send <name> <text>` | type text literally, then Enter |
| `keys <name> <key>...` | send raw tmux keys (`C-c`, `Up`, `Escape`) |
| `wait <name> <ERE> [s]` | wait for the screen to match (default 10 s); on timeout, print the screen and exit 1 |
| `capture <name> <file>` | write the visible screen |
| `stop <name>` | kill the private server; idempotent |
| `evidence-dir <project>` | create and print a fresh evidence directory |

The recipes read, beyond yach's source: the `justfile` (`dev` recipe), `devenv.nix`/`devenv.yaml`/`devenv.lock`, `rust-toolchain.toml`, the Cargo manifests (`bench` feature of `crates/yach-cli`), and the scripted provider's event format in `crates/yach-backend/src/provider.rs` and `bench_loop.rs`.
