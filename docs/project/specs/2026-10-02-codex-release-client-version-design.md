# Codex Model Listing Follows Codex Releases

**Outcome:** plane:YACH-17

Status: approved by owner, 2026-10-02. Origin: Kata `yach#np83`.

## Problem

The ChatGPT Codex backend (`/codex/models?client_version=…`) returns only the
models it considers available to the Codex release identified by
`client_version`. Yach sends `yach_catalog::baked_codex_protocol_version()`:
the highest `minimal_client_version` among listed models in the baked
`crates/yach-catalog/data/codex-models.json`
(`crates/yach-catalog/src/lib.rs:256-266`). Two facts make that wrong:

- **The field is not the gate.** `gpt-6.1-sol` declares
  `minimal_client_version` `0.153.0`, but the backend omitted it for
  `client_version` `0.155`, `0.156` and `0.158` and returned it for `0.160`
  (probed 2026-10-02). It first appears in the `rust-v0.160.0` release's
  `codex-rs/models-manager/models.json`; `rust-v0.158.0` and `rust-v0.159.0`
  do not list it. [INFERENCE] The backend gates on the release that first
  shipped a model, not on the declared floor.
- **The pin is not a release.** `codex-models.pin` holds an openai/codex
  `main` commit (`e7ea5f4a`, 182 commits past what `rust-v0.160.0` built
  from). `main` builds report version `0.0.0`, so the pin carries no version
  of its own.

Updating the version therefore needs a code change and a release. A model
OpenAI ships in a new Codex release stays invisible until both happen, and
cache windows add up to 4h more.

## Baseline (main at `ee68eaff`)

No earlier spec records the Codex catalog; this section is the reference.

- `crates/yach-catalog/data/codex-models.pin`: one line, an openai/codex
  commit SHA. `just catalog-codex-snapshot` (`justfile:214-244`) fetches
  `codex-rs/models-manager/models.json` at that commit into
  `codex-models.json`, which is baked with `include_str!`
  (`crates/yach-catalog/src/lib.rs:241,262`).
- `baked_codex_protocol_version()` (`lib.rs:256-266`) returns the highest
  `minimal_client_version` among `visibility: "list"`, API-supported
  models, parsed once per process. Callers send it as `client_version`:
  discovery (`crates/yach-cli/src/provider_connections.rs:255,285`,
  `crates/yach-cli/src/main.rs:4090`), the Codex catalog fetch
  (`provider_connections.rs:1523`), and the discovery cache's freshness key
  (`crates/yach-cli/src/model_discovery_cache.rs:331`).
- Caches: model discovery (`~/.yach/model-discovery.json`, fresh for 2h,
  `CACHE_FRESHNESS_SECONDS`, `provider_connections.rs:30`) and the fetched
  Codex catalog (`~/.yach/catalog/codex-models.json`, every 4h with an ETag,
  `REMOTE_CATALOG_REFRESH_INTERVAL_MS`, `catalog_refresh.rs:20`). Since #288
  both record the `client_version` they were fetched under; a different or
  missing version is stale, and the catalog refetch drops the old ETag.
- Triggers: `refresh_models` (`provider_connections.rs:433`) runs discovery
  and spawns the Codex catalog refresh (`:456`, skipped while one is in
  flight via the `CODEX_CATALOG_REFRESH_IN_FLIGHT` atomic, `:32,1532`). It is
  called only when the `/model` picker opens (`AvailableModelsRequested`,
  `crates/yach-backend/src/runner.rs:1894-1913`) and after a successful
  `/connect` mutation (`ConnectionFlowEffect::RefreshModels`,
  `crates/yach-backend/src/provider_connections.rs:780`, `runner.rs:662`).
  Nothing Codex-related runs at startup (`FirstRenderCompleted`,
  `runner.rs:1724`, only resolves the session model target).

## Outcome

ChatGPT subscription users see every model OpenAI lists for the current
stable Codex release without editing files or upgrading yach. A forced
refresh is always available for the window between a release and yach's next
scheduled check.

Out of scope: models.dev catalog refresh (it feeds the next launch by design,
`main.rs:910-914`, and does not decide which models exist),
Anthropic/OpenAI/compatible discovery semantics beyond the forced-refresh
bypass, a version cap setting, and the 200k fallback for unlisted models
(`yach#r3pp`).

## Design

### 1. Pin the baked snapshot to releases

- `codex-models.pin` format becomes one line: `<tag> <commit>`, e.g.
  `rust-v0.160.0 1a2b…`. The tag must match `^rust-v\d+\.\d+\.\d+$`. A
  `yach-catalog` unit test parses the committed pin and fails on any other
  shape.
- `just catalog-codex-snapshot` with no overrides resolves the latest stable
  release via `GET https://api.github.com/repos/openai/codex/releases/latest`
  (that endpoint excludes drafts and prereleases), resolves the tag to its
  commit, fetches `codex-rs/models-manager/models.json` at that commit, and
  writes both files. `CODEX_MODELS_TAG=<tag>` pins a specific release. The
  local-file override (`CODEX_MODELS_JSON`) requires `CODEX_MODELS_TAG` and
  `CODEX_MODELS_PIN` and still validates the tag shape. Non-release refs
  are rejected.
- `yach_catalog::baked_codex_release_version() -> &'static str` returns the
  pin tag's version (`0.160.0`), parsed once. It replaces
  `baked_codex_protocol_version`; `max_listed_codex_protocol_version` and its
  test are deleted.
- Re-pin to `rust-v0.160.0` in the same change. Its listed models match
  today's snapshot (`gpt-6-astra`, `gpt-6.1-sol`, `gpt-6-sol`, `gpt-6-luna`,
  `gpt-5.6-sol/terra/luna`, `gpt-5.5`).

### 2. Track the latest stable release at runtime

New module `crates/yach-cli/src/codex_release.rs`:

- Cache file `~/.yach/catalog/codex-release.json`:
  `{ "version": "0.162.0", "tag": "rust-v0.162.0", "etag": "…",
  "checked_at_unix_ms": 1759… }`. Written with the existing atomic
  temp-file-and-rename helper (`catalog_refresh::write_cache_to`).
- Check: `GET …/releases/latest` with `User-Agent: yach/<version>`,
  `Accept: application/vnd.github+json`, `If-None-Match` from the cache, and
  a 5s timeout. Through `reqwest::blocking` like the models.dev fetch
  (`catalog_refresh.rs:416`), run on a blocking thread. Only a `tag_name`
  matching `^rust-v\d+\.\d+\.\d+$` is accepted; anything else counts as a
  failure.
- Interval: due when `checked_at_unix_ms` is absent, in the future, or older
  than `REMOTE_CATALOG_REFRESH_INTERVAL_MS` (4h). Reuse `refresh_due`'s clock
  rules. A 304 or a failure updates only `checked_at_unix_ms` and keeps the
  last version (mirrors `cache_after_not_modified` /
  `cache_after_failed_response`).
- **Effective client version** =
  `max(baked_codex_release_version(), cached latest)` using
  `compare_dotted_versions`. It never decreases below the baked release; a
  missing or corrupt cache file means "baked only". The value is held in
  process state (`RwLock<String>`) that the check updates, so a version
  change applies to the next discovery or catalog request in the same
  session.
- Every current caller of `baked_codex_protocol_version()` switches to the
  effective version: `provider_connections.rs:255,285,1523`,
  `main.rs:4090`, and `model_discovery_cache::listing_client_version`. The
  #288 rules stay unchanged: a Codex discovery entry or Codex catalog from
  another version is stale, and the catalog drops its old ETag.

### 3. When checks run

| Trigger | Today | New behaviour |
|---|---|---|
| Launch with a ChatGPT subscription connection (TUI and RPC) | nothing | **new trigger**: after `CliProviderConnectionRuntime::system` is built (`main.rs:946,4039`), spawn the release check in the background if due; never blocks startup or the first render |
| `/model` picker opens (`AvailableModelsRequested` → `refresh_models`) | Codex catalog refresh if due; discovery per 2h freshness | if the release check is due, it runs first, bounded by its 5s timeout; then the existing catalog refresh and discovery under the effective version |
| `/connect` mutation succeeds (`RefreshModels` effect → `refresh_models`) | same as picker open | same as picker open |
| Forced refresh (below) | — | everything, ignoring every interval |

The release check gates only the start of `refresh_models`; discovery still
runs per connection concurrently (`MAX_DISCOVERIES_IN_FLIGHT`). One picker
open therefore picks up a new release, at the cost of at most 5s of "loading
available models" once per 4h when the launch check has not already run.
The picker shows cached rows while loading (`app.rs:2283-2295`,
`runner.rs:1894-1913`).

### 4. Forced refresh

Entry points, one backend path:

- `/model refresh`: `parse_slash_command` (`slash_commands.rs:187-231`)
  returns `ArgumentsUnsupported` for `/model <args>` today. `SlashAction::Model`
  joins the argument allowlist, and the app accepts exactly `refresh`; any
  other argument (e.g. `/model gpt-5`) shows `usage: /model [refresh]` and
  does not open the picker. `parser_rejects_arguments_for_alpha_commands`
  (`slash_commands.rs:336`) drops `/model` from its rejected set. `/model
  refresh` opens the picker and requests a forced refresh.
- `Ctrl+R` in the `/model` picker. The picker is a typed filter (plain
  characters edit the query, `app.rs:2553`), so the binding needs a
  modifier. The picker title shows the hint: `Select Model · Ctrl+R refresh`.
- `yach models refresh`: a new CLI subcommand (parsed next to `preset` and
  `extension` in `CliArgs::from_args`, `main.rs:141-190`). It runs the same
  chain without a TUI, prints the outcome line, and exits non-zero if every
  step failed.

Protocol: an additive `ClientEvent::AvailableModelsRefreshRequested`
(`crates/yach-proto/src/lib.rs:846`, next to `AvailableModelsRequested`).
The runner handles it like `AvailableModelsRequested` but calls
`ProviderConnectionRuntime::refresh_models` with a new `RefreshMode::Forced`
argument (`Normal` for all existing callers). Without a connection runtime
(legacy env-configured provider) it falls back to `AvailableModelsRequested`
behaviour and reports `forced refresh needs a stored connection`.

`RefreshMode::Forced` in `CliProviderConnectionRuntime::refresh_models`:

1. Release check now, ignoring the interval (still conditional on the ETag).
2. Codex catalog refresh now, awaited and ignoring the interval. The old
   ETag is sent only if the effective version is unchanged. The
   process-wide `CODEX_CATALOG_REFRESH_IN_FLIGHT` `AtomicBool`
   (`provider_connections.rs:32,1532`) currently drops a second request; it
   becomes a `tokio::sync::Mutex<()>` so a forced refresh waits for an
   in-flight one and then runs instead of being dropped. Normal mode keeps
   today's skip-if-busy behaviour through `try_lock`.
3. Discovery for every ready connection, ignoring `CACHE_FRESHNESS_SECONDS`.
   On failure the cached rows remain the fallback, as today
   (`provider_connections.rs:1254-1278`).

The outcome reaches the user as one status line replacing
`provider models refreshed` for forced runs:
`models refreshed · Codex 0.160.0 → 0.162.0 · +2 models`, or
`models refreshed · no changes`. Step failures are appended:
`· release check failed (using 0.160.0)`. `+N models` counts picker rows
not present in the previously advertised catalog.

### 5. Scheduled re-pin

`.github/workflows/codex-catalog-repin.yml`: `schedule` weekly plus
`workflow_dispatch`, `permissions: contents: write, pull-requests: write`.
It runs `just catalog-codex-snapshot`; when the pin or snapshot changed, it
pushes branch `codex-catalog/<tag>` and opens a PR titled
`Re-pin baked Codex catalog to <tag>`, unless one with that branch already
exists. The PR is reviewed like any data change. Shipping it raises the
baked floor; the runtime check already covers users in between.

## Error handling

- Every GitHub request sends `User-Agent: yach/<version>` (the API answers
  403 without one) and `If-None-Match` when an ETag is cached; 304s do not
  count against the 60/h unauthenticated limit.
- The GitHub API is unreachable, times out, answers 403/429 (rate limit), or
  returns an unexpected body: a soft failure. Keep the last version, wait
  out the interval (normal mode), never block more than 5s, and surface it
  only in the forced-refresh status line, never as an error dialog.
- Cache unreadable or a malformed version: treated as absent (baked
  release only); the next successful check rewrites it.
- The effective version never goes below the baked release, so a stale or
  hostile cache can only fail to advance it.
- A version change during an in-flight discovery: that refresh's rows still
  publish; the version-aware caches (#288) refetch on the next request.

## Testing

Unit, following existing patterns in `catalog_refresh.rs` and
`model_discovery_cache.rs`:

- Pin parsing accepts `rust-v0.160.0 <sha>` and rejects a bare SHA, an
  alpha tag, and a missing commit. A test against the committed pin file
  guards the format.
- Release response parsing: accepts a stable tag; rejects
  `rust-v0.162.0-alpha.7`, a non-`rust-v` tag, and malformed JSON.
- Effective version: max of baked and cached; a cached version below the
  baked one is ignored; no cache means baked.
- Release-cache state machine: due/not due at the interval boundary and for
  a future timestamp; a 304 and a failure keep the version and advance
  `checked_at`.
- Forced mode bypasses every interval: discovery fresh-hit is ignored, the
  catalog refreshes inside the interval, and the release check runs inside
  the interval. Tested with injected fetchers like the existing `discoverer`
  seam.
- A forced Codex catalog refresh waits for, instead of being dropped by, an
  in-flight normal refresh.
- UI: `/model refresh` parses to the forced action, `/model foo` shows
  usage, and `Ctrl+R` in the picker sends `AvailableModelsRefreshRequested`.
- CLI: `yach models refresh` parses; unknown `models` subcommands are
  rejected.

Live acceptance (recorded in the PR):

1. Build with the pin temporarily set to `rust-v0.158.0` (no
   `gpt-6.1-sol`), using a copy of a real `~/.yach` with a ChatGPT
   subscription connection, a `codex-release.json` recording `0.158.0` and
   checked now (so the launch check is not due), and a saved default that
   is not `gpt-6.1-sol` (the active model is always listed, which would
   mask the result).
2. Open `/model`: `gpt-6.1-sol` is absent (neither the `rust-v0.158.0`
   snapshot nor the `0.158.0` live listing contains it).
3. Press `Ctrl+R`: the status shows `Codex 0.158.0 → 0.160.0 · +1 models`,
   `gpt-6.1-sol` appears in the picker, and `model-discovery.json` lists it
   under `client_version` `0.160.0`.
4. `yach models refresh` on the same home reports `no changes`.

## Documentation

- README: `/model refresh`, `Ctrl+R` in the picker, `yach models refresh`,
  and one sentence on how yach chooses the Codex client version.
- The `catalog-codex-snapshot` recipe comment describes the release pin.

## Decisions (owner, 2026-10-02)

- **models.dev stays out of the forced chain.** Its refresh feeds the next
  launch by design and does not change which models exist.
- **Re-pin PR CI: accept the `GITHUB_TOKEN` limitation.** GitHub does not
  run `pull_request` workflows for a PR opened with the default
  `GITHUB_TOKEN`; the owner closes and reopens the re-pin PR to run CI. The
  repository setting "Allow GitHub Actions to create and approve pull
  requests" must be enabled before the workflow can open PRs. No extra
  token secret.
- **No version cap setting.** Advertising the newest Codex release asserts
  yach handles what that release does; a model needing a protocol feature
  yach lacks could appear and fail when used. Add a cap only if that
  happens.
