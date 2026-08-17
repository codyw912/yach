//! ChatGPT subscription login, logout, and activation helpers.

use std::path::PathBuf;

use yach_connections::{
    prepare_chatgpt_auth_file, AuthFileProblem, ConnectionAuth, ConnectionPolicy, ProviderConnection,
    ProviderConnectionStore,
};

use crate::provider_connections::ConnectionRuntimeFailure;
use crate::rig_adapter::{MaxTokensParam, RigProviderAdapterConfig, RigProviderConfig};

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
) -> Result<String, ConnectionRuntimeFailure> {
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
            guard.authorize_account().await.map_err(map_auth_error)
        })
    })
    .await
    .map_err(|_| ConnectionRuntimeFailure::Unavailable)??;
    authorized
        .account_id
        .filter(|id| !id.is_empty())
        .ok_or(ConnectionRuntimeFailure::Authentication)
}

/// Start device flow only when the auth file is absent.
pub async fn start_chatgpt_device_login(
    auth_file: PathBuf,
) -> Result<String, ConnectionRuntimeFailure> {
    let completion = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ConnectionRuntimeFailure::Unavailable)?;
        runtime.block_on(async move {
            let authenticator = rig::providers::chatgpt::auth::Authenticator::new(
                rig::providers::chatgpt::auth::AuthSource::OAuth,
                Some(auth_file),
                rig::providers::chatgpt::auth::DeviceCodeHandler::default(),
                true,
                None,
            );
            authenticator
                .login_device_flow_expecting(
                    rig::providers::chatgpt::auth::ExpectedAuthEntry::Absent,
                )
                .await
                .map_err(map_auth_error)
        })
    })
    .await
    .map_err(|_| ConnectionRuntimeFailure::Unavailable)??;
    Ok(completion.account_id)
}

/// Probe the policy path, then adopt or start device login.
pub async fn login_chatgpt_subscription(
    store: &ProviderConnectionStore,
    label: Option<String>,
) -> Result<(), ConnectionRuntimeFailure> {
    let policy = ConnectionPolicy::user_default()
        .map_err(|_| ConnectionRuntimeFailure::Validation)?;
    let account_id = match prepare_chatgpt_auth_file(&policy.chatgpt_auth_file) {
        Ok(yach_connections::AuthFilePreparation::Missing) => {
            start_chatgpt_device_login(policy.chatgpt_auth_file.clone()).await?
        }
        Ok(_) => adopt_existing_chatgpt_login(policy.chatgpt_auth_file.clone()).await?,
        Err(AuthFileProblem::Symlink | AuthFileProblem::NonRegular) => {
            return Err(ConnectionRuntimeFailure::Conflict);
        }
    };
    persist_managed_chatgpt(store, account_id, label)
}

/// Delete the managed auth file, then the registry row.
pub fn logout_chatgpt_subscription(
    store: &ProviderConnectionStore,
    connection: &ProviderConnection,
) -> Result<(), ConnectionRuntimeFailure> {
    if let ConnectionAuth::ChatGptSubscriptionManaged { auth_file, .. } = &connection.authentication
    {
        match prepare_chatgpt_auth_file(auth_file) {
            Ok(_) | Err(AuthFileProblem::Symlink) => {}
            Err(AuthFileProblem::NonRegular) => return Err(ConnectionRuntimeFailure::Conflict),
        }
        let guard = rig::providers::chatgpt::auth::AuthFileGuard::acquire(auth_file)
            .map_err(|_| ConnectionRuntimeFailure::Conflict)?;
        if let Ok(stat) = guard.stat() {
            if let Some(token) = stat.token {
                match guard.delete_if_unchanged(&token) {
                    Ok(()) | Err(rig::providers::chatgpt::auth::AuthError::AuthConflict) => {}
                    Err(_) => return Err(ConnectionRuntimeFailure::Failed),
                }
            }
        }
    }
    store.remove(&connection.id).map_err(store_failure)
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
                .map_err(map_auth_error)
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

fn map_auth_error(error: rig::providers::chatgpt::auth::AuthError) -> ConnectionRuntimeFailure {
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
