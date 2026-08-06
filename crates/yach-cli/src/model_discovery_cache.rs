use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;
use yach_backend::model_discovery::DiscoveredProviderModel;
use yach_connections::{ConnectionId, ProviderConnection, ProviderKind};

const CACHE_SCHEMA: &str = "yach.model-discovery.v1";
const MAX_CACHE_BYTES: usize = 1_048_576;
const MAX_CONNECTIONS: usize = 64;
const MAX_ROWS_PER_CONNECTION: usize = 256;
const MAX_MODEL_ID_BYTES: usize = 512;
const MAX_DISPLAY_NAME_BYTES: usize = 512;
const MAX_ENDPOINT_BYTES: usize = 2_048;
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DiscoveryCache {
    entries: BTreeMap<ConnectionId, CachedConnectionDiscovery>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedModels {
    pub(crate) models: Vec<DiscoveredProviderModel>,
    pub(crate) fresh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedConnectionDiscovery {
    provider: ProviderKind,
    endpoint: Option<String>,
    refreshed_at: u64,
    models: Vec<DiscoveredProviderModel>,
    truncated: bool,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct CacheDocument {
    schema: String,
    connections: Vec<CacheConnectionDocument>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct CacheConnectionDocument {
    connection_id: String,
    provider: ProviderKind,
    endpoint: Option<String>,
    refreshed_at: u64,
    models: Vec<CacheModelDocument>,
    #[serde(default)]
    truncated: bool,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct CacheModelDocument {
    id: String,
    display_name: Option<String>,
}

impl DiscoveryCache {
    #[must_use]
    pub(crate) fn load(path: &Path) -> Self {
        let Ok(metadata) = fs::metadata(path) else {
            return Self::default();
        };
        if !metadata.is_file() {
            return Self::default();
        }
        let Ok(length) = usize::try_from(metadata.len()) else {
            return Self::default();
        };
        if length > MAX_CACHE_BYTES {
            return Self::default();
        }
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(document) = serde_json::from_slice::<CacheDocument>(&bytes) else {
            return Self::default();
        };
        if document.schema != CACHE_SCHEMA || document.connections.len() > MAX_CONNECTIONS {
            return Self::default();
        }

        let mut entries = BTreeMap::new();
        for entry in document.connections {
            let Ok(connection_id) = ConnectionId::parse_stored(&entry.connection_id) else {
                return Self::default();
            };
            if entry
                .endpoint
                .as_ref()
                .is_some_and(|endpoint| endpoint.len() > MAX_ENDPOINT_BYTES)
                || entry.models.len() > MAX_ROWS_PER_CONNECTION
                || !entry
                    .models
                    .iter()
                    .all(|model| valid_model_fields(&model.id, model.display_name.as_deref()))
                || entries.contains_key(&connection_id)
            {
                return Self::default();
            }
            entries.insert(
                connection_id,
                CachedConnectionDiscovery {
                    provider: entry.provider,
                    endpoint: entry.endpoint,
                    refreshed_at: entry.refreshed_at,
                    models: entry
                        .models
                        .into_iter()
                        .map(|model| DiscoveredProviderModel {
                            id: model.id,
                            display_name: model.display_name,
                        })
                        .collect(),
                    truncated: entry.truncated,
                },
            );
        }
        Self { entries }
    }

    #[must_use]
    pub(crate) fn models_for(
        &self,
        connection: &ProviderConnection,
        now: u64,
        freshness_seconds: u64,
    ) -> Option<CachedModels> {
        let entry = self.entries.get(&connection.id)?;
        if entry.provider != connection.provider || entry.endpoint != connection.base_url {
            return None;
        }
        Some(CachedModels {
            models: entry.models.clone(),
            fresh: !entry.truncated
                && entry.refreshed_at <= now
                && now - entry.refreshed_at < freshness_seconds,
        })
    }

    pub(crate) fn update(
        &mut self,
        connection: &ProviderConnection,
        refreshed_at: u64,
        models: Vec<DiscoveredProviderModel>,
    ) {
        let mut bounded_models = Vec::with_capacity(models.len().min(MAX_ROWS_PER_CONNECTION));
        let mut truncated = false;
        for mut model in models {
            if model.id.len() > MAX_MODEL_ID_BYTES {
                truncated = true;
                continue;
            }
            if model
                .display_name
                .as_ref()
                .is_some_and(|name| name.len() > MAX_DISPLAY_NAME_BYTES)
            {
                model.display_name = None;
            }
            if bounded_models.len() == MAX_ROWS_PER_CONNECTION {
                truncated = true;
                break;
            }
            bounded_models.push(model);
        }
        self.entries.insert(
            connection.id.clone(),
            CachedConnectionDiscovery {
                provider: connection.provider,
                endpoint: connection.base_url.clone(),
                refreshed_at,
                models: bounded_models,
                truncated,
            },
        );
        self.enforce_persisted_bounds(&connection.id);
    }

    fn enforce_persisted_bounds(&mut self, updated_id: &ConnectionId) {
        let environment_id = ConnectionId::environment();
        while self
            .entries
            .keys()
            .filter(|connection_id| *connection_id != &environment_id)
            .count()
            > MAX_CONNECTIONS
        {
            let Some(oldest_id) = self.oldest_persisted_entry(updated_id) else {
                return;
            };
            self.entries.remove(&oldest_id);
        }

        while !self.persisted_document_within_limit() {
            let Some(oldest_id) = self.oldest_persisted_entry(updated_id) else {
                break;
            };
            self.entries.remove(&oldest_id);
        }

        while !self.persisted_document_within_limit() {
            let Some(entry) = self.entries.get_mut(updated_id) else {
                return;
            };
            if entry.models.pop().is_none() {
                self.entries.remove(updated_id);
                return;
            }
            entry.truncated = true;
        }
    }

    fn oldest_persisted_entry(&self, excluded_id: &ConnectionId) -> Option<ConnectionId> {
        let environment_id = ConnectionId::environment();
        let mut oldest = None;
        for (connection_id, entry) in &self.entries {
            if connection_id == &environment_id || connection_id == excluded_id {
                continue;
            }
            if oldest.is_none_or(|(oldest_id, oldest_refreshed_at)| {
                entry.refreshed_at < oldest_refreshed_at
                    || (entry.refreshed_at == oldest_refreshed_at && connection_id < oldest_id)
            }) {
                oldest = Some((connection_id, entry.refreshed_at));
            }
        }
        oldest.map(|(connection_id, _)| connection_id.clone())
    }

    fn persisted_document_within_limit(&self) -> bool {
        serde_json::to_vec(&self.cache_document())
            .is_ok_and(|document| document.len() <= MAX_CACHE_BYTES)
    }

    fn cache_document(&self) -> CacheDocument {
        let environment_id = ConnectionId::environment();
        CacheDocument {
            schema: String::from(CACHE_SCHEMA),
            connections: self
                .entries
                .iter()
                .filter(|(connection_id, _)| *connection_id != &environment_id)
                .map(|(connection_id, entry)| CacheConnectionDocument {
                    connection_id: connection_id.as_str().to_owned(),
                    provider: entry.provider,
                    endpoint: entry.endpoint.clone(),
                    refreshed_at: entry.refreshed_at,
                    models: entry
                        .models
                        .iter()
                        .map(|model| CacheModelDocument {
                            id: model.id.clone(),
                            display_name: model.display_name.clone(),
                        })
                        .collect(),
                    truncated: entry.truncated,
                })
                .collect(),
        }
    }

    pub(crate) fn invalidate(&mut self, id: &ConnectionId) -> bool {
        self.entries.remove(id).is_some()
    }

    pub(crate) fn persist(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("cache has no parent"))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::other("cache path has no file name"))?;
        let document = self.cache_document();
        let bytes = serde_json::to_vec(&document)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() > MAX_CACHE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "model discovery cache exceeds the maximum size",
            ));
        }
        let parent_was_missing = !parent.exists();
        fs::create_dir_all(parent)?;
        if parent_was_missing {
            set_permissions(parent, 0o700)?;
        }
        let temporary_path = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut temporary = options.open(&temporary_path)?;
        if let Err(error) = temporary
            .write_all(&bytes)
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.sync_all())
        {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        drop(temporary);
        if let Err(error) = fs::rename(&temporary_path, path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        sync_parent_directory(parent)
    }
}

#[must_use]
pub(crate) fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn valid_model_fields(id: &str, display_name: Option<&str>) -> bool {
    id.len() <= MAX_MODEL_ID_BYTES
        && display_name.is_none_or(|name| name.len() <= MAX_DISPLAY_NAME_BYTES)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent).and_then(|directory| directory.sync_all())
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_permissions(_: &Path, _: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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
    fn missing_cache_loads_as_empty() {
        assert_eq!(
            DiscoveryCache::load(Path::new("/definitely/missing/cache.json")),
            DiscoveryCache::default()
        );
    }

    #[test]
    fn malformed_or_oversized_cache_is_absent() {
        let path = std::env::temp_dir().join(format!("yach-cache-{}.json", Uuid::new_v4()));

        fs::write(&path, b"not json").test_unwrap();
        assert_eq!(DiscoveryCache::load(&path), DiscoveryCache::default());

        fs::write(&path, vec![b' '; MAX_CACHE_BYTES + 1]).test_unwrap();
        assert_eq!(DiscoveryCache::load(&path), DiscoveryCache::default());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cache_ignores_mismatched_provider_or_endpoint() {
        let connection = ProviderConnection::stored(
            ConnectionId::new_stored(),
            ProviderKind::OpenAiCompatible,
            Some(String::from("Fixture")),
            Some(String::from("http://one.invalid/v1")),
            yach_connections::ConnectionState::Ready,
        )
        .test_unwrap();
        let mut cache = DiscoveryCache::default();
        cache.update(
            &connection,
            1,
            vec![DiscoveredProviderModel {
                id: String::from("cached-model"),
                display_name: None,
            }],
        );
        let endpoint_changed = ProviderConnection::stored(
            connection.id.clone(),
            ProviderKind::OpenAiCompatible,
            Some(String::from("Fixture")),
            Some(String::from("http://two.invalid/v1")),
            yach_connections::ConnectionState::Ready,
        )
        .test_unwrap();

        assert!(cache.models_for(&endpoint_changed, 1, 1).is_none());
    }

    #[test]
    fn future_and_boundary_timestamps_are_stale() {
        let connection = fixture_connection("http://fixture.invalid/v1");
        let mut cache = DiscoveryCache::default();
        cache.update(
            &connection,
            100,
            vec![DiscoveredProviderModel {
                id: String::from("fixture"),
                display_name: None,
            }],
        );

        assert!(!cache.models_for(&connection, 99, 7_200).test_unwrap().fresh);
        assert!(
            !cache
                .models_for(&connection, 7_300, 7_200)
                .test_unwrap()
                .fresh
        );
        assert!(
            cache
                .models_for(&connection, 7_299, 7_200)
                .test_unwrap()
                .fresh
        );
    }

    #[test]
    fn environment_discovery_is_cached_in_memory() {
        let connection = ProviderConnection {
            id: ConnectionId::environment(),
            provider: ProviderKind::OpenAi,
            label: Some(String::from("Environment")),
            base_url: None,
            authentication: yach_connections::ConnectionAuth::ApiKey {
                source: yach_connections::CredentialSource::Environment,
            },
            state: yach_connections::ConnectionState::Ready,
        };
        let mut cache = DiscoveryCache::default();
        cache.update(
            &connection,
            100,
            vec![DiscoveredProviderModel {
                id: String::from("environment-model"),
                display_name: None,
            }],
        );

        let cached = cache.models_for(&connection, 100, 7_200).test_unwrap();
        assert!(cached.fresh);
        assert_eq!(cached.models[0].id, "environment-model");
    }

    #[test]
    fn oversized_display_name_keeps_the_model_identity() {
        let connection = fixture_connection("http://fixture.invalid/v1");
        let mut cache = DiscoveryCache::default();
        cache.update(
            &connection,
            100,
            vec![DiscoveredProviderModel {
                id: String::from("model-with-long-name"),
                display_name: Some("x".repeat(MAX_DISPLAY_NAME_BYTES + 1)),
            }],
        );

        let cached = cache.models_for(&connection, 100, 7_200).test_unwrap();
        assert!(cached.fresh);
        assert_eq!(cached.models.len(), 1);
        assert_eq!(cached.models[0].id, "model-with-long-name");
        assert!(cached.models[0].display_name.is_none());
    }

    #[test]
    fn truncated_snapshots_remain_stale_across_persistence() {
        let path = std::env::temp_dir().join(format!("yach-cache-{}.json", Uuid::new_v4()));
        let connection = fixture_connection("http://fixture.invalid/v1");
        let mut cache = DiscoveryCache::default();
        cache.update(
            &connection,
            100,
            (0..=MAX_ROWS_PER_CONNECTION)
                .map(|index| DiscoveredProviderModel {
                    id: format!("model-{index}"),
                    display_name: None,
                })
                .collect(),
        );

        let cached = cache.models_for(&connection, 100, 7_200).test_unwrap();
        assert!(!cached.fresh);
        assert_eq!(cached.models.len(), MAX_ROWS_PER_CONNECTION);
        cache.persist(&path).test_unwrap();
        assert!(
            !DiscoveryCache::load(&path)
                .models_for(&connection, 100, 7_200)
                .test_unwrap()
                .fresh
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn updates_keep_the_persisted_document_within_the_load_limit() {
        let path = std::env::temp_dir().join(format!("yach-cache-{}.json", Uuid::new_v4()));
        let mut cache = DiscoveryCache::default();
        for index in 0..MAX_CONNECTIONS {
            let connection = fixture_connection(&format!("http://{index}.invalid/v1"));
            cache.update(
                &connection,
                1,
                (0..MAX_ROWS_PER_CONNECTION)
                    .map(|model| DiscoveredProviderModel {
                        id: format!("{model:04}-{}", "x".repeat(MAX_MODEL_ID_BYTES - 5)),
                        display_name: None,
                    })
                    .collect(),
            );
        }

        cache.persist(&path).test_unwrap();
        assert!(fs::metadata(&path).test_unwrap().len() <= MAX_CACHE_BYTES as u64);
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn persist_creates_a_private_parent_directory() {
        use std::os::unix::fs::PermissionsExt;

        let parent = std::env::temp_dir().join(format!("yach-cache-{}", Uuid::new_v4()));
        let path = parent.join("model-discovery.json");
        DiscoveryCache::default().persist(&path).test_unwrap();

        assert_eq!(
            fs::metadata(&parent).test_unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(parent);
    }

    fn fixture_connection(endpoint: &str) -> ProviderConnection {
        ProviderConnection::stored(
            ConnectionId::new_stored(),
            ProviderKind::OpenAiCompatible,
            Some(String::from("Fixture")),
            Some(String::from(endpoint)),
            yach_connections::ConnectionState::Ready,
        )
        .test_unwrap()
    }
}
