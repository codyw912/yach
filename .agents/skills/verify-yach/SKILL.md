---
name: verify-yach
description: Use when verifying yach behavior end to end (TUI prompts and replies, tool review, resume, headless runs) for verifying-by-risk recipe evidence or before handing off a user-facing change
---

# Verify yach

Drives a real bench-enabled `yach` binary in private tmux servers. Most recipes use the scripted provider; semantic recipes use a request-validating loopback Responses server. Each run isolates HOME, sessions and workspace under `$XDG_STATE_HOME/sjujperpowers/verify-yach/`; operator state and real credentials are not used. Local protocol proofs are not live-provider-family or human-assessment evidence.

Run a full pass, keeping the first failure's exit status and always cleaning up:

```bash
s=.agents/skills/verify-yach/scripts
$s/doctor && $s/launch && $s/drive-prompt-reply && $s/drive-tool-review && $s/drive-resume && $s/drive-headless && $s/drive-provider-failure && $s/drive-cancellation && $s/drive-multi-file-edit && $s/drive-provider-resume && $s/drive-compaction && $s/drive-modal-transitions; rc=$?
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
| `scripts/drive-resume` | [Transcript persistence](features/resume.md): restored transcript and same-session fixture continuation; not provider restoration |
| `scripts/drive-headless` | [Headless](features/headless.md): `yach run` outcome JSON, `sessions list/show` |
| `scripts/drive-provider-failure` | [Provider failure](features/provider-failure.md): a scripted `rate_limited` and `authentication` failure each end the turn failed, one attempt, no retry |
| `scripts/drive-cancellation` | [Cancellation](features/cancellation.md): a scripted provider cancellation leaves the turn and attempt cancelled, not failed |
| `scripts/drive-multi-file-edit` | [Two-file edit](features/multi-file-edit.md): scripted reads, two hashline edits and a verifier run; both files change and the verifier exits 0 |
| `scripts/drive-provider-resume` | [Provider recovery](features/provider-resume.md): explicit model search/selection after resume, same session, new provider attempt and restored wire history |
| `scripts/drive-compaction` | [Summary compaction](features/compaction.md): new checkpoint, correlated summary attempt and continuation using the compacted instruction, not folded history |
| `scripts/drive-modal-transitions` | [Modal transitions](features/modal-transitions.md): one-Down edit rejection leaves bytes unchanged; model-selector Escape restores composer input |

Reply scripts are built by `scripts/lib/common.sh` (`text_round`, `bash_round`, `failed_round`, `cancelled_round`, `tool_round`, `file_tag`, `write_script`): a JSON array of provider rounds, each an array of provider stream events. A tool-call round is followed by the round that answers after the tool result. To cover a new behavior, add a drive script and a feature file rather than improvising tmux commands.

## Evidence

Under `$XDG_STATE_HOME/sjujperpowers/evidence/yach/<UTC time>-<pid>/` (default `~/.local/state/...`), outside the repository. Numbered text captures show the current visible pane, not scrollback. Session JSONL/inspector, file bytes and protocol request assertions prove durable effects; a settled screen or composer hint alone does not prove turn completion. Semantic recipes retain actual loopback request bodies and validation results; never copy live bodies or credentials into the repository.

## Cleanup

`scripts/cleanup` kills the private tmux server and removes the run directory. Safe to run twice; evidence is kept.

Semantic recipes trap exit/signals to stop their own server/tmux and remove private state while retaining evidence. The provider-resume recipe explicitly recovers through `/model`; it does not establish automatic restoration. The original scripted recipes can refresh the public model catalog at startup; new local recipes seed a fresh isolated catalog cache to avoid that unrelated network access.

## Helpers

`scripts/lib/tmux-tty.sh` is sjujperpowers' driver, copied unchanged. Put yach quirks in `lib/common.sh` or the drive scripts, not in the driver.

Use yach-specific `common.sh` UI helpers for interactions. `ui_wait PATTERN [SECONDS]` requires two equal matching current-pane frames; `ui_send TEXT PATTERN [SECONDS]` and `ui_keys PATTERN KEY...` additionally require a changed post-action frame. Patterns are per-line extended regexes. They fail with current-pane diagnostics. Pair screen readiness with correlated durable completion before the next action; do not use transient footer counts or archived text as readiness.

The copied driver's `wait` and `capture` include scrollback; they are not fresh-state verification. `common.sh`'s `capture FILE` saves the current pane. `lib/semantic-provider.sh` owns the local server lifecycle and durable waits; `lib/semantic-provider.rs` validates actual requests and provides the feature docs' offline negative controls. Run these recipes through `just dev`/`just dev-shell` so `rustc` and jq use the declared environment.

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
