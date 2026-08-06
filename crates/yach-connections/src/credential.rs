use std::{error::Error, fmt};

use crate::{ConnectionId, ProviderSecret};

/// Bounded categories for platform credential-store failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    /// No credential exists for the connection.
    Missing,
    /// The platform credential service is unavailable.
    Unavailable,
    /// The platform credential service denied or locked access.
    AccessDenied,
    /// The supplied credential could not be stored or decoded.
    Invalid,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Missing => "Credential is unavailable.",
            Self::Unavailable => "System credential storage is unavailable.",
            Self::AccessDenied => "System credential storage access was denied.",
            Self::Invalid => "System credential storage rejected the credential.",
        };
        formatter.write_str(message)
    }
}

impl Error for CredentialError {}

/// Injectable credentials used by durable provider connections.
pub trait CredentialStore: Send + Sync {
    /// Stores a credential under a stable connection identity.
    fn put(&self, id: &ConnectionId, secret: &ProviderSecret) -> Result<(), CredentialError>;

    /// Loads the credential when it is present and accessible.
    fn get(&self, id: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError>;

    /// Removes the credential associated with a stable connection identity.
    fn remove(&self, id: &ConnectionId) -> Result<(), CredentialError>;
}

const CREDENTIALS_SCHEMA: &str = "yach.credentials.v1";

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CredentialsDocument {
    schema: String,
    credentials: std::collections::BTreeMap<String, StoredCredential>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredCredential {
    api_key: String,
}

/// The production credential store: a permissioned plaintext JSON document
/// (`0600`, parent directory `0700`) written atomically. Chosen over the OS
/// credential manager because ad-hoc-signed binaries prompt on every keychain
/// access; see `docs/superpowers/specs/2026-08-05-file-credential-store-design.md`.
#[derive(Debug)]
pub struct FileCredentialStore {
    path: std::path::PathBuf,
}

impl FileCredentialStore {
    /// Creates a file-backed credential store rooted at `path`.
    #[must_use]
    pub const fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    /// Missing, malformed, and foreign-schema documents all read as empty:
    /// a corrupt file must never block startup or lock users out of repair.
    fn read_document(&self) -> CredentialsDocument {
        let empty = || CredentialsDocument {
            schema: CREDENTIALS_SCHEMA.to_owned(),
            credentials: std::collections::BTreeMap::new(),
        };
        let Ok(bytes) = std::fs::read(&self.path) else {
            return empty();
        };
        match serde_json::from_slice::<CredentialsDocument>(&bytes) {
            Ok(document) if document.schema == CREDENTIALS_SCHEMA => document,
            _ => empty(),
        }
    }

    fn write_document(&self, document: &CredentialsDocument) -> Result<(), CredentialError> {
        let bytes = serde_json::to_vec(document).map_err(|_| CredentialError::Invalid)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| CredentialError::Unavailable)?;
            set_permissions(parent, 0o700);
        }
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes).map_err(|_| CredentialError::Unavailable)?;
        set_permissions(&temporary, 0o600);
        std::fs::rename(&temporary, &self.path).map_err(|_| CredentialError::Unavailable)
    }
}

#[cfg(unix)]
fn set_permissions(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_permissions(_: &std::path::Path, _: u32) {}

impl CredentialStore for FileCredentialStore {
    fn put(&self, id: &ConnectionId, secret: &ProviderSecret) -> Result<(), CredentialError> {
        let mut document = self.read_document();
        document.credentials.insert(
            id.as_str().to_owned(),
            StoredCredential {
                api_key: secret.as_str().to_owned(),
            },
        );
        self.write_document(&document)
    }

    fn get(&self, id: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError> {
        Ok(self
            .read_document()
            .credentials
            .get(id.as_str())
            .map(|stored| ProviderSecret::new(stored.api_key.clone())))
    }

    fn remove(&self, id: &ConnectionId) -> Result<(), CredentialError> {
        let mut document = self.read_document();
        if document.credentials.remove(id.as_str()).is_some() {
            self.write_document(&document)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "yach-file-credentials-{}-{name}",
                std::process::id()
            ))
            .join("credentials.json")
    }

    fn cleanup(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[cfg(unix)]
    fn mode(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).test_unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn file_store_round_trips_put_get_remove() {
        let path = fixture_path("round-trip");
        let store = FileCredentialStore::new(path.clone());
        let id = ConnectionId::new_stored();

        assert_eq!(store.get(&id), Ok(None));

        store
            .put(&id, &ProviderSecret::new(String::from("sk-round-trip")))
            .test_unwrap();
        let loaded = store.get(&id).test_unwrap().test_unwrap();
        assert_eq!(loaded.as_str(), "sk-round-trip");

        store.remove(&id).test_unwrap();
        assert_eq!(store.get(&id), Ok(None));
        cleanup(&path);
    }

    #[cfg(unix)]
    #[test]
    fn file_store_enforces_owner_only_permissions() {
        let path = fixture_path("permissions");
        let store = FileCredentialStore::new(path.clone());
        let id = ConnectionId::new_stored();

        store
            .put(&id, &ProviderSecret::new(String::from("sk-perms")))
            .test_unwrap();

        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().test_unwrap()), 0o700);
        cleanup(&path);
    }

    #[test]
    fn file_store_reads_missing_malformed_and_foreign_documents_as_empty() {
        let path = fixture_path("tolerance");
        let store = FileCredentialStore::new(path.clone());
        let id = ConnectionId::new_stored();

        assert_eq!(store.get(&id), Ok(None));

        std::fs::create_dir_all(path.parent().test_unwrap()).test_unwrap();
        std::fs::write(&path, b"not json").test_unwrap();
        assert_eq!(store.get(&id), Ok(None));

        std::fs::write(&path, br#"{"schema":"other.schema","credentials":{}}"#).test_unwrap();
        assert_eq!(store.get(&id), Ok(None));

        cleanup(&path);
    }

    #[test]
    fn file_store_write_is_atomic_and_removes_unknown_ids_cleanly() {
        let path = fixture_path("atomic");
        let store = FileCredentialStore::new(path.clone());
        let id = ConnectionId::new_stored();

        store.remove(&id).test_unwrap();
        store
            .put(&id, &ProviderSecret::new(String::from("sk-atomic")))
            .test_unwrap();

        let temporary = path.with_extension("json.tmp");
        assert!(!temporary.exists(), "no temp file may survive a write");
        cleanup(&path);
    }
}
