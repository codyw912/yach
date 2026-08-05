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

/// The production platform credential store.
#[derive(Debug, Default)]
pub struct SystemCredentialStore;

impl SystemCredentialStore {
    /// Creates the platform-backed credential store.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn entry(id: &ConnectionId) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new("yach", id.as_str()).map_err(|error| map_keyring_error(&error))
    }
}

impl CredentialStore for SystemCredentialStore {
    fn put(&self, id: &ConnectionId, secret: &ProviderSecret) -> Result<(), CredentialError> {
        Self::entry(id)?
            .set_password(secret.as_str())
            .map_err(|error| map_keyring_error(&error))
    }

    fn get(&self, id: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError> {
        match Self::entry(id)?.get_password() {
            Ok(value) => Ok(Some(ProviderSecret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_keyring_error(&error)),
        }
    }

    fn remove(&self, id: &ConnectionId) -> Result<(), CredentialError> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring_error(&error)),
        }
    }
}

pub(crate) fn map_keyring_error(error: &keyring::Error) -> CredentialError {
    match error {
        keyring::Error::NoEntry => CredentialError::Missing,
        keyring::Error::NoStorageAccess(_) => CredentialError::AccessDenied,
        keyring::Error::TooLong(_, _)
        | keyring::Error::Invalid(_, _)
        | keyring::Error::BadEncoding(_)
        | keyring::Error::BadDataFormat(_, _)
        | keyring::Error::BadStoreFormat(_) => CredentialError::Invalid,
        _ => CredentialError::Unavailable,
    }
}
