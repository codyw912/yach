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

## Quickstart

```sh
export YACH_RIG_ANTHROPIC_API_KEY=sk-ant-...
cd your-project
yach
```

Plain `yach` starts an interactive session. Type a prompt; when the model
wants to create or edit a file, yach shows a preview and waits for your
approval before writing. `/help` lists commands; the useful ones day-to-day
are `/resume` (pick a prior session), `/model`, `/fork`, and `/quit`.

Flags: `yach --resume` continues the latest session; `yach --backend
native-fixture` runs a provider-free fixture backend (useful without
credentials).

Headless: `yach run --prompt "..."` runs a non-interactive session
(read-only-safe by default; `--full-auto` approves edits and commands —
use it only in disposable working directories, ideally containers).
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

### Session logs and privacy

Sessions are append-only JSONL under `<project>/.yach/sessions/`
(directory `0700`, files `0600`). They record the full model-visible
transcript — including tool arguments and results — so resume is lossless.
That means session logs contain project file content the model read; the
sensitive-file deny list is what keeps secrets out of both the model's
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

## License

[MIT](LICENSE)
