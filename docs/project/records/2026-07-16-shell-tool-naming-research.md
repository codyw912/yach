# Shell Tool Names And Schemas Across The Cohort

Date: 2026-07-16

Context: the shell execution design initially proposed a `run_command` tool.
Owner review flagged the name as an unjustified divergence from the cohort.
This record verifies the exact model-facing tool names and schemas from
primary sources.

## Verified tool surface per harness

| Harness | Tool name | Key params | Session semantics |
| --- | --- | --- | --- |
| Claude Code | `Bash` | `command` (string, required); `timeout` (number, ms, default 120000, max 600000); `description`; `run_in_background` | Per-command process; cwd persists between calls, shell state does not |
| opencode | `bash` (source file renamed to shell.ts but the tool ID is explicitly frozen as `bash` "for compatibility with existing plugins, users, and saved permissions") | `command` (string, required); `timeout` (positive int, ms, default 120000); `workdir` ("use this instead of 'cd'") | Per-command spawn (its "persistent shell session" description text is inherited prose, not behavior) |
| Codex CLI | historically `shell` (argv array) → currently `shell_command` (string) and `exec_command`/`write_stdin` (persistent PTY) — and it permanently routes alias handlers for `shell`, `container.exec`, `local_shell` because models keep emitting trained names | `command` (string, required); `workdir` ("defaults to the turn cwd... do not use cd"); `timeout_ms`; `login`; sandbox/justification params | `shell_command` per-command; `exec_command` persistent PTY sessions |
| Pi | `bash` | `command` (string, required); `timeout` (number, **seconds** — the cohort outlier) | Per-command spawn, fixed cwd, kill-tree on abort |
| Gemini CLI | `run_shell_command` | `command`; `description`; `dir_path`; `is_background` | Per-command |
| Cline | legacy `execute_command`, current `bash` (docs mark the old name legacy); input normalizer accepts `command`/`cmd`/bare string — evidence that models emit varied shapes | `commands` (string[]) | Per-batch |
| aider | none — no function-calling shell tool; the model suggests commands in prose and the user confirms | — | — |

## The two load-bearing findings

1. Codex abandoned its argv-array `shell` schema for the string-based
   `shell_command`, and registers permanent alias handlers for its old tool
   names "so older prompts remain compatible" — models emit the names they
   were trained on regardless of the advertised schema. opencode froze its
   tool ID at `bash` mid-rename for the same reason.
2. The de-facto consensus schema is: `command` as a single shell string
   (never argv), `timeout` in milliseconds with a stated default and cap,
   and `workdir` as a parameter that the description steers the model to
   use instead of `cd` (opencode and Codex converged on this
   independently). Per-command spawn is the behavioral norm; nobody
   persists shell state, and only Claude Code persists cwd.

## Decision adopted

Yach's tool is named `bash` with the consensus schema: `command` (string,
required), `timeout` (ms, optional, clamped to a configured cap), `workdir`
(optional, project-relative, "use instead of cd"). Per-command spawn, no
shell-state persistence. The tool description steers toward yach's
dedicated read/search/list tools, Claude Code style, since yach ships
those. If the tool is ever renamed, the old name stays routed as an alias
(the Codex lesson).
