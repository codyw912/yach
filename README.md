# yach

Yet Another Coding Harness.

Yach is a minimal, extensible coding harness written in Rust: a terminal UI
where a model reads, searches, and edits your project through tools you can
see, review, and audit. It is inspired by
[Pi](https://github.com/badlogic/pi-mono), with the whole stack owned
natively — the UI/backend protocol, session model, provider loop, tool
execution, edit boundary, and extension runtime.

Design positions: the harness owns sessions, tools, and transcripts
(provider SDKs sit below yach-owned seams); every local mutation goes
through an explicit hash-checked edit transaction with user review;
sensitive files are denied to tools by default; configuration and session
state are plain files you can read; and performance claims come from
benchmarks in-repo.

## Status: 0.1.0

Yach is used for real coding sessions, and every item on its
[MVP checklist](docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md)
passes against a live provider. It is still early: expect sharp edges and
breaking changes.

Works today:

- Interactive TUI (`yach`) with streaming responses.
- Anthropic provider out of the box; `openai-compatible` chat-completions
  endpoints (aggregators, Fireworks-style hosts) and `chatgpt-subscription`
  also wired, plus an Anthropic base-URL override for messages-compatible
  aggregators.
- Yach-owned tools: `read_text_file`, `search_project`, `list_project_paths`,
  and exact-match `edit_text_file` / `create_text_file` with interactive
  review before anything is written.
- A `bash` tool that runs project commands (tests, builds) after your
  review, with live output streaming into the transcript; trusted commands
  can auto-run via a parse-aware config allowlist, and secret-shaped
  environment variables are stripped from subprocesses by default.
- Multi-round tool loops; recoverable tool failures with actionable errors.
- Context compaction: automatic at a configurable threshold (checked at
  turn start and between tool rounds), manual via `/compact [focus]`,
  with a live context meter in the status bar. Validated on real
  long-running sessions.
- Headless mode: `yach run --prompt "..."` (or `--script turns.jsonl`)
  runs one full-auto-optional session and emits a machine-readable
  outcome document on stdout with stable exit codes — usable from
  scripts, containers, and eval launchers.
- Protocol mode: `yach rpc --backend fixture` serves the complete
  `ClientEvent`/`ServerEvent` surface as UTF-8 JSONL over stdin/stdout;
  malformed lines are recoverable and diagnostics stay on stderr.
- Sessions persisted as inspectable JSONL; `/resume` and `--resume` restore
  the transcript (including tool output) and the model's context.
- Sensitive files (`.env*`, key material, credential stores) denied to all
  file tools by default, overridable in config.
- Early extension runtime: manifest-based, process-hosted extension tools
  with install/enable/stop/reload lifecycle (`yach extension ...`).

Not yet:

- Provider setup is environment-variable based; a friendlier
  connect/model-selection surface is on the
  [board](docs/project/board.md), as is a model catalog (per-model
  windows, budgets, cost reporting).
- No background processes or sandboxed executors (isolation is a
  deliberately open design question).
- No MCP, no network tools, no broad write/patch/delete tools.
- UI polish is minimal.

## Install

```sh
cargo install yach
```

From a checkout, `cargo install --path crates/yach-cli` does the same.

Use `cargo install yach --force` to replace an older installed release. The
crates.io release is the supported install boundary; `main` may contain
unreleased changes.

## Quickstart

```sh
export YACH_RIG_ANTHROPIC_API_KEY=sk-ant-...
cd your-project
yach
```

Plain `yach` starts an interactive session in conservative `review` mode. Open
the `/approval` picker to choose `accept-edits`, which auto-applies hash-checked
project edits while keeping non-allowlisted commands behind review. Choose
`full-access` for uninterrupted edits and host commands; Yach shows an explicit
host-access warning because commands are not sandboxed and may access files,
credentials, networks, and processes outside the project. `full-access` lasts
only for the current session, is never persisted, and resets on restart or
session switch. Direct `/approval full-access` opens the same warning rather
than bypassing it. The picker works during active turns; a change affects future
tool requests without changing a pending review. The active mode stays visible
in the status bar and `/status`. `/help` lists commands; useful day-to-day
commands include `/resume`, `/model`, `/approval`, `/fork`, and `/quit`.

`Ctrl+T` selects provider thinking effort. An explicit selection is owned by the
backend, recorded in the session, remembered for new sessions in the same
project, and carried into Anthropic, OpenAI/ChatGPT Responses, and
OpenAI-compatible requests. With no saved selection, Yach shows `off` and
preserves the provider's previous request defaults.

Applied edits show a bounded changed-line preview plus the next
`[path#SNAPSHOT]` tag instead of only `[applied]`. The TUI uses the normal
terminal screen without mouse capture: native mouse selection/copy remains
available, and starting the next turn archives the completed prior transcript
into terminal scrollback for Herdr, tmux, and terminal copy modes.

Flags: `yach --resume` continues the latest session; `yach --backend
native-fixture` runs a provider-free fixture backend (useful without
credentials).

Headless: `yach run --prompt "..."` runs a non-interactive session
(read-only-safe by default; `--full-auto` explicitly selects the same
session-only, unsandboxed `full-access` backend posture). Use `--full-auto` only
in disposable working directories, ideally containers.
Fresh session per invocation by default; `--session <id>` names one and
continues it on every rerun — context, turn numbering, and compaction
state carry forward, so repeated invocations form one long-running
session. Multi-turn scripts via `--script turns.jsonl`; the outcome
document lands on stdout, streaming progress on stderr, exit codes
0/1/2/3/4 for completed/failed/setup/approval-required/timeout.

Environment:

| Variable | Default | Purpose |
| --- | --- | --- |
| `YACH_RIG_PROVIDER` | `anthropic` | Provider selection (`anthropic`, `openai`, `openai-compatible`, `chatgpt-subscription`). |
| `YACH_RIG_ANTHROPIC_API_KEY` | — | Required for the default provider. |
| `YACH_RIG_ANTHROPIC_MODEL` | `claude-sonnet-5` | Model id (interactive default). |
| `YACH_RIG_ANTHROPIC_BASE_URL` | Anthropic API | Override for messages-compatible aggregators. |
| `YACH_RIG_OPENAI_API_KEY` | — | API key for OpenAI (Responses API). |
| `YACH_RIG_OPENAI_MODEL` | — | Model id; required when the provider is `openai`. |
| `YACH_RIG_OPENAI_COMPAT_BASE_URL` / `_API_KEY` / `_MODEL` | — | Required when `YACH_RIG_PROVIDER=openai-compatible`. |
| `YACH_RIG_PROVIDER_TIMEOUT_SECS` / `..._MAX_TOKENS` | sane bounds | Request tuning. |
| `YACH_THEME` | auto-discovered | Explicit path to a TUI theme JSON file. |
| `YACH_SESSION_DIR` | project-keyed directory under `~/.yach/sessions/` | Absolute override for session storage and lookup. |

Launching without credentials still opens the TUI and explains what is
missing; prompts fail with the setup error until the environment is fixed.

## Configuration

File-first and inspectable:

- `AGENTS.md` (project root and nested) — project instructions injected into
  the model's context.
- `.yach/APPEND_SYSTEM.md` — explicit extra system guidance for this project.
- `~/.yach/config.json` and `<project>/.yach/config.json` — currently the
  `files` section controls the sensitive-file deny list:

```json
{
  "files": {
    "deny": ["internal-secrets/**"],
    "allow": [".env.ci"],
    "use_default_deny": true
  }
}
```

Defaults deny `.env*` (except `.env.example` and friends), key material
(`*.pem`, `id_rsa*`, ...), and credential stores (`.netrc`,
`.aws/credentials`, `.ssh/`, ...). Denied paths are excluded from search and
listings and refuse reads/edits with an explanation the model can act on.
Invalid config fails closed to the defaults.

TUI themes use `~/.yach/theme.json` for personal settings or
`<project>/.yach/theme.json` for project settings. `YACH_THEME=/path/to/theme.json`
selects any named theme file and takes precedence over both; project settings
take precedence over personal settings. Without a file, yach uses its built-in
Pi-inspired dark theme.

```json
{
  "vars": {
    "surface": "#282832"
  },
  "colors": {
    "accent": "#00d7ff",
    "userMessageBackground": "#343541",
    "toolPendingBackground": "surface",
    "diffAdded": 22
  },
  "spacing": {
    "userMessageHorizontalPadding": 1,
    "userMessageVerticalPadding": 1,
    "toolHorizontalPadding": 1,
    "toolVerticalPadding": 1,
    "toolGap": 1
  }
}
```

Colors accept `#rrggbb`, ANSI names (`red`, `darkGray`, `default`, and
others), 256-color indices, or entries from `vars`. Available color tokens:
`accent`, `border`, `success`, `error`, `warning`, `muted`, `dim`, `text`,
`selectedBackground`, `selectedText`, `userMessageBackground`,
`userMessageText`, `toolPendingBackground`, `toolSuccessBackground`,
`toolErrorBackground`, `toolTitle`, `toolOutput`, `diffAdded`, `diffRemoved`,
`diffContext`, `diffHunk`, and `harness`. Unknown fields, tokens, colors, or
variables fail before the TUI opens rather than being ignored.

### Session logs and privacy

Sessions are append-only JSONL under
`~/.yach/sessions/<project-slug>--<canonical-path-sha256>/` (directories
`0700`, files `0600`). The canonical project path gives each checkout and
worktree its own collision-resistant history without writing generated state
into the repository. `YACH_SESSION_DIR` provides an explicit absolute override;
`--session-path` remains available to headless and RPC callers.

The 0.1 project-local path is a clean cutover: existing
`<project>/.yach/sessions/` logs are not moved or loaded automatically. To keep
them, submit one turn so the new project directory exists, copy the desired
JSONL files there, then remove the old directory. Session logs record the full
model-visible transcript — including tool arguments and results — so resume is
lossless. The sensitive-file deny list keeps secrets out of both the model's
context and these logs. Logs never leave your machine.

## Workspace layout

- `crates/yach-cli` — the `yach` binary and command parsing.
- `crates/yach-ui` — the terminal UI; speaks only the yach protocol.
- `crates/yach-proto` — the UI/backend protocol seam.
- `crates/yach-backend` — sessions, tools, edit engine, provider loop,
  extension runtime.
- `crates/yach-bench` — startup, session, and edit benchmarks.

## Development

Dev commands run through `just` (nix/devenv shell): `just run tui`,
`just test`, `just lint`. Contributor conventions, including the strict
clippy policy, live in [AGENTS.md](AGENTS.md).

Design decisions are recorded: the original product direction is
[PRD-v0.1.md](PRD-v0.1.md), active planning starts at
[docs/project/README.md](docs/project/README.md), and nontrivial features
get a design doc in `docs/superpowers/specs/` backed by research records in
`docs/project/records/` before implementation.

## Releasing

All publishable workspace crates use one synchronized version. A release bump
updates the `version` field in `yach-proto`, `yach-catalog`,
`yach-connections`, `yach-hashline-extension`, `yach-ui`, `yach-backend`, and
`yach`, plus every versioned path dependency between them and `Cargo.lock`.
While Yach is pre-1.0, use a patch bump for backward-compatible fixes and a
minor bump for features or incompatible public CLI, protocol, persistence, or
library changes.

Run `just release-check` on the release change before merging. It verifies the
synchronized package set and dependency requirements, runs formatting, Clippy,
the full test suite, and deterministic `just eval-validate`, then validates
every package's distributable file list without uploading.

Publication is currently blocked at the vendored Rig boundary. Workspace tests
resolve `rig-core` through `[patch.crates-io]`, but published packages resolve
the registry release. `release-check` therefore builds `yach-backend` in an
isolated workspace without the root patch and also refuses an active
`vendor/rig-core` resolution even if that build happens to compile. The release
can proceed only after the load-bearing Rig changes are available from a
registry release or from a published Yach-owned crate strategy.

After the release change is merged, update the local Jujutsu checkout so the
empty working change is directly above synchronized `main`. Run one
credentialed `just eval-gate` over every task with the pinned live profile and
inspect its green artifacts under `evals/.gate/`. Then attest that exact
synchronized version for the publish invocation:

```sh
just eval-gate
YACH_RELEASE_EVAL_GATE_VERSION=0.2.0 just publish
```

Replace `0.2.0` with the version in the release manifests. The environment
variable is an explicit operator attestation, not a substitute for running and
reviewing the live gate; `just publish` requires an exact version match.

The recipe fetches `main@origin`, refuses changes, conflicts, divergent `main`,
or any working-copy parent other than `main`, repeats the preflight, and
publishes to crates.io in dependency order:

1. `yach-proto`, `yach-catalog`, `yach-connections`,
   `yach-hashline-extension`
2. `yach-ui`, `yach-backend`
3. `yach`

Cargo waits for each upload to reach the registry index. If that wait times
out after an upload, rerun `just publish`; already-visible dependency crates
are skipped safely until the final `yach` crate is published.

## License

[MIT](LICENSE)
