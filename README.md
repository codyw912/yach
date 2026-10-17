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

Yach has a strong native foundation and is used for real coding sessions, but
it is not yet close to the usability bar for daily work. Direction and
milestones are in [docs/project/roadmap.md](docs/project/roadmap.md). Expect
sharp edges and breaking changes.

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

- Onboarding is rough: `/connect` stores named provider connections and
  `/model` picks from a catalog, but there is no guided first-run flow.
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

The default install applies the `full` preset on first run, which enables
yach's reference components (the project tools, baseline guidance, and the
bundled hashline and Jev extensions). To build a core binary without the
bundled extension code, install with `--no-default-features`. See
[docs/presets.md](docs/presets.md) for presets, components, and removal.

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
`auto-review` delegates approval to a reviewer extension
(e.g. Jev) that assesses each request's hazards and scope against your recent
messages; user restrictions are enforced by Yach, not the reviewer. The
reviewer sees your recent user messages plus the action being requested; it is
not a sandbox — the extension host runs with the agent's privileges. In this
build automatic execution is compiled off (`AUTO_REVIEW_EXECUTION_ENABLED =
false`): actions the reviewer would have allowed are shown to you for
approval, while reviewer holds (risk, clarification, restrictions) still apply
and human-performs restrictions are handed off, not run — until the
evaluation gates in `docs/project/records/2026-09-25-auto-review-signals-evaluation.md`
pass.
`auto-review` is session-only like `full-access`, never persisted,
and resets on restart. The picker works during active turns; a change affects
future tool requests without changing a pending review. The active mode stays
visible in the status bar and `/status`. `/help` lists commands; useful
day-to-day commands include `/resume`, `/model`, `/approval`, `/fork`, and
`/quit`.

`/model refresh` (or `Ctrl+R` in the `/model` picker) re-checks every provider's
model list now, ignoring cache windows; `yach models refresh` does the same from
the shell. For ChatGPT subscriptions, Yach identifies as the latest stable Codex
release, checked at most every four hours.

`Ctrl+T` selects provider thinking effort. An explicit selection is owned by the
backend, recorded in the session, and written to
`~/.yach/config.toml` as the global default for new sessions. Session evidence
still wins on resume. The selected level is carried into Anthropic,
OpenAI/ChatGPT Responses, and OpenAI-compatible requests. With no configured
default, Yach shows `off` and preserves the provider's previous request
defaults.

Applied edits show a bounded changed-line preview plus the next
`[path#SNAPSHOT]` tag instead of only `[applied]`. The TUI uses the normal
terminal screen without mouse capture: native mouse selection/copy remains
available, and starting the next turn archives the completed prior transcript
into terminal scrollback for Herdr, tmux, and terminal copy modes.
(read-only-safe by default; `--full-auto` explicitly selects the same
session-only, unsandboxed `full-access` backend posture; `--auto-review
<reviewer-id>` selects the named reviewer extension for automatic approval).
Use `--full-auto` only in disposable working directories, ideally containers.
`--full-auto` and `--auto-review` are mutually exclusive.

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

Inspect a session afterwards with `yach sessions list` (newest first) and
`yach sessions show latest` (or a session id). `show` prints a per-turn
timeline of prompts, tool calls, and provider attempts with their timings;
add `--json` to either command for one machine-readable line.

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
| `YACH_THEME` | user file, then `auto` | Preset (`dark`, `light`, `system`, `auto`), bundled palette name, or explicit path to a theme JSON file. A non-empty `NO_COLOR` overrides it. |
| `NO_COLOR` | unset | Non-empty selects the monochrome theme and overrides `YACH_THEME` and `~/.yach/theme.json`. |
| `COLORFGBG` | unset | Read only when the theme is `auto`. Last field below 8 selects `dark`; otherwise `light`. Absent or unparsable selects `dark`. |
| `YACH_SESSION_DIR` | project-keyed directory under `~/.yach/sessions/` | Absolute override for session storage and lookup. |
| `YACH_CAPTURE_REQUESTS` | off | Absolute directory. Writes each OpenAI, OpenAI-compatible, ChatGPT-subscription, and native-compaction request body, one private file per attempt. Anthropic requests are not captured. Authentication headers are never written; bodies are the exact request bodies, so they contain whatever the prompt and files contain. Keep capture off unless diagnosing a request. |

Launching without credentials still opens the TUI and explains what is
missing; prompts fail with the setup error until the environment is fixed.

## Configuration

File-first and inspectable:

- `AGENTS.md` (project root and nested) — project instructions injected into
  the model's context.
- `.yach/APPEND_SYSTEM.md` — explicit extra system guidance for this project.
- `~/.yach/config.toml` — user-owned defaults, including the default model
  and thinking level.
- `~/.yach/config.json` — user-owned authority such as shell allowlists.
- `<project>/.yach/config.json` — project file policy, compaction, and
  restrictive shell runtime settings; repository content cannot grant shell or
  environment authority.

`~/.yach/config.toml` (defaults; written by `/model` save-as-default and
`Ctrl+T`, preserving unrelated content):

```toml
[thinking]
default = "high"

[model.default]
provider = "anthropic"
model = "claude-sonnet-5"

[preset]
applied = "full"

[components]
project-tools = true
baseline-guidance = true

[bundled]
removed = []
```

`preset.applied` is the last applied preset (absent means the first run has
not happened yet). `[components]` toggles kernel components;
`[bundled] removed` remembers bundled extension ids you removed. See
[docs/presets.md](docs/presets.md).

`thinking.default` accepts `off`, `low`, `medium`, `high`, or `max`. A resumed
session uses its own recorded level; a new session in any project uses the
user default.

`<project>/.yach/config.json` (file policy):

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

### TUI appearance

TUI appearance is chosen once, at startup. `NO_COLOR` set to any non-empty
value wins over everything else and selects the monochrome theme. Otherwise
the first match wins:

1. Non-empty `YACH_THEME`: a preset (`dark`, `light`, `system`, `auto`), a
   bundled palette name, or an explicit path to a theme file. Empty is unset.
2. `~/.yach/theme.json`.
3. `auto`.

A value that is absolute, starts with `.` or `~`, or contains a path
separator is a path; `~/` uses your home directory. Other values are theme
names, even if a bare file of that name exists. Use `./dark` for a file named
`dark`. Unknown names show a diagnostic listing built-in names and fall back
to `auto`. An explicit path that is not a regular file (including a FIFO or
directory), cannot be read, or cannot be applied shows one startup notice
and uses `auto`.

`<project>/.yach/theme.json` is not read as a project theme. Unless it resolves
to the same file as the personal theme, the TUI shows one notice when it exists
and leaves the file untouched: "project themes are not
supported; move it to `~/.yach/theme.json` or set `YACH_THEME`". For a
per-project look, set `YACH_THEME` in that project's environment.

`auto` reads `COLORFGBG` only. The last semicolon-separated field is the
background index (`fg;bg` or `fg;default;bg`). An index below 8 selects
`dark`, any other index selects `light`, and an absent or unparsable value
selects `dark`. Only some terminals set `COLORFGBG`; shells and multiplexers
can inherit stale values, and it does not update when the terminal switches
appearance. When the guess is wrong, set `YACH_THEME=light`,
`YACH_THEME=dark`, or `YACH_THEME=system`. `system` paints no panel surfaces
and uses the terminal's default text colors.

| Preset | Surfaces |
| --- | --- |
| `dark` | Painted dark panels; the fallback behind `auto` when detection says dark. |
| `light` | Painted light panels with dark text on every surface. |
| `system` | No panel backgrounds. Status uses the terminal's ANSI colors. |
| `auto` | Resolves to `dark` or `light` from `COLORFGBG`. |

Bundled palettes are named themes, selected the same way (`YACH_THEME=nord`
or `"base": "nord"`). They are 24-bit; on a limited-color terminal use
`system`. The names are `catppuccin-latte`, `catppuccin-frappe`,
`catppuccin-macchiato`, `catppuccin-mocha`, `rose-pine`, `rose-pine-moon`,
`rose-pine-dawn`, `gruvbox-dark`, `gruvbox-light`, `tokyo-night-dark`,
`tokyo-night-storm`, `tokyo-night-moon`, `tokyo-night-light`,
`solarized-dark`, `solarized-light`, `dracula`, `nord`, and `monokai`.
Attribution is in [the palette NOTICE](crates/yach-ui/themes/NOTICE.md).

Bundled palettes and painted presets are tested for body-text contrast of
at least 4.5:1 on their painted surfaces. Muted, success, warning, error,
and harness colors clear 3:1 on both pending and error panels; accent clears
3:1 on pending and user-message panels. Bundled palettes also require muted
and dim colors to clear 3:1 on the scheme's page background (`base00`).
Presets leave the page color to the terminal, so that pair cannot be tested.
The `dark` preset uses explicit RGB foregrounds on its painted panels rather
than relying on the terminal's ANSI palette.

A theme file is strict JSON. `base` names the theme its overrides apply to
and defaults to `dark`, so an existing file without `base` keeps its meaning.
`base` accepts a preset, `auto`, a bundled palette name, or
`<extension-id>:<theme-id>` contributed by an installed user-scope extension
(see [Contributing themes](docs/extensions.md#contributing-themes)). Extension
themes are selected only through a file's `base`, not directly through a
bare `YACH_THEME` name. Colors in the file override that base. No file is
merged with another file. An unresolved `base` shows one notice and keeps
the file's overrides over `auto`.

```json
{
  "base": "auto",
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
variables in the personal file fail before the TUI opens rather than being
ignored; explicit `YACH_THEME` file failures instead show the fallback notice.

Monochrome (`NO_COLOR`) draws no color and no panel background. Metadata is
dim, focused titles, tool names, and errors are bold, and the selected row is
reversed. Glyphs (`✓`, `✗`, `⚙`, `›`), harness outcome labels (`! failed`,
for example), and the tool row's `┃` rule carry the distinctions that color
carried. Tool successes use neutral panels; only failure surfaces are tinted.

Selected confirm, approve, and reject labels keep their action-colored
background. They use `selectedText` when it reaches 4.5:1 contrast against
that background; otherwise they use whichever of black or white contrasts
better. This also applies to the dark preset. Terminal-default backgrounds
cannot be measured, so they retain the requested text color.

For repeatable theme captures, use `just tui-visual session narrow light
nocolor`; `just dev bash tests/visual/check-nocolor.sh` checks the raw TUI
bytes independently of VHS. Release size is checked by the relative per-PR
size gate: `just perf --filter 'binary/size_bytes' --deterministic`.

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

Each session line carries a wall-clock `at_ms` timestamp. Tool results record
when they started and how long they ran. Every provider attempt records one
bounded event: its outcome, error class, HTTP status, retry delay, duration,
time to first event, and the provider's request ID from the response headers.
Request bodies, headers, and credentials are never written to the session
log. With `YACH_CAPTURE_REQUESTS` set, OpenAI, OpenAI-compatible,
ChatGPT-subscription, and native-compaction request bodies go to separate
`0600` files under `<dir>/<session-id>/`, one per attempt and never
overwritten, and the attempt event names its capture file. Anthropic
requests are not captured. Authentication headers are never written to
capture files; bodies are the exact request bodies, so they contain whatever
the prompt and files contain. `yach sessions show` reads only the session
log.

## Workspace layout

- `crates/yach-cli` — the `yach` binary and command parsing.
- `crates/yach-ui` — the terminal UI; speaks only the yach protocol.
- `crates/yach-proto` — the UI/backend protocol seam.
- `crates/yach-backend` — sessions, tools, edit engine, provider loop,
  extension runtime.
- `crates/yach-bench` — startup, session, and edit benchmarks.

## Development

Install Nix and devenv CLI **2.3.1**. The contributor environment is owned by
`devenv.nix`, `devenv.yaml`, and `devenv.lock`; no private workstation configuration
or `dev-init` installation is required. From the repository root:

```sh
devenv shell -- just test
devenv shell -- just lint
devenv shell -- just build
```

`devenv test` runs the same locked workspace tests as `just test`. Inside an
activated environment, use the existing `just` commands normally. For optional
automatic activation, install direnv, review `.envrc`, and run `direnv allow`.
After environment changes, reload it with `direnv reload`.

The default development profile keeps line tables for useful backtraces while
avoiding the full-debug artifact cost. For full debug information when needed,
use:

```sh
just dev cargo build --profile debug-full
just dev cargo test --profile debug-full
```

The shell reads the Rust version from `rust-toolchain.toml` and provides the project's tools and custom Cargo hooks.
Shell entry does not initialize a Cargo project. Keep personal changes in ignored
`devenv.local.nix` or `devenv.local.yaml`; required contributor settings belong in
the shared configuration. Update inputs deliberately with `devenv update <name>`.

Packaging still uses `nix build`. Its retained `flake.lock` pins those builds
independently of the contributor environment's `devenv.lock`.

Hook definitions live in `devenv.nix`. Shell entry regenerates
`.pre-commit-config.yaml` as a regular file; keep that generated file tracked so
shared hooks also work in linked worktrees. Change the Nix definitions rather than
editing the generated file, and commit both when hook behavior changes.

Static checks (`cargo fmt --check`, Clippy with `-D warnings`) are also declared
for [hk](https://hk.jdx.dev) in `hk.pkl`, which amends the template base
`.hk/base/rust.pkl`. The dev shell provides the pinned hk binary from
`nix/hk.nix`. Agents run them through sjujperpowers' `verifying-by-risk`
`scripts/hk-check`, which ignores local overrides; hk's git hooks are not
installed, and tests stay in `just test`. Re-pin hk with
`bash scripts/update-hk [vX.Y.Z]`, which updates the binary hashes and the hk
Config package version together; do not use `devenv update` for it.

The runtime image in `containers/yach-runtime/` and evaluation containers remain
supported. The old
template-generated devcontainer and `devc` helper are no longer provided.

Contributor conventions, including the strict Clippy policy, live in
[AGENTS.md](AGENTS.md). CI deliberately uses its own explicit Rust toolchain;
a local check does not establish a passing GitHub Actions run.

Design decisions are recorded: the original product direction is
[PRD-v0.1.md](PRD-v0.1.md), nontrivial features get a design doc in
`docs/project/specs/` backed by research records in
`docs/project/records/`, and [docs/README.md](docs/README.md) indexes the rest.

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
