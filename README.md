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
- Anthropic provider out of the box (`chatgpt-subscription` also wired).
- Yach-owned tools: `read_text_file`, `search_project`, `list_project_paths`,
  and exact-match `edit_text_file` / `create_text_file` with interactive
  review before anything is written.
- A `bash` tool that runs project commands (tests, builds) after your
  review; trusted commands can auto-run via a parse-aware config allowlist,
  and secret-shaped environment variables are stripped from subprocesses by
  default.
- Multi-round tool loops; recoverable tool failures with actionable errors.
- Sessions persisted as inspectable JSONL; `/resume` and `--resume` restore
  the transcript (including tool output) and the model's context.
- Sensitive files (`.env*`, key material, credential stores) denied to all
  file tools by default, overridable in config.
- Early extension runtime: manifest-based, process-hosted extension tools
  with install/enable/stop/reload lifecycle (`yach extension ...`).

Not yet:

- No context compaction — long sessions eventually hit the provider's
  context limit. Slated soon.
- Command output does not stream into the transcript yet (it appears when
  the command finishes); no background processes or sandboxed executors.
- No one-shot/non-interactive mode.
- No MCP, no network tools, no broad write/patch/delete tools.
- No packaged releases; install is via cargo from source.
- UI polish is minimal.

## Install

```sh
cargo install --git https://github.com/codyw912/yach yach-cli
```

This builds and installs a `yach` binary. From a checkout,
`cargo install --path crates/yach-cli` does the same.

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

Environment:

| Variable | Default | Purpose |
| --- | --- | --- |
| `YACH_RIG_PROVIDER` | `anthropic` | Provider selection (`anthropic`, `chatgpt-subscription`). |
| `YACH_RIG_ANTHROPIC_API_KEY` | — | Required for the default provider. |
| `YACH_RIG_ANTHROPIC_MODEL` | `claude-haiku-4-5` | Model id. |
| `YACH_RIG_ANTHROPIC_TIMEOUT_SECS` / `..._MAX_TOKENS` | sane bounds | Request tuning. |

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

Sessions are append-only JSONL under `<project>/.yach/native-sessions/`
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
