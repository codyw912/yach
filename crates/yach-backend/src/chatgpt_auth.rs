//! ChatGPT subscription login, logout, and activation helpers.

use std::path::PathBuf;

use yach_connections::{
    AuthFileProblem, ConnectionAuth, ConnectionPolicy, ProviderConnection, ProviderConnectionStore,
    prepare_chatgpt_auth_file,
};

use crate::provider_connections::{ConnectionRuntimeFailure, DeviceCodeCallback};
use crate::rig_adapter::{MaxTokensParam, RigProviderAdapterConfig, RigProviderConfig};

/// Metadata-only probe token. Debug is redacted; never serialized.
#[derive(Clone, PartialEq, Eq)]
pub struct ChatGptAuthEntry(rig::providers::chatgpt::auth::AuthEntryToken);

impl std::fmt::Debug for ChatGptAuthEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl ChatGptAuthEntry {
    #[cfg(test)]
    pub fn test_dummy() -> Self {
        use rig::providers::chatgpt::auth::{AuthEntryToken, AuthFileType, FileIdentity};
        Self(AuthEntryToken {
            resolved_path: PathBuf::from("/tmp/chatgpt-subscription.json"),
            file_type: AuthFileType::Regular,
            identity: FileIdentity {
                device: 0,
                inode: 0,
            },
            mtime: None,
            ctime: None,
            size: 0,
        })
    }
}

/// Persist a managed ChatGPT row after a successful probe or device login.
pub fn persist_managed_chatgpt(
    store: &ProviderConnectionStore,
    account_id: String,
    label: Option<String>,
) -> Result<(), ConnectionRuntimeFailure> {
    store
        .create_managed_subscription(account_id, label)
        .map(|_| ())
        .map_err(store_failure)
}

/// Adopt an existing auth file without starting device flow.
pub async fn adopt_existing_chatgpt_login(
    auth_file: PathBuf,
) -> Result<(String, ChatGptAuthEntry), ConnectionRuntimeFailure> {
    match prepare_chatgpt_auth_file(&auth_file) {
        Ok(_) => {}
        Err(AuthFileProblem::Symlink | AuthFileProblem::NonRegular) => {
            return Err(ConnectionRuntimeFailure::Conflict);
        }
    }
    let authorized = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ConnectionRuntimeFailure::Unavailable)?;
        runtime.block_on(async move {
            let guard = rig::providers::chatgpt::auth::AuthFileGuard::acquire(&auth_file)
                .map_err(|_| ConnectionRuntimeFailure::Conflict)?;
            guard
                .authorize_account()
                .await
                .map_err(|error| map_auth_error(&error))
        })
    })
    .await
    .map_err(|_| ConnectionRuntimeFailure::Unavailable)??;
    let account_id = authorized
        .account_id
        .filter(|id| !id.is_empty())
        .ok_or(ConnectionRuntimeFailure::Authentication)?;
    Ok((account_id, ChatGptAuthEntry(authorized.entry)))
}

/// Inspect the policy auth file without starting device flow.
pub async fn probe_chatgpt_subscription() -> crate::ChatGptProbeOutcome {
    let Ok(policy) = ConnectionPolicy::user_default() else {
        return crate::ChatGptProbeOutcome::Unusable(ConnectionRuntimeFailure::Validation);
    };
    match prepare_chatgpt_auth_file(&policy.chatgpt_auth_file) {
        Ok(yach_connections::AuthFilePreparation::Missing) => crate::ChatGptProbeOutcome::Missing,
        Ok(_) => match adopt_existing_chatgpt_login(policy.chatgpt_auth_file).await {
            Ok((account_id, entry)) => crate::ChatGptProbeOutcome::Existing { account_id, entry },
            Err(failure) => crate::ChatGptProbeOutcome::Unusable(failure),
        },
        Err(AuthFileProblem::Symlink | AuthFileProblem::NonRegular) => {
            crate::ChatGptProbeOutcome::Unusable(ConnectionRuntimeFailure::Conflict)
        }
    }
}

/// Start device flow only when the auth file is absent.
pub async fn start_chatgpt_device_login(
    auth_file: PathBuf,
    on_device_code: Option<DeviceCodeCallback>,
) -> Result<String, ConnectionRuntimeFailure> {
    let handler = match on_device_code {
        Some(callback) => rig::providers::chatgpt::auth::DeviceCodeHandler::new(
            move |prompt: rig::providers::chatgpt::auth::DeviceCodePrompt| {
                callback(prompt.verification_uri, prompt.user_code);
            },
        ),
        None => rig::providers::chatgpt::auth::DeviceCodeHandler::default(),
    };
    let authenticator = rig::providers::chatgpt::auth::Authenticator::new(
        rig::providers::chatgpt::auth::AuthSource::OAuth,
        Some(auth_file),
        handler,
        true,
        None,
    );
    let completion = authenticator
        .login_device_flow_expecting(rig::providers::chatgpt::auth::ExpectedAuthEntry::Absent)
        .await
        .map_err(|error| map_auth_error(&error))?;
    Ok(completion.account_id)
}
/// Adopt an existing login after the user confirms it.
pub async fn adopt_chatgpt_subscription(
    store: &ProviderConnectionStore,
    label: Option<String>,
    entry: ChatGptAuthEntry,
) -> Result<(), ConnectionRuntimeFailure> {
    let policy =
        ConnectionPolicy::user_default().map_err(|_| ConnectionRuntimeFailure::Validation)?;
    let auth_file = policy.chatgpt_auth_file;
    match prepare_chatgpt_auth_file(&auth_file) {
        Ok(_) => {}
        Err(AuthFileProblem::Symlink | AuthFileProblem::NonRegular) => {
            return Err(ConnectionRuntimeFailure::Conflict);
        }
    }
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ConnectionRuntimeFailure::Unavailable)?;
        runtime.block_on(async move {
            let guard = rig::providers::chatgpt::auth::AuthFileGuard::acquire(&auth_file)
                .map_err(|_| ConnectionRuntimeFailure::Conflict)?;
            let current = guard
                .stat()
                .map_err(|_| ConnectionRuntimeFailure::Conflict)?
                .token
                .ok_or(ConnectionRuntimeFailure::Conflict)?;
            if ChatGptAuthEntry(current) != entry {
                return Err(ConnectionRuntimeFailure::Conflict);
            }
            let authorized = guard
                .authorize_account()
                .await
                .map_err(|error| map_auth_error(&error))?;
            let account_id = authorized
                .account_id
                .filter(|id| !id.is_empty())
                .ok_or(ConnectionRuntimeFailure::Authentication)?;
            persist_managed_chatgpt(&store, account_id, label)
        })
    })
    .await
    .map_err(|_| ConnectionRuntimeFailure::Unavailable)?
}

/// Delete the probed auth file only if it still matches `entry`, then log in.
pub async fn relogin_chatgpt_subscription(
    store: &ProviderConnectionStore,
    label: Option<String>,
    entry: ChatGptAuthEntry,
    on_device_code: Option<DeviceCodeCallback>,
) -> Result<(), ConnectionRuntimeFailure> {
    let policy =
        ConnectionPolicy::user_default().map_err(|_| ConnectionRuntimeFailure::Validation)?;
    delete_probed_chatgpt_auth_file(policy.chatgpt_auth_file.clone(), entry).await?;
    login_chatgpt_subscription(store, label, on_device_code).await
}

async fn delete_probed_chatgpt_auth_file(
    auth_file: PathBuf,
    entry: ChatGptAuthEntry,
) -> Result<(), ConnectionRuntimeFailure> {
    match prepare_chatgpt_auth_file(&auth_file) {
        Ok(_) => {}
        Err(AuthFileProblem::Symlink | AuthFileProblem::NonRegular) => {
            return Err(ConnectionRuntimeFailure::Conflict);
        }
    }
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ConnectionRuntimeFailure::Unavailable)?;
        runtime.block_on(async move {
            let guard = rig::providers::chatgpt::auth::AuthFileGuard::acquire(&auth_file)
                .map_err(|_| ConnectionRuntimeFailure::Conflict)?;
            guard
                .delete_if_unchanged(&entry.0)
                .map_err(|_| ConnectionRuntimeFailure::Conflict)?;
            Ok(())
        })
    })
    .await
    .map_err(|_| ConnectionRuntimeFailure::Unavailable)?
}

/// Start device login and persist the resulting managed row.
pub async fn login_chatgpt_subscription(
    store: &ProviderConnectionStore,
    label: Option<String>,
    on_device_code: Option<DeviceCodeCallback>,
) -> Result<(), ConnectionRuntimeFailure> {
    let policy =
        ConnectionPolicy::user_default().map_err(|_| ConnectionRuntimeFailure::Validation)?;
    match prepare_chatgpt_auth_file(&policy.chatgpt_auth_file) {
        Ok(yach_connections::AuthFilePreparation::Missing) => {}
        Ok(_) | Err(AuthFileProblem::Symlink | AuthFileProblem::NonRegular) => {
            return Err(ConnectionRuntimeFailure::Conflict);
        }
    }
    let account_id = start_chatgpt_device_login(policy.chatgpt_auth_file, on_device_code).await?;
    persist_managed_chatgpt(store, account_id, label)
}

/// Delete the managed auth file, then start a new device login.
pub async fn reauth_chatgpt_subscription(
    store: &ProviderConnectionStore,
    connection: &ProviderConnection,
    on_device_code: Option<DeviceCodeCallback>,
) -> Result<(), ConnectionRuntimeFailure> {
    delete_managed_chatgpt_auth_file(connection)?;
    login_chatgpt_subscription(store, connection.label.clone(), on_device_code).await
}

/// Delete the managed auth file, then the registry row.
pub fn logout_chatgpt_subscription(
    store: &ProviderConnectionStore,
    connection: &ProviderConnection,
) -> Result<(), ConnectionRuntimeFailure> {
    delete_managed_chatgpt_auth_file(connection)?;
    store.remove(&connection.id).map_err(store_failure)
}

fn delete_managed_chatgpt_auth_file(
    connection: &ProviderConnection,
) -> Result<(), ConnectionRuntimeFailure> {
    if let ConnectionAuth::ChatGptSubscriptionManaged { auth_file, .. } = &connection.authentication
    {
        match prepare_chatgpt_auth_file(auth_file) {
            Ok(_) => {}
            Err(AuthFileProblem::Symlink | AuthFileProblem::NonRegular) => {
                return Err(ConnectionRuntimeFailure::Conflict);
            }
        }
        let guard = rig::providers::chatgpt::auth::AuthFileGuard::acquire(auth_file)
            .map_err(|_| ConnectionRuntimeFailure::Conflict)?;
        match guard.stat() {
            Ok(stat) => {
                if let Some(token) = stat.token {
                    guard
                        .delete_if_unchanged(&token)
                        .map_err(|_| ConnectionRuntimeFailure::Failed)?;
                }
            }
            Err(
                rig::providers::chatgpt::auth::AuthError::UnsafeAuthFile { .. }
                | rig::providers::chatgpt::auth::AuthError::UnsafeLockFile,
            ) => {
                return Err(ConnectionRuntimeFailure::Conflict);
            }
            Err(_) => return Err(ConnectionRuntimeFailure::Failed),
        }
    }
    Ok(())
}

/// Enforce the stored account before activation.
pub async fn authorize_managed_chatgpt(
    auth_file: PathBuf,
    account_id: String,
) -> Result<(), ConnectionRuntimeFailure> {
    match prepare_chatgpt_auth_file(&auth_file) {
        Ok(_) => {}
        Err(AuthFileProblem::Symlink | AuthFileProblem::NonRegular) => {
            return Err(ConnectionRuntimeFailure::Conflict);
        }
    }
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ConnectionRuntimeFailure::Unavailable)?;
        runtime.block_on(async move {
            let authenticator = rig::providers::chatgpt::auth::Authenticator::new(
                rig::providers::chatgpt::auth::AuthSource::OAuth,
                Some(auth_file),
                rig::providers::chatgpt::auth::DeviceCodeHandler::default(),
                false,
                None,
            );
            authenticator
                .authorize_expected(Some(account_id.as_str()))
                .await
                .map_err(|error| map_auth_error(&error))
        })
    })
    .await
    .map_err(|_| ConnectionRuntimeFailure::Unavailable)?
    .map(|_| ())
}

/// Build a request adapter that points at the managed auth file.
pub fn managed_chatgpt_adapter(
    connection: &ProviderConnection,
    timeout: std::time::Duration,
    max_tokens: u64,
    context_window: u64,
    max_tokens_param: MaxTokensParam,
) -> Result<RigProviderAdapterConfig, ConnectionRuntimeFailure> {
    let ConnectionAuth::ChatGptSubscriptionManaged { auth_file, .. } = &connection.authentication
    else {
        return Err(ConnectionRuntimeFailure::Validation);
    };
    Ok(RigProviderAdapterConfig {
        provider: RigProviderConfig::ChatGptSubscription {
            auth_file: auth_file.clone(),
        },
        timeout,
        max_tokens,
        context_window,
        max_tokens_param,
    })
}

fn map_auth_error(error: &rig::providers::chatgpt::auth::AuthError) -> ConnectionRuntimeFailure {
    match error {
        rig::providers::chatgpt::auth::AuthError::DeviceFlowDisabled
        | rig::providers::chatgpt::auth::AuthError::AccountMismatch { .. } => {
            ConnectionRuntimeFailure::Authentication
        }
        rig::providers::chatgpt::auth::AuthError::AuthBusy
        | rig::providers::chatgpt::auth::AuthError::AuthConflict
        | rig::providers::chatgpt::auth::AuthError::UnsafeAuthFile { .. }
        | rig::providers::chatgpt::auth::AuthError::UnsafeLockFile
        | rig::providers::chatgpt::auth::AuthError::RepairRequired { .. } => {
            ConnectionRuntimeFailure::Conflict
        }
        _ => ConnectionRuntimeFailure::Failed,
    }
}

fn store_failure(error: yach_connections::ConnectionStoreError) -> ConnectionRuntimeFailure {
    match error {
        yach_connections::ConnectionStoreError::Validation(_)
        | yach_connections::ConnectionStoreError::Credential(
            yach_connections::CredentialError::Invalid,
        ) => ConnectionRuntimeFailure::Validation,
        yach_connections::ConnectionStoreError::NotFound => ConnectionRuntimeFailure::NotFound,
        yach_connections::ConnectionStoreError::AlreadyExists => ConnectionRuntimeFailure::Conflict,
        _ => ConnectionRuntimeFailure::Failed,
    }
}
