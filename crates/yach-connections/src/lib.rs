//! Durable provider-connection metadata and platform-managed credentials.

mod chatgpt_auth;
mod credential;
mod registry;
use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroize;

pub use chatgpt_auth::{AuthFilePreparation, AuthFileProblem, prepare_chatgpt_auth_file};
pub use credential::{CredentialError, CredentialStore, FileCredentialStore};
pub use registry::{
    ConnectionMetadataStore, JsonConnectionMetadataStore, LockedConnectionMetadata, RegistryError,
};

/// The opaque stable identity of one provider connection.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ConnectionId(String);

impl ConnectionId {
    /// Generates a new UUID-backed stored connection identifier.
    #[must_use]
    pub fn new_stored() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Returns the reserved identity for the one transient environment connection.
    #[must_use]
    pub fn environment() -> Self {
        Self("environment".to_owned())
    }

    /// Parses a valid persisted UUID identity, rejecting the transient environment ID.
    pub fn parse_stored(value: &str) -> Result<Self, ValidationError> {
        let id = Self(value.to_owned());
        id.validate_stored()?;
        Ok(id)
    }

    /// Returns the stable textual identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validates that this identifier is valid for persisted metadata.
    pub fn validate_stored(&self) -> Result<(), ValidationError> {
        let Ok(uuid) = Uuid::parse_str(&self.0) else {
            return Err(ValidationError::InvalidStoredId);
        };
        if self.0 == "environment" || uuid.to_string() != self.0 {
            return Err(ValidationError::InvalidStoredId);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ConnectionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let id = Self(String::deserialize(deserializer)?);
        if id.0 == "environment" {
            return Ok(id);
        }
        id.validate_stored()
            .map_err(|_| serde::de::Error::custom("Connection identity is invalid."))?;
        Ok(id)
    }
}

/// Providers supported by the connection domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    /// Anthropic API-key provider.
    #[serde(rename = "anthropic")]
    Anthropic,
    /// OpenAI API-key provider.
    #[serde(rename = "openai")]
    OpenAi,
    /// An OpenAI-compatible API-key endpoint.
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    /// ChatGPT Plus/Pro Codex subscription (OAuth).
    #[serde(rename = "openai-codex", alias = "chatgpt-subscription")]
    ChatGptSubscription,
}

impl ProviderKind {
    /// Human-readable provider name for presentation-only labels.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::OpenAiCompatible => "OpenAI-compatible",
            Self::ChatGptSubscription => "OpenAI Codex",
        }
    }

    /// Returns the canonical configuration identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenAiCompatible => "openai-compatible",
            Self::ChatGptSubscription => "openai-codex",
        }
    }

    /// Parses a canonical configuration identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            "openai-compatible" => Some(Self::OpenAiCompatible),
            "openai-codex" => Some(Self::ChatGptSubscription),
            _ => None,
        }
    }

    const fn supports_persisted_api_key(self) -> bool {
        matches!(
            self,
            Self::Anthropic | Self::OpenAi | Self::OpenAiCompatible
        )
    }
}

/// Immutable user-facing identity for a stored provider connection.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ConnectionKey(String);

impl ConnectionKey {
    /// Parses the canonical lowercase configuration-key grammar.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 64
            || value == "environment"
            || !bytes[0].is_ascii_lowercase()
            || bytes.iter().any(|byte| {
                !(byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || *byte == b'_'
                    || *byte == b'-')
            })
        {
            return Err(ValidationError::InvalidConnectionKey);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the stable configuration identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ConnectionKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(|_| serde::de::Error::custom("Connection key is invalid."))
    }
}

/// The location of an API-key credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    /// The operating-system credential manager owns the API key.
    System,
    /// The current process environment owns the transient API key.
    Environment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionAuth {
    /// API-key authentication with a separately held secret.
    #[serde(rename = "api_key")]
    ApiKey {
        /// The place where the API key is resolved.
        source: CredentialSource,
    },
    /// Transient-only ChatGPT subscription authentication.
    #[serde(rename = "chatgpt_subscription")]
    ChatGptSubscriptionEnvironment {
        /// The subscription token directory configured by the environment.
        token_dir: PathBuf,
    },
    /// Persisted ChatGPT subscription authentication owned by yach.
    #[serde(rename = "chatgpt_subscription_managed")]
    ChatGptSubscriptionManaged {
        /// Logical auth-file path stamped from connection policy.
        auth_file: PathBuf,
        /// Nonempty account identity captured at successful login.
        account_id: String,
    },
}

/// Whether persisted metadata can be used for discovery and activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// Metadata exists but credential setup did not complete.
    PendingCredential,
    /// Metadata and the system credential have both been written.
    Ready,
}

/// Persisted or transient provider configuration without its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConnection {
    /// Stable opaque connection identity.
    pub id: ConnectionId,
    /// Provider client kind.
    pub provider: ProviderKind,
    /// Optional presentation-only label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Optional immutable identity used by human configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<ConnectionKey>,
    /// Optional endpoint for OpenAI-compatible providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Authentication shape without an API key.
    pub authentication: ConnectionAuth,
    /// Credential durability state.
    pub state: ConnectionState,
}

impl ProviderConnection {
    /// Constructs a validated persisted connection with system API-key authentication.
    pub fn stored(
        id: ConnectionId,
        provider: ProviderKind,
        label: Option<String>,
        base_url: Option<String>,
        state: ConnectionState,
    ) -> Result<Self, ValidationError> {
        let draft = NewConnectionDraft::new(provider, label, base_url)?;
        let connection = draft.into_pending(id);
        let connection = Self {
            state,
            ..connection
        };
        connection.validate_persisted()?;
        Ok(connection)
    }

    pub fn validate_persisted(&self) -> Result<(), ValidationError> {
        self.id.validate_stored()?;
        match &self.authentication {
            ConnectionAuth::ApiKey {
                source: CredentialSource::System,
            } if self.provider.supports_persisted_api_key() => {}
            ConnectionAuth::ChatGptSubscriptionManaged {
                auth_file,
                account_id,
            } if self.provider == ProviderKind::ChatGptSubscription
                && self.state == ConnectionState::Ready
                && self.base_url.is_none() =>
            {
                validate_managed_subscription(auth_file, account_id)?;
                let label = normalize_label(self.label.clone())?;
                if label != self.label {
                    return Err(ValidationError::NonCanonicalMetadata);
                }
                return Ok(());
            }
            _ => return Err(ValidationError::TransientAuthentication),
        }
        let normalized =
            NewConnectionDraft::new(self.provider, self.label.clone(), self.base_url.clone())?;
        if normalized.label != self.label || normalized.base_url != self.base_url {
            return Err(ValidationError::NonCanonicalMetadata);
        }
        Ok(())
    }

    /// Returns the deterministic presentation label among a connection list.
    #[must_use]
    pub fn display_label(&self, connections: &[Self]) -> String {
        let label = self.effective_label();
        if connections
            .iter()
            .filter(|connection| connection.effective_label() == label)
            .count()
            < 2
        {
            return label.to_owned();
        }

        let id = self.id.as_str();
        let id_length = id.chars().count();
        let initial_length = id_length.min(8);
        let prefix_length = (initial_length..=id_length)
            .find(|&length| {
                connections
                    .iter()
                    .filter(|connection| {
                        connection.effective_label() == label && connection.id != self.id
                    })
                    .all(|connection| {
                        !id.chars()
                            .take(length)
                            .eq(connection.id.as_str().chars().take(length))
                    })
            })
            .unwrap_or(id_length);
        let id_prefix = id.chars().take(prefix_length).collect::<String>();
        format!("{label} ({}, {id_prefix})", self.provider.display_name())
    }

    fn effective_label(&self) -> &str {
        self.label
            .as_deref()
            .unwrap_or_else(|| self.provider.display_name())
    }
}

/// A validated API-key connection draft before a generated ID is assigned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewConnectionDraft {
    provider: ProviderKind,
    label: Option<String>,
    key: Option<ConnectionKey>,
    base_url: Option<String>,
}

impl NewConnectionDraft {
    /// Validates and normalizes a new stored API-key connection draft.
    pub fn new(
        provider: ProviderKind,
        label: Option<String>,
        base_url: Option<String>,
    ) -> Result<Self, ValidationError> {
        if !provider.supports_persisted_api_key() {
            return Err(ValidationError::UnsupportedProvider);
        }
        let label = normalize_label(label)?;
        let base_url = match provider {
            ProviderKind::OpenAiCompatible => Some(normalize_base_url(
                &base_url.ok_or(ValidationError::MissingBaseUrl)?,
            )?),
            ProviderKind::Anthropic | ProviderKind::OpenAi => {
                if base_url.is_some() {
                    return Err(ValidationError::BaseUrlNotAllowed);
                }
                None
            }
            ProviderKind::ChatGptSubscription => return Err(ValidationError::UnsupportedProvider),
        };
        Ok(Self {
            provider,
            label,
            base_url,
            key: None,
        })
    }

    /// Returns the selected provider for candidate validation.
    #[must_use]
    pub const fn provider(&self) -> ProviderKind {
        self.provider
    }

    /// Returns the normalized optional presentation label.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
    /// Returns the optional immutable configuration key.
    #[must_use]
    pub fn key(&self) -> Option<&ConnectionKey> {
        self.key.as_ref()
    }

    /// Returns the normalized optional OpenAI-compatible endpoint.
    #[must_use]
    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// Assigns a validated immutable configuration key before persistence.
    #[must_use]
    pub fn with_key(mut self, key: ConnectionKey) -> Self {
        self.key = Some(key);
        self
    }

    fn into_pending(self, id: ConnectionId) -> ProviderConnection {
        ProviderConnection {
            id,
            provider: self.provider,
            label: self.label,
            key: self.key,
            base_url: self.base_url,
            authentication: ConnectionAuth::ApiKey {
                source: CredentialSource::System,
            },
            state: ConnectionState::PendingCredential,
        }
    }
}

/// An owned runtime API key that never serializes, clones its bytes, or
/// exposes a borrowed value outside a narrow provider-client construction
/// closure. Clones share one opaque, zeroizing allocation so model-profile
/// changes can retain an environment credential without duplicating it.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderSecret(Arc<SecretValue>);

#[derive(PartialEq, Eq)]
struct SecretValue(String);

impl ProviderSecret {
    /// Takes ownership of a provider credential.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(Arc::new(SecretValue(value)))
    }

    /// Returns whether the submitted credential is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.0.is_empty()
    }

    /// Consumes the wrapper for direct provider-client construction.
    #[must_use]
    pub fn into_inner(self) -> String {
        Arc::try_unwrap(self.0).map_or_else(
            |secret| secret.0.clone(),
            |mut secret| std::mem::take(&mut secret.0),
        )
    }

    /// Makes the key available only for one immediate provider-client call.
    ///
    /// The closure result must not retain the borrowed value; this is the sole
    /// cross-crate exposure point for Rig client construction.
    pub fn with_exposed<T>(&self, build: impl FnOnce(&str) -> T) -> T {
        build(&self.0.0)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0.0
    }
}

impl fmt::Debug for ProviderSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Bounded validation errors that never include user-supplied values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// A stored identity must be a UUID and may not use the environment identity.
    InvalidStoredId,
    /// Stored metadata attempted to represent transient authentication.
    TransientAuthentication,
    /// The provider cannot be saved as an API-key connection.
    UnsupportedProvider,
    /// A non-compatible provider included a base URL.
    BaseUrlNotAllowed,
    /// An OpenAI-compatible connection omitted its base URL.
    MissingBaseUrl,
    /// A base URL is malformed or uses forbidden components.
    InvalidBaseUrl,
    /// A base URL exceeds the metadata byte bound.
    BaseUrlTooLong,
    /// A label exceeds the 80-Unicode-scalar bound.
    LabelTooLong,
    /// A connection configuration key is malformed or reserved.
    InvalidConnectionKey,
    /// A second same-provider connection requires immutable keys.
    ConnectionKeyRequired,
    /// A submitted credential is empty.
    EmptySecret,
    /// Metadata was not stored in canonical normalized form.
    NonCanonicalMetadata,
    /// A managed subscription account id is empty or too long.
    InvalidAccountId,
    /// A managed subscription auth-file path is not well formed.
    InvalidAuthFile,
    /// HOME is unset so the default ChatGPT auth path cannot be resolved.
    HomeDirectoryMissing,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidStoredId => "Connection identity is invalid.",
            Self::TransientAuthentication => "Transient authentication cannot be stored.",
            Self::UnsupportedProvider => "This provider cannot be stored with an API key.",
            Self::BaseUrlNotAllowed => "This provider does not accept a custom base URL.",
            Self::MissingBaseUrl => "An OpenAI-compatible base URL is required.",
            Self::InvalidBaseUrl => "The base URL is invalid.",
            Self::BaseUrlTooLong => "The base URL is too long.",
            Self::LabelTooLong => "The connection label is too long.",
            Self::InvalidConnectionKey => "The connection configuration key is invalid.",
            Self::EmptySecret => "A credential is required.",
            Self::NonCanonicalMetadata => "Connection metadata is not normalized.",
            Self::ConnectionKeyRequired => {
                "Multiple connections for one provider require configuration keys."
            }
            Self::InvalidAccountId => "The ChatGPT account identity is invalid.",
            Self::InvalidAuthFile => "The ChatGPT auth file path is invalid.",
            Self::HomeDirectoryMissing => {
                "HOME is unset; cannot resolve the ChatGPT auth file path."
            }
        };
        formatter.write_str(message)
    }
}

impl Error for ValidationError {}

/// Bounded connection-store transaction failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStoreError {
    /// Input validation failed before a durable mutation started.
    Validation(ValidationError),
    /// Metadata persistence failed.
    Metadata(RegistryError),
    /// Credential persistence failed.
    Credential(CredentialError),
    /// The connection was absent after its per-ID lock was acquired.
    NotFound,
    /// Repair was requested for a record that is not pending.
    NotPending,
    /// Replacement was requested for a record that is not ready.
    NotReady,
    /// A generated identity unexpectedly already exists.
    AlreadyExists,
}

impl fmt::Display for ConnectionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::Metadata(error) => error.fmt(formatter),
            Self::Credential(error) => error.fmt(formatter),
            Self::NotFound => formatter.write_str("Provider connection was not found."),
            Self::NotPending => formatter.write_str("Provider connection is not awaiting repair."),
            Self::NotReady => {
                formatter.write_str("Provider connection is not ready for replacement.")
            }
            Self::AlreadyExists => formatter.write_str("Provider connection already exists."),
        }
    }
}

impl Error for ConnectionStoreError {}

/// The observable result of a create attempt, preserving a durable pending
/// identity when credentials or ready-state persistence fails after metadata.
#[derive(Debug)]
pub enum CreateConnectionOutcome {
    Created(ProviderConnection),
    FailedBeforePending(ConnectionStoreError),
    FailedAfterPending {
        id: ConnectionId,
        error: ConnectionStoreError,
    },
}

#[derive(Clone)]
enum ResolvedPolicy {
    Ready(ConnectionPolicy),
    Unavailable(ValidationError),
}

/// Transactional durable metadata and credential service.
#[derive(Clone)]
pub struct ProviderConnectionStore {
    metadata: Arc<dyn ConnectionMetadataStore>,
    credentials: Arc<dyn CredentialStore>,
    policy: ResolvedPolicy,
}

impl ProviderConnectionStore {
    /// Creates a connection service over injectable storage implementations.
    #[must_use]
    pub fn new(
        metadata: Arc<dyn ConnectionMetadataStore>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Self {
        Self {
            metadata,
            credentials,
            policy: match ConnectionPolicy::user_default() {
                Ok(policy) => ResolvedPolicy::Ready(policy),
                Err(error) => ResolvedPolicy::Unavailable(error),
            },
        }
    }

    /// Creates a store with an injected ChatGPT auth-file policy.
    #[must_use]
    pub fn with_policy(
        metadata: Arc<dyn ConnectionMetadataStore>,
        credentials: Arc<dyn CredentialStore>,
        policy: ConnectionPolicy,
    ) -> Self {
        Self {
            metadata,
            credentials,
            policy: ResolvedPolicy::Ready(policy),
        }
    }

    fn chatgpt_policy(&self) -> Result<&ConnectionPolicy, ConnectionStoreError> {
        match &self.policy {
            ResolvedPolicy::Ready(policy) => Ok(policy),
            ResolvedPolicy::Unavailable(error) => Err(ConnectionStoreError::Validation(*error)),
        }
    }

    /// Loads stored metadata without resolving any platform credential.
    pub fn list(&self) -> Result<Vec<ProviderConnection>, ConnectionStoreError> {
        self.metadata.load().map_err(ConnectionStoreError::Metadata)
    }

    /// Persists pending metadata, then the credential, then ready metadata.
    ///
    /// Only a successful pending metadata write exposes an ID for repair. A
    /// committed-but-indeterminate pending write never reaches the credential
    /// store and remains a fresh create attempt.
    pub fn create_validated(
        &self,
        draft: NewConnectionDraft,
        secret: &ProviderSecret,
    ) -> CreateConnectionOutcome {
        if let Err(error) = validate_secret(secret) {
            return CreateConnectionOutcome::FailedBeforePending(error);
        }
        let existing = match self.list() {
            Ok(existing) => existing,
            Err(error) => return CreateConnectionOutcome::FailedBeforePending(error),
        };
        let same_provider = existing
            .iter()
            .filter(|connection| {
                connection.provider == draft.provider()
                    && connection.state == ConnectionState::Ready
            })
            .collect::<Vec<_>>();
        if !same_provider.is_empty()
            && (draft.key().is_none()
                || same_provider
                    .iter()
                    .any(|connection| connection.key.is_none()))
        {
            return CreateConnectionOutcome::FailedBeforePending(ConnectionStoreError::Validation(
                ValidationError::ConnectionKeyRequired,
            ));
        }
        let id = ConnectionId::new_stored();
        let pending = draft.into_pending(id.clone());
        let mut locked = match self.lock(&id) {
            Ok(locked) => locked,
            Err(error) => return CreateConnectionOutcome::FailedBeforePending(error),
        };
        let existing = match locked.load().map_err(ConnectionStoreError::Metadata) {
            Ok(existing) => existing,
            Err(error) => return CreateConnectionOutcome::FailedBeforePending(error),
        };
        if existing.iter().any(|connection| connection.id == id) {
            return CreateConnectionOutcome::FailedBeforePending(
                ConnectionStoreError::AlreadyExists,
            );
        }
        let pending_write = locked.create_pending(pending.clone());
        if let Err(error) = reconcile_metadata_mutation(&mut *locked, pending_write) {
            return CreateConnectionOutcome::FailedBeforePending(error);
        }
        if let Err(error) = self
            .credentials
            .put(&id, secret)
            .map_err(ConnectionStoreError::Credential)
        {
            return CreateConnectionOutcome::FailedAfterPending { id, error };
        }
        let ready_write = locked.mark_ready(&id);
        if let Err(error) = reconcile_metadata_mutation(&mut *locked, ready_write) {
            return CreateConnectionOutcome::FailedAfterPending { id, error };
        }
        CreateConnectionOutcome::Created(ProviderConnection {
            state: ConnectionState::Ready,
            ..pending
        })
    }

    /// Writes a credential for an existing pending record and marks it ready.
    ///
    /// A committed-but-indeterminate ready write remains observable as ready but returns its
    /// durability error.
    pub fn repair_validated(
        &self,
        id: &ConnectionId,
        secret: &ProviderSecret,
    ) -> Result<ProviderConnection, ConnectionStoreError> {
        validate_secret(secret)?;
        let mut locked = self.lock(id)?;
        let mut connection = find_locked(&mut *locked, id)?;
        if connection.state != ConnectionState::PendingCredential {
            return Err(ConnectionStoreError::NotPending);
        }
        self.credentials
            .put(id, secret)
            .map_err(ConnectionStoreError::Credential)?;
        let ready_write = locked.mark_ready(id);
        reconcile_metadata_mutation(&mut *locked, ready_write)?;
        connection.state = ConnectionState::Ready;
        Ok(connection)
    }

    /// Replaces a credential for a ready record only after the caller has
    /// established that its existing credential is unavailable.
    ///
    /// The ready-state recheck is performed while holding the per-connection
    /// lock, so a concurrent metadata change cannot turn this repair into an
    /// unchecked replacement.
    pub fn repair_unavailable_ready_validated(
        &self,
        id: &ConnectionId,
        secret: &ProviderSecret,
    ) -> Result<ProviderConnection, ConnectionStoreError> {
        validate_secret(secret)?;
        let mut locked = self.lock(id)?;
        let connection = find_locked(&mut *locked, id)?;
        if connection.state != ConnectionState::Ready {
            return Err(ConnectionStoreError::NotReady);
        }
        if self
            .credentials
            .get(id)
            .map_err(ConnectionStoreError::Credential)?
            .is_some()
        {
            return Err(ConnectionStoreError::AlreadyExists);
        }
        self.credentials
            .put(id, secret)
            .map_err(ConnectionStoreError::Credential)?;
        Ok(connection)
    }

    /// Validates a replacement before locking, then replaces only a ready record's credential.
    ///
    /// Borrowing retains the validated candidate adapter as the credential's sole owner until
    /// the caller completes its in-memory active-configuration swap.
    pub fn replace_validated(
        &self,
        id: &ConnectionId,
        secret: &ProviderSecret,
    ) -> Result<(), ConnectionStoreError> {
        validate_secret(secret)?;
        let mut locked = self.lock(id)?;
        let connection = find_locked(&mut *locked, id)?;
        if connection.state != ConnectionState::Ready {
            return Err(ConnectionStoreError::NotReady);
        }
        self.credentials
            .put(id, secret)
            .map_err(ConnectionStoreError::Credential)
    }

    /// Updates only the presentation label while holding the per-ID lock.
    ///
    /// A committed-but-indeterminate rename returns its durability error after reloading.
    pub fn rename(
        &self,
        id: &ConnectionId,
        label: Option<String>,
    ) -> Result<ProviderConnection, ConnectionStoreError> {
        let label = normalize_label(label).map_err(ConnectionStoreError::Validation)?;
        let mut locked = self.lock(id)?;
        let mut connection = find_locked(&mut *locked, id)?;
        let rename = locked.rename(id, label.clone());
        reconcile_metadata_mutation(&mut *locked, rename)?;
        connection.label = label;
        Ok(connection)
    }

    /// Assigns a connection's immutable configuration key exactly once.
    pub fn assign_key(
        &self,
        id: &ConnectionId,
        key: ConnectionKey,
    ) -> Result<ProviderConnection, ConnectionStoreError> {
        let mut locked = self.lock(id)?;
        let mut connection = find_locked(&mut *locked, id)?;
        if connection.key.is_some() {
            return Err(ConnectionStoreError::AlreadyExists);
        }
        let assign = locked.assign_key(id, key.clone());
        reconcile_metadata_mutation(&mut *locked, assign)?;
        connection.key = Some(key);
        Ok(connection)
    }

    /// Creates or updates the single managed ChatGPT subscription row.
    pub fn create_managed_subscription(
        &self,
        account_id: String,
        label: Option<String>,
    ) -> Result<ProviderConnection, ConnectionStoreError> {
        let policy = self.chatgpt_policy()?;
        validate_managed_subscription(&policy.chatgpt_auth_file, &account_id)
            .map_err(ConnectionStoreError::Validation)?;
        let label = normalize_label(label).map_err(ConnectionStoreError::Validation)?;
        let existing = self.list()?;
        if let Some(existing) = existing
            .iter()
            .find(|connection| connection.provider == ProviderKind::ChatGptSubscription)
        {
            return self.update_managed_account(&existing.id, account_id);
        }
        let id = ConnectionId::new_stored();
        let connection = ProviderConnection {
            id: id.clone(),
            provider: ProviderKind::ChatGptSubscription,
            label,
            key: None,
            base_url: None,
            authentication: ConnectionAuth::ChatGptSubscriptionManaged {
                auth_file: policy.chatgpt_auth_file.clone(),
                account_id: account_id.clone(),
            },
            state: ConnectionState::Ready,
        };
        connection
            .validate_persisted()
            .map_err(ConnectionStoreError::Validation)?;
        let mut locked = self.lock(&id)?;
        let loaded = locked.load().map_err(ConnectionStoreError::Metadata)?;
        if let Some(found) = loaded
            .iter()
            .find(|connection| connection.provider == ProviderKind::ChatGptSubscription)
        {
            let found_id = found.id.clone();
            drop(locked);
            return self.update_managed_account(&found_id, account_id);
        }
        let upsert = locked.upsert_ready(connection.clone());
        match reconcile_metadata_mutation(&mut *locked, upsert) {
            Ok(()) => Ok(connection),
            Err(ConnectionStoreError::Metadata(RegistryError::InvalidConnection)) => {
                drop(locked);
                if let Some(found) = self
                    .list()?
                    .into_iter()
                    .find(|connection| connection.provider == ProviderKind::ChatGptSubscription)
                {
                    self.update_managed_account(&found.id, account_id)
                } else {
                    Err(ConnectionStoreError::Metadata(
                        RegistryError::InvalidConnection,
                    ))
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn update_managed_account(
        &self,
        id: &ConnectionId,
        account_id: String,
    ) -> Result<ProviderConnection, ConnectionStoreError> {
        let policy = self.chatgpt_policy()?;
        validate_managed_subscription(&policy.chatgpt_auth_file, &account_id)
            .map_err(ConnectionStoreError::Validation)?;
        let mut locked = self.lock(id)?;
        let mut connection = find_locked(&mut *locked, id)?;
        match &mut connection.authentication {
            ConnectionAuth::ChatGptSubscriptionManaged {
                auth_file,
                account_id: stored,
            } => {
                auth_file.clone_from(&policy.chatgpt_auth_file);
                *stored = account_id;
            }
            _ => {
                return Err(ConnectionStoreError::Validation(
                    ValidationError::UnsupportedProvider,
                ));
            }
        }
        connection
            .validate_persisted()
            .map_err(ConnectionStoreError::Validation)?;
        let upsert = locked.upsert_ready(connection.clone());
        reconcile_metadata_mutation(&mut *locked, upsert)?;
        Ok(connection)
    }

    /// Deletes the credential before removing metadata while holding the per-ID lock.
    ///
    /// A committed-but-indeterminate removal returns its durability error after reloading.
    pub fn remove(&self, id: &ConnectionId) -> Result<(), ConnectionStoreError> {
        let mut locked = self.lock(id)?;
        let _connection = find_locked(&mut *locked, id)?;
        match self.credentials.remove(id) {
            Ok(()) | Err(CredentialError::Missing) => {}
            Err(error) => return Err(ConnectionStoreError::Credential(error)),
        }
        let removal = locked.remove(id);
        reconcile_metadata_mutation(&mut *locked, removal)
    }

    fn lock(
        &self,
        id: &ConnectionId,
    ) -> Result<Box<dyn LockedConnectionMetadata>, ConnectionStoreError> {
        id.validate_stored()
            .map_err(ConnectionStoreError::Validation)?;
        self.metadata
            .lock_connection(id)
            .map_err(ConnectionStoreError::Metadata)
    }
}

fn find_locked(
    locked: &mut dyn LockedConnectionMetadata,
    id: &ConnectionId,
) -> Result<ProviderConnection, ConnectionStoreError> {
    locked
        .load()
        .map_err(ConnectionStoreError::Metadata)?
        .into_iter()
        .find(|connection| connection.id == *id)
        .ok_or(ConnectionStoreError::NotFound)
}

fn default_chatgpt_auth_file() -> Result<PathBuf, ValidationError> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".yach/auth/chatgpt-subscription.json"))
        .ok_or(ValidationError::HomeDirectoryMissing)
}

/// Retains the post-rename outcome even when reloading the visible registry fails.
fn reconcile_metadata_mutation(
    locked: &mut dyn LockedConnectionMetadata,
    mutation: Result<(), RegistryError>,
) -> Result<(), ConnectionStoreError> {
    match mutation {
        Ok(()) => Ok(()),
        Err(RegistryError::DurabilityUnknown) => {
            let _ = locked.load();
            Err(ConnectionStoreError::Metadata(
                RegistryError::DurabilityUnknown,
            ))
        }
        Err(error) => Err(ConnectionStoreError::Metadata(error)),
    }
}

fn validate_secret(secret: &ProviderSecret) -> Result<(), ConnectionStoreError> {
    if secret.is_empty() {
        return Err(ConnectionStoreError::Validation(
            ValidationError::EmptySecret,
        ));
    }
    Ok(())
}

pub(crate) fn normalize_label(label: Option<String>) -> Result<Option<String>, ValidationError> {
    let label = label
        .map(|label| label.trim().to_owned())
        .filter(|label| !label.is_empty());
    if label
        .as_deref()
        .is_some_and(|label| label.chars().count() > 80)
    {
        return Err(ValidationError::LabelTooLong);
    }
    Ok(label)
}

fn normalize_base_url(value: &str) -> Result<String, ValidationError> {
    let value = value.trim();
    if value.len() > 2_048 {
        return Err(ValidationError::BaseUrlTooLong);
    }
    let url = Url::parse(value).map_err(|_| ValidationError::InvalidBaseUrl)?;

    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ValidationError::InvalidBaseUrl);
    }
    let normalized = url.as_str().trim_end_matches('/').to_owned();
    if normalized.is_empty() || normalized.len() > 2_048 {
        return Err(ValidationError::BaseUrlTooLong);
    }
    Ok(normalized)
}

const MAX_ACCOUNT_ID_CHARS: usize = 128;

/// Injected ChatGPT subscription filesystem policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionPolicy {
    /// Logical auth-file path stamped onto managed subscription rows.
    pub chatgpt_auth_file: PathBuf,
}

impl ConnectionPolicy {
    /// Resolves `~/.yach/auth/chatgpt-subscription.json` from HOME.
    pub fn user_default() -> Result<Self, ValidationError> {
        Ok(Self {
            chatgpt_auth_file: default_chatgpt_auth_file()?,
        })
    }
}

fn validate_managed_subscription(
    auth_file: &Path,
    account_id: &str,
) -> Result<(), ValidationError> {
    if auth_file.as_os_str().is_empty() || auth_file.file_name().is_none() {
        return Err(ValidationError::InvalidAuthFile);
    }
    if account_id.is_empty() || account_id.chars().count() > MAX_ACCOUNT_ID_CHARS {
        return Err(ValidationError::InvalidAccountId);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        sync::{
            Arc,
            mpsc::{Receiver, SyncSender, sync_channel},
        },
        thread,
    };

    use parking_lot::Mutex;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

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

    struct TemporaryRegistry {
        directory: PathBuf,
    }

    impl TemporaryRegistry {
        fn new() -> Self {
            let directory = std::env::temp_dir()
                .join(format!("yach-connections-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&directory).test_unwrap();
            Self { directory }
        }

        fn registry_path(&self) -> PathBuf {
            self.directory.join("connections.json")
        }
    }

    impl Drop for TemporaryRegistry {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn metadata_store(temporary: &TemporaryRegistry) -> JsonConnectionMetadataStore {
        JsonConnectionMetadataStore::new(temporary.registry_path())
    }

    fn stored_connection(
        id: ConnectionId,
        label: Option<&str>,
        state: ConnectionState,
    ) -> ProviderConnection {
        ProviderConnection::stored(
            id,
            ProviderKind::OpenAi,
            label.map(str::to_owned),
            None,
            state,
        )
        .test_unwrap()
    }

    fn draft() -> NewConnectionDraft {
        NewConnectionDraft::new(ProviderKind::OpenAi, Some("primary".to_owned()), None)
            .test_unwrap()
    }

    fn secret(value: &str) -> ProviderSecret {
        ProviderSecret::new(value.to_owned())
    }

    fn write_pending(metadata: &dyn ConnectionMetadataStore, mut connection: ProviderConnection) {
        connection.state = ConnectionState::PendingCredential;
        let mut lock = metadata.lock_connection(&connection.id).test_unwrap();
        lock.create_pending(connection).test_unwrap();
    }

    #[test]
    fn stored_ids_are_generated_as_uuids_and_reject_reserved_or_invalid_values() {
        let id = ConnectionId::new_stored();
        assert!(uuid::Uuid::parse_str(id.as_str()).is_ok());
        assert!(ConnectionId::parse_stored(id.as_str()).is_ok());
        assert!(ConnectionId::parse_stored("environment").is_err());
        assert!(ConnectionId::parse_stored("not-a-uuid").is_err());
    }

    #[test]
    fn connection_id_deserialization_rejects_noncanonical_values_and_short_duplicate_display_ids() {
        let canonical = r#""00000000-0000-4000-8000-000000000001""#;
        assert_eq!(
            serde_json::from_str::<ConnectionId>(canonical)
                .test_unwrap()
                .as_str(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(
            serde_json::from_str::<ConnectionId>(r#""environment""#)
                .test_unwrap()
                .validate_stored(),
            Err(ValidationError::InvalidStoredId)
        );
        for invalid in [
            r#""short""#,
            r#""00000000000040008000000000000001""#,
            r#""00000000-0000-4000-8000-00000000000A""#,
        ] {
            assert!(serde_json::from_str::<ConnectionId>(invalid).is_err());
        }

        let first = ProviderConnection {
            id: ConnectionId("short".to_owned()),
            provider: ProviderKind::OpenAi,
            label: Some("duplicate".to_owned()),
            key: None,
            base_url: None,
            authentication: ConnectionAuth::ApiKey {
                source: CredentialSource::System,
            },
            state: ConnectionState::Ready,
        };
        let second = ProviderConnection {
            id: ConnectionId("other".to_owned()),
            ..first.clone()
        };
        let all = [first.clone(), second];
        assert_eq!(first.display_label(&all), "duplicate (OpenAI, short)");
    }

    #[test]
    fn registry_rejects_transient_authentication() {
        let temporary = TemporaryRegistry::new();
        let path = temporary.registry_path();
        let contents = r#"{
  "schema": "yach.connections.v1",
  "connections": [{
    "id": "00000000-0000-4000-8000-000000000001",
    "provider": "openai",
    "authentication": { "kind": "api_key", "source": "environment" },
    "state": "ready"
  }]
}"#;
        fs::write(&path, contents).test_unwrap();

        let store = metadata_store(&temporary);
        assert!(matches!(
            store.load(),
            Err(RegistryError::InvalidConnection)
        ));
        assert_eq!(fs::read_to_string(path).test_unwrap(), contents);
    }

    #[test]
    fn registry_rejects_environment_shaped_chatgpt_subscription() {
        let temporary = TemporaryRegistry::new();
        let path = temporary.registry_path();
        let contents = r#"{
  "schema": "yach.connections.v1",
  "connections": [{
    "id": "00000000-0000-4000-8000-000000000001",
    "provider": "openai-codex",
    "authentication": { "kind": "chatgpt_subscription", "token_dir": "/tmp/tokens" },
    "state": "ready"
  }]
}"#;
        fs::write(&path, contents).test_unwrap();
        assert!(matches!(
            metadata_store(&temporary).load(),
            Err(RegistryError::InvalidConnection)
        ));
    }

    #[test]
    fn create_managed_subscription_stamps_policy_path_and_reuses_id() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(metadata_store(&temporary));
        let credentials = Arc::new(TestCredentials::default());
        let auth_file = temporary.directory.join("chatgpt-subscription.json");
        let store = ProviderConnectionStore::with_policy(
            metadata,
            credentials,
            ConnectionPolicy {
                chatgpt_auth_file: auth_file.clone(),
            },
        );
        let first = store
            .create_managed_subscription(String::from("acct_a"), Some(String::from("ChatGPT")))
            .test_unwrap();
        assert_eq!(first.provider, ProviderKind::ChatGptSubscription);
        assert_eq!(first.state, ConnectionState::Ready);
        let ConnectionAuth::ChatGptSubscriptionManaged {
            auth_file: stored,
            account_id,
        } = &first.authentication
        else {
            unreachable!("expected managed auth");
        };
        assert_eq!(stored, &auth_file);
        assert_eq!(account_id, "acct_a");
        assert_eq!(store.list().test_unwrap().len(), 1);
        let second = store
            .create_managed_subscription(String::from("acct_b"), None)
            .test_unwrap();
        assert_eq!(second.id, first.id);
        let ConnectionAuth::ChatGptSubscriptionManaged { account_id, .. } = second.authentication
        else {
            unreachable!("expected managed auth");
        };
        assert_eq!(account_id, "acct_b");
        assert_eq!(store.list().test_unwrap().len(), 1);
    }

    #[test]
    fn user_default_policy_uses_home_yach_auth_file() {
        let policy = ConnectionPolicy::user_default().test_unwrap();
        assert!(
            policy
                .chatgpt_auth_file
                .ends_with(".yach/auth/chatgpt-subscription.json")
        );
        assert!(!policy.chatgpt_auth_file.starts_with("."));
    }

    #[test]
    fn managed_subscription_rejects_empty_account_id() {
        let temporary = TemporaryRegistry::new();
        let store = ProviderConnectionStore::with_policy(
            Arc::new(metadata_store(&temporary)),
            Arc::new(TestCredentials::default()),
            ConnectionPolicy {
                chatgpt_auth_file: temporary.directory.join("chatgpt-subscription.json"),
            },
        );
        assert!(matches!(
            store.create_managed_subscription(String::new(), None),
            Err(ConnectionStoreError::Validation(
                ValidationError::InvalidAccountId
            ))
        ));
    }

    #[test]
    fn registry_round_trips_pending_and_ready_system_key_records() {
        let temporary = TemporaryRegistry::new();
        let store = metadata_store(&temporary);
        let pending = stored_connection(
            ConnectionId::new_stored(),
            Some("pending"),
            ConnectionState::PendingCredential,
        );
        let ready = stored_connection(
            ConnectionId::new_stored(),
            Some("ready"),
            ConnectionState::Ready,
        );

        write_pending(&store, pending.clone());
        write_pending(&store, ready.clone());
        let mut ready_lock = store.lock_connection(&ready.id).test_unwrap();
        ready_lock.mark_ready(&ready.id).test_unwrap();

        let mut expected = vec![pending, ready];
        expected.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(store.load().test_unwrap(), expected);
    }

    #[test]
    fn registry_load_sorts_valid_hand_written_records_without_rewriting_them() {
        let temporary = TemporaryRegistry::new();
        let path = temporary.registry_path();
        let contents = r#"{
  "schema": "yach.connections.v1",
  "connections": [
    {
      "id": "00000000-0000-4000-8000-000000000002",
      "provider": "openai",
      "authentication": { "kind": "api_key", "source": "system" },
      "state": "ready"
    },
    {
      "id": "00000000-0000-4000-8000-000000000001",
      "provider": "openai",
      "authentication": { "kind": "api_key", "source": "system" },
      "state": "ready"
    }
  ]
}"#;
        fs::write(&path, contents).test_unwrap();

        let loaded = metadata_store(&temporary).load().test_unwrap();
        assert_eq!(
            loaded
                .iter()
                .map(|connection| connection.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "00000000-0000-4000-8000-000000000001",
                "00000000-0000-4000-8000-000000000002",
            ]
        );
        assert_eq!(fs::read_to_string(path).test_unwrap(), contents);
    }

    #[test]
    fn missing_registry_is_empty_and_malformed_registry_is_preserved() {
        let temporary = TemporaryRegistry::new();
        let path = temporary.registry_path();
        let store = metadata_store(&temporary);
        assert!(store.load().test_unwrap().is_empty());

        let malformed = "{ malformed";
        fs::write(&path, malformed).test_unwrap();
        let connection = stored_connection(
            ConnectionId::new_stored(),
            Some("ignored"),
            ConnectionState::PendingCredential,
        );
        let mut lock = store.lock_connection(&connection.id).test_unwrap();
        assert!(matches!(
            lock.create_pending(connection),
            Err(RegistryError::Malformed)
        ));
        assert_eq!(fs::read_to_string(path).test_unwrap(), malformed);
    }

    #[test]
    fn locked_metadata_rejects_foreign_id_mutations_without_touching_foreign_record() {
        let temporary = TemporaryRegistry::new();
        let store = metadata_store(&temporary);
        let locked_id = ConnectionId::new_stored();
        let foreign = stored_connection(
            ConnectionId::new_stored(),
            Some("foreign"),
            ConnectionState::PendingCredential,
        );
        let untracked = stored_connection(
            ConnectionId::new_stored(),
            Some("untracked"),
            ConnectionState::PendingCredential,
        );
        write_pending(&store, foreign.clone());

        let mut locked = store.lock_connection(&locked_id).test_unwrap();
        assert!(matches!(
            locked.create_pending(untracked),
            Err(RegistryError::InvalidConnection)
        ));
        assert!(matches!(
            locked.mark_ready(&foreign.id),
            Err(RegistryError::InvalidConnection)
        ));
        assert!(matches!(
            locked.rename(&foreign.id, Some("renamed".to_owned())),
            Err(RegistryError::InvalidConnection)
        ));
        assert!(matches!(
            locked.remove(&foreign.id),
            Err(RegistryError::InvalidConnection)
        ));
        drop(locked);

        assert_eq!(store.load().test_unwrap(), vec![foreign]);
    }

    #[test]
    fn labels_trim_whitespace_and_cap_unicode_scalars() {
        let draft =
            NewConnectionDraft::new(ProviderKind::OpenAi, Some("  work  ".to_owned()), None)
                .test_unwrap();
        assert_eq!(draft.label(), Some("work"));

        let too_long = "🦀".repeat(81);
        assert!(matches!(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some(too_long), None),
            Err(ValidationError::LabelTooLong)
        ));
        assert_eq!(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some("   ".to_owned()), None)
                .test_unwrap()
                .label(),
            None
        );
    }

    #[test]
    fn draft_exposes_its_validated_provider_without_exposing_credentials() {
        let draft = NewConnectionDraft::new(
            ProviderKind::OpenAiCompatible,
            Some(String::from("gateway")),
            Some(String::from("https://gateway.example/v1")),
        )
        .test_unwrap();

        assert_eq!(draft.provider(), ProviderKind::OpenAiCompatible);
    }

    #[test]
    fn compatible_base_urls_remove_trailing_slashes_without_inventing_a_path() {
        let endpoint = NewConnectionDraft::new(
            ProviderKind::OpenAiCompatible,
            None,
            Some("https://gateway.example/v1///".to_owned()),
        )
        .test_unwrap();
        assert_eq!(endpoint.base_url(), Some("https://gateway.example/v1"));

        let origin = NewConnectionDraft::new(
            ProviderKind::OpenAiCompatible,
            None,
            Some("https://gateway.example/".to_owned()),
        )
        .test_unwrap();
        assert_eq!(origin.base_url(), Some("https://gateway.example"));
    }

    #[test]
    fn base_urls_reject_unsafe_or_unsupported_components() {
        for input in [
            "ftp://gateway.example",
            "https://user@gateway.example",
            "https://gateway.example?token=unsafe",
            "https://gateway.example#fragment",
            &format!("https://gateway.example/{}", "a".repeat(2_049)),
        ] {
            assert!(
                NewConnectionDraft::new(
                    ProviderKind::OpenAiCompatible,
                    None,
                    Some(input.to_owned()),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn duplicate_labels_include_provider_and_stable_short_id() {
        let first = stored_connection(
            ConnectionId::parse_stored("11111111-1111-4111-8111-111111111111").test_unwrap(),
            Some("work"),
            ConnectionState::Ready,
        );
        let second = stored_connection(
            ConnectionId::parse_stored("22222222-2222-4222-8222-222222222222").test_unwrap(),
            Some("work"),
            ConnectionState::Ready,
        );
        let all = [first.clone(), second.clone()];

        assert_eq!(first.display_label(&all), "work (OpenAI, 11111111)");
        assert_eq!(second.display_label(&all), "work (OpenAI, 22222222)");
    }

    #[test]
    fn effective_label_collisions_suffix_every_row_with_a_unique_stable_prefix() {
        let unlabeled_first = stored_connection(
            ConnectionId::parse_stored("11111111-1111-4111-8111-111111111111").test_unwrap(),
            None,
            ConnectionState::Ready,
        );
        let unlabeled_second = stored_connection(
            ConnectionId::parse_stored("22222222-2222-4222-8222-222222222222").test_unwrap(),
            None,
            ConnectionState::Ready,
        );
        let unlabeled = [unlabeled_first.clone(), unlabeled_second.clone()];
        assert_eq!(
            unlabeled_first.display_label(&unlabeled),
            "OpenAI (OpenAI, 11111111)"
        );
        assert_eq!(
            unlabeled_second.display_label(&unlabeled),
            "OpenAI (OpenAI, 22222222)"
        );

        let explicit_provider_name = stored_connection(
            ConnectionId::parse_stored("33333333-3333-4333-8333-333333333333").test_unwrap(),
            Some("OpenAI"),
            ConnectionState::Ready,
        );
        let mixed = [unlabeled_first.clone(), explicit_provider_name.clone()];
        assert_eq!(
            unlabeled_first.display_label(&mixed),
            "OpenAI (OpenAI, 11111111)"
        );
        assert_eq!(
            explicit_provider_name.display_label(&mixed),
            "OpenAI (OpenAI, 33333333)"
        );

        let shared_prefix_first = stored_connection(
            ConnectionId::parse_stored("aaaaaaaa-1000-4000-8000-000000000001").test_unwrap(),
            Some("same-prefix"),
            ConnectionState::Ready,
        );
        let shared_prefix_second = stored_connection(
            ConnectionId::parse_stored("aaaaaaaa-2000-4000-8000-000000000002").test_unwrap(),
            Some("same-prefix"),
            ConnectionState::Ready,
        );
        let shared_prefix = [shared_prefix_first.clone(), shared_prefix_second.clone()];
        assert_eq!(
            shared_prefix_first.display_label(&shared_prefix),
            "same-prefix (OpenAI, aaaaaaaa-1)"
        );
        assert_eq!(
            shared_prefix_second.display_label(&shared_prefix),
            "same-prefix (OpenAI, aaaaaaaa-2)"
        );
    }

    #[test]
    fn registry_rejects_a_sixty_fifth_connection() {
        let temporary = TemporaryRegistry::new();
        let store = metadata_store(&temporary);
        for _ in 0..64 {
            let connection = stored_connection(
                ConnectionId::new_stored(),
                None,
                ConnectionState::PendingCredential,
            );
            write_pending(&store, connection);
        }

        let extra = stored_connection(
            ConnectionId::new_stored(),
            None,
            ConnectionState::PendingCredential,
        );
        let mut lock = store.lock_connection(&extra.id).test_unwrap();
        assert!(matches!(
            lock.create_pending(extra),
            Err(RegistryError::CapacityExceeded)
        ));
    }

    #[test]
    fn registry_rejects_files_with_more_than_sixty_four_connections() {
        let temporary = TemporaryRegistry::new();
        let connections = (0..65)
            .map(|_| stored_connection(ConnectionId::new_stored(), None, ConnectionState::Ready))
            .collect::<Vec<_>>();
        fs::write(
            temporary.registry_path(),
            serde_json::json!({
                "schema": "yach.connections.v1",
                "connections": connections,
            })
            .to_string(),
        )
        .test_unwrap();

        assert!(matches!(
            metadata_store(&temporary).load(),
            Err(RegistryError::CapacityExceeded)
        ));
    }

    #[test]
    fn provider_secrets_redact_debug_and_only_expose_by_consuming_ownership() {
        let sentinel = "test-only-secret";
        let provider_secret = secret(sentinel);
        assert_eq!(format!("{provider_secret:?}"), "[REDACTED]");
        assert_eq!(provider_secret.into_inner(), sentinel);
    }

    #[test]
    fn errors_display_bounded_secret_free_messages() {
        for error in [
            ConnectionStoreError::NotFound,
            ConnectionStoreError::NotPending,
            ConnectionStoreError::Validation(ValidationError::EmptySecret),
            ConnectionStoreError::Credential(CredentialError::Unavailable),
            ConnectionStoreError::Metadata(RegistryError::Malformed),
        ] {
            assert!(!error.to_string().contains("secret"));
            assert!(!error.to_string().contains("test-only-secret"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn registry_writes_user_only_permissions() {
        let temporary = TemporaryRegistry::new();
        let store = metadata_store(&temporary);
        write_pending(
            &store,
            stored_connection(
                ConnectionId::new_stored(),
                None,
                ConnectionState::PendingCredential,
            ),
        );

        let mode = fs::metadata(temporary.registry_path())
            .test_unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn post_rename_sync_failure_is_indeterminate_without_credential_or_metadata_lies() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(JsonConnectionMetadataStore::with_directory_sync_error(
            temporary.registry_path(),
            std::io::ErrorKind::Other,
        ));
        let credentials = Arc::new(TestCredentials::default());
        let store = service(Arc::clone(&metadata), Arc::clone(&credentials));

        assert!(matches!(
            store.create_validated(draft(), &secret("one")),
            CreateConnectionOutcome::FailedBeforePending(ConnectionStoreError::Metadata(
                RegistryError::DurabilityUnknown
            ))
        ));
        assert_eq!(credentials.put_calls(), 0);
        let pending = metadata.load().test_unwrap().pop().test_unwrap();
        assert_eq!(pending.state, ConnectionState::PendingCredential);

        assert!(matches!(
            store.repair_validated(&pending.id, &secret("two")),
            Err(ConnectionStoreError::Metadata(
                RegistryError::DurabilityUnknown
            ))
        ));
        assert!(credentials.contains(&pending.id));
        assert_eq!(
            metadata.load().test_unwrap().pop().test_unwrap().state,
            ConnectionState::Ready
        );

        assert!(matches!(
            store.remove(&pending.id),
            Err(ConnectionStoreError::Metadata(
                RegistryError::DurabilityUnknown
            ))
        ));
        assert!(!credentials.contains(&pending.id));
        assert!(metadata.load().test_unwrap().is_empty());

        let unsupported = JsonConnectionMetadataStore::with_directory_sync_error(
            temporary.directory.join("unsupported.json"),
            std::io::ErrorKind::Unsupported,
        );
        let connection = stored_connection(
            ConnectionId::new_stored(),
            None,
            ConnectionState::PendingCredential,
        );
        let mut lock = unsupported.lock_connection(&connection.id).test_unwrap();
        assert!(lock.create_pending(connection).is_ok());
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum MetadataOperation {
        CreatePending,
        MarkReady,
        Remove,
    }

    struct FailingMetadataStore {
        inner: Arc<dyn ConnectionMetadataStore>,
        failures: Arc<Mutex<HashMap<MetadataOperation, RegistryError>>>,
    }

    impl FailingMetadataStore {
        fn new(inner: Arc<dyn ConnectionMetadataStore>) -> Self {
            Self {
                inner,
                failures: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        fn fail_next(&self, operation: MetadataOperation, error: RegistryError) {
            self.failures.lock().insert(operation, error);
        }
    }

    impl ConnectionMetadataStore for FailingMetadataStore {
        fn load(&self) -> Result<Vec<ProviderConnection>, RegistryError> {
            self.inner.load()
        }

        fn lock_connection(
            &self,
            id: &ConnectionId,
        ) -> Result<Box<dyn LockedConnectionMetadata>, RegistryError> {
            Ok(Box::new(FailingMetadataLock {
                inner: self.inner.lock_connection(id)?,
                failures: Arc::clone(&self.failures),
            }))
        }
    }

    struct FailingMetadataLock {
        inner: Box<dyn LockedConnectionMetadata>,
        failures: Arc<Mutex<HashMap<MetadataOperation, RegistryError>>>,
    }

    impl FailingMetadataLock {
        fn failure(&self, operation: MetadataOperation) -> Option<RegistryError> {
            self.failures.lock().remove(&operation)
        }
    }

    impl LockedConnectionMetadata for FailingMetadataLock {
        fn load(&mut self) -> Result<Vec<ProviderConnection>, RegistryError> {
            self.inner.load()
        }

        fn create_pending(&mut self, connection: ProviderConnection) -> Result<(), RegistryError> {
            if let Some(error) = self.failure(MetadataOperation::CreatePending) {
                return Err(error);
            }
            self.inner.create_pending(connection)
        }

        fn mark_ready(&mut self, id: &ConnectionId) -> Result<(), RegistryError> {
            if let Some(error) = self.failure(MetadataOperation::MarkReady) {
                return Err(error);
            }
            self.inner.mark_ready(id)
        }

        fn rename(
            &mut self,
            id: &ConnectionId,
            label: Option<String>,
        ) -> Result<(), RegistryError> {
            self.inner.rename(id, label)
        }

        fn assign_key(
            &mut self,
            id: &ConnectionId,
            key: ConnectionKey,
        ) -> Result<(), RegistryError> {
            self.inner.assign_key(id, key)
        }

        fn remove(&mut self, id: &ConnectionId) -> Result<(), RegistryError> {
            if let Some(error) = self.failure(MetadataOperation::Remove) {
                return Err(error);
            }
            self.inner.remove(id)
        }

        fn upsert_ready(&mut self, connection: ProviderConnection) -> Result<(), RegistryError> {
            self.inner.upsert_ready(connection)
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum CredentialOperation {
        Put,
        Remove,
    }

    struct CredentialGate {
        operation: CredentialOperation,
        entered: SyncSender<()>,
        resume: Receiver<()>,
    }

    #[derive(Default)]
    struct TestCredentialState {
        values: HashMap<ConnectionId, String>,
        put_calls: usize,
        remove_calls: usize,
        fail_put: Option<CredentialError>,
        fail_remove: Option<CredentialError>,
        gate: Option<CredentialGate>,
    }

    #[derive(Default)]
    struct TestCredentials {
        state: Mutex<TestCredentialState>,
    }

    impl TestCredentials {
        fn fail_next_put(&self, error: CredentialError) {
            self.state.lock().fail_put = Some(error);
        }

        fn block_next(&self, operation: CredentialOperation) -> (Receiver<()>, SyncSender<()>) {
            let (entered_sender, entered_receiver) = sync_channel(0);
            let (resume_sender, resume_receiver) = sync_channel(0);
            self.state.lock().gate = Some(CredentialGate {
                operation,
                entered: entered_sender,
                resume: resume_receiver,
            });
            (entered_receiver, resume_sender)
        }

        fn seed(&self, id: ConnectionId) {
            self.state.lock().values.insert(id, "existing".to_owned());
        }

        fn contains(&self, id: &ConnectionId) -> bool {
            self.state.lock().values.contains_key(id)
        }

        fn put_calls(&self) -> usize {
            self.state.lock().put_calls
        }

        fn pause_if_needed(&self, operation: CredentialOperation) {
            let gate = {
                let mut state = self.state.lock();
                match state.gate.as_ref() {
                    Some(gate) if gate.operation == operation => state.gate.take(),
                    _ => None,
                }
            };
            if let Some(gate) = gate {
                gate.entered.send(()).test_unwrap();
                gate.resume.recv().test_unwrap();
            }
        }
    }

    impl CredentialStore for TestCredentials {
        fn put(&self, id: &ConnectionId, _secret: &ProviderSecret) -> Result<(), CredentialError> {
            self.pause_if_needed(CredentialOperation::Put);
            let mut state = self.state.lock();
            state.put_calls += 1;
            if let Some(error) = state.fail_put.take() {
                return Err(error);
            }
            state.values.insert(id.clone(), "replacement".to_owned());
            Ok(())
        }

        fn get(&self, id: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError> {
            Ok(self
                .contains(id)
                .then(|| ProviderSecret::new("test-credential".to_owned())))
        }

        fn remove(&self, id: &ConnectionId) -> Result<(), CredentialError> {
            self.pause_if_needed(CredentialOperation::Remove);
            let mut state = self.state.lock();
            state.remove_calls += 1;
            if let Some(error) = state.fail_remove.take() {
                return Err(error);
            }
            state.values.remove(id);
            Ok(())
        }
    }

    fn service<M, C>(metadata: Arc<M>, credentials: Arc<C>) -> ProviderConnectionStore
    where
        M: ConnectionMetadataStore + 'static,
        C: CredentialStore + 'static,
    {
        ProviderConnectionStore::new(metadata, credentials)
    }

    #[test]
    fn pending_metadata_write_failure_does_not_write_a_credential() {
        let temporary = TemporaryRegistry::new();
        let actual = Arc::new(metadata_store(&temporary));
        let metadata = Arc::new(FailingMetadataStore::new(actual));
        metadata.fail_next(MetadataOperation::CreatePending, RegistryError::Io);
        let credentials = Arc::new(TestCredentials::default());

        assert!(matches!(
            service(metadata, Arc::clone(&credentials)).create_validated(draft(), &secret("one")),
            CreateConnectionOutcome::FailedBeforePending(ConnectionStoreError::Metadata(
                RegistryError::Io
            ))
        ));
        assert_eq!(credentials.put_calls(), 0);
    }

    #[test]
    fn credential_write_failure_leaves_visible_pending_metadata() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(metadata_store(&temporary));
        let credentials = Arc::new(TestCredentials::default());
        credentials.fail_next_put(CredentialError::Unavailable);

        assert!(matches!(
            service(Arc::clone(&metadata), Arc::clone(&credentials))
                .create_validated(draft(), &secret("one")),
            CreateConnectionOutcome::FailedAfterPending {
                error: ConnectionStoreError::Credential(CredentialError::Unavailable),
                ..
            }
        ));
        assert_eq!(
            metadata.load().test_unwrap()[0].state,
            ConnectionState::PendingCredential
        );
    }

    #[test]
    fn ready_write_failure_leaves_repairable_pending_record() {
        let temporary = TemporaryRegistry::new();
        let actual = Arc::new(metadata_store(&temporary));
        let metadata = Arc::new(FailingMetadataStore::new(actual.clone()));
        metadata.fail_next(MetadataOperation::MarkReady, RegistryError::Io);
        let credentials = Arc::new(TestCredentials::default());

        assert!(matches!(
            service(metadata, Arc::clone(&credentials)).create_validated(draft(), &secret("one")),
            CreateConnectionOutcome::FailedAfterPending {
                error: ConnectionStoreError::Metadata(RegistryError::Io),
                ..
            }
        ));
        let pending = actual.load().test_unwrap().pop().test_unwrap();
        assert_eq!(pending.state, ConnectionState::PendingCredential);
        assert!(credentials.contains(&pending.id));
    }

    #[test]
    fn repair_marks_the_same_pending_connection_ready() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(metadata_store(&temporary));
        let credentials = Arc::new(TestCredentials::default());
        let connection = stored_connection(
            ConnectionId::new_stored(),
            Some("repair"),
            ConnectionState::PendingCredential,
        );
        write_pending(&*metadata, connection.clone());

        let repaired = service(Arc::clone(&metadata), Arc::clone(&credentials))
            .repair_validated(&connection.id, &secret("one"))
            .test_unwrap();
        assert_eq!(repaired.id, connection.id);
        assert_eq!(repaired.state, ConnectionState::Ready);
        assert!(credentials.contains(&connection.id));
    }

    #[test]
    fn ready_repair_conflicts_without_overwriting_a_credential_restored_before_its_lock() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(metadata_store(&temporary));
        let credentials = Arc::new(TestCredentials::default());
        let connection = stored_connection(
            ConnectionId::new_stored(),
            Some("ready repair"),
            ConnectionState::Ready,
        );
        write_pending(&*metadata, connection.clone());
        let mut lock = metadata.lock_connection(&connection.id).test_unwrap();
        lock.mark_ready(&connection.id).test_unwrap();
        drop(lock);
        credentials.seed(connection.id.clone());

        assert!(matches!(
            service(Arc::clone(&metadata), Arc::clone(&credentials))
                .repair_unavailable_ready_validated(&connection.id, &secret("replacement")),
            Err(ConnectionStoreError::AlreadyExists)
        ));
        assert_eq!(credentials.put_calls(), 0);
        assert!(credentials.contains(&connection.id));
    }

    #[test]
    fn removal_keeps_unavailable_metadata_when_registry_remove_fails() {
        let temporary = TemporaryRegistry::new();
        let actual = Arc::new(metadata_store(&temporary));
        let metadata = Arc::new(FailingMetadataStore::new(actual.clone()));
        let credentials = Arc::new(TestCredentials::default());
        let connection =
            stored_connection(ConnectionId::new_stored(), None, ConnectionState::Ready);
        write_pending(&*actual, connection.clone());
        let mut lock = actual.lock_connection(&connection.id).test_unwrap();
        lock.mark_ready(&connection.id).test_unwrap();
        drop(lock);
        credentials.seed(connection.id.clone());
        metadata.fail_next(MetadataOperation::Remove, RegistryError::Io);

        assert!(matches!(
            service(metadata, Arc::clone(&credentials)).remove(&connection.id),
            Err(ConnectionStoreError::Metadata(RegistryError::Io))
        ));
        assert_eq!(actual.load().test_unwrap(), vec![connection.clone()]);
        assert!(!credentials.contains(&connection.id));
    }

    #[test]
    fn failed_replacement_validation_never_writes_or_replaces_the_old_credential() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(metadata_store(&temporary));
        let credentials = Arc::new(TestCredentials::default());
        let connection =
            stored_connection(ConnectionId::new_stored(), None, ConnectionState::Ready);
        write_pending(&*metadata, connection.clone());
        let mut lock = metadata.lock_connection(&connection.id).test_unwrap();
        lock.mark_ready(&connection.id).test_unwrap();
        credentials.seed(connection.id.clone());

        assert!(matches!(
            service(Arc::clone(&metadata), Arc::clone(&credentials))
                .replace_validated(&connection.id, &secret("")),
            Err(ConnectionStoreError::Validation(
                ValidationError::EmptySecret
            ))
        ));
        assert_eq!(credentials.put_calls(), 0);
        assert!(credentials.contains(&connection.id));
    }

    #[test]
    fn replacement_borrows_the_validated_secret_without_consuming_it() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(metadata_store(&temporary));
        let credentials = Arc::new(TestCredentials::default());
        let connection =
            stored_connection(ConnectionId::new_stored(), None, ConnectionState::Ready);
        write_pending(&*metadata, connection.clone());
        let mut lock = metadata.lock_connection(&connection.id).test_unwrap();
        lock.mark_ready(&connection.id).test_unwrap();
        drop(lock);
        let replacement = secret("replacement");

        service(Arc::clone(&metadata), Arc::clone(&credentials))
            .replace_validated(&connection.id, &replacement)
            .test_unwrap();

        assert_eq!(replacement.with_exposed(str::to_owned), "replacement");
        assert_eq!(credentials.put_calls(), 1);
    }

    #[test]
    fn interleaved_replace_and_remove_leave_no_orphaned_credential_or_metadata() {
        let temporary = TemporaryRegistry::new();
        let path = temporary.registry_path();
        let first_metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let second_metadata = Arc::new(JsonConnectionMetadataStore::new(path));
        let credentials = Arc::new(TestCredentials::default());
        let connection =
            stored_connection(ConnectionId::new_stored(), None, ConnectionState::Ready);
        write_pending(&*first_metadata, connection.clone());
        let mut lock = first_metadata.lock_connection(&connection.id).test_unwrap();
        lock.mark_ready(&connection.id).test_unwrap();
        drop(lock);
        credentials.seed(connection.id.clone());
        let (entered, resume) = credentials.block_next(CredentialOperation::Put);

        let replace_store = service(Arc::clone(&first_metadata), Arc::clone(&credentials));
        let replace_id = connection.id.clone();
        let replace =
            thread::spawn(move || replace_store.replace_validated(&replace_id, &secret("new")));
        entered.recv().test_unwrap();

        let remove_store = service(Arc::clone(&second_metadata), Arc::clone(&credentials));
        let remove_id = connection.id.clone();
        let remove = thread::spawn(move || remove_store.remove(&remove_id));
        resume.send(()).test_unwrap();

        replace.join().test_unwrap().test_unwrap();
        remove.join().test_unwrap().test_unwrap();
        assert!(first_metadata.load().test_unwrap().is_empty());
        assert!(!credentials.contains(&connection.id));
    }

    #[test]
    fn interleaved_repair_and_remove_cannot_leave_ready_metadata_without_a_credential() {
        let temporary = TemporaryRegistry::new();
        let path = temporary.registry_path();
        let first_metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let second_metadata = Arc::new(JsonConnectionMetadataStore::new(path));
        let credentials = Arc::new(TestCredentials::default());
        let connection = stored_connection(
            ConnectionId::new_stored(),
            None,
            ConnectionState::PendingCredential,
        );
        write_pending(&*first_metadata, connection.clone());
        let (entered, resume) = credentials.block_next(CredentialOperation::Put);

        let repair_store = service(Arc::clone(&first_metadata), Arc::clone(&credentials));
        let repair_id = connection.id.clone();
        let repair =
            thread::spawn(move || repair_store.repair_validated(&repair_id, &secret("new")));
        entered.recv().test_unwrap();

        let remove_store = service(Arc::clone(&second_metadata), Arc::clone(&credentials));
        let remove_id = connection.id.clone();
        let remove = thread::spawn(move || remove_store.remove(&remove_id));
        resume.send(()).test_unwrap();

        repair.join().test_unwrap().test_unwrap();
        remove.join().test_unwrap().test_unwrap();
        assert!(first_metadata.load().test_unwrap().is_empty());
        assert!(!credentials.contains(&connection.id));
    }

    #[test]
    fn rename_rechecks_existence_after_waiting_for_an_interleaved_removal() {
        let temporary = TemporaryRegistry::new();
        let path = temporary.registry_path();
        let first_metadata = Arc::new(JsonConnectionMetadataStore::new(path.clone()));
        let second_metadata = Arc::new(JsonConnectionMetadataStore::new(path));
        let credentials = Arc::new(TestCredentials::default());
        let connection = stored_connection(
            ConnectionId::new_stored(),
            Some("before"),
            ConnectionState::Ready,
        );
        write_pending(&*first_metadata, connection.clone());
        let mut lock = first_metadata.lock_connection(&connection.id).test_unwrap();
        lock.mark_ready(&connection.id).test_unwrap();
        drop(lock);
        credentials.seed(connection.id.clone());
        let (entered, resume) = credentials.block_next(CredentialOperation::Remove);

        let remove_store = service(Arc::clone(&first_metadata), Arc::clone(&credentials));
        let remove_id = connection.id.clone();
        let remove = thread::spawn(move || remove_store.remove(&remove_id));
        entered.recv().test_unwrap();

        let rename_store = service(Arc::clone(&second_metadata), Arc::clone(&credentials));
        let rename_id = connection.id.clone();
        let rename =
            thread::spawn(move || rename_store.rename(&rename_id, Some("after".to_owned())));
        resume.send(()).test_unwrap();

        remove.join().test_unwrap().test_unwrap();
        assert!(matches!(
            rename.join().test_unwrap(),
            Err(ConnectionStoreError::NotFound)
        ));
        assert!(first_metadata.load().test_unwrap().is_empty());
    }

    #[test]
    fn connection_keys_are_immutable_unique_and_retired() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(temporary.registry_path()));
        let credentials = Arc::new(TestCredentials::default());
        let store = service(Arc::clone(&metadata), Arc::clone(&credentials));

        let CreateConnectionOutcome::Created(first) = store.create_validated(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some("First".to_owned()), None)
                .test_unwrap()
                .with_key(ConnectionKey::parse("work").test_unwrap()),
            &secret("first"),
        ) else {
            unreachable!("first keyed connection should be created");
        };
        let CreateConnectionOutcome::Created(second) = store.create_validated(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some("Second".to_owned()), None)
                .test_unwrap()
                .with_key(ConnectionKey::parse("home").test_unwrap()),
            &secret("second"),
        ) else {
            unreachable!("second keyed connection should be created");
        };
        assert_eq!(
            store.assign_key(&second.id, ConnectionKey::parse("other").test_unwrap()),
            Err(ConnectionStoreError::AlreadyExists)
        );
        store.remove(&first.id).test_unwrap();
        let reused = store.create_validated(
            NewConnectionDraft::new(ProviderKind::OpenAi, Some("Replacement".to_owned()), None)
                .test_unwrap()
                .with_key(ConnectionKey::parse("work").test_unwrap()),
            &secret("replacement"),
        );
        assert!(matches!(
            reused,
            CreateConnectionOutcome::FailedBeforePending(ConnectionStoreError::Metadata(
                RegistryError::InvalidConnection
            ))
        ));
    }

    #[test]
    fn second_same_provider_connection_requires_keys() {
        let temporary = TemporaryRegistry::new();
        let metadata = Arc::new(JsonConnectionMetadataStore::new(temporary.registry_path()));
        let credentials = Arc::new(TestCredentials::default());
        let store = service(metadata, credentials);
        assert!(matches!(
            store.create_validated(draft(), &secret("first")),
            CreateConnectionOutcome::Created(_)
        ));
        assert!(matches!(
            store.create_validated(
                NewConnectionDraft::new(ProviderKind::OpenAi, Some("Second".to_owned()), None)
                    .test_unwrap()
                    .with_key(ConnectionKey::parse("second").test_unwrap()),
                &secret("second"),
            ),
            CreateConnectionOutcome::FailedBeforePending(ConnectionStoreError::Validation(
                ValidationError::ConnectionKeyRequired
            ))
        ));
    }
}
