use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::{StreamExt, stream};
use tokio::task::spawn_blocking;
use yach_backend::{
    ActiveModelTarget, CatalogModelEntry, ConnectionListOutcome, ConnectionMutationFuture,
    ConnectionMutationOutcome, ConnectionReplacementFuture, ConnectionReplacementOutcome,
    ConnectionRuntimeFailure, ModelDiscoveryFuture, ModelDiscoveryOutcome,
    ProviderActivationFuture, ProviderActivationOutcome, ProviderConfig, ProviderConnectionRuntime,
    model_discovery::{ModelDiscoveryError, discover_provider_models},
    rig_adapter::{MaxTokensParam, RigProviderAdapterConfig, RigProviderConfig},
};
use yach_connections::{
    ConnectionAuth, ConnectionId, ConnectionMetadataStore, ConnectionState,
    CreateConnectionOutcome, CredentialError, CredentialSource, CredentialStore,
    JsonConnectionMetadataStore, NewConnectionDraft, ProviderConnection, ProviderConnectionStore,
    ProviderKind, ProviderSecret, SystemCredentialStore,
};

const MAX_CONNECTIONS: usize = 64;
const MAX_SNAPSHOT_ROWS: usize = 4_096;
const MAX_DISCOVERIES_IN_FLIGHT: usize = 8;

type DiscoveryFuture = Pin<
    Box<
        dyn Future<
                Output = Result<
                    Vec<yach_backend::model_discovery::DiscoveredProviderModel>,
                    ModelDiscoveryError,
                >,
            > + Send,
    >,
>;
type ModelDiscoverer = Arc<dyn Fn(Arc<RigProviderAdapterConfig>) -> DiscoveryFuture + Send + Sync>;
/// `~/.yach/connections.json`, using the CLI's shared HOME convention.
#[must_use]
pub(crate) fn registry_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".yach/connections.json"))
}

/// True when the system registry holds at least one stored connection, so a
/// missing legacy env config can be supplied through `/connect` instead.
#[must_use]
pub(crate) fn has_stored_connections() -> bool {
    registry_path().is_some_and(|path| {
        registry_has_stored_connections(&JsonConnectionMetadataStore::new(path))
    })
}

const ACTIVE_SELECTION_SCHEMA: &str = "yach.active-model.v1";

#[derive(serde::Serialize, serde::Deserialize)]
struct ActiveSelectionDocument {
    schema: String,
    connection_id: String,
    model_id: String,
}

fn active_selection_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".yach/active-model.json"))
}

/// Reads the remembered activation target. Missing, malformed, foreign-schema,
/// or invalid-identity documents all read as no memory — a stale file must
/// never block startup.
fn read_active_selection(path: &Path) -> Option<ActiveModelTarget> {
    let bytes = std::fs::read(path).ok()?;
    let document = serde_json::from_slice::<ActiveSelectionDocument>(&bytes).ok()?;
    if document.schema != ACTIVE_SELECTION_SCHEMA {
        return None;
    }
    let connection_id = if document.connection_id == "environment" {
        ConnectionId::environment()
    } else {
        ConnectionId::parse_stored(&document.connection_id).ok()?
    };
    Some(ActiveModelTarget {
        connection_id,
        model: document.model_id,
    })
}

/// Persists the remembered activation target atomically (temp file + rename).
fn write_active_selection(path: &Path, target: &ActiveModelTarget) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let document = ActiveSelectionDocument {
        schema: String::from(ACTIVE_SELECTION_SCHEMA),
        connection_id: target.connection_id.as_str().to_owned(),
        model_id: target.model.clone(),
    };
    let bytes = serde_json::to_vec(&document)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

fn registry_has_stored_connections(store: &JsonConnectionMetadataStore) -> bool {
    store
        .load()
        .is_ok_and(|connections| !connections.is_empty())
}

/// Read-only legacy environment configuration, represented as the one transient connection.
#[derive(Clone)]
pub(crate) struct EnvironmentConnection {
    connection: ProviderConnection,
    adapter: Arc<RigProviderAdapterConfig>,
}

impl EnvironmentConnection {
    #[must_use]
    pub(crate) fn new(adapter: Arc<RigProviderAdapterConfig>) -> Self {
        let (provider, authentication) = match &adapter.provider {
            RigProviderConfig::Anthropic { .. } => (
                ProviderKind::Anthropic,
                ConnectionAuth::ApiKey {
                    source: CredentialSource::Environment,
                },
            ),
            RigProviderConfig::OpenAi { .. } => (
                ProviderKind::OpenAi,
                ConnectionAuth::ApiKey {
                    source: CredentialSource::Environment,
                },
            ),
            RigProviderConfig::OpenAiCompatible { .. } => (
                ProviderKind::OpenAiCompatible,
                ConnectionAuth::ApiKey {
                    source: CredentialSource::Environment,
                },
            ),
            RigProviderConfig::ChatGptSubscription { token_dir } => (
                ProviderKind::ChatGptSubscription,
                ConnectionAuth::ChatGptSubscription {
                    token_dir: token_dir.clone(),
                },
            ),
        };
        let base_url = match &adapter.provider {
            RigProviderConfig::Anthropic { base_url, .. } => base_url.clone(),
            RigProviderConfig::OpenAiCompatible { base_url, .. } => Some(base_url.clone()),
            RigProviderConfig::OpenAi { .. } | RigProviderConfig::ChatGptSubscription { .. } => {
                None
            }
        };
        Self {
            connection: ProviderConnection {
                id: ConnectionId::environment(),
                provider,
                label: Some(String::from("Environment")),
                base_url,
                authentication,
                state: ConnectionState::Ready,
            },
            adapter,
        }
    }

    /// Builds a runtime-owned adapter allocation while retaining the same
    /// opaque credential allocation as the runner's selectable adapter.
    #[must_use]
    pub(crate) fn from_runtime_adapter(adapter: &Arc<RigProviderAdapterConfig>) -> Self {
        Self::new(Arc::new((**adapter).clone()))
    }
}

#[derive(Clone, Copy)]
struct AdapterDefaults {
    timeout: Duration,
    max_tokens: u64,
    context_window: u64,
    max_tokens_param: MaxTokensParam,
    test_delay_ms: Option<u64>,
}

impl Default for AdapterDefaults {
    fn default() -> Self {
        Self {
            timeout: Duration::from_mins(2),
            max_tokens: 32_000,
            context_window: 200_000,
            max_tokens_param: MaxTokensParam::MaxTokens,
            test_delay_ms: None,
        }
    }
}

impl AdapterDefaults {
    fn from_environment(environment: Option<&EnvironmentConnection>) -> Self {
        environment.map_or_else(Self::default, |environment| Self {
            timeout: environment.adapter.timeout,
            max_tokens: environment.adapter.max_tokens,
            context_window: environment.adapter.context_window,
            max_tokens_param: environment.adapter.max_tokens_param,
            test_delay_ms: None,
        })
    }
}

#[derive(Default)]
struct AvailabilityCache {
    generation: u64,
    refresh_generation: u64,
    snapshot: Option<Arc<[CatalogModelEntry]>>,
}

/// Process-lifetime credential reads. System credential stores (macOS
/// Keychain) may prompt on every access, so a resolved secret — including a
/// confirmed-missing read — is cached per connection and cleared by every
/// successful mutation through `invalidate`. Errors are never cached: a
/// transient store failure must be retried, not remembered.
#[derive(Default)]
struct CredentialCache {
    entries: std::collections::HashMap<ConnectionId, Option<ProviderSecret>>,
}

#[derive(Clone)]
struct RuntimeState {
    store: ProviderConnectionStore,
    credentials: Arc<dyn CredentialStore>,
    environment: Option<EnvironmentConnection>,
    layers: super::ModelOverrideLayers,
    defaults: AdapterDefaults,
    cache: Arc<Mutex<AvailabilityCache>>,
    credential_cache: Arc<Mutex<CredentialCache>>,
    discoverer: ModelDiscoverer,
    /// `Some` only for the system runtime: fixture/test runtimes must never
    /// persist a selection into a real home directory.
    selection_path: Option<PathBuf>,
}

/// Lazy provider-connection runtime used exclusively by the native backend.
///
/// Construction only captures injected stores, configuration, and paths. Registry, keyring, and
/// provider I/O begin at the corresponding runtime operation.
pub(crate) struct CliProviderConnectionRuntime {
    state: RuntimeState,
}

impl CliProviderConnectionRuntime {
    #[must_use]
    pub(crate) fn system(
        layers: super::ModelOverrideLayers,
        environment: Option<EnvironmentConnection>,
        timeout: Duration,
        test_delay_ms: Option<u64>,
    ) -> Option<Self> {
        let path = registry_path()?;
        let metadata: Arc<dyn ConnectionMetadataStore> =
            Arc::new(JsonConnectionMetadataStore::new(path));
        let credentials: Arc<dyn CredentialStore> = Arc::new(SystemCredentialStore::new());
        let mut defaults = AdapterDefaults::from_environment(environment.as_ref());
        defaults.timeout = timeout;
        defaults.test_delay_ms = test_delay_ms;
        let selection_path = active_selection_path();
        Some(Self::with_stores_and_discoverer_and_defaults(
            metadata,
            credentials,
            layers,
            environment,
            Arc::new(|adapter| {
                Box::pin(async move {
                    discover_provider_models(&adapter.provider, adapter.timeout).await
                })
            }),
            defaults,
            selection_path,
        ))
    }

    #[must_use]
    pub(crate) fn with_stores(
        metadata: Arc<dyn ConnectionMetadataStore>,
        credentials: Arc<dyn CredentialStore>,
        layers: super::ModelOverrideLayers,
        environment: Option<EnvironmentConnection>,
    ) -> Self {
        Self::with_stores_and_discoverer(
            metadata,
            credentials,
            layers,
            environment,
            Arc::new(|adapter| {
                Box::pin(async move {
                    discover_provider_models(&adapter.provider, adapter.timeout).await
                })
            }),
        )
    }

    fn with_stores_and_discoverer(
        metadata: Arc<dyn ConnectionMetadataStore>,
        credentials: Arc<dyn CredentialStore>,
        layers: super::ModelOverrideLayers,
        environment: Option<EnvironmentConnection>,
        discoverer: ModelDiscoverer,
    ) -> Self {
        let defaults = AdapterDefaults::from_environment(environment.as_ref());
        Self::with_stores_and_discoverer_and_defaults(
            metadata,
            credentials,
            layers,
            environment,
            discoverer,
            defaults,
            None,
        )
    }

    fn with_stores_and_discoverer_and_defaults(
        metadata: Arc<dyn ConnectionMetadataStore>,
        credentials: Arc<dyn CredentialStore>,
        layers: super::ModelOverrideLayers,
        environment: Option<EnvironmentConnection>,
        discoverer: ModelDiscoverer,
        defaults: AdapterDefaults,
        selection_path: Option<PathBuf>,
    ) -> Self {
        Self {
            state: RuntimeState {
                store: ProviderConnectionStore::new(metadata, credentials.clone()),
                credentials,
                environment,
                layers,
                defaults,
                cache: Arc::new(Mutex::new(AvailabilityCache::default())),
                credential_cache: Arc::new(Mutex::new(CredentialCache::default())),
                discoverer,
                selection_path,
            },
        }
    }

    fn invalidate(state: &RuntimeState) {
        {
            let mut cache = lock_cache(&state.cache);
            cache.generation = cache.generation.wrapping_add(1);
            cache.snapshot = None;
        }
        lock_credential_cache(&state.credential_cache)
            .entries
            .clear();
    }
}

impl ProviderConnectionRuntime for CliProviderConnectionRuntime {
    fn list(&self) -> yach_backend::ConnectionListFuture {
        let state = self.state.clone();
        Box::pin(async move {
            match spawn_blocking(move || list_connections(&state)).await {
                Ok(Ok(connections)) => ConnectionListOutcome::available(connections),
                Ok(Err(failure)) => ConnectionListOutcome::Failed(failure),
                Err(_) => ConnectionListOutcome::Failed(ConnectionRuntimeFailure::Unavailable),
            }
        })
    }

    fn cached_models(&self) -> Option<Arc<[CatalogModelEntry]>> {
        lock_cache(&self.state.cache).snapshot.clone()
    }

    fn refresh_models(&self, active: Option<ActiveModelTarget>) -> ModelDiscoveryFuture {
        let state = self.state.clone();
        let (generation, refresh_generation) = {
            let mut cache = lock_cache(&state.cache);
            cache.refresh_generation = cache.refresh_generation.wrapping_add(1);
            (cache.generation, cache.refresh_generation)
        };
        Box::pin(async move {
            let Ok(Ok(resolved)) = spawn_blocking({
                let state = state.clone();
                move || resolve_ready_connections(&state)
            })
            .await
            else {
                return ModelDiscoveryOutcome::Failed {
                    message: String::from("provider connection discovery is unavailable"),
                };
            };

            let discoverer = state.discoverer.clone();
            let layers = state.layers.clone();
            let active_for_discovery = active.clone();
            let discovered: Vec<ConnectionDiscovery> = stream::iter(resolved.connections)
                .map(move |connection| {
                    discover_connection_models(
                        connection,
                        active_for_discovery.clone(),
                        layers.clone(),
                        discoverer.clone(),
                    )
                })
                .buffer_unordered(MAX_DISCOVERIES_IN_FLIGHT)
                .collect()
                .await;
            let mut warnings = resolved.warnings;
            let mut entries = Vec::new();
            for discovery in discovered {
                entries.extend(discovery.entries);
                if let Some(failure) = discovery.failure {
                    warnings.push(failure.status_message().to_owned());
                }
            }
            entries.sort_by(|left, right| entry_order(left, right, active.as_ref()));
            let truncated = entries.len() > MAX_SNAPSHOT_ROWS;
            entries.truncate(MAX_SNAPSHOT_ROWS);
            if truncated {
                warnings.push(String::from("provider model list truncated"));
            }
            let snapshot: Arc<[CatalogModelEntry]> = entries.into();

            if !publish_snapshot(&state, generation, refresh_generation, snapshot.clone()) {
                return ModelDiscoveryOutcome::Superseded;
            }
            let entries = snapshot.as_ref().to_vec();
            if warnings.is_empty() {
                ModelDiscoveryOutcome::Available(entries)
            } else {
                warnings.truncate(MAX_CONNECTIONS + 2);
                ModelDiscoveryOutcome::AvailableWithWarnings { entries, warnings }
            }
        })
    }

    fn create(
        &self,
        draft: NewConnectionDraft,
        secret: ProviderSecret,
    ) -> ConnectionMutationFuture {
        let state = self.state.clone();
        Box::pin(async move {
            let count = match spawn_blocking({
                let state = state.clone();
                move || state.store.list().map(|connections| connections.len())
            })
            .await
            {
                Ok(Ok(count)) => count,
                Ok(Err(error)) => return ConnectionMutationOutcome::Failed(store_failure(error)),
                Err(_) => {
                    return ConnectionMutationOutcome::Failed(
                        ConnectionRuntimeFailure::Unavailable,
                    );
                }
            };
            if count >= MAX_CONNECTIONS {
                return ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Conflict);
            }
            let adapter = Arc::new(adapter_for_draft(&state, &draft, secret));
            if let Err(error) = validate_adapter(&state, adapter.clone()).await {
                return ConnectionMutationOutcome::Failed(discovery_failure(error));
            }
            let store = state.store.clone();
            match spawn_blocking(move || store.create_validated(draft, secret_ref(&adapter))).await
            {
                Ok(CreateConnectionOutcome::Created(_)) => {
                    Self::invalidate(&state);
                    ConnectionMutationOutcome::Succeeded
                }
                Ok(CreateConnectionOutcome::FailedBeforePending(error)) => {
                    ConnectionMutationOutcome::Failed(store_failure(error))
                }
                Ok(CreateConnectionOutcome::FailedAfterPending { id, error }) => {
                    ConnectionMutationOutcome::FailedAfterCreatePending {
                        id,
                        failure: store_failure(error),
                    }
                }
                Err(_) => ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Unavailable),
            }
        })
    }

    fn repair(&self, id: ConnectionId, secret: ProviderSecret) -> ConnectionMutationFuture {
        let state = self.state.clone();
        Box::pin(async move {
            let connection = match load_connection(&state, &id).await {
                Ok(connection) => connection,
                Err(failure) => return ConnectionMutationOutcome::Failed(failure),
            };
            let adapter = Arc::new(adapter_for_connection(&state, &connection, secret));
            if let Err(error) = validate_adapter(&state, adapter.clone()).await {
                return ConnectionMutationOutcome::Failed(discovery_failure(error));
            }
            let store = state.store.clone();
            let repair = matches!(connection.state, ConnectionState::PendingCredential);
            match spawn_blocking(move || {
                if repair {
                    store.repair_validated(&id, secret_ref(&adapter))
                } else {
                    store.repair_unavailable_ready_validated(&id, secret_ref(&adapter))
                }
            })
            .await
            {
                Ok(Ok(_)) => {
                    Self::invalidate(&state);
                    ConnectionMutationOutcome::Succeeded
                }
                Ok(Err(error)) => ConnectionMutationOutcome::Failed(store_failure(error)),
                Err(_) => ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Unavailable),
            }
        })
    }

    fn replace(
        &self,
        id: ConnectionId,
        model: Option<String>,
        secret: ProviderSecret,
    ) -> ConnectionReplacementFuture {
        let state = self.state.clone();
        Box::pin(async move {
            let connection = match load_connection(&state, &id).await {
                Ok(connection) => connection,
                Err(failure) => return ConnectionReplacementOutcome::Failed(failure),
            };
            let mut configured = adapter_for_connection(&state, &connection, secret);
            if let Some(model) = model.as_deref() {
                apply_profile(&state, &connection, model, &mut configured);
            }
            let adapter = Arc::new(configured);
            if let Err(error) = validate_adapter(&state, adapter.clone()).await {
                return ConnectionReplacementOutcome::Failed(discovery_failure(error));
            }
            let store = state.store.clone();
            let storage_adapter = adapter.clone();
            let result =
                spawn_blocking(move || store.replace_validated(&id, secret_ref(&storage_adapter)))
                    .await;
            match result {
                Ok(Ok(())) => {
                    Self::invalidate(&state);
                    let candidate = model.map(|model| ProviderConfig {
                        adapter,
                        model,
                        connection_id: Some(connection.id.clone()),
                        connection_display: connection.label.clone(),
                        test_delay_ms: state.defaults.test_delay_ms,
                        catalog_models: state.cached_snapshot(),
                    });
                    ConnectionReplacementOutcome::Succeeded { candidate }
                }
                Ok(Err(error)) => ConnectionReplacementOutcome::Failed(store_failure(error)),
                Err(_) => {
                    ConnectionReplacementOutcome::Failed(ConnectionRuntimeFailure::Unavailable)
                }
            }
        })
    }

    fn rename(&self, id: ConnectionId, label: Option<String>) -> ConnectionMutationFuture {
        let state = self.state.clone();
        Box::pin(async move {
            let store = state.store.clone();
            let persisted_id = id.clone();
            let persisted_label = label.clone();
            let outcome =
                spawn_blocking(move || store.rename(&persisted_id, persisted_label)).await;
            match outcome {
                Ok(Ok(_)) => {
                    Self::invalidate(&state);
                    ConnectionMutationOutcome::Renamed { id, display: label }
                }
                Ok(Err(error)) => ConnectionMutationOutcome::Failed(store_failure(error)),
                Err(_) => ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Unavailable),
            }
        })
    }

    fn remove(&self, id: ConnectionId) -> ConnectionMutationFuture {
        mutation_future(self.state.clone(), move |state| state.store.remove(&id))
    }

    fn remembered_selection(&self) -> Option<ActiveModelTarget> {
        self.state
            .selection_path
            .as_deref()
            .and_then(read_active_selection)
    }

    fn remember_selection(&self, target: ActiveModelTarget) {
        if let Some(path) = self.state.selection_path.as_deref() {
            let _ = write_active_selection(path, &target);
        }
    }

    fn activate(&self, id: ConnectionId, model: String) -> ProviderActivationFuture {
        let state = self.state.clone();
        Box::pin(async move {
            if let Some(environment) = state
                .environment
                .as_ref()
                .filter(|environment| environment.connection.id == id)
            {
                let mut adapter = (*environment.adapter).clone();
                apply_profile(&state, &environment.connection, &model, &mut adapter);
                return ProviderActivationOutcome::Activated(ProviderConfig {
                    adapter: Arc::new(adapter),
                    model,
                    connection_id: Some(environment.connection.id.clone()),
                    connection_display: environment.connection.label.clone(),
                    test_delay_ms: state.defaults.test_delay_ms,
                    catalog_models: state.cached_snapshot(),
                });
            }
            let connection = match load_connection(&state, &id).await {
                Ok(connection) => connection,
                Err(failure) => return ProviderActivationOutcome::Failed(failure),
            };
            let Ok(Ok(Some(secret))) = spawn_blocking({
                let state = state.clone();
                let id = id.clone();
                move || cached_credential(&state, &id)
            })
            .await
            else {
                return ProviderActivationOutcome::Failed(ConnectionRuntimeFailure::Unavailable);
            };
            let mut adapter = adapter_for_connection(&state, &connection, secret);
            apply_profile(&state, &connection, &model, &mut adapter);
            ProviderActivationOutcome::Activated(ProviderConfig {
                adapter: Arc::new(adapter),
                model,
                connection_id: Some(connection.id.clone()),
                connection_display: connection.label.clone(),
                test_delay_ms: state.defaults.test_delay_ms,
                catalog_models: state.cached_snapshot(),
            })
        })
    }
}

impl RuntimeState {
    fn cached_snapshot(&self) -> Arc<[CatalogModelEntry]> {
        lock_cache(&self.cache)
            .snapshot
            .clone()
            .unwrap_or_else(|| Arc::from([]))
    }
}

fn lock_cache(cache: &Mutex<AvailabilityCache>) -> MutexGuard<'_, AvailabilityCache> {
    match cache.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn lock_credential_cache(cache: &Mutex<CredentialCache>) -> MutexGuard<'_, CredentialCache> {
    match cache.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Read a credential through the process-lifetime cache: one store access per
/// connection until a mutation invalidates. Only successful reads (present or
/// confirmed-missing) are cached; errors propagate without caching.
fn cached_credential(
    state: &RuntimeState,
    id: &ConnectionId,
) -> Result<Option<ProviderSecret>, yach_connections::CredentialError> {
    if let Some(cached) = lock_credential_cache(&state.credential_cache)
        .entries
        .get(id)
    {
        return Ok(cached.clone());
    }
    let resolved = state.credentials.get(id)?;
    lock_credential_cache(&state.credential_cache)
        .entries
        .insert(id.clone(), resolved.clone());
    Ok(resolved)
}

fn publish_snapshot(
    state: &RuntimeState,
    generation: u64,
    refresh_generation: u64,
    snapshot: Arc<[CatalogModelEntry]>,
) -> bool {
    let mut cache = lock_cache(&state.cache);
    if cache.generation != generation || cache.refresh_generation != refresh_generation {
        return false;
    }
    cache.snapshot = Some(snapshot);
    true
}

fn mutation_future(
    state: RuntimeState,
    operation: impl FnOnce(&RuntimeState) -> Result<(), yach_connections::ConnectionStoreError>
    + Send
    + 'static,
) -> ConnectionMutationFuture {
    Box::pin(async move {
        let operation_state = state.clone();
        match spawn_blocking(move || operation(&operation_state)).await {
            Ok(Ok(())) => {
                CliProviderConnectionRuntime::invalidate(&state);
                ConnectionMutationOutcome::Succeeded
            }
            Ok(Err(error)) => ConnectionMutationOutcome::Failed(store_failure(error)),
            Err(_) => ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Unavailable),
        }
    })
}

fn list_connections(
    state: &RuntimeState,
) -> Result<Vec<ProviderConnection>, ConnectionRuntimeFailure> {
    let mut stored = match state.store.list() {
        Ok(stored) => stored,
        Err(_error) if state.environment.is_some() => Vec::new(),
        Err(error) => return Err(store_failure(error)),
    };
    let all_for_labels = stored.clone();
    for connection in &mut stored {
        if connection.state == ConnectionState::Ready {
            match cached_credential(state, &connection.id) {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => connection.state = ConnectionState::PendingCredential,
            }
        }
    }
    stored.sort_by(|left, right| connection_order(left, right, &all_for_labels));
    let mut listed = state
        .environment
        .as_ref()
        .map(|environment| vec![environment.connection.clone()])
        .unwrap_or_default();
    listed.extend(stored);
    Ok(listed)
}

fn resolve_ready_connections(
    state: &RuntimeState,
) -> Result<ResolvedConnectionList, ConnectionRuntimeFailure> {
    let (stored, mut warnings) = match state.store.list() {
        Ok(stored) => (stored, Vec::new()),
        Err(_error) if state.environment.is_some() => (
            Vec::new(),
            vec![String::from("provider connection registry is unavailable")],
        ),
        Err(error) => return Err(store_failure(error)),
    };
    let mut all_connections = stored.clone();
    if let Some(environment) = &state.environment {
        all_connections.push(environment.connection.clone());
    }
    let mut connections = Vec::new();
    if let Some(environment) = &state.environment {
        connections.push(ResolvedConnection {
            connection: environment.connection.clone(),
            display: String::from("Environment"),
            adapter: environment.adapter.clone(),
        });
    }
    for connection in stored {
        if connection.state != ConnectionState::Ready {
            continue;
        }
        let Ok(Some(secret)) = cached_credential(state, &connection.id) else {
            warnings.push(String::from(
                "provider connection credential is unavailable",
            ));
            continue;
        };
        connections.push(ResolvedConnection {
            display: connection.display_label(&all_connections),
            adapter: Arc::new(adapter_for_connection(state, &connection, secret)),
            connection,
        });
    }
    connections.sort_by(|left, right| {
        connection_sort_key(&left.connection, &left.display)
            .cmp(&connection_sort_key(&right.connection, &right.display))
    });
    connections.truncate(MAX_CONNECTIONS + usize::from(state.environment.is_some()));
    Ok(ResolvedConnectionList {
        connections,
        warnings,
    })
}

struct ResolvedConnectionList {
    connections: Vec<ResolvedConnection>,
    warnings: Vec<String>,
}

struct ResolvedConnection {
    connection: ProviderConnection,
    display: String,
    adapter: Arc<RigProviderAdapterConfig>,
}

struct ConnectionDiscovery {
    entries: Vec<CatalogModelEntry>,
    failure: Option<ConnectionRuntimeFailure>,
}

async fn discover_connection_models(
    connection: ResolvedConnection,
    active: Option<ActiveModelTarget>,
    layers: super::ModelOverrideLayers,
    discoverer: ModelDiscoverer,
) -> ConnectionDiscovery {
    let active_model = active
        .as_ref()
        .filter(|active| active.connection_id == connection.connection.id);
    if matches!(
        connection.connection.provider,
        ProviderKind::ChatGptSubscription
    ) {
        return ConnectionDiscovery {
            entries: active_model.map_or_else(Vec::new, |active| {
                vec![catalog_entry_for_model(
                    &layers,
                    &connection.connection,
                    &connection.display,
                    &active.model,
                )]
            }),
            failure: None,
        };
    }
    let discovered = match discoverer(connection.adapter.clone()).await {
        Ok(discovered) => discovered,
        Err(error) => {
            return ConnectionDiscovery {
                entries: active_model.map_or_else(Vec::new, |active| {
                    vec![catalog_entry_for_model(
                        &layers,
                        &connection.connection,
                        &connection.display,
                        &active.model,
                    )]
                }),
                failure: Some(discovery_failure(error)),
            };
        }
    };
    let provider = provider_label(connection.connection.provider);
    let mut entries = super::catalog_entries_from_discovery(
        provider,
        discovered,
        &layers,
        yach_catalog::baked_catalog(),
    );
    for entry in &mut entries {
        entry.info.connection_id = Some(connection.connection.id.as_str().to_owned());
        entry.info.connection_display = Some(connection.display.clone());
    }
    if let Some(active) = active_model
        && !entries.iter().any(|entry| entry.info.id == active.model)
    {
        entries.push(catalog_entry_for_model(
            &layers,
            &connection.connection,
            &connection.display,
            &active.model,
        ));
    }
    entries.sort_by(|left, right| left.info.id.cmp(&right.info.id));
    ConnectionDiscovery {
        entries,
        failure: None,
    }
}

fn catalog_entry_for_model(
    layers: &super::ModelOverrideLayers,
    connection: &ProviderConnection,
    display: &str,
    model: &str,
) -> CatalogModelEntry {
    let profile = layers.resolve(provider_label(connection.provider), model);
    let output_budget = yach_catalog::effective_output_budget(&profile, layers.env.max_tokens);
    CatalogModelEntry {
        info: yach_proto::ModelInfo {
            id: model.to_owned(),
            name: profile.display_name.value,
            provider: String::from(provider_label(connection.provider)),
            connection_id: Some(connection.id.as_str().to_owned()),
            connection_display: Some(String::from(display)),
        },
        context_window: profile.context_window.value,
        output_budget: output_budget.value,
        max_tokens_param: super::max_tokens_param_from_catalog(profile.output_tokens_param.value),
    }
}

fn entry_order(
    left: &CatalogModelEntry,
    right: &CatalogModelEntry,
    active: Option<&ActiveModelTarget>,
) -> std::cmp::Ordering {
    let left_active = active.is_some_and(|active| {
        left.info.connection_id.as_deref() == Some(active.connection_id.as_str())
            && left.info.id == active.model
    });
    let right_active = active.is_some_and(|active| {
        right.info.connection_id.as_deref() == Some(active.connection_id.as_str())
            && right.info.id == active.model
    });
    let left_active_connection = active.is_some_and(|active| {
        left.info.connection_id.as_deref() == Some(active.connection_id.as_str())
    });
    let right_active_connection = active.is_some_and(|active| {
        right.info.connection_id.as_deref() == Some(active.connection_id.as_str())
    });
    right_active
        .cmp(&left_active)
        .then_with(|| right_active_connection.cmp(&left_active_connection))
        .then_with(|| left.info.provider.cmp(&right.info.provider))
        .then_with(|| {
            left.info
                .connection_display
                .cmp(&right.info.connection_display)
        })
        .then_with(|| left.info.connection_id.cmp(&right.info.connection_id))
        .then_with(|| left.info.id.cmp(&right.info.id))
}

fn connection_order(
    left: &ProviderConnection,
    right: &ProviderConnection,
    all: &[ProviderConnection],
) -> std::cmp::Ordering {
    connection_sort_key(left, &left.display_label(all))
        .cmp(&connection_sort_key(right, &right.display_label(all)))
}

fn connection_sort_key(
    connection: &ProviderConnection,
    display: &str,
) -> (&'static str, String, String) {
    (
        provider_label(connection.provider),
        String::from(display),
        connection.id.as_str().to_owned(),
    )
}

fn load_connection(
    state: &RuntimeState,
    id: &ConnectionId,
) -> impl Future<Output = Result<ProviderConnection, ConnectionRuntimeFailure>> + Send + 'static {
    let state = state.clone();
    let id = id.clone();
    async move {
        match spawn_blocking(move || {
            state
                .store
                .list()
                .map_err(store_failure)?
                .into_iter()
                .find(|connection| connection.id == id)
                .ok_or(ConnectionRuntimeFailure::NotFound)
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ConnectionRuntimeFailure::Unavailable),
        }
    }
}

fn adapter_for_draft(
    state: &RuntimeState,
    draft: &NewConnectionDraft,
    secret: ProviderSecret,
) -> RigProviderAdapterConfig {
    adapter_for_parts(state, draft.provider(), draft.base_url(), secret)
}

fn adapter_for_connection(
    state: &RuntimeState,
    connection: &ProviderConnection,
    secret: ProviderSecret,
) -> RigProviderAdapterConfig {
    adapter_for_parts(
        state,
        connection.provider,
        connection.base_url.as_deref(),
        secret,
    )
}

fn adapter_for_parts(
    state: &RuntimeState,
    provider: ProviderKind,
    base_url: Option<&str>,
    secret: ProviderSecret,
) -> RigProviderAdapterConfig {
    let provider = match provider {
        ProviderKind::Anthropic => RigProviderConfig::Anthropic {
            api_key: secret,
            base_url: base_url.map(String::from),
        },
        ProviderKind::OpenAi => RigProviderConfig::OpenAi { api_key: secret },
        ProviderKind::OpenAiCompatible => RigProviderConfig::OpenAiCompatible {
            base_url: base_url.unwrap_or_default().to_owned(),
            api_key: secret,
        },
        ProviderKind::ChatGptSubscription => unreachable!("subscription cannot use an API secret"),
    };
    RigProviderAdapterConfig {
        provider,
        timeout: state.defaults.timeout,

        max_tokens: state.defaults.max_tokens,
        context_window: state.defaults.context_window,
        max_tokens_param: state.defaults.max_tokens_param,
    }
}
fn discovery_failure(error: ModelDiscoveryError) -> ConnectionRuntimeFailure {
    match error {
        ModelDiscoveryError::Provider(provider)
            if matches!(
                provider.kind,
                yach_backend::ProviderErrorKind::Authentication
            ) =>
        {
            ConnectionRuntimeFailure::Authentication
        }
        ModelDiscoveryError::Provider(provider)
            if matches!(
                provider.kind,
                yach_backend::ProviderErrorKind::Network | yach_backend::ProviderErrorKind::Timeout
            ) =>
        {
            ConnectionRuntimeFailure::Network
        }
        ModelDiscoveryError::Unsupported { .. } | ModelDiscoveryError::Provider(_) => {
            ConnectionRuntimeFailure::Validation
        }
    }
}

fn apply_profile(
    state: &RuntimeState,
    connection: &ProviderConnection,
    model: &str,
    adapter: &mut RigProviderAdapterConfig,
) {
    let profile = state
        .layers
        .resolve(provider_label(connection.provider), model);
    let output_budget =
        yach_catalog::effective_output_budget(&profile, state.layers.env.max_tokens);
    adapter.max_tokens = output_budget.value;
    adapter.context_window = profile.context_window.value;
    adapter.max_tokens_param =
        super::max_tokens_param_from_catalog(profile.output_tokens_param.value);
}
async fn validate_adapter(
    state: &RuntimeState,
    adapter: Arc<RigProviderAdapterConfig>,
) -> Result<(), ModelDiscoveryError> {
    (state.discoverer)(adapter).await.map(|_| ())
}

fn secret_ref(adapter: &RigProviderAdapterConfig) -> &ProviderSecret {
    match &adapter.provider {
        RigProviderConfig::Anthropic { api_key, .. }
        | RigProviderConfig::OpenAi { api_key }
        | RigProviderConfig::OpenAiCompatible { api_key, .. } => api_key,
        RigProviderConfig::ChatGptSubscription { .. } => {
            unreachable!("subscription has no API secret")
        }
    }
}

fn provider_label(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAi => "openai",
        ProviderKind::OpenAiCompatible => "openai-compatible",
        ProviderKind::ChatGptSubscription => "chatgpt-subscription",
    }
}

fn store_failure(error: yach_connections::ConnectionStoreError) -> ConnectionRuntimeFailure {
    match error {
        yach_connections::ConnectionStoreError::Validation(_)
        | yach_connections::ConnectionStoreError::Credential(CredentialError::Invalid) => {
            ConnectionRuntimeFailure::Validation
        }
        yach_connections::ConnectionStoreError::Metadata(_)
        | yach_connections::ConnectionStoreError::Credential(
            CredentialError::Missing | CredentialError::AccessDenied | CredentialError::Unavailable,
        ) => ConnectionRuntimeFailure::Unavailable,
        yach_connections::ConnectionStoreError::NotFound => ConnectionRuntimeFailure::NotFound,
        yach_connections::ConnectionStoreError::NotPending
        | yach_connections::ConnectionStoreError::NotReady
        | yach_connections::ConnectionStoreError::AlreadyExists => {
            ConnectionRuntimeFailure::Conflict
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use yach_backend::{
        ProviderMessage, ProviderModel, ProviderRequest, Role, TurnId,
        rig_adapter::run_provider_request,
    };

    use super::*;

    trait TestUnwrap {
        type Output;

        fn test_unwrap(self) -> Self::Output;
    }

    impl<T, E> TestUnwrap for Result<T, E> {
        type Output = T;

        fn test_unwrap(self) -> Self::Output {
            assert!(self.is_ok());
            match self {
                Ok(value) => value,
                Err(_) => unreachable!(),
            }
        }
    }

    impl<T> TestUnwrap for Option<T> {
        type Output = T;

        fn test_unwrap(self) -> Self::Output {
            assert!(self.is_some());
            match self {
                Some(value) => value,
                None => unreachable!(),
            }
        }
    }

    #[test]
    fn constructor_is_inert_and_cached_models_are_io_free() {
        let metadata = Arc::new(CountingMetadata::default());
        let credentials = Arc::new(CountingCredentials::default());
        let runtime = CliProviderConnectionRuntime::with_stores(
            metadata.clone(),
            credentials.clone(),
            super::super::model_layers_fixture(),
            None,
        );

        assert!(runtime.cached_models().is_none());

        assert_eq!(metadata.loads.load(Ordering::SeqCst), 0);
        assert_eq!(credentials.gets.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn runtime_environment_uses_a_distinct_adapter_arc_from_the_runner() {
        let runner_adapter = Arc::new(RigProviderAdapterConfig {
            provider: RigProviderConfig::OpenAi {
                api_key: ProviderSecret::new(String::from("environment-test-secret")),
            },
            timeout: Duration::from_secs(1),
            max_tokens: 1,
            context_window: 1,
            max_tokens_param: MaxTokensParam::MaxTokens,
        });

        let environment = EnvironmentConnection::from_runtime_adapter(&runner_adapter);

        assert!(!Arc::ptr_eq(&runner_adapter, &environment.adapter));
        assert_eq!(environment.adapter.timeout, runner_adapter.timeout);
    }

    #[test]
    fn list_places_environment_first_and_marks_missing_credentials_repairable() {
        let alpha = ProviderConnection::stored(
            ConnectionId::new_stored(),
            ProviderKind::OpenAi,
            Some(String::from("Alpha")),
            None,
            ConnectionState::Ready,
        )
        .test_unwrap();
        let beta = ProviderConnection::stored(
            ConnectionId::new_stored(),
            ProviderKind::OpenAi,
            Some(String::from("Beta")),
            None,
            ConnectionState::Ready,
        )
        .test_unwrap();
        let metadata = Arc::new(FixedMetadata {
            records: vec![beta.clone(), alpha.clone()],
        });
        let credentials = Arc::new(CountingCredentials::default());
        let environment = EnvironmentConnection::new(Arc::new(RigProviderAdapterConfig {
            provider: RigProviderConfig::OpenAi {
                api_key: ProviderSecret::new(String::from("environment-test-secret")),
            },
            timeout: Duration::from_secs(1),
            max_tokens: 1,
            context_window: 1,
            max_tokens_param: MaxTokensParam::MaxTokens,
        }));

        let runtime = CliProviderConnectionRuntime::with_stores(
            metadata,
            credentials.clone(),
            super::super::model_layers_fixture(),
            Some(environment),
        );

        let outcome = tokio::runtime::Runtime::new()
            .test_unwrap()
            .block_on(runtime.list());
        let ConnectionListOutcome::Available(list) = outcome else {
            unreachable!("fixture registry should list");
        };
        let connections = list.as_slice();
        assert_eq!(connections[0].id, ConnectionId::environment());
        assert_eq!(connections[1].id, alpha.id);
        assert_eq!(connections[2].id, beta.id);
        assert!(
            connections[1..]
                .iter()
                .all(|connection| connection.state == ConnectionState::PendingCredential)
        );
        assert_eq!(credentials.gets.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn malformed_registry_keeps_environment_list_and_discovery_available() {
        let environment = EnvironmentConnection::new(Arc::new(RigProviderAdapterConfig {
            provider: RigProviderConfig::OpenAi {
                api_key: ProviderSecret::new(String::from("environment-test-secret")),
            },
            timeout: Duration::from_secs(1),
            max_tokens: 1,
            context_window: 1,
            max_tokens_param: MaxTokensParam::MaxTokens,
        }));
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            Arc::new(MalformedMetadata),
            Arc::new(ReadyCredentials),
            super::super::model_layers_fixture(),
            Some(environment),
            Arc::new(|_| {
                Box::pin(async {
                    Ok(vec![
                        yach_backend::model_discovery::DiscoveredProviderModel {
                            id: String::from("environment-model"),
                            display_name: None,
                        },
                    ])
                })
            }),
        );
        let test_runtime = tokio::runtime::Runtime::new().test_unwrap();

        let ConnectionListOutcome::Available(list) = test_runtime.block_on(runtime.list()) else {
            unreachable!("environment remains listable despite malformed registry");
        };
        assert_eq!(list.as_slice().len(), 1);
        assert_eq!(list.as_slice()[0].id, ConnectionId::environment());
        let ModelDiscoveryOutcome::AvailableWithWarnings { entries, warnings } = test_runtime
            .block_on(runtime.refresh_models(Some(ActiveModelTarget {
                connection_id: ConnectionId::environment(),
                model: String::from("environment-model"),
            })))
        else {
            unreachable!("environment remains discoverable despite malformed registry");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(
            warnings,
            &[String::from("provider connection registry is unavailable")]
        );
    }

    struct FixedMetadata {
        records: Vec<ProviderConnection>,
    }

    struct MalformedMetadata;

    impl ConnectionMetadataStore for MalformedMetadata {
        fn load(&self) -> Result<Vec<ProviderConnection>, yach_connections::RegistryError> {
            Err(yach_connections::RegistryError::Malformed)
        }

        fn lock_connection(
            &self,
            _: &ConnectionId,
        ) -> Result<
            Box<dyn yach_connections::LockedConnectionMetadata>,
            yach_connections::RegistryError,
        > {
            unreachable!("malformed metadata does not support mutation")
        }
    }

    #[test]
    fn invalidation_clears_cached_snapshot_and_rejects_a_stale_generation_publication() {
        let runtime = CliProviderConnectionRuntime::with_stores(
            Arc::new(CountingMetadata::default()),
            Arc::new(CountingCredentials::default()),
            super::super::model_layers_fixture(),
            None,
        );
        let cached: Arc<[CatalogModelEntry]> = vec![fixture_entry("cached", "first")].into();
        {
            let mut cache = runtime.state.cache.lock().test_unwrap();
            cache.snapshot = Some(cached);
        }
        let stale_generation = runtime.state.cache.lock().test_unwrap().generation;
        CliProviderConnectionRuntime::invalidate(&runtime.state);
        let replacement: Arc<[CatalogModelEntry]> = vec![fixture_entry("fresh", "second")].into();

        assert!(!publish_snapshot(
            &runtime.state,
            stale_generation,
            0,
            replacement.clone(),
        ));
        assert!(
            runtime.cached_models().is_none(),
            "a successful mutation must not leave invalidated rows selectable"
        );
        let current_generation = runtime.state.cache.lock().test_unwrap().generation;
        assert!(publish_snapshot(
            &runtime.state,
            current_generation,
            0,
            replacement.clone(),
        ));
        assert!(Arc::ptr_eq(
            &replacement,
            &runtime.cached_models().test_unwrap()
        ));
    }

    #[test]
    fn newer_refresh_keeps_its_snapshot_when_an_older_refresh_finishes_last() {
        let connection = ready_compatible("Refresh", "http://refresh.invalid/v1");
        let (first_sender, first_receiver) = tokio::sync::oneshot::channel();
        let (second_sender, second_receiver) = tokio::sync::oneshot::channel();
        let receivers = Arc::new(Mutex::new(std::collections::VecDeque::from([
            first_receiver,
            second_receiver,
        ])));
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            Arc::new(FixedMetadata {
                records: vec![connection],
            }),
            Arc::new(ReadyCredentials),
            super::super::model_layers_fixture(),
            None,
            Arc::new(move |_| {
                started_sender.send(()).test_unwrap();
                let receiver = receivers.lock().test_unwrap().pop_front().test_unwrap();
                Box::pin(async move { receiver.await.test_unwrap() })
            }),
        );
        let test_runtime = tokio::runtime::Runtime::new().test_unwrap();
        let first = test_runtime.spawn(runtime.refresh_models(None));
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .test_unwrap();
        let second = test_runtime.spawn(runtime.refresh_models(None));
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .test_unwrap();

        second_sender
            .send(Ok(vec![
                yach_backend::model_discovery::DiscoveredProviderModel {
                    id: String::from("newer"),
                    display_name: None,
                },
            ]))
            .test_unwrap();
        assert!(matches!(
            test_runtime.block_on(second).test_unwrap(),
            ModelDiscoveryOutcome::Available(_)
        ));
        first_sender
            .send(Ok(vec![
                yach_backend::model_discovery::DiscoveredProviderModel {
                    id: String::from("older"),
                    display_name: None,
                },
            ]))
            .test_unwrap();
        assert!(matches!(
            test_runtime.block_on(first).test_unwrap(),
            ModelDiscoveryOutcome::Superseded
        ));
        assert_eq!(runtime.cached_models().test_unwrap()[0].info.id, "newer");
    }

    #[test]
    fn refresh_returns_superseded_after_runtime_generation_invalidation() {
        let connection = ready_compatible("Stale", "http://stale.invalid/v1");
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            Arc::new(FixedMetadata {
                records: vec![connection],
            }),
            Arc::new(ReadyCredentials),
            super::super::model_layers_fixture(),
            None,
            Arc::new(|_| {
                Box::pin(async {
                    Ok(vec![
                        yach_backend::model_discovery::DiscoveredProviderModel {
                            id: String::from("stale-model"),
                            display_name: None,
                        },
                    ])
                })
            }),
        );
        let refresh = runtime.refresh_models(None);
        CliProviderConnectionRuntime::invalidate(&runtime.state);

        assert!(matches!(
            tokio::runtime::Runtime::new()
                .test_unwrap()
                .block_on(refresh),
            ModelDiscoveryOutcome::Superseded
        ));
    }

    #[test]
    fn refresh_keeps_same_model_from_two_connections_as_exact_rows() {
        let first = ready_compatible("First", "http://one.invalid/v1");
        let second = ready_compatible("Second", "http://two.invalid/v1");
        let discoverer: ModelDiscoverer = Arc::new(|_| {
            Box::pin(async {
                Ok(vec![
                    yach_backend::model_discovery::DiscoveredProviderModel {
                        id: String::from("shared-model"),

                        display_name: Some(String::from("Shared model")),
                    },
                ])
            })
        });
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            Arc::new(FixedMetadata {
                records: vec![first.clone(), second.clone()],
            }),
            Arc::new(ReadyCredentials),
            super::super::model_layers_fixture(),
            None,
            discoverer,
        );

        let outcome = tokio::runtime::Runtime::new()
            .test_unwrap()
            .block_on(runtime.refresh_models(None));
        let ModelDiscoveryOutcome::Available(entries) = outcome else {
            unreachable!("fixture discovery should succeed");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].info.id, "shared-model");
        assert_eq!(entries[1].info.id, "shared-model");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.info.connection_id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some(first.id.as_str()), Some(second.id.as_str())]
        );
    }
    #[test]
    fn refresh_synthesizes_omitted_active_model() {
        let connection = ready_compatible("Active", "http://active.invalid/v1");
        let active = ActiveModelTarget {
            connection_id: connection.id.clone(),
            model: String::from("active-model"),
        };
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            Arc::new(FixedMetadata {
                records: vec![connection.clone()],
            }),
            Arc::new(ReadyCredentials),
            super::super::model_layers_fixture(),
            None,
            Arc::new(|_| {
                Box::pin(async {
                    Ok(vec![
                        yach_backend::model_discovery::DiscoveredProviderModel {
                            id: String::from("other-model"),
                            display_name: None,
                        },
                    ])
                })
            }),
        );

        let ModelDiscoveryOutcome::Available(entries) = tokio::runtime::Runtime::new()
            .test_unwrap()
            .block_on(runtime.refresh_models(Some(active)))
        else {
            unreachable!("active fallback remains available");
        };

        assert_eq!(entries[0].info.id, "active-model");
        assert_eq!(
            entries[0].info.connection_id.as_deref(),
            Some(connection.id.as_str())
        );
    }

    #[test]
    fn refresh_isolates_auth_failure_without_exposing_provider_body_or_key() {
        let successful = ready_compatible("Successful", "http://success.invalid/v1");
        let failing = ready_compatible("Failing", "http://failure.invalid/v1");
        let discoverer: ModelDiscoverer = Arc::new(|adapter| {
            let fails = matches!(
                &adapter.provider,
                RigProviderConfig::OpenAiCompatible { base_url, .. }
                    if base_url == "http://failure.invalid/v1"
            );
            Box::pin(async move {
                if fails {
                    Err(ModelDiscoveryError::Provider(yach_backend::ProviderError {
                        kind: yach_backend::ProviderErrorKind::Authentication,
                        message: String::from("response-body-sentinel"),
                        redacted_debug: Some(String::from("submitted-key-sentinel")),
                    }))
                } else {
                    Ok(vec![
                        yach_backend::model_discovery::DiscoveredProviderModel {
                            id: String::from("available-model"),
                            display_name: None,
                        },
                    ])
                }
            })
        });
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            Arc::new(FixedMetadata {
                records: vec![failing, successful.clone()],
            }),
            Arc::new(ReadyCredentials),
            super::super::model_layers_fixture(),
            None,
            discoverer,
        );

        let outcome = tokio::runtime::Runtime::new()
            .test_unwrap()
            .block_on(runtime.refresh_models(None));
        let ModelDiscoveryOutcome::AvailableWithWarnings { entries, warnings } = &outcome else {
            unreachable!(
                "successful connection must survive its peer failure with a bounded warning"
            );
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].info.connection_id.as_deref(),
            Some(successful.id.as_str())
        );
        assert_eq!(
            warnings,
            &[String::from("connection authentication failed")]
        );
        let rendered = format!("{outcome:?}");
        assert!(!rendered.contains("response-body-sentinel"));
        assert!(!rendered.contains("submitted-key-sentinel"));
        assert!(!rendered.contains("fixture-secret"));
    }

    #[test]
    fn refresh_represents_active_environment_chatgpt_without_persisted_subscription_auth() {
        let environment = EnvironmentConnection::new(Arc::new(RigProviderAdapterConfig {
            provider: RigProviderConfig::ChatGptSubscription {
                token_dir: PathBuf::from("/tmp/unused-chatgpt-token-dir"),
            },
            timeout: Duration::from_secs(1),
            max_tokens: 1,
            context_window: 1,
            max_tokens_param: MaxTokensParam::MaxTokens,
        }));
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            Arc::new(FixedMetadata {
                records: Vec::new(),
            }),
            Arc::new(ReadyCredentials),
            super::super::model_layers_fixture(),
            Some(environment),
            Arc::new(|_| unreachable!("subscription discovery must remain active-only")),
        );

        let outcome =
            tokio::runtime::Runtime::new()
                .test_unwrap()
                .block_on(runtime.refresh_models(Some(ActiveModelTarget {
                    connection_id: ConnectionId::environment(),
                    model: String::from("gpt-5"),
                })));
        let ModelDiscoveryOutcome::Available(entries) = outcome else {
            unreachable!("active environment subscription must be visible");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].info.connection_id.as_deref(),
            Some("environment")
        );
        assert_eq!(entries[0].info.id, "gpt-5");
    }

    #[test]
    fn provider_connection_switch_a_b_a_restores_complete_config() {
        let (a_url, a_requests) = local_provider_request_fixture(2);
        let (b_url, b_requests) = local_provider_request_fixture(1);
        let a = ready_compatible("A", &a_url);
        let b = ready_compatible("B", &b_url);
        let credentials = Arc::new(MutableCredentials::default());
        credentials
            .put(&a.id, &ProviderSecret::new(String::from("key-a-sentinel")))
            .test_unwrap();
        credentials
            .put(&b.id, &ProviderSecret::new(String::from("key-b-sentinel")))
            .test_unwrap();
        let layers = super::super::model_layers_fixture();
        let expected = layers.resolve("openai-compatible", "gpt-4.1");
        let expected_budget =
            yach_catalog::effective_output_budget(&expected, layers.env.max_tokens);
        let runtime = CliProviderConnectionRuntime::with_stores(
            Arc::new(FixedMetadata {
                records: vec![a.clone(), b.clone()],
            }),
            credentials,
            layers,
            None,
        );
        let test_runtime = tokio::runtime::Runtime::new().test_unwrap();
        let activate =
            |id: ConnectionId| test_runtime.block_on(runtime.activate(id, String::from("gpt-4.1")));

        let ProviderActivationOutcome::Activated(config_a) = activate(a.id.clone()) else {
            unreachable!("A activation succeeds");
        };
        let ProviderActivationOutcome::Activated(config_b) = activate(b.id.clone()) else {
            unreachable!("B activation succeeds");
        };
        let ProviderActivationOutcome::Activated(config_a_again) = activate(a.id.clone()) else {
            unreachable!("A reactivation succeeds");
        };

        for (config, connection, expected_key, expected_url, expected_display) in [
            (&config_a, &a, "key-a-sentinel", &a_url, "A"),
            (&config_b, &b, "key-b-sentinel", &b_url, "B"),
            (&config_a_again, &a, "key-a-sentinel", &a_url, "A"),
        ] {
            assert_eq!(config.connection_id.as_ref(), Some(&connection.id));
            assert_eq!(config.connection_display.as_deref(), Some(expected_display));
            assert_eq!(config.model, "gpt-4.1");
            assert_eq!(config.adapter.context_window, expected.context_window.value);
            assert_eq!(config.adapter.max_tokens, expected_budget.value);
            assert_eq!(
                config.adapter.max_tokens_param,
                super::super::max_tokens_param_from_catalog(expected.output_tokens_param.value)
            );
            let RigProviderConfig::OpenAiCompatible { api_key, base_url } =
                &config.adapter.provider
            else {
                unreachable!("fixture uses compatible adapter");
            };
            assert_eq!(base_url, expected_url);
            api_key.with_exposed(|key| assert_eq!(key, expected_key));
        }

        for config in [&config_a, &config_b, &config_a_again] {
            let _ = test_runtime.block_on(run_provider_request(
                &config.adapter,
                ProviderRequest {
                    turn_id: TurnId(String::from("connection-switch-turn")),
                    model: ProviderModel {
                        provider: String::from("openai-compatible"),
                        model: config.model.clone(),
                    },
                    messages: vec![ProviderMessage::text(
                        Role::User,
                        String::from("connection switch fixture"),
                    )],
                    extensions: Vec::new(),
                },
            ));
        }

        let expected_param = match config_a.adapter.max_tokens_param {
            MaxTokensParam::MaxTokens => "\"max_tokens\"",
            MaxTokensParam::MaxCompletionTokens => "\"max_completion_tokens\"",
        };
        for (request, expected_key) in [
            (a_requests.recv().test_unwrap(), "key-a-sentinel"),
            (b_requests.recv().test_unwrap(), "key-b-sentinel"),
            (a_requests.recv().test_unwrap(), "key-a-sentinel"),
        ] {
            assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
            assert!(request.contains(&format!("authorization: Bearer {expected_key}")));
            assert!(request.contains("\"model\":\"gpt-4.1\""));
            assert!(request.contains(expected_param));
        }
    }

    #[test]
    fn environment_activation_applies_selected_model_profile() {
        let adapter = Arc::new(RigProviderAdapterConfig {
            provider: RigProviderConfig::OpenAi {
                api_key: ProviderSecret::new(String::from("environment-test-secret")),
            },
            timeout: Duration::from_secs(7),
            max_tokens: 1,
            context_window: 1,
            max_tokens_param: MaxTokensParam::MaxTokens,
        });
        let layers = super::super::model_layers_fixture();
        let expected = layers.resolve("openai", "gpt-4.1");
        let expected_budget =
            yach_catalog::effective_output_budget(&expected, layers.env.max_tokens);
        let runtime = CliProviderConnectionRuntime::with_stores(
            Arc::new(CountingMetadata::default()),
            Arc::new(CountingCredentials::default()),
            layers,
            Some(EnvironmentConnection::new(adapter)),
        );

        let ProviderActivationOutcome::Activated(config) = tokio::runtime::Runtime::new()
            .test_unwrap()
            .block_on(runtime.activate(ConnectionId::environment(), String::from("gpt-4.1")))
        else {
            unreachable!("environment activation succeeds");
        };

        assert_eq!(config.adapter.context_window, expected.context_window.value);
        assert_eq!(config.adapter.max_tokens, expected_budget.value);
        assert_eq!(
            config.adapter.max_tokens_param,
            super::super::max_tokens_param_from_catalog(expected.output_tokens_param.value)
        );
    }
    #[test]
    fn refresh_bounds_64_stored_plus_environment_to_eight_in_flight_and_preserves_active() {
        let stored: Vec<ProviderConnection> = (0..64)
            .map(|index| {
                ready_compatible(
                    &format!("stored-{index:03}"),
                    &format!("http://gateway-{index}.invalid/v1"),
                )
            })
            .collect();
        let active_connection = stored.last().test_unwrap().id.clone();
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let discoverer: ModelDiscoverer = {
            let in_flight = in_flight.clone();
            let peak = peak.clone();
            Arc::new(move |_| {
                let in_flight = in_flight.clone();
                let peak = peak.clone();
                Box::pin(async move {
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok((0..100)
                        .map(
                            |index| yach_backend::model_discovery::DiscoveredProviderModel {
                                id: format!("fixture-model-{index:03}"),
                                display_name: None,
                            },
                        )
                        .collect())
                })
            })
        };
        let environment = EnvironmentConnection::new(Arc::new(RigProviderAdapterConfig {
            provider: RigProviderConfig::OpenAi {
                api_key: ProviderSecret::new(String::from("environment-fixture-secret")),
            },
            timeout: Duration::from_secs(1),
            max_tokens: 1,
            context_window: 1,
            max_tokens_param: MaxTokensParam::MaxTokens,
        }));
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            Arc::new(FixedMetadata { records: stored }),
            Arc::new(ReadyCredentials),
            super::super::model_layers_fixture(),
            Some(environment),
            discoverer,
        );

        let outcome =
            tokio::runtime::Runtime::new()
                .test_unwrap()
                .block_on(runtime.refresh_models(Some(ActiveModelTarget {
                    connection_id: active_connection.clone(),
                    model: String::from("fixture-model-099"),
                })));
        let ModelDiscoveryOutcome::AvailableWithWarnings { entries, warnings } = outcome else {
            unreachable!("bounded fixture discovery should complete with truncation warning");
        };

        assert_eq!(entries.len(), MAX_SNAPSHOT_ROWS);
        assert_eq!(warnings, &[String::from("provider model list truncated")]);
        assert!(peak.load(Ordering::SeqCst) <= MAX_DISCOVERIES_IN_FLIGHT);
        assert_eq!(
            entries[0].info.connection_id.as_deref(),
            Some(active_connection.as_str())
        );
        assert_eq!(entries[0].info.id, "fixture-model-099");
        assert!(
            entries[..100].iter().all(
                |entry| entry.info.connection_id.as_deref() == Some(active_connection.as_str())
            )
        );
    }

    #[test]
    fn injected_validation_failure_never_reaches_store() {
        let metadata = Arc::new(CountingMetadata::default());
        let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
            metadata.clone(),
            Arc::new(CountingCredentials::default()),
            super::super::model_layers_fixture(),
            None,
            Arc::new(|_| {
                Box::pin(async {
                    Err(ModelDiscoveryError::Provider(yach_backend::ProviderError {
                        kind: yach_backend::ProviderErrorKind::Authentication,
                        message: String::from("redacted"),
                        redacted_debug: None,
                    }))
                })
            }),
        );
        let outcome = tokio::runtime::Runtime::new().test_unwrap().block_on(
            runtime.create(
                NewConnectionDraft::new(
                    ProviderKind::OpenAiCompatible,
                    Some(String::from("fixture")),
                    Some(String::from("http://fixture.invalid/v1")),
                )
                .test_unwrap(),
                ProviderSecret::new(String::from("validation-key")),
            ),
        );
        assert!(matches!(
            outcome,
            ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Authentication)
        ));
        assert_eq!(metadata.loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn persisted_create_failure_retries_by_repairing_the_same_pending_connection() {
        let path = registry_fixture_path();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let credentials = Arc::new(MutableCredentials::default());
        credentials.fail_next_put();
        let runtime = mutable_runtime(metadata, credentials);
        let test_runtime = tokio::runtime::Runtime::new().test_unwrap();
        let draft = NewConnectionDraft::new(
            ProviderKind::OpenAi,
            Some(String::from("Retry fixture")),
            None,
        )
        .test_unwrap();

        let pending_id = match test_runtime
            .block_on(runtime.create(draft, ProviderSecret::new(String::from("first-secret"))))
        {
            ConnectionMutationOutcome::FailedAfterCreatePending { id, .. } => id,
            outcome => unreachable!("create must report its durable pending ID: {outcome:?}"),
        };

        let ConnectionListOutcome::Available(list) = test_runtime.block_on(runtime.list()) else {
            unreachable!("pending connection must remain listable");
        };
        assert_eq!(list.as_slice().len(), 1);
        assert_eq!(list.as_slice()[0].id, pending_id);
        assert_eq!(list.as_slice()[0].state, ConnectionState::PendingCredential);

        assert!(matches!(
            test_runtime.block_on(runtime.repair(
                pending_id.clone(),
                ProviderSecret::new(String::from("second-secret")),
            )),
            ConnectionMutationOutcome::Succeeded
        ));
        let ConnectionListOutcome::Available(list) = test_runtime.block_on(runtime.list()) else {
            unreachable!("repaired connection must remain listable");
        };
        assert_eq!(list.as_slice().len(), 1);
        assert_eq!(list.as_slice()[0].id, pending_id);
        assert_eq!(list.as_slice()[0].state, ConnectionState::Ready);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn local_compatible_models_validation_sends_bearer_and_redacts_server_body() {
        let (base_url, received_expected_request) =
            local_models_fixture("/models", "authorization: Bearer fixture-bearer-sentinel");
        let metadata = Arc::new(CountingMetadata::default());
        let runtime = CliProviderConnectionRuntime::with_stores(
            metadata.clone(),
            Arc::new(CountingCredentials::default()),
            super::super::model_layers_fixture(),
            None,
        );
        let outcome = tokio::runtime::Runtime::new().test_unwrap().block_on(
            runtime.create(
                NewConnectionDraft::new(
                    ProviderKind::OpenAiCompatible,
                    Some(String::from("fixture")),
                    Some(base_url),
                )
                .test_unwrap(),
                ProviderSecret::new(String::from("fixture-bearer-sentinel")),
            ),
        );
        assert!(
            matches!(
                outcome,
                ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Network)
            ),
            "unexpected validation outcome: {outcome:?}"
        );
        assert!(received_expected_request.recv().test_unwrap());
        assert!(!format!("{outcome:?}").contains("fixture-server-body-sentinel"));
        assert_eq!(metadata.loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn replacement_blocks_old_generation_and_refreshes_fresh_rows() {
        let path = registry_fixture_path();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let credentials = Arc::new(MutableCredentials::default());
        let connection = match ProviderConnectionStore::new(metadata.clone(), credentials.clone())
            .create_validated(
                NewConnectionDraft::new(
                    ProviderKind::OpenAiCompatible,
                    Some(String::from("Old")),
                    Some(String::from("http://fixture.invalid/v1")),
                )
                .test_unwrap(),
                &ProviderSecret::new(String::from("old-key")),
            ) {
            CreateConnectionOutcome::Created(connection) => connection,
            outcome => unreachable!("fixture connection must be created: {outcome:?}"),
        };
        let runtime = mutable_runtime(metadata, credentials);
        let generation = runtime.state.cache.lock().test_unwrap().generation;
        {
            let mut cache = runtime.state.cache.lock().test_unwrap();
            cache.snapshot = Some(vec![fixture_entry("old-key-row", "old")].into());
        }
        let result = tokio::runtime::Runtime::new()
            .test_unwrap()
            .block_on(runtime.replace(
                connection.id.clone(),
                Some(String::from("active-model")),
                ProviderSecret::new(String::from("fresh-key")),
            ));
        let ConnectionReplacementOutcome::Succeeded {
            candidate: Some(candidate),
        } = result
        else {
            unreachable!("fixture replacement should produce an active candidate");
        };
        assert!(
            candidate.catalog_models.is_empty(),
            "a replacement candidate must not retain the invalidated snapshot"
        );
        assert!(!publish_snapshot(
            &runtime.state,
            generation,
            0,
            vec![fixture_entry("old-key-row", "old")].into(),
        ));
        let ModelDiscoveryOutcome::Available(rows) = tokio::runtime::Runtime::new()
            .test_unwrap()
            .block_on(runtime.refresh_models(None))
        else {
            unreachable!("same runtime refreshes after replacement");
        };
        assert_eq!(
            rows[0].info.connection_id.as_deref(),
            Some(connection.id.as_str())
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rename_blocks_old_label_generation_and_refreshes_new_label() {
        let path = registry_fixture_path();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let credentials = Arc::new(MutableCredentials::default());
        let connection = match ProviderConnectionStore::new(metadata.clone(), credentials.clone())
            .create_validated(
                NewConnectionDraft::new(
                    ProviderKind::OpenAiCompatible,
                    Some(String::from("Old")),
                    Some(String::from("http://fixture.invalid/v1")),
                )
                .test_unwrap(),
                &ProviderSecret::new(String::from("key")),
            ) {
            CreateConnectionOutcome::Created(connection) => connection,
            outcome => unreachable!("fixture connection must be created: {outcome:?}"),
        };
        let runtime = mutable_runtime(metadata, credentials);
        let generation = runtime.state.cache.lock().test_unwrap().generation;
        assert!(matches!(
            tokio::runtime::Runtime::new()
                .test_unwrap()
                .block_on(runtime.rename(connection.id.clone(), Some(String::from("New")))),
            ConnectionMutationOutcome::Renamed {
                id,
                display: Some(display),
            } if id == connection.id && display == "New"
        ));
        assert!(!publish_snapshot(
            &runtime.state,
            generation,
            0,
            vec![fixture_entry("old-label-row", "Old")].into(),
        ));
        let ModelDiscoveryOutcome::Available(rows) = tokio::runtime::Runtime::new()
            .test_unwrap()
            .block_on(runtime.refresh_models(None))
        else {
            unreachable!("same runtime refreshes after rename");
        };
        assert_eq!(rows[0].info.connection_display.as_deref(), Some("New"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn create_repair_and_remove_invalidate_and_refresh_on_the_same_runtime() {
        let path = registry_fixture_path();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let credentials = Arc::new(MutableCredentials::default());
        let runtime = mutable_runtime(metadata.clone(), credentials.clone());
        let test_runtime = tokio::runtime::Runtime::new().test_unwrap();

        let create_generation = runtime.state.cache.lock().test_unwrap().generation;
        assert!(matches!(
            test_runtime.block_on(
                runtime.create(
                    NewConnectionDraft::new(
                        ProviderKind::OpenAiCompatible,
                        Some(String::from("Fixture")),
                        Some(String::from("http://fixture.invalid/v1")),
                    )
                    .test_unwrap(),
                    ProviderSecret::new(String::from("create-key")),
                )
            ),
            ConnectionMutationOutcome::Succeeded
        ));
        assert!(!publish_snapshot(
            &runtime.state,
            create_generation,
            0,
            vec![fixture_entry("stale-create", "Fixture")].into(),
        ));
        assert!(matches!(
            test_runtime.block_on(runtime.refresh_models(None)),
            ModelDiscoveryOutcome::Available(rows) if rows.len() == 1
        ));
        let repair_connection = ProviderConnection::stored(
            ConnectionId::new_stored(),
            ProviderKind::OpenAiCompatible,
            Some(String::from("Repair")),
            Some(String::from("http://repair.invalid/v1")),
            ConnectionState::PendingCredential,
        )
        .test_unwrap();
        metadata
            .lock_connection(&repair_connection.id)
            .test_unwrap()
            .create_pending(repair_connection.clone())
            .test_unwrap();

        let repair_generation = runtime.state.cache.lock().test_unwrap().generation;
        let repair = test_runtime.block_on(runtime.repair(
            repair_connection.id.clone(),
            ProviderSecret::new(String::from("repair-key")),
        ));
        assert!(
            matches!(repair, ConnectionMutationOutcome::Succeeded),
            "repair result: {repair:?}"
        );
        assert!(!publish_snapshot(
            &runtime.state,
            repair_generation,
            0,
            vec![fixture_entry("stale-repair", "Fixture")].into(),
        ));
        assert!(matches!(
            test_runtime.block_on(runtime.refresh_models(None)),
            ModelDiscoveryOutcome::Available(rows) if rows.len() == 2
        ));

        let remove_generation = runtime.state.cache.lock().test_unwrap().generation;
        assert!(matches!(
            test_runtime.block_on(runtime.remove(repair_connection.id)),
            ConnectionMutationOutcome::Succeeded
        ));
        assert!(!publish_snapshot(
            &runtime.state,
            remove_generation,
            0,
            vec![fixture_entry("stale-remove", "Fixture")].into(),
        ));
        assert!(matches!(
            test_runtime.block_on(runtime.refresh_models(None)),
            ModelDiscoveryOutcome::Available(rows) if rows.len() == 1
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn active_selection_round_trips_through_the_state_file() {
        let path = registry_fixture_path();
        let target = ActiveModelTarget {
            connection_id: ConnectionId::new_stored(),
            model: String::from("picked-model"),
        };

        write_active_selection(&path, &target).test_unwrap();

        assert_eq!(read_active_selection(&path), Some(target));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn active_selection_round_trips_the_environment_id() {
        let path = registry_fixture_path();
        let target = ActiveModelTarget {
            connection_id: ConnectionId::environment(),
            model: String::from("env-model"),
        };

        write_active_selection(&path, &target).test_unwrap();

        assert_eq!(read_active_selection(&path), Some(target));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_malformed_or_foreign_active_selection_reads_as_none() {
        let path = registry_fixture_path();
        assert_eq!(read_active_selection(&path), None);

        std::fs::write(&path, b"not json").test_unwrap();
        assert_eq!(read_active_selection(&path), None);

        std::fs::write(
            &path,
            br#"{"schema":"other.schema","connection_id":"environment","model_id":"m"}"#,
        )
        .test_unwrap();
        assert_eq!(read_active_selection(&path), None);

        std::fs::write(
            &path,
            br#"{"schema":"yach.active-model.v1","connection_id":"not-a-uuid","model_id":"m"}"#,
        )
        .test_unwrap();
        assert_eq!(read_active_selection(&path), None);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn registry_has_stored_connections_only_when_a_record_persists() {
        let path = registry_fixture_path();
        let metadata = JsonConnectionMetadataStore::new(path.clone());
        assert!(!registry_has_stored_connections(&metadata));

        let credentials = Arc::new(MutableCredentials::default());
        let store = ProviderConnectionStore::new(
            Arc::new(JsonConnectionMetadataStore::new(path.clone())),
            credentials,
        );
        let outcome = store.create_validated(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some(String::from("Stored")), None)
                .test_unwrap(),
            &ProviderSecret::new(String::from("stored-secret")),
        );
        assert!(matches!(outcome, CreateConnectionOutcome::Created(_)));

        assert!(registry_has_stored_connections(&metadata));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn credentials_are_read_once_per_connection_across_lists_refreshes_and_activation() {
        let path = registry_fixture_path();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let credentials = Arc::new(MutableCredentials::default());
        let store = ProviderConnectionStore::new(metadata.clone(), credentials.clone());
        let connection = match store.create_validated(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some(String::from("Cached")), None)
                .test_unwrap(),
            &ProviderSecret::new(String::from("cached-secret")),
        ) {
            CreateConnectionOutcome::Created(connection) => connection,
            outcome => unreachable!("fixture connection must be created: {outcome:?}"),
        };
        let runtime = mutable_runtime(metadata, credentials.clone());
        let test_runtime = tokio::runtime::Runtime::new().test_unwrap();

        for _ in 0..2 {
            assert!(matches!(
                test_runtime.block_on(runtime.list()),
                ConnectionListOutcome::Available(_)
            ));
        }
        assert!(matches!(
            test_runtime.block_on(runtime.refresh_models(None)),
            ModelDiscoveryOutcome::Available(_)
        ));
        assert!(matches!(
            test_runtime
                .block_on(runtime.activate(connection.id.clone(), String::from("fresh-model"))),
            ProviderActivationOutcome::Activated(_)
        ));

        assert_eq!(credentials.gets.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_successful_mutation_invalidates_cached_credentials() {
        let path = registry_fixture_path();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let credentials = Arc::new(MutableCredentials::default());
        let store = ProviderConnectionStore::new(metadata.clone(), credentials.clone());
        let outcome = store.create_validated(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some(String::from("First")), None)
                .test_unwrap(),
            &ProviderSecret::new(String::from("first-secret")),
        );
        assert!(matches!(outcome, CreateConnectionOutcome::Created(_)));
        let runtime = mutable_runtime(metadata, credentials.clone());
        let test_runtime = tokio::runtime::Runtime::new().test_unwrap();

        assert!(matches!(
            test_runtime.block_on(runtime.list()),
            ConnectionListOutcome::Available(_)
        ));
        assert_eq!(credentials.gets.load(Ordering::SeqCst), 1);

        let draft =
            NewConnectionDraft::new(ProviderKind::OpenAi, Some(String::from("Second")), None)
                .test_unwrap();
        assert!(matches!(
            test_runtime.block_on(
                runtime.create(draft, ProviderSecret::new(String::from("second-secret")))
            ),
            ConnectionMutationOutcome::Succeeded
        ));

        assert!(matches!(
            test_runtime.block_on(runtime.list()),
            ConnectionListOutcome::Available(_)
        ));
        assert_eq!(credentials.gets.load(Ordering::SeqCst), 3);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_credential_downgrade_is_cached_across_lists() {
        let path = registry_fixture_path();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let credentials = Arc::new(MutableCredentials::default());
        let store = ProviderConnectionStore::new(metadata.clone(), credentials.clone());
        let connection = match store.create_validated(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some(String::from("Missing")), None)
                .test_unwrap(),
            &ProviderSecret::new(String::from("removed-secret")),
        ) {
            CreateConnectionOutcome::Created(connection) => connection,
            outcome => unreachable!("fixture connection must be created: {outcome:?}"),
        };
        credentials.remove(&connection.id).test_unwrap();
        let runtime = mutable_runtime(metadata, credentials.clone());
        let test_runtime = tokio::runtime::Runtime::new().test_unwrap();

        for _ in 0..2 {
            let ConnectionListOutcome::Available(list) = test_runtime.block_on(runtime.list())
            else {
                unreachable!("fixture registry should list");
            };
            assert!(matches!(
                list.as_slice()[0].state,
                ConnectionState::PendingCredential
            ));
        }

        assert_eq!(credentials.gets.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn repair_replaces_missing_credential_for_ready_connection() {
        let path = registry_fixture_path();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let credentials = Arc::new(MutableCredentials::default());
        let store = ProviderConnectionStore::new(metadata.clone(), credentials.clone());
        let connection = match store.create_validated(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some(String::from("Ready")), None)
                .test_unwrap(),
            &ProviderSecret::new(String::from("old-secret")),
        ) {
            CreateConnectionOutcome::Created(connection) => connection,
            outcome => unreachable!("fixture ready connection must be created: {outcome:?}"),
        };
        credentials.remove(&connection.id).test_unwrap();
        let runtime = mutable_runtime(metadata, credentials.clone());

        assert!(matches!(
            tokio::runtime::Runtime::new()
                .test_unwrap()
                .block_on(runtime.repair(
                    connection.id.clone(),
                    ProviderSecret::new(String::from("replacement-secret")),
                )),
            ConnectionMutationOutcome::Succeeded
        ));
        assert!(credentials.get(&connection.id).test_unwrap().is_some());
        let _ = std::fs::remove_file(path);
    }
    fn fixture_entry(id: &str, connection: &str) -> CatalogModelEntry {
        CatalogModelEntry {
            info: yach_proto::ModelInfo {
                id: String::from(id),
                name: String::from(id),
                provider: String::from("openai"),
                connection_id: Some(String::from(connection)),
                connection_display: Some(String::from(connection)),
            },
            context_window: 1,
            output_budget: 1,
            max_tokens_param: MaxTokensParam::MaxTokens,
        }
    }
    #[derive(Default)]
    struct MutableCredentials {
        values: Mutex<std::collections::BTreeMap<String, String>>,
        failing_puts: AtomicUsize,
        gets: AtomicUsize,
    }

    impl MutableCredentials {
        fn fail_next_put(&self) {
            self.failing_puts.store(1, Ordering::SeqCst);
        }
    }

    impl CredentialStore for MutableCredentials {
        fn put(&self, id: &ConnectionId, secret: &ProviderSecret) -> Result<(), CredentialError> {
            if self
                .failing_puts
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(CredentialError::Unavailable);
            }
            self.values
                .lock()
                .test_unwrap()
                .insert(id.as_str().to_owned(), secret.with_exposed(str::to_owned));
            Ok(())
        }
        fn get(&self, id: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .values
                .lock()
                .test_unwrap()
                .get(id.as_str())
                .cloned()
                .map(ProviderSecret::new))
        }

        fn remove(&self, id: &ConnectionId) -> Result<(), CredentialError> {
            self.values.lock().test_unwrap().remove(id.as_str());
            Ok(())
        }
    }

    fn mutable_runtime(
        metadata: Arc<JsonConnectionMetadataStore>,
        credentials: Arc<MutableCredentials>,
    ) -> CliProviderConnectionRuntime {
        CliProviderConnectionRuntime::with_stores_and_discoverer(
            metadata,
            credentials,
            super::super::model_layers_fixture(),
            None,
            Arc::new(|_| {
                Box::pin(async {
                    Ok(vec![
                        yach_backend::model_discovery::DiscoveredProviderModel {
                            id: String::from("fresh-model"),
                            display_name: None,
                        },
                    ])
                })
            }),
        )
    }

    fn local_provider_request_fixture(
        expected_requests: usize,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").test_unwrap();
        let address = listener.local_addr().test_unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..expected_requests {
                let (mut stream, _) = listener.accept().test_unwrap();
                let mut bytes = [0; 16_384];
                let count = stream.read(&mut bytes).test_unwrap();
                sender
                    .send(String::from_utf8_lossy(&bytes[..count]).into_owned())
                    .test_unwrap();
                stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").test_unwrap();
            }
        });
        (format!("http://{address}/v1"), receiver)
    }

    fn registry_fixture_path() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "yach-runtime-fixture-{}-{}.json",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst),
        ))
    }

    fn ready_compatible(label: &str, base_url: &str) -> ProviderConnection {
        ProviderConnection::stored(
            ConnectionId::new_stored(),
            ProviderKind::OpenAiCompatible,
            Some(String::from(label)),
            Some(String::from(base_url)),
            ConnectionState::Ready,
        )
        .test_unwrap()
    }

    struct ReadyCredentials;

    fn local_models_fixture(
        expected_path: &'static str,
        expected_authorization: &'static str,
    ) -> (String, std::sync::mpsc::Receiver<bool>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").test_unwrap();
        let address = listener.local_addr().test_unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().test_unwrap();
            let mut bytes = [0; 4096];
            let count = stream.read(&mut bytes).test_unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]);
            let matches = request
                .lines()
                .next()
                .is_some_and(|line| line == format!("GET {expected_path} HTTP/1.1"))
                && request.lines().any(|line| line == expected_authorization);
            sender.send(matches).test_unwrap();
            stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 28\r\nConnection: close\r\n\r\nfixture-server-body-sentinel").test_unwrap();
        });
        (format!("http://{address}"), receiver)
    }
    impl CredentialStore for ReadyCredentials {
        fn put(&self, _: &ConnectionId, _: &ProviderSecret) -> Result<(), CredentialError> {
            Ok(())
        }

        fn get(&self, _: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError> {
            Ok(Some(ProviderSecret::new(String::from("fixture-secret"))))
        }

        fn remove(&self, _: &ConnectionId) -> Result<(), CredentialError> {
            Ok(())
        }
    }

    impl ConnectionMetadataStore for FixedMetadata {
        fn load(&self) -> Result<Vec<ProviderConnection>, yach_connections::RegistryError> {
            Ok(self.records.clone())
        }

        fn lock_connection(
            &self,
            _: &ConnectionId,
        ) -> Result<
            Box<dyn yach_connections::LockedConnectionMetadata>,
            yach_connections::RegistryError,
        > {
            unreachable!("list does not mutate metadata")
        }
    }

    #[derive(Default)]
    struct CountingMetadata {
        loads: AtomicUsize,
    }

    impl ConnectionMetadataStore for CountingMetadata {
        fn load(&self) -> Result<Vec<ProviderConnection>, yach_connections::RegistryError> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        fn lock_connection(
            &self,
            _: &ConnectionId,
        ) -> Result<
            Box<dyn yach_connections::LockedConnectionMetadata>,
            yach_connections::RegistryError,
        > {
            unreachable!("inert constructor must not lock metadata")
        }
    }

    #[derive(Default)]
    struct CountingCredentials {
        gets: AtomicUsize,
    }

    impl CredentialStore for CountingCredentials {
        fn put(&self, _: &ConnectionId, _: &ProviderSecret) -> Result<(), CredentialError> {
            unreachable!("inert constructor must not store credentials")
        }

        fn get(&self, _: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        fn remove(&self, _: &ConnectionId) -> Result<(), CredentialError> {
            unreachable!("inert constructor must not remove credentials")
        }
    }
}
