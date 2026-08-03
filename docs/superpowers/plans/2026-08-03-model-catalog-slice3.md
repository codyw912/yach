# Model Catalog Slice 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add provider `/models` discovery so `/model` shows the models the configured credential can access, joined with catalog metadata for correct display and model-switch rehydration.

**Architecture:** Discovery is lazy: opening `/model` starts one inert-at-launch future, so provider network work remains off the first-frame path. `yach-backend` uses Rig 0.41.0's existing Anthropic/OpenAI model listers and normalizes their results; `yach-cli` joins discovered existence with baked/fetched/override metadata; the runner caches the resulting `CatalogModelEntry` list for the session and updates an open picker asynchronously. Discovery never supplies context windows, output ceilings, costs, or request-parameter spelling. Failure and the unsupported ChatGPT-subscription shape degrade to the active model only.

**Tech Stack:** Rust 2024 workspace; Tokio; Rig 0.41.0 `ModelListingClient`; reqwest only through Rig for provider discovery and the existing CLI models.dev refresher; Ratatui UI; Jujutsu.

## Global Constraints

- The accepted layer contract remains: baked snapshot -> fetched models.dev metadata -> provider discovery for existence only -> user/project override -> env override. Discovery MUST NOT overwrite metadata fields or provenance.
- Provider discovery starts only in response to `ClientEvent::AvailableModelsRequested`; constructing its boxed future performs no work. No provider `/models` request before first render.
- Anthropic, OpenAI Responses, and OpenAI-compatible configurations use Rig's public `ModelListingClient::list_models()`. `ChatGptSubscription` is explicitly unsupported because Rig declares `ModelListing = Nothing` and the subscription backend is private.
- A discovery failure, timeout, malformed response, or unsupported provider is recoverable: retain the configured active model, emit a redacted status message, and keep prompting available.
- The provider-returned ID set is authoritative. Remove the dated-suffix alias heuristic rather than replacing it with another name heuristic; if the provider returns a dated ID, it is accessible and may be shown.
- Known catalog entries are picker candidates only when non-env metadata supplies both a positive context window and a positive output ceiling. Unknown discovered IDs remain selectable with default metadata, preserving support for newly released and aggregator-only models.
- Provider/model product UX, auth/connect flows, role routing, quirks, error-dialect classification, `sum_log_usage`, and sweep credential renewal are separate board items and out of scope.
- Use `just dev cargo ...` or `just ...`; strict Clippy uses `-D warnings`. Tests use the house `let Ok/Some(...) = ... else { unreachable!(...) }` idiom, never `unwrap()` or `panic!`.
- Follow TDD for every behavior: add one focused test, run it and observe the expected failure, implement the minimum, then rerun it green.
- Create JJ checkpoints with `jj describe -m "<completed intent>"` followed by `jj new`; do not use Git commits.

---

### Task 1: Catalog provider-fallback scope and cache check state

**Files:**
- Modify: `crates/yach-catalog/src/lib.rs`

**Interfaces:**
- Produces: native providers resolve metadata only from their own provider namespace.
- Produces: `openai-compatible` alone may fall back to a matching model ID under another provider.
- Produces:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedCatalog {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    #[serde(default)]
    pub checked_at_unix_ms: Option<u64>,
    pub retrieved: String,
    pub catalog: Catalog,
}
```

- Task 3 consumes `checked_at_unix_ms` for the four-hour refresh throttle.

- [ ] **Step 1: Add failing fallback-scope tests**

Add tests beside the existing `entry_by_model_id_falls_back_to_a_non_configured_provider` coverage:

```rust
#[test]
fn native_provider_does_not_borrow_another_providers_metadata() {
    let mut baked = Catalog::empty("2026-08-03");
    baked.insert(
        "deepseek",
        "deepseek-chat",
        CatalogEntry {
            context_window: Some(128_000),
            ..CatalogEntry::default()
        },
    );

    let profile = resolve(
        "anthropic",
        "deepseek-chat",
        &baked,
        None,
        None,
        None,
        &EnvOverrides::default(),
    );

    assert_eq!(profile.context_window.value, DEFAULT_CONTEXT_WINDOW);
    assert!(matches!(profile.context_window.source, CatalogSource::Default));
}

#[test]
fn openai_compatible_may_borrow_metadata_by_model_id() {
    let mut baked = Catalog::empty("2026-08-03");
    baked.insert(
        "deepseek",
        "deepseek-chat",
        CatalogEntry {
            context_window: Some(128_000),
            ..CatalogEntry::default()
        },
    );

    let profile = resolve(
        "openai-compatible",
        "deepseek-chat",
        &baked,
        None,
        None,
        None,
        &EnvOverrides::default(),
    );

    assert_eq!(profile.context_window.value, 128_000);
    assert!(matches!(profile.context_window.source, CatalogSource::Baked { .. }));
}
```

- [ ] **Step 2: Verify RED**

Run `just dev cargo test -p yach-catalog native_provider_does_not_borrow_another_providers_metadata`.

Expected: assertion failure because `resolve()` currently calls `entry_by_model_id()` for every provider.

- [ ] **Step 3: Scope the fallback in one helper**

Use one private helper for both baked and fetched lookups:

```rust
fn entry_for_provider<'a>(
    catalog: &'a Catalog,
    provider: &str,
    model: &str,
) -> Option<&'a CatalogEntry> {
    catalog.entry(provider, model).or_else(|| {
        (provider == "openai-compatible")
            .then(|| catalog.entry_by_model_id(model))
            .flatten()
    })
}
```

Replace both unconditional fallback chains in `resolve()` with this helper. Keep user/project overrides exact-provider only.

- [ ] **Step 4: Add backward-compatible cache check state**

Add `checked_at_unix_ms` to `CachedCatalog` with `#[serde(default)]`. Update every constructor fixture to set it explicitly. Extend `cached_catalog_round_trips_with_validators` to assert the timestamp round-trips, and add a test parsing the pre-slice-3 JSON shape without the field and asserting `None`.

- [ ] **Step 5: Verify GREEN and checkpoint**

Run:

```text
just dev cargo test -p yach-catalog
just dev cargo check --workspace
```

Then `jj describe -m "fix: scope catalog metadata fallback to compatible providers"` and `jj new`.

---

### Task 2: Provider-native model discovery adapter

**Files:**
- Create: `crates/yach-backend/src/model_discovery.rs`
- Modify: `crates/yach-backend/src/lib.rs`

**Interfaces:**
- Consumes: `RigProviderConfig` from `rig_adapter`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProviderModel {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDiscoveryError {
    Unsupported { provider: &'static str },
    Provider(ProviderError),
}

pub async fn discover_provider_models(
    provider: &RigProviderConfig,
    timeout: Duration,
) -> Result<Vec<DiscoveredProviderModel>, ModelDiscoveryError>;
```

- The returned vector is ID-deduplicated and sorted by ID. Empty IDs, IDs over 256 bytes, and entries beyond `MAX_DISCOVERED_MODELS = 2_048` are discarded.
- `ModelListingError` messages are never forwarded because Rig includes bounded provider response bodies in them. Map only typed variant/status information into `ProviderError`.

- [ ] **Step 1: Add failing normalization and error-redaction tests**

Tests construct `rig::model::ModelList` directly; no socket or mock server:

```rust
#[test]
fn normalize_models_deduplicates_sorts_and_keeps_provider_names() {
    let list = rig::model::ModelList::new(vec![
        rig::model::Model::new("z-model", "Z Model"),
        rig::model::Model::from_id("a-model"),
        rig::model::Model::new("z-model", "Duplicate"),
        rig::model::Model::from_id(""),
    ]);

    assert_eq!(
        normalize_model_list(list),
        vec![
            DiscoveredProviderModel {
                id: String::from("a-model"),
                display_name: None,
            },
            DiscoveredProviderModel {
                id: String::from("z-model"),
                display_name: Some(String::from("Z Model")),
            },
        ]
    );
}

#[test]
fn listing_api_error_maps_status_without_forwarding_the_body() {
    let error = map_listing_error(rig::model::ModelListingError::ApiError {
        status_code: 401,
        message: String::from("response_body_preview: secret provider detail"),
    });

    let ModelDiscoveryError::Provider(error) = error else {
        unreachable!("401 must map to a provider error");
    };
    assert_eq!(error.kind, ProviderErrorKind::Authentication);
    assert!(!error.message.contains("secret"));
    assert_eq!(error.redacted_debug.as_deref(), Some("model_listing_status=401"));
}
```

Add parallel assertions for 429 -> `RateLimited`, request failure -> `Network`, parse failure -> `MalformedStream`, 5xx -> `ProviderInternal`, and timeout -> `Timeout`.

- [ ] **Step 2: Verify RED**

Run `just dev cargo test -p yach-backend normalize_models_deduplicates_sorts_and_keeps_provider_names`.

Expected: compile failure because the module and normalization function do not exist.

- [ ] **Step 3: Implement the provider dispatch with Rig's listers**

Import `rig::client::ModelListingClient`. Build clients exactly like the existing request adapter:

```rust
match provider {
    RigProviderConfig::Anthropic { api_key, base_url } => {
        let mut builder = rig::providers::anthropic::Client::builder().api_key(api_key);
        if let Some(base_url) = base_url.as_deref() {
            builder = builder.base_url(base_url);
        }
        let client = builder.build().map_err(|_| {
            ModelDiscoveryError::Provider(redacted_discovery_error(
                ProviderErrorKind::ProviderInternal,
                "model_client_build",
            ))
        })?;
        list_with_timeout(client.list_models(), timeout).await
    }
    RigProviderConfig::OpenAi { api_key } => {
        let client = rig::providers::openai::Client::builder()
            .api_key(api_key)
            .build()
            .map_err(|_| {
                ModelDiscoveryError::Provider(redacted_discovery_error(
                    ProviderErrorKind::ProviderInternal,
                    "model_client_build",
                ))
            })?;
        list_with_timeout(client.list_models(), timeout).await
    }
    RigProviderConfig::OpenAiCompatible { base_url, api_key } => {
        let client = rig::providers::openai::Client::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|_| {
                ModelDiscoveryError::Provider(redacted_discovery_error(
                    ProviderErrorKind::ProviderInternal,
                    "model_client_build",
                ))
            })?;
        list_with_timeout(client.list_models(), timeout).await
    }
    RigProviderConfig::ChatGptSubscription { .. } => Err(
        ModelDiscoveryError::Unsupported {
            provider: "chatgpt-subscription",
        },
    ),
}
```

Implement `fn redacted_discovery_error(kind: ProviderErrorKind, debug: &'static str) -> ProviderError` with the stable public message `provider model discovery failed` and `redacted_debug: Some(debug.to_owned())`. Implement `async fn list_with_timeout<F>(future: F, timeout: Duration) -> Result<Vec<DiscoveredProviderModel>, ModelDiscoveryError> where F: Future<Output = Result<rig::model::ModelList, rig::model::ModelListingError>>`; it wraps `tokio::time::timeout`, maps Rig's typed error through `map_listing_error`, then calls `normalize_model_list`. Rig already owns Anthropic base-URL normalization, `x-api-key`/`anthropic-version`, pagination, OpenAI bearer auth, and OpenAI-compatible base-URL joining; do not duplicate them with reqwest.

- [ ] **Step 4: Verify GREEN and checkpoint**

Run:

```text
just dev cargo test -p yach-backend model_discovery
just dev cargo test -p yach-backend normalize_models
just dev cargo check --workspace
```

Then `jj describe -m "feat: provider-native model discovery adapter"` and `jj new`.

---

### Task 3: Root-aware override loading and four-hour catalog refresh throttle

**Files:**
- Modify: `crates/yach-cli/src/catalog_refresh.rs`
- Modify: `crates/yach-cli/src/main.rs`

**Interfaces:**
- Consumes: Task 1's `CachedCatalog.checked_at_unix_ms`.
- Produces:

```rust
const REMOTE_CATALOG_REFRESH_INTERVAL_MS: u64 = 4 * 60 * 60 * 1_000;

impl ModelOverrideLayers {
    fn load_for_project(project_root: Option<&Path>) -> Self;
}

pub fn spawn_refresh_status(
    existing: Option<yach_catalog::CachedCatalog>,
) -> std::sync::mpsc::Receiver<String>;
```

- `run_headless_cli_command` passes `options.project_root.as_deref()`; the TUI resolves `std::env::current_dir()` once and passes the same root to both layer loading and `RunnerConfig.project_root`.
- The models.dev refresh uses the cache already loaded into `ModelOverrideLayers`, removing the fetch thread's second cache read.

- [ ] **Step 1: Add failing project-root tests**

Create two temporary roots with different `.yach/models.toml` values, call `ModelOverrideLayers::load_for_project(Some(&second_root))`, and assert resolution uses the second root even when process cwd is elsewhere. Keep cwd mutation out of the test; the path argument is the contract.

Also extend the headless parse/setup coverage so `--project-root <root>` selects `<root>/.yach/models.toml` rather than `./.yach/models.toml`.

- [ ] **Step 2: Verify project-root RED**

Run `just dev cargo test -p yach model_override_layers_loads_from_explicit_project_root`.

Expected: compile failure because `load_for_project` does not exist.

- [ ] **Step 3: Implement root-aware loading**

Use:

```rust
let project_path = project_root
    .unwrap_or_else(|| Path::new("."))
    .join(".yach/models.toml");
let project = load_model_overrides(&project_path);
```

Keep the user path at `$HOME/.yach/models.toml`. Update all callers; no provider-config helper may perform a second load.

- [ ] **Step 4: Add failing throttle tests**

Add pure tests for:

```rust
#[test]
fn refresh_is_skipped_inside_the_four_hour_checked_at_window() {
    let cache = cached_fixture_with_checked_at(Some(1_000));
    assert!(!refresh_due(Some(&cache), 1_000 + REMOTE_CATALOG_REFRESH_INTERVAL_MS - 1));
    assert!(refresh_due(Some(&cache), 1_000 + REMOTE_CATALOG_REFRESH_INTERVAL_MS));
}

#[test]
fn failed_http_response_advances_checked_at_without_replacing_catalog_data() {
    let cache = cached_fixture_with_checked_at(Some(1_000));
    let updated = cache_after_failed_response(&cache, 2_000);
    assert_eq!(updated.checked_at_unix_ms, Some(2_000));
    assert_eq!(updated.retrieved, cache.retrieved);
    assert_eq!(updated.etag, cache.etag);
}
```

Backward-clock movement (`now < checked_at`) is inside the throttle window. A missing cache remains refresh-due.

- [ ] **Step 5: Implement the throttle and collapse the double read**

Before building the HTTP client, `spawn_refresh` computes current Unix milliseconds and returns `NotModified` without network when `refresh_due` is false. A 200 or 304 writes `checked_at_unix_ms = now`; retain the existing 304 behavior that advances `retrieved` because it means the remote catalog was confirmed current. A non-success HTTP response with an existing cache writes a clone with only `checked_at_unix_ms` advanced, then returns the existing redacted failure status. Transport failures before a response do not advance the timestamp.

Change startup ordering to:

```rust
let layers = ModelOverrideLayers::load_for_project(project_root);
let catalog_refresh = catalog_refresh::spawn_refresh_status(layers.fetched.clone());
```

The refresh still feeds a later invocation; the current invocation continues resolving from the already-loaded clone.

- [ ] **Step 6: Verify GREEN and checkpoint**

Run:

```text
just dev cargo test -p yach model_override_layers
just dev cargo test -p yach catalog_refresh
just dev cargo check --workspace
```

Then `jj describe -m "fix: honor project roots and throttle catalog refresh"` and `jj new`.

---

### Task 4: Lazy discovery, metadata join, and truthful picker

**Files:**
- Modify: `crates/yach-backend/src/runner.rs`
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-cli/src/headless.rs`
- Modify: `crates/yach-ui/src/app.rs`
- Modify: runner/config fixtures in `crates/yach-cli/src/main.rs`, `crates/yach-cli/src/headless.rs`, and `crates/yach-backend/src/runner.rs`

**Interfaces:**
- Consumes: Task 2's `discover_provider_models` and `DiscoveredProviderModel`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDiscoveryOutcome {
    Available(Vec<CatalogModelEntry>),
    Failed { message: String },
}

pub type ModelDiscoveryFuture = Pin<
    Box<dyn Future<Output = ModelDiscoveryOutcome> + Send + 'static>
>;

pub struct RunnerConfig {
    // existing fields...
    pub model_discovery: Option<ModelDiscoveryFuture>,
}
```

- Produces CLI helpers:

```rust
fn model_discovery_future(
    adapter: RigProviderAdapterConfig,
    provider_label: String,
    layers: ModelOverrideLayers,
) -> ModelDiscoveryFuture;

fn catalog_entries_from_discovery(
    provider_label: &str,
    discovered: Vec<DiscoveredProviderModel>,
    layers: &ModelOverrideLayers,
    baked: &yach_catalog::Catalog,
) -> Vec<CatalogModelEntry>;
```

- `ProviderConfig.catalog_models` becomes the completed session discovery snapshot; it starts empty and is never populated from the baked provider roster alone.

- [ ] **Step 1: Add failing metadata-join tests**

Construct fixture baked/fetched/override layers directly and use these local helpers:

```rust
fn discovered(id: &str, display_name: Option<&str>) -> DiscoveredProviderModel {
    DiscoveredProviderModel {
        id: id.to_owned(),
        display_name: display_name.map(str::to_owned),
    }
}

fn entry_ids(entries: &[CatalogModelEntry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.info.id.as_str()).collect()
}

#[test]
fn discovery_keeps_unknown_ids_but_filters_known_non_generation_entries() {
    let mut baked = yach_catalog::Catalog::empty("test");
    baked.insert(
        "openai",
        "known-chat",
        yach_catalog::CatalogEntry {
            context_window: Some(128_000),
            output_ceiling: Some(16_000),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    baked.insert(
        "openai",
        "known-embedding",
        yach_catalog::CatalogEntry {
            context_window: Some(0),
            output_ceiling: Some(0),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    let layers = model_layers_fixture();

    let entries = catalog_entries_from_discovery(
        "openai",
        vec![
            discovered("known-chat", None),
            discovered("known-embedding", None),
            discovered("brand-new", Some("Brand New")),
        ],
        &layers,
        &baked,
    );

    assert_eq!(entry_ids(&entries), vec!["brand-new", "known-chat"]);
}

#[test]
fn discovery_uses_catalog_name_then_provider_name_then_id() {
    let mut baked = yach_catalog::Catalog::empty("test");
    baked.insert(
        "anthropic",
        "known",
        yach_catalog::CatalogEntry {
            context_window: Some(128_000),
            output_ceiling: Some(16_000),
            display_name: Some(String::from("Catalog Name")),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    let layers = model_layers_fixture();
    let entries = catalog_entries_from_discovery(
        "anthropic",
        vec![
            discovered("known", Some("Provider Name")),
            discovered("provider-named", Some("Provider Name")),
            discovered("id-only", None),
        ],
        &layers,
        &baked,
    );

    let names: Vec<&str> = entries
        .iter()
        .map(|entry| entry.info.name.as_str())
        .collect();
    assert_eq!(names, vec!["id-only", "Provider Name", "Catalog Name"]);
}

#[test]
fn discovery_preserves_hyphenated_and_compact_dated_ids_returned_by_provider() {
    let baked = yach_catalog::Catalog::empty("test");
    let layers = model_layers_fixture();
    let entries = catalog_entries_from_discovery(
        "openai",
        vec![
            discovered("gpt-4o-2024-05-13", None),
            discovered("claude-x-20260101", None),
        ],
        &layers,
        &baked,
    );

    assert_eq!(
        entry_ids(&entries),
        vec!["claude-x-20260101", "gpt-4o-2024-05-13"]
    );
}
```

`model_layers_fixture() -> ModelOverrideLayers` is a test-only constructor beside the tests: set `user`, `project`, and `fetched` to `None` and `env` to `EnvOverrides::default()`. Production calls `catalog_entries_from_discovery(..., yach_catalog::baked_catalog())`; tests pass their local catalog through the same explicit `baked` parameter. Do not mutate the global baked `OnceLock`.

For generation classification, resolve once with `EnvOverrides::default()` and require non-`Default` sources for both context and output ceiling when any baked/fetched/user/project entry knows the model. Then perform the real resolution with `layers.env` for runtime numbers. This prevents a process-wide env budget from turning a known embedding/image entry into a chat model.

- [ ] **Step 2: Verify metadata-join RED**

Run `just dev cargo test -p yach discovery_keeps_unknown_ids_but_filters_known_non_generation_entries`.

Expected: compile failure because the discovery join does not exist.

- [ ] **Step 3: Replace baked picker assembly with the discovery future**

Delete `is_dated_snapshot_alias`, `undated_model_ids`, and `catalog_models_for_provider` plus their old tests. `model_discovery_future` awaits `discover_provider_models(&adapter.provider, adapter.timeout)` and maps success through `catalog_entries_from_discovery`. Map errors to stable, redacted status copy:

```text
model discovery unavailable for chatgpt-subscription; showing active model
model discovery failed (authentication); showing active model
model discovery failed (rate_limited); showing active model
model discovery failed (timeout); showing active model
model discovery failed (provider); showing active model
```

The future is boxed but not spawned by the CLI. Headless runs set `model_discovery: None`; interactive configured-provider runs pass `Some(model_discovery_future(...))`; fixture and unconfigured paths pass `None`.

- [ ] **Step 4: Add failing runner orchestration tests**

Use a ready boxed future, not network:

```rust
let discovery: ModelDiscoveryFuture = Box::pin(async {
    ModelDiscoveryOutcome::Available(vec![catalog_entry(
        "gpt-new",
        "GPT New",
        "openai",
    )])
});
```

Run the real native loop and verify:

1. Initialization emits only the active model and does not poll the discovery future.
2. `AvailableModelsRequested` starts the future once.
3. Completion emits `AvailableModelsUpdated` with active first and the discovered model without duplication.
4. A failed outcome emits the redacted status and leaves only active available.
5. A second `AvailableModelsRequested` reuses the completed snapshot and performs no second discovery.
6. An A -> B -> A selection sequence restores each model's own context window, output budget, and parameter spelling.

- [ ] **Step 5: Implement asynchronous runner ownership**

Change the event loop from `while let Some(event) = rx.recv().await` to a `tokio::select!` over client events and a private unbounded discovery-update channel. On the first `AvailableModelsRequested`, take `model_discovery`, spawn it, and forward exactly one outcome. While loading, send the active-only list. On `Available`, replace `provider.catalog_models`, emit a loaded status, and call `send_native_models`; on `Failed`, emit the message and call `send_native_models` without changing the empty snapshot.

Remove `ANTHROPIC_MODEL_CHOICES` and `native_models_from_curated_anthropic_list`. `send_native_models` becomes two cases only: completed non-empty discovery snapshot -> active-first list; otherwise -> active only. Keep `apply_native_model_selection` rehydration from `CatalogModelEntry` unchanged.

The private channel must not keep the runner alive after the client channel closes. Disable its `tokio::select!` arm after the one outcome so no busy loop is possible.

- [ ] **Step 6: Make every picker open request current availability**

Change `App::open_model_selector` to always send `AvailableModelsRequested`, even when `available_models` already contains the initial active model or a prior snapshot. Set `loading available models` only when the request send succeeds. Add a UI test that seeds a stale model list, opens `/model`, and observes a fresh request while retaining the existing list until the backend update arrives.

- [ ] **Step 7: Verify GREEN and checkpoint**

Run:

```text
just dev cargo test -p yach discovery_
just dev cargo test -p yach-backend model_discovery
just dev cargo test -p yach-backend model_selection
just dev cargo test -p yach-backend send_native_models
just dev cargo test -p yach-ui model_selector
just dev cargo check --workspace
```

Then `jj describe -m "feat: key-truthful model picker from provider discovery"` and `jj new`.

---

## Integration Verification

After all four tasks are green:

1. `just fmt-check`
2. `just lint`
3. `just test`
4. `just check`
5. Run a provider-configured TUI, confirm no model-discovery request/status before first render, open `/model`, and observe a provider-truthful list with the active model first.
6. Select a different known model, send a prompt, and verify the status/context budget uses that model's catalog profile; switch back to the original model and verify its values return.
7. Run once with an invalid provider credential: `/model` must degrade to active-only with a redacted status while prompt failure behavior remains unchanged.
8. Run the existing startup profile check to confirm provider discovery remains off the first-frame path.
