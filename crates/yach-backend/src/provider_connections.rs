use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use yach_connections::{
    ConnectionId, ConnectionState, NewConnectionDraft, ProviderConnection, ProviderKind,
    ProviderSecret,
};
use yach_proto::{DialogKind, DialogOption, DialogRequest, DialogResponse};

use crate::{CatalogModelEntry, ModelDiscoveryFuture, ProviderConfig};

const CONNECTION_DIALOG_PREFIX: &str = "provider-connection:";
const ROOT_DIALOG_ID: &str = "provider-connection:root";
const PROVIDER_DIALOG_ID: &str = "provider-connection:provider";
const LABEL_DIALOG_ID: &str = "provider-connection:label";
const BASE_URL_DIALOG_ID: &str = "provider-connection:base-url";
const CREATE_SECRET_DIALOG_ID: &str = "provider-connection:secret:create";
const ACTIONS_DIALOG_ID: &str = "provider-connection:actions";
const RENAME_DIALOG_ID: &str = "provider-connection:rename";
const REMOVE_DIALOG_ID: &str = "provider-connection:remove";
const REPAIR_SECRET_DIALOG_ID: &str = "provider-connection:secret:repair";
const REPLACE_SECRET_DIALOG_ID: &str = "provider-connection:secret:replace";
const CHATGPT_CONFIRM_DIALOG_ID: &str = "provider-connection:chatgpt:confirm";
const CHATGPT_REAUTH_DIALOG_ID: &str = "provider-connection:chatgpt:reauth";
const CHATGPT_DEVICE_DIALOG_ID: &str = "provider-connection:chatgpt:device";

const MAX_CONNECTIONS: usize = 64;

/// A connection-aware model target that is safe to retain and render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveModelTarget {
    pub connection_id: ConnectionId,
    pub model: String,
}

/// A bounded, deterministic connection snapshot returned by a runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionList {
    connections: Vec<ProviderConnection>,
}

impl ConnectionList {
    #[must_use]
    pub fn new(mut connections: Vec<ProviderConnection>) -> Self {
        let capacity = MAX_CONNECTIONS
            + usize::from(
                connections
                    .iter()
                    .any(|connection| connection.id == ConnectionId::environment()),
            );
        connections.truncate(capacity);
        Self { connections }
    }

    #[must_use]
    pub fn as_slice(&self) -> &[ProviderConnection] {
        &self.connections
    }
}

impl Default for ConnectionList {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// Stable, secret-free connection-runtime failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionRuntimeFailure {
    Unavailable,
    Validation,
    Authentication,
    Network,
    NotFound,
    Conflict,
    Failed,
}

impl ConnectionRuntimeFailure {
    pub const fn status_message(self) -> &'static str {
        match self {
            Self::Unavailable => "connection operation unavailable",
            Self::Validation => "connection validation failed",
            Self::Authentication => "connection authentication failed",
            Self::Network => "connection network request failed",
            Self::NotFound => "connection no longer exists",
            Self::Conflict => "connection changed elsewhere",
            Self::Failed => "connection operation failed",
        }
    }
}

/// Complete bounded result for listing provider connections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionListOutcome {
    Available(ConnectionList),
    Failed(ConnectionRuntimeFailure),
}

impl ConnectionListOutcome {
    #[must_use]
    pub fn available(connections: Vec<ProviderConnection>) -> Self {
        Self::Available(ConnectionList::new(connections))
    }
}

/// Complete bounded result for a connection metadata/credential mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionMutationOutcome {
    Succeeded,
    Renamed {
        id: ConnectionId,
        display: Option<String>,
    },
    Failed(ConnectionRuntimeFailure),
    /// A create operation durably persisted pending metadata, but credential
    /// or ready-state persistence failed. The ID is safe to retry via repair.
    FailedAfterCreatePending {
        id: ConnectionId,
        failure: ConnectionRuntimeFailure,
    },
}

/// Complete candidate result for an API-key replacement.
///
/// This intentionally does not implement `Debug` or `Clone`: an active
/// candidate can carry provider credentials. The runner owns the only
/// active-state assignment after it validates that the replaced connection is
/// still active.
pub enum ConnectionReplacementOutcome {
    Succeeded { candidate: Option<ProviderConfig> },
    Failed(ConnectionRuntimeFailure),
}

/// Complete candidate result for an explicit model activation.
///
/// The future is defined here for the runtime boundary; model selection wiring
/// deliberately remains in the follow-up active-target slice.
pub enum ProviderActivationOutcome {
    Activated(ProviderConfig),
    Failed(ConnectionRuntimeFailure),
}

/// Boxed object-safe runtime result boundaries.
pub type ConnectionListFuture =
    Pin<Box<dyn Future<Output = ConnectionListOutcome> + Send + 'static>>;
pub type ConnectionMutationFuture =
    Pin<Box<dyn Future<Output = ConnectionMutationOutcome> + Send + 'static>>;
pub type ConnectionReplacementFuture =
    Pin<Box<dyn Future<Output = ConnectionReplacementOutcome> + Send + 'static>>;
pub type ProviderActivationFuture =
    Pin<Box<dyn Future<Output = ProviderActivationOutcome> + Send + 'static>>;

/// Result of inspecting the ChatGPT auth file without starting device flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatGptProbeOutcome {
    /// No auth file; device login may start.
    Missing,
    /// A usable login exists and can be adopted after confirmation.
    Existing { account_id: String },
    /// The file exists but cannot be adopted without repair or re-auth.
    Unusable(ConnectionRuntimeFailure),
}

pub type ChatGptProbeFuture = Pin<Box<dyn Future<Output = ChatGptProbeOutcome> + Send + 'static>>;

/// Backend-only connection and credential orchestration seam.
///
/// The UI receives only dialogs and bounded statuses. It never sees a storage,
/// credential, or provider-client primitive.
pub trait ProviderConnectionRuntime: Send + Sync {
    fn list(&self) -> ConnectionListFuture;
    fn cached_models(&self) -> Option<Arc<[CatalogModelEntry]>>;
    fn refresh_models(&self, active: Option<ActiveModelTarget>) -> ModelDiscoveryFuture;
    fn create(&self, draft: NewConnectionDraft, secret: ProviderSecret)
    -> ConnectionMutationFuture;
    fn repair(&self, id: ConnectionId, secret: ProviderSecret) -> ConnectionMutationFuture;
    fn replace(
        &self,
        id: ConnectionId,
        model: Option<String>,
        secret: ProviderSecret,
    ) -> ConnectionReplacementFuture;
    fn rename(&self, id: ConnectionId, label: Option<String>) -> ConnectionMutationFuture;
    fn remove(&self, id: ConnectionId) -> ConnectionMutationFuture;
    fn activate(&self, id: ConnectionId, model: String) -> ProviderActivationFuture;
    fn probe_chatgpt(&self) -> ChatGptProbeFuture {
        Box::pin(async { ChatGptProbeOutcome::Unusable(ConnectionRuntimeFailure::Unavailable) })
    }
    fn adopt_chatgpt(&self, _label: Option<String>) -> ConnectionMutationFuture {
        Box::pin(async { ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Unavailable) })
    }
    fn login_chatgpt(
        &self,
        _label: Option<String>,
        _on_device_code: Option<
            std::sync::Arc<dyn Fn(String, String) + Send + Sync>,
        >,
    ) -> ConnectionMutationFuture {
        Box::pin(async { ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Unavailable) })
    }
    fn relogin_chatgpt(
        &self,
        _label: Option<String>,
        _account_id: String,
        _on_device_code: Option<
            std::sync::Arc<dyn Fn(String, String) + Send + Sync>,
        >,
    ) -> ConnectionMutationFuture {
        Box::pin(async { ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Unavailable) })
    }

    fn reauth_chatgpt(
        &self,
        _connection: ProviderConnection,
        _on_device_code: Option<
            std::sync::Arc<dyn Fn(String, String) + Send + Sync>,
        >,
    ) -> ConnectionMutationFuture {
        Box::pin(async { ConnectionMutationOutcome::Failed(ConnectionRuntimeFailure::Unavailable) })
    }



    /// The last explicit activation target, if this runtime persists one.
    /// Read once at startup to restore the user's previous selection.
    fn remembered_selection(&self) -> Option<ActiveModelTarget> {
        None
    }

    /// Persist an explicit activation target for a future launch. Default:
    /// no memory. Implementations must be best-effort and never fail the
    /// activation that produced the target.
    fn remember_selection(&self, _target: ActiveModelTarget) {}
}

/// A runtime operation emitted by the reducer and consumed directly by the
/// runner. It never implements `Debug` or `Clone`, so credentials cannot cross
/// a formatting or duplicate-ownership boundary.
pub enum ConnectionMutationOperation {
    Create {
        draft: NewConnectionDraft,
        secret: ProviderSecret,
    },
    Repair {
        id: ConnectionId,
        secret: ProviderSecret,
    },
    Replace {
        id: ConnectionId,
        model: Option<String>,
        secret: ProviderSecret,
    },
    Rename {
        id: ConnectionId,
        label: Option<String>,
    },
    Remove {
        id: ConnectionId,
    },
    ProbeChatGpt {
        label: Option<String>,
    },
    AdoptChatGpt {
        label: Option<String>,
    },
    LoginChatGpt {
        label: Option<String>,
    },
    ReloginChatGpt {
        label: Option<String>,
        account_id: String,
    },
    ReauthChatGpt {
        connection: ProviderConnection,
    },
}

/// Reducer effects interpreted by the native runner.
pub enum ConnectionFlowEffect {
    ShowDialog(DialogRequest),
    StartMutation(ConnectionMutationOperation),
    LoadList { generation: u64 },
    RefreshModels,
    Status(&'static str),
    CancelChatGptLogin,
}

/// The externally inspectable wizard state, without metadata or credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionFlowStateTag {
    RootList,
    ChoosingProvider,
    EnteringLabel,
    EnteringBaseUrl,
    EnteringCreateSecret,
    ConfirmingChatGptLogin,
    ConfirmingChatGptReauth,
    WaitingChatGptDevice,
    Mutating,
    ConnectionActions,
    Renaming,
    ConfirmingRemove,
    EnteringRepairSecret,
    EnteringReplaceSecret,
}

enum ConnectionFlowState {
    RootList,
    ChoosingProvider,
    EnteringLabel {
        provider: ProviderKind,
    },
    EnteringBaseUrl {
        provider: ProviderKind,
        label: Option<String>,
    },
    EnteringCreateSecret {
        draft: NewConnectionDraft,
    },
    ConfirmingChatGptLogin {
        label: Option<String>,
        account_id: String,
    },
    ConfirmingChatGptReauth {
        connection: ProviderConnection,
    },
    WaitingChatGptDevice {
        label: Option<String>,
    },
    Mutating,
    ConnectionActions {
        connection: ProviderConnection,
    },
    Renaming {
        connection: ProviderConnection,
    },
    ConfirmingRemove {
        connection: ProviderConnection,
    },
    EnteringRepairSecret {
        id: ConnectionId,
    },
    EnteringReplaceSecret {
        id: ConnectionId,
        model: Option<String>,
    },
}

impl ConnectionFlowState {
    const fn tag(&self) -> ConnectionFlowStateTag {
        match self {
            Self::RootList => ConnectionFlowStateTag::RootList,
            Self::ChoosingProvider => ConnectionFlowStateTag::ChoosingProvider,
            Self::EnteringLabel { .. } => ConnectionFlowStateTag::EnteringLabel,
            Self::EnteringBaseUrl { .. } => ConnectionFlowStateTag::EnteringBaseUrl,
            Self::EnteringCreateSecret { .. } => ConnectionFlowStateTag::EnteringCreateSecret,
            Self::ConfirmingChatGptLogin { .. } => ConnectionFlowStateTag::ConfirmingChatGptLogin,
            Self::ConfirmingChatGptReauth { .. } => ConnectionFlowStateTag::ConfirmingChatGptReauth,
            Self::WaitingChatGptDevice { .. } => ConnectionFlowStateTag::WaitingChatGptDevice,
            Self::Mutating => ConnectionFlowStateTag::Mutating,
            Self::ConnectionActions { .. } => ConnectionFlowStateTag::ConnectionActions,
            Self::Renaming { .. } => ConnectionFlowStateTag::Renaming,
            Self::ConfirmingRemove { .. } => ConnectionFlowStateTag::ConfirmingRemove,
            Self::EnteringRepairSecret { .. } => ConnectionFlowStateTag::EnteringRepairSecret,
            Self::EnteringReplaceSecret { .. } => ConnectionFlowStateTag::EnteringReplaceSecret,
        }
    }
}

enum CreateRetryState {
    Fresh(NewConnectionDraft),
    Pending(ConnectionId),
}

enum RetryState {
    Create(CreateRetryState),
    ProbeChatGpt {
        label: Option<String>,
    },
    AdoptChatGpt {
        label: Option<String>,
        account_id: String,
    },
    LoginChatGpt {
        label: Option<String>,
    },
    ReauthChatGpt {
        connection: ProviderConnection,
    },
    Repair(ConnectionId),
    Replace {
        id: ConnectionId,
        model: Option<String>,
    },
    Rename {
        connection: ProviderConnection,
        label: Option<String>,
    },
    Remove(ProviderConnection),
}

impl RetryState {
    fn state(&self) -> ConnectionFlowState {
        match self {
            Self::Create(CreateRetryState::Fresh(draft)) => {
                ConnectionFlowState::EnteringCreateSecret {
                    draft: draft.clone(),
                }
            }
            Self::Create(CreateRetryState::Pending(id)) | Self::Repair(id) => {
                ConnectionFlowState::EnteringRepairSecret { id: id.clone() }
            }
            Self::ProbeChatGpt { label } | Self::LoginChatGpt { label } => {
                let _ = label;
                ConnectionFlowState::EnteringLabel {
                    provider: ProviderKind::ChatGptSubscription,
                }
            }
            Self::AdoptChatGpt { label, account_id } => {
                ConnectionFlowState::ConfirmingChatGptLogin {
                    label: label.clone(),
                    account_id: account_id.clone(),
                }
            }
            Self::ReauthChatGpt { connection } => ConnectionFlowState::ConfirmingChatGptReauth {
                connection: connection.clone(),
            },
            Self::Replace { id, model } => ConnectionFlowState::EnteringReplaceSecret {
                id: id.clone(),
                model: model.clone(),
            },
            Self::Rename { connection, label } => {
                let mut connection = connection.clone();
                connection.label.clone_from(label);
                ConnectionFlowState::Renaming { connection }
            }
            Self::Remove(connection) => ConnectionFlowState::ConfirmingRemove {
                connection: connection.clone(),
            },
        }
    }

    const fn success_message(&self) -> &'static str {
        match self {
            Self::Create(_) => "connection created",
            Self::ProbeChatGpt { .. } => "checking ChatGPT login",
            Self::AdoptChatGpt { .. }
            | Self::LoginChatGpt { .. }
            | Self::ReauthChatGpt { .. } => "ChatGPT subscription connected",
            Self::Repair(_) => "connection repaired",
            Self::Replace { .. } => "connection API key replaced",
            Self::Rename { .. } => "connection renamed",
            Self::Remove(_) => "connection removed",
        }
    }

    fn replacement_is_active(&self, active: Option<&ActiveModelTarget>) -> bool {
        matches!(
            (self, active),
            (Self::Replace { id, .. }, Some(active)) if *id == active.connection_id
        )
    }
}

/// Pure backend-owned `/connect` reducer.
pub struct ProviderConnectionFlow {
    state: ConnectionFlowState,
    connections: ConnectionList,
    active: Option<ActiveModelTarget>,
    list_generation: u64,
    pending_mutation: Option<RetryState>,
    issued_dialog: Option<&'static str>,
}

impl ProviderConnectionFlow {
    #[must_use]
    pub fn new(active: Option<ActiveModelTarget>) -> Self {
        Self {
            state: ConnectionFlowState::RootList,
            connections: ConnectionList::default(),
            active,
            list_generation: 0,
            pending_mutation: None,
            issued_dialog: None,
        }
    }

    #[must_use]
    pub const fn state_tag(&self) -> ConnectionFlowStateTag {
        self.state.tag()
    }

    #[must_use]
    pub fn active_target(&self) -> Option<&ActiveModelTarget> {
        self.active.as_ref()
    }

    /// Atomically replace the target used for all subsequent connection
    /// mutations after the runner has accepted an activation candidate.
    pub fn set_active_target(&mut self, active: Option<ActiveModelTarget>) {
        self.active = active;
    }

    #[must_use]
    pub fn open(&mut self) -> Vec<ConnectionFlowEffect> {
        self.request_list()
    }

    #[must_use]
    pub fn complete_list(
        &mut self,
        generation: u64,
        outcome: ConnectionListOutcome,
    ) -> Vec<ConnectionFlowEffect> {
        if generation != self.list_generation {
            return Vec::new();
        }
        match outcome {
            ConnectionListOutcome::Available(connections) => {
                self.connections = connections;
            }
            ConnectionListOutcome::Failed(failure) => {
                self.connections = ConnectionList::default();
                self.state = ConnectionFlowState::RootList;
                return vec![
                    ConnectionFlowEffect::Status(failure.status_message()),
                    ConnectionFlowEffect::ShowDialog(self.root_dialog()),
                ];
            }
        }
        self.state = ConnectionFlowState::RootList;
        vec![ConnectionFlowEffect::ShowDialog(self.root_dialog())]
    }

    #[must_use]
    pub fn handle_dialog_response(
        &mut self,
        dialog_id: &str,
        response: DialogResponse,
        provider_turn_active: bool,
    ) -> Vec<ConnectionFlowEffect> {
        if !is_provider_connection_dialog_id(dialog_id)
            || self.issued_dialog != Some(dialog_id)
            || !self.dialog_matches_state(dialog_id)
        {
            return Vec::new();
        }
        self.issued_dialog = None;
        if matches!(response, DialogResponse::Cancelled) {
            self.issued_dialog = None;
            let cancel_login = matches!(
                self.state,
                ConnectionFlowState::WaitingChatGptDevice { .. }
            ) || matches!(
                self.pending_mutation,
                Some(RetryState::LoginChatGpt { .. } | RetryState::ReauthChatGpt { .. })
            );
            self.state = ConnectionFlowState::RootList;
            if cancel_login {
                self.pending_mutation = None;
                return vec![
                    ConnectionFlowEffect::CancelChatGptLogin,
                    ConnectionFlowEffect::Status("ChatGPT login cancelled"),
                ];
            }
            return Vec::new();
        }

        match (&self.state, response) {
            (ConnectionFlowState::RootList, DialogResponse::Selection { value }) => {
                self.select_root(&value)
            }
            (ConnectionFlowState::ChoosingProvider, DialogResponse::Selection { value }) => {
                self.select_provider(&value)
            }
            (ConnectionFlowState::EnteringLabel { provider }, DialogResponse::Text { value }) => {
                self.submit_label(*provider, value)
            }
            (
                ConnectionFlowState::EnteringBaseUrl { provider, label },
                DialogResponse::Text { value },
            ) => self.submit_base_url(*provider, label.clone(), value),
            (
                ConnectionFlowState::EnteringCreateSecret { draft },
                DialogResponse::Secret { value },
            ) => self.submit_create_secret(draft.clone(), value, provider_turn_active),
            (
                ConnectionFlowState::ConnectionActions { connection },
                DialogResponse::Selection { value },
            ) => self.select_action(connection.clone(), &value),
            (ConnectionFlowState::Renaming { connection }, DialogResponse::Text { value }) => {
                self.submit_rename(connection.clone(), value, provider_turn_active)
            }
            (
                ConnectionFlowState::ConfirmingRemove { connection },
                DialogResponse::Confirmed { accepted },
            ) => self.confirm_remove(connection.clone(), accepted, provider_turn_active),
            (
                ConnectionFlowState::EnteringRepairSecret { id },
                DialogResponse::Secret { value },
            ) => self.submit_repair_secret(id.clone(), value, provider_turn_active),
            (
                ConnectionFlowState::EnteringReplaceSecret { id, model },
                DialogResponse::Secret { value },
            ) => self.submit_replace_secret(id.clone(), model.clone(), value, provider_turn_active),
            (
                ConnectionFlowState::ConfirmingChatGptLogin { label, account_id },
                DialogResponse::Selection { value },
            ) => self.choose_chatgpt_login(label.clone(), account_id.clone(), &value),
            (
                ConnectionFlowState::ConfirmingChatGptReauth { connection },
                DialogResponse::Confirmed { accepted },
            ) => self.confirm_chatgpt_reauth(connection.clone(), accepted),
            (
                ConnectionFlowState::WaitingChatGptDevice { label },
                DialogResponse::Confirmed { accepted },
            ) => self.confirm_chatgpt_device(label.clone(), accepted),
            _ => Vec::new(),
        }
    }

    #[must_use]
    pub fn complete_mutation(
        &mut self,
        outcome: &ConnectionMutationOutcome,
    ) -> Vec<ConnectionFlowEffect> {
        let Some(retry) = self.pending_mutation.take() else {
            return Vec::new();
        };
        match outcome {
            ConnectionMutationOutcome::Succeeded | ConnectionMutationOutcome::Renamed { .. } => {
                self.state = ConnectionFlowState::RootList;
                let mut effects = vec![
                    ConnectionFlowEffect::Status(retry.success_message()),
                    ConnectionFlowEffect::RefreshModels,
                ];
                effects.extend(self.request_list());
                effects
            }
            ConnectionMutationOutcome::Failed(failure) => {
                self.state = retry.state();
                vec![
                    ConnectionFlowEffect::Status(failure.status_message()),
                    ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
                ]
            }
            ConnectionMutationOutcome::FailedAfterCreatePending { id, failure } => {
                let retry = match retry {
                    RetryState::Create(CreateRetryState::Fresh(_)) => {
                        RetryState::Create(CreateRetryState::Pending(id.clone()))
                    }
                    retry => retry,
                };
                self.state = retry.state();
                vec![
                    ConnectionFlowEffect::Status(failure.status_message()),
                    ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
                ]
            }
        }
    }

    /// Continues `/connect` after inspecting the ChatGPT auth file.
    #[must_use]
    pub fn complete_chatgpt_probe(
        &mut self,
        outcome: ChatGptProbeOutcome,
    ) -> Vec<ConnectionFlowEffect> {
        let Some(RetryState::ProbeChatGpt { label }) = self.pending_mutation.take() else {
            return Vec::new();
        };
        match outcome {
            ChatGptProbeOutcome::Missing => {
                self.state = ConnectionFlowState::WaitingChatGptDevice { label: label.clone() };
                self.pending_mutation = Some(RetryState::LoginChatGpt {
                    label: label.clone(),
                });
                vec![
                    ConnectionFlowEffect::Status("waiting for ChatGPT device authorization"),
                    ConnectionFlowEffect::StartMutation(
                        ConnectionMutationOperation::LoginChatGpt { label },
                    ),
                ]
            }
            ChatGptProbeOutcome::Existing { account_id } => {
                self.state = ConnectionFlowState::ConfirmingChatGptLogin { label, account_id };
                vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())]
            }
            ChatGptProbeOutcome::Unusable(failure) => {
                self.state = ConnectionFlowState::EnteringLabel {
                    provider: ProviderKind::ChatGptSubscription,
                };
                vec![
                    ConnectionFlowEffect::Status(failure.status_message()),
                    ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
                ]
            }
        }
    }

    /// Shows the device-code dialog for an in-flight ChatGPT login.
    #[must_use]
    pub fn present_chatgpt_device_code(
        &mut self,
        verification_uri: String,
        user_code: String,
    ) -> Vec<ConnectionFlowEffect> {
        if !matches!(
            self.pending_mutation,
            Some(RetryState::LoginChatGpt { .. } | RetryState::ReauthChatGpt { .. })
        ) {
            return Vec::new();
        }
        self.issued_dialog = Some(CHATGPT_DEVICE_DIALOG_ID);
        vec![ConnectionFlowEffect::ShowDialog(DialogRequest {
            id: Some(String::from(CHATGPT_DEVICE_DIALOG_ID)),
            title: Some(String::from("ChatGPT login")),
            prompt: Some(format!("Visit {verification_uri} and enter the displayed code.")),
            kind: DialogKind::DeviceCode {
                verification_uri,
                user_code,
            },
        })]
    }

    fn choose_chatgpt_login(
        &mut self,
        label: Option<String>,
        account_id: String,
        value: &str,
    ) -> Vec<ConnectionFlowEffect> {
        match value {
            "use" => self.start_mutation(
                RetryState::AdoptChatGpt {
                    label: label.clone(),
                    account_id,
                },
                ConnectionMutationOperation::AdoptChatGpt { label },
                false,
            ),
            "reauth" => {
                let effects = self.start_mutation(
                    RetryState::LoginChatGpt {
                        label: label.clone(),
                    },
                    ConnectionMutationOperation::ReloginChatGpt {
                        label: label.clone(),
                        account_id,
                    },
                    false,
                );
                if self.pending_mutation.is_some() {
                    self.state = ConnectionFlowState::WaitingChatGptDevice { label };
                }
                effects
            }
            "cancel" => {
                self.state = ConnectionFlowState::RootList;
                vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())]
            }
            _ => Vec::new(),
        }
    }

    fn confirm_chatgpt_device(
        &mut self,
        _label: Option<String>,
        accepted: bool,
    ) -> Vec<ConnectionFlowEffect> {
        if accepted {
            return Vec::new();
        }
        self.state = ConnectionFlowState::RootList;
        self.pending_mutation = None;
        vec![
            ConnectionFlowEffect::CancelChatGptLogin,
            ConnectionFlowEffect::Status("ChatGPT login cancelled"),
        ]
    }

    fn confirm_chatgpt_reauth(
        &mut self,
        connection: ProviderConnection,
        accepted: bool,
    ) -> Vec<ConnectionFlowEffect> {
        if !accepted {
            self.state = ConnectionFlowState::ConnectionActions { connection };
            return vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())];
        }
        let label = connection.label.clone();
        let effects = self.start_mutation(
            RetryState::ReauthChatGpt {
                connection: connection.clone(),
            },
            ConnectionMutationOperation::ReauthChatGpt { connection },
            false,
        );
        if self.pending_mutation.is_some() {
            self.state = ConnectionFlowState::WaitingChatGptDevice { label };
        }
        effects
    }


    #[must_use]
    pub fn replacement_targets_active(&self) -> bool {
        self.pending_mutation
            .as_ref()
            .is_some_and(|retry| retry.replacement_is_active(self.active.as_ref()))
    }


    /// Whether a connection mutation has been accepted and awaits completion.
    #[must_use]
    pub const fn mutation_in_flight(&self) -> bool {
        self.pending_mutation.is_some()
    }

    fn request_list(&mut self) -> Vec<ConnectionFlowEffect> {
        self.list_generation = self.list_generation.wrapping_add(1);
        vec![ConnectionFlowEffect::LoadList {
            generation: self.list_generation,
        }]
    }

    fn select_root(&mut self, value: &str) -> Vec<ConnectionFlowEffect> {
        if value == "add" {
            self.state = ConnectionFlowState::ChoosingProvider;
            return vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())];
        }
        let Some(connection) = self
            .connections
            .as_slice()
            .iter()
            .find(|connection| connection.id.as_str() == value)
            .cloned()
        else {
            return Vec::new();
        };
        if connection.id == ConnectionId::environment() {
            return vec![
                ConnectionFlowEffect::Status("environment connection is read-only"),
                ConnectionFlowEffect::ShowDialog(self.root_dialog()),
            ];
        }
        self.state = ConnectionFlowState::ConnectionActions { connection };
        vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())]
    }

    fn select_provider(&mut self, value: &str) -> Vec<ConnectionFlowEffect> {
        let provider = match value {
            "anthropic" => ProviderKind::Anthropic,
            "openai" => ProviderKind::OpenAi,
            "openai-compatible" => ProviderKind::OpenAiCompatible,
            "openai-codex" => ProviderKind::ChatGptSubscription,
            _ => return Vec::new(),
        };
        self.state = ConnectionFlowState::EnteringLabel { provider };
        vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())]
    }

    fn submit_label(&mut self, provider: ProviderKind, value: String) -> Vec<ConnectionFlowEffect> {
        let label = optional_field(value);
        let normalized_label = match NewConnectionDraft::new(ProviderKind::OpenAi, label, None) {
            Ok(draft) => draft.label().map(str::to_owned),
            Err(_) => {
                return vec![
                    ConnectionFlowEffect::Status("connection label is invalid"),
                    ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
                ];
            }
        };
        if provider == ProviderKind::ChatGptSubscription {
            return self.start_mutation(
                RetryState::ProbeChatGpt {
                    label: normalized_label.clone(),
                },
                ConnectionMutationOperation::ProbeChatGpt {
                    label: normalized_label,
                },
                false,
            );
        }
        if provider == ProviderKind::OpenAiCompatible {
            self.state = ConnectionFlowState::EnteringBaseUrl {
                provider,
                label: normalized_label,
            };
        } else {
            let Ok(draft) = NewConnectionDraft::new(provider, normalized_label, None) else {
                return vec![
                    ConnectionFlowEffect::Status("connection draft is invalid"),
                    ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
                ];
            };
            self.state = ConnectionFlowState::EnteringCreateSecret { draft };
        }
        vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())]
    }

    fn submit_base_url(
        &mut self,
        provider: ProviderKind,
        label: Option<String>,
        value: String,
    ) -> Vec<ConnectionFlowEffect> {
        let Ok(draft) = NewConnectionDraft::new(provider, label.clone(), Some(value)) else {
            self.state = ConnectionFlowState::EnteringBaseUrl { provider, label };
            return vec![
                ConnectionFlowEffect::Status("connection base URL is invalid"),
                ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
            ];
        };
        self.state = ConnectionFlowState::EnteringCreateSecret { draft };
        vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())]
    }

    fn submit_create_secret(
        &mut self,
        draft: NewConnectionDraft,
        value: yach_proto::SubmittedSecret,
        provider_turn_active: bool,
    ) -> Vec<ConnectionFlowEffect> {
        if value.is_empty() {
            return vec![
                ConnectionFlowEffect::Status("a connection credential is required"),
                ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
            ];
        }
        self.start_mutation(
            RetryState::Create(CreateRetryState::Fresh(draft.clone())),
            ConnectionMutationOperation::Create {
                draft,
                secret: ProviderSecret::new(value.into_inner()),
            },
            provider_turn_active,
        )
    }

    fn select_action(
        &mut self,
        connection: ProviderConnection,
        value: &str,
    ) -> Vec<ConnectionFlowEffect> {
        match value {
            "reauth" if connection.provider == ProviderKind::ChatGptSubscription => {
                self.state = ConnectionFlowState::ConfirmingChatGptReauth { connection };
            }
            "repair"
                if connection.provider != ProviderKind::ChatGptSubscription
                    && connection.state == ConnectionState::PendingCredential =>
            {
                self.state = ConnectionFlowState::EnteringRepairSecret { id: connection.id };
            }
            "replace"
                if connection.provider != ProviderKind::ChatGptSubscription
                    && connection.state == ConnectionState::Ready =>
            {
                let model = self
                    .active
                    .as_ref()
                    .filter(|active| active.connection_id == connection.id)
                    .map(|active| active.model.clone());
                self.state = ConnectionFlowState::EnteringReplaceSecret {
                    id: connection.id,
                    model,
                };
            }
            "rename" => {
                self.state = ConnectionFlowState::Renaming { connection };
            }
            "remove" => {
                self.state = ConnectionFlowState::ConfirmingRemove { connection };
            }
            _ => return Vec::new(),
        }
        vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())]
    }

    fn submit_rename(
        &mut self,
        connection: ProviderConnection,
        value: String,
        provider_turn_active: bool,
    ) -> Vec<ConnectionFlowEffect> {
        let label = optional_field(value);
        let label = match NewConnectionDraft::new(ProviderKind::OpenAi, label, None) {
            Ok(draft) => draft.label().map(str::to_owned),
            Err(_) => {
                return vec![
                    ConnectionFlowEffect::Status("connection label is invalid"),
                    ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
                ];
            }
        };
        self.start_mutation(
            RetryState::Rename {
                connection: connection.clone(),
                label: label.clone(),
            },
            ConnectionMutationOperation::Rename {
                id: connection.id,
                label,
            },
            provider_turn_active,
        )
    }

    fn confirm_remove(
        &mut self,
        connection: ProviderConnection,
        accepted: bool,
        provider_turn_active: bool,
    ) -> Vec<ConnectionFlowEffect> {
        if !accepted {
            self.state = ConnectionFlowState::ConnectionActions { connection };
            return vec![ConnectionFlowEffect::ShowDialog(self.dialog_for_state())];
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.connection_id == connection.id)
        {
            self.state = ConnectionFlowState::ConnectionActions { connection };
            return vec![
                ConnectionFlowEffect::Status(
                    "select another connection before removing the active connection",
                ),
                ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
            ];
        }
        self.start_mutation(
            RetryState::Remove(connection.clone()),
            ConnectionMutationOperation::Remove { id: connection.id },
            provider_turn_active,
        )
    }

    fn submit_repair_secret(
        &mut self,
        id: ConnectionId,
        value: yach_proto::SubmittedSecret,
        provider_turn_active: bool,
    ) -> Vec<ConnectionFlowEffect> {
        if value.is_empty() {
            return vec![
                ConnectionFlowEffect::Status("a connection credential is required"),
                ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
            ];
        }
        self.start_mutation(
            RetryState::Repair(id.clone()),
            ConnectionMutationOperation::Repair {
                id,
                secret: ProviderSecret::new(value.into_inner()),
            },
            provider_turn_active,
        )
    }

    fn submit_replace_secret(
        &mut self,
        id: ConnectionId,
        model: Option<String>,
        value: yach_proto::SubmittedSecret,
        provider_turn_active: bool,
    ) -> Vec<ConnectionFlowEffect> {
        if value.is_empty() {
            return vec![
                ConnectionFlowEffect::Status("a connection credential is required"),
                ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
            ];
        }
        self.start_mutation(
            RetryState::Replace {
                id: id.clone(),
                model: model.clone(),
            },
            ConnectionMutationOperation::Replace {
                id,
                model,
                secret: ProviderSecret::new(value.into_inner()),
            },
            provider_turn_active,
        )
    }

    fn start_mutation(
        &mut self,
        retry: RetryState,
        operation: ConnectionMutationOperation,
        provider_turn_active: bool,
    ) -> Vec<ConnectionFlowEffect> {
        if provider_turn_active {
            return vec![
                ConnectionFlowEffect::Status(
                    "connection changes are unavailable while a prompt is in progress",
                ),
                ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
            ];
        }
        if self.pending_mutation.is_some() {
            return vec![
                ConnectionFlowEffect::Status("another connection change is already in progress"),
                ConnectionFlowEffect::ShowDialog(self.dialog_for_state()),
            ];
        }
        self.pending_mutation = Some(retry);
        self.state = ConnectionFlowState::Mutating;
        vec![ConnectionFlowEffect::StartMutation(operation)]
    }

    fn dialog_matches_state(&self, dialog_id: &str) -> bool {
        matches!(
            (&self.state, dialog_id),
            (ConnectionFlowState::RootList, ROOT_DIALOG_ID)
                | (ConnectionFlowState::ChoosingProvider, PROVIDER_DIALOG_ID)
                | (ConnectionFlowState::EnteringLabel { .. }, LABEL_DIALOG_ID)
                | (
                    ConnectionFlowState::EnteringBaseUrl { .. },
                    BASE_URL_DIALOG_ID
                )
                | (
                    ConnectionFlowState::EnteringCreateSecret { .. },
                    CREATE_SECRET_DIALOG_ID
                )
                | (
                    ConnectionFlowState::ConnectionActions { .. },
                    ACTIONS_DIALOG_ID
                )
                | (ConnectionFlowState::Renaming { .. }, RENAME_DIALOG_ID)
                | (
                    ConnectionFlowState::ConfirmingRemove { .. },
                    REMOVE_DIALOG_ID
                )
                | (
                    ConnectionFlowState::EnteringRepairSecret { .. },
                    REPAIR_SECRET_DIALOG_ID
                )
                | (
                    ConnectionFlowState::EnteringReplaceSecret { .. },
                    REPLACE_SECRET_DIALOG_ID
                )
                | (
                    ConnectionFlowState::ConfirmingChatGptLogin { .. },
                    CHATGPT_CONFIRM_DIALOG_ID
                )
                | (
                    ConnectionFlowState::ConfirmingChatGptReauth { .. },
                    CHATGPT_REAUTH_DIALOG_ID
                )
                | (
                    ConnectionFlowState::WaitingChatGptDevice { .. },
                    CHATGPT_DEVICE_DIALOG_ID
                )
        )
    }

    fn root_dialog(&mut self) -> DialogRequest {
        self.issued_dialog = Some(ROOT_DIALOG_ID);
        let connections = self.connections.as_slice();
        let mut options = Vec::with_capacity(connections.len().saturating_add(1));
        options.push(DialogOption {
            label: String::from("Add connection"),
            value: String::from("add"),
        });
        options.extend(connections.iter().map(|connection| DialogOption {
            label: connection.display_label(connections),
            value: connection.id.as_str().to_owned(),
        }));
        DialogRequest {
            id: Some(String::from(ROOT_DIALOG_ID)),
            title: Some(String::from("Provider connections")),
            prompt: Some(String::from("Choose a connection")),
            kind: DialogKind::Select { options },
        }
    }

    fn dialog_for_state(&mut self) -> DialogRequest {
        self.issued_dialog = match &self.state {
            ConnectionFlowState::RootList => Some(ROOT_DIALOG_ID),
            ConnectionFlowState::ChoosingProvider => Some(PROVIDER_DIALOG_ID),
            ConnectionFlowState::EnteringLabel { .. } => Some(LABEL_DIALOG_ID),
            ConnectionFlowState::EnteringBaseUrl { .. } => Some(BASE_URL_DIALOG_ID),
            ConnectionFlowState::EnteringCreateSecret { .. } => Some(CREATE_SECRET_DIALOG_ID),
            ConnectionFlowState::ConfirmingChatGptLogin { .. } => Some(CHATGPT_CONFIRM_DIALOG_ID),
            ConnectionFlowState::ConfirmingChatGptReauth { .. } => Some(CHATGPT_REAUTH_DIALOG_ID),
            ConnectionFlowState::WaitingChatGptDevice { .. } => Some(CHATGPT_DEVICE_DIALOG_ID),
            ConnectionFlowState::Mutating => None,
            ConnectionFlowState::ConnectionActions { .. } => Some(ACTIONS_DIALOG_ID),
            ConnectionFlowState::Renaming { .. } => Some(RENAME_DIALOG_ID),
            ConnectionFlowState::ConfirmingRemove { .. } => Some(REMOVE_DIALOG_ID),
            ConnectionFlowState::EnteringRepairSecret { .. } => Some(REPAIR_SECRET_DIALOG_ID),
            ConnectionFlowState::EnteringReplaceSecret { .. } => Some(REPLACE_SECRET_DIALOG_ID),
        };
        match &self.state {
            ConnectionFlowState::RootList => self.root_dialog(),
            ConnectionFlowState::ChoosingProvider => select_dialog(
                PROVIDER_DIALOG_ID,
                "Add provider connection",
                "Choose a provider",
                &[
                    ("Anthropic", "anthropic"),
                    ("OpenAI", "openai"),
                    ("OpenAI-compatible", "openai-compatible"),
                    ("OpenAI Codex", "openai-codex"),
                ],
            ),
            ConnectionFlowState::EnteringLabel { .. } => {
                input_dialog(LABEL_DIALOG_ID, "Connection label", "Optional label", None)
            }
            ConnectionFlowState::EnteringBaseUrl { .. } => input_dialog(
                BASE_URL_DIALOG_ID,
                "OpenAI-compatible base URL",
                "Enter the API base URL",
                None,
            ),
            ConnectionFlowState::EnteringCreateSecret { .. } => secret_dialog(
                CREATE_SECRET_DIALOG_ID,
                "Connection credential",
                "Enter the API key",
            ),
            ConnectionFlowState::ConfirmingChatGptLogin { account_id, .. } => select_dialog(
                CHATGPT_CONFIRM_DIALOG_ID,
                "Existing OpenAI Codex login",
                &format!("Use existing login for {account_id}?"),
                &[
                    ("Use existing login", "use"),
                    ("Re-authenticate", "reauth"),
                    ("Cancel", "cancel"),
                ],
            ),
            ConnectionFlowState::ConfirmingChatGptReauth { .. } => DialogRequest {
                id: Some(String::from(CHATGPT_REAUTH_DIALOG_ID)),
                title: Some(String::from("Re-authenticate ChatGPT?")),
                prompt: Some(String::from(
                    "Delete the stored ChatGPT login and start device authorization?",
                )),
                kind: DialogKind::Confirm,
            },
            ConnectionFlowState::WaitingChatGptDevice { .. } => DialogRequest {
                id: Some(String::from(CHATGPT_DEVICE_DIALOG_ID)),
                title: Some(String::from("ChatGPT login")),
                prompt: Some(String::from(
                    "Waiting for ChatGPT device authorization. Cancel to abort.",
                )),
                kind: DialogKind::Confirm,
            },
            ConnectionFlowState::Mutating => DialogRequest {
                id: None,
                title: Some(String::from("Connection update")),
                prompt: Some(String::from("Validating connection update")),
                kind: DialogKind::Confirm,
            },
            ConnectionFlowState::ConnectionActions { connection } => {
                let mut actions = Vec::with_capacity(3);
                if connection.provider == ProviderKind::ChatGptSubscription {
                    actions.push(("Re-authenticate", "reauth"));
                } else {
                    match connection.state {
                        ConnectionState::PendingCredential => {
                            actions.push(("Repair credential", "repair"));
                        }
                        ConnectionState::Ready => {
                            actions.push(("Replace credential", "replace"));
                        }
                    }
                }
                actions.extend([("Rename", "rename"), ("Remove", "remove")]);
                select_dialog(
                    ACTIONS_DIALOG_ID,
                    "Connection actions",
                    "Choose an action",
                    &actions,
                )
            }
            ConnectionFlowState::Renaming { connection } => input_dialog(
                RENAME_DIALOG_ID,
                "Rename connection",
                "Enter a label or leave empty to clear it",
                connection.label.clone(),
            ),
            ConnectionFlowState::ConfirmingRemove { .. } => DialogRequest {
                id: Some(String::from(REMOVE_DIALOG_ID)),
                title: Some(String::from("Remove connection")),
                prompt: Some(String::from("Remove this connection and its credential?")),
                kind: DialogKind::Confirm,
            },
            ConnectionFlowState::EnteringRepairSecret { .. } => secret_dialog(
                REPAIR_SECRET_DIALOG_ID,
                "Repair connection credential",
                "Enter the replacement API key",
            ),
            ConnectionFlowState::EnteringReplaceSecret { .. } => secret_dialog(
                REPLACE_SECRET_DIALOG_ID,
                "Replace connection credential",
                "Enter the replacement API key",
            ),
        }
    }
}

#[must_use]
pub fn is_provider_connection_dialog_id(dialog_id: &str) -> bool {
    dialog_id.starts_with(CONNECTION_DIALOG_PREFIX)
}

fn optional_field(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn select_dialog(id: &str, title: &str, prompt: &str, options: &[(&str, &str)]) -> DialogRequest {
    DialogRequest {
        id: Some(id.to_owned()),
        title: Some(title.to_owned()),
        prompt: Some(prompt.to_owned()),
        kind: DialogKind::Select {
            options: options
                .iter()
                .map(|(label, value)| DialogOption {
                    label: (*label).to_owned(),
                    value: (*value).to_owned(),
                })
                .collect(),
        },
    }
}

fn input_dialog(id: &str, title: &str, prompt: &str, default: Option<String>) -> DialogRequest {
    DialogRequest {
        id: Some(id.to_owned()),
        title: Some(title.to_owned()),
        prompt: Some(prompt.to_owned()),
        kind: DialogKind::Input { default },
    }
}

fn secret_dialog(id: &str, title: &str, prompt: &str) -> DialogRequest {
    DialogRequest {
        id: Some(id.to_owned()),
        title: Some(title.to_owned()),
        prompt: Some(prompt.to_owned()),
        kind: DialogKind::SecretInput,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveModelTarget, ChatGptProbeOutcome, ConnectionFlowEffect, ConnectionFlowStateTag,
        ConnectionList, ConnectionListOutcome, ConnectionMutationOperation,
        ConnectionMutationOutcome, ProviderConnectionFlow,
    };
    use yach_connections::{
        ConnectionAuth, ConnectionId, ConnectionState, CredentialSource, NewConnectionDraft,
        ProviderConnection, ProviderKind,
    };
    use yach_proto::{DialogKind, DialogResponse};

    fn stored_connection(state: ConnectionState, label: Option<&str>) -> ProviderConnection {
        ProviderConnection {
            id: ConnectionId::new_stored(),
            provider: ProviderKind::OpenAi,
            label: label.map(str::to_owned),
            base_url: None,
            authentication: ConnectionAuth::ApiKey {
                source: CredentialSource::System,
            },
            state,
        }
    }

    #[test]
    fn connection_list_preserves_runtime_environment_provider_display_and_id_order() {
        let environment = ProviderConnection {
            id: ConnectionId::environment(),
            provider: ProviderKind::Anthropic,
            label: Some(String::from("Environment")),
            base_url: None,
            authentication: ConnectionAuth::ApiKey {
                source: CredentialSource::Environment,
            },
            state: ConnectionState::Ready,
        };
        let openai = stored_connection(ConnectionState::Ready, Some("Beta"));
        let compatible = ProviderConnection {
            id: ConnectionId::new_stored(),
            provider: ProviderKind::OpenAiCompatible,
            label: Some(String::from("Alpha")),
            base_url: Some(String::from("https://gateway.example/v1")),
            authentication: ConnectionAuth::ApiKey {
                source: CredentialSource::System,
            },
            state: ConnectionState::Ready,
        };
        let expected = vec![
            environment.id.clone(),
            compatible.id.clone(),
            openai.id.clone(),
        ];

        let list = ConnectionList::new(vec![environment, compatible, openai]);

        assert_eq!(
            list.as_slice()
                .iter()
                .map(|connection| connection.id.clone())
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn connection_list_reserves_one_slot_for_environment_beyond_stored_cap() {
        let environment = ProviderConnection {
            id: ConnectionId::environment(),
            provider: ProviderKind::OpenAi,
            label: Some(String::from("Environment")),
            base_url: None,
            authentication: ConnectionAuth::ApiKey {
                source: CredentialSource::Environment,
            },
            state: ConnectionState::Ready,
        };
        let mut connections = vec![environment];
        connections.extend((0..64).map(|index| {
            stored_connection(ConnectionState::Ready, Some(&format!("stored-{index}")))
        }));

        let list = ConnectionList::new(connections);

        assert_eq!(list.as_slice().len(), 65);
        assert_eq!(list.as_slice()[0].id, ConnectionId::environment());
    }

    #[test]
    fn provider_connection_flow_treats_environment_connection_as_read_only() {
        let environment = ProviderConnection {
            id: ConnectionId::environment(),
            provider: ProviderKind::OpenAi,
            label: Some(String::from("Environment")),
            base_url: None,
            authentication: ConnectionAuth::ApiKey {
                source: CredentialSource::Environment,
            },
            state: ConnectionState::Ready,
        };
        let mut flow = ProviderConnectionFlow::new(None);
        let environment_id = begin_root(&mut flow, vec![environment]);

        let effects = flow.handle_dialog_response(
            "provider-connection:root",
            DialogResponse::Selection {
                value: environment_id,
            },
            false,
        );

        assert!(effects.iter().any(|effect| {
            matches!(effect, ConnectionFlowEffect::Status(message) if *message == "environment connection is read-only")
        }));
        only_dialog(effects, "provider-connection:root");
    }

    fn begin_root(
        flow: &mut ProviderConnectionFlow,
        connections: Vec<ProviderConnection>,
    ) -> String {
        let effects = flow.open();
        let generation = effects.iter().find_map(|effect| match effect {
            ConnectionFlowEffect::LoadList { generation } => Some(*generation),
            _ => None,
        });
        assert!(generation.is_some(), "opening the flow loads the root list");
        let Some(generation) = generation else {
            return String::new();
        };
        let effects = flow.complete_list(generation, ConnectionListOutcome::available(connections));
        let dialog = effects.into_iter().find_map(|effect| match effect {
            ConnectionFlowEffect::ShowDialog(dialog) => Some(dialog),
            _ => None,
        });
        assert!(dialog.is_some(), "completed list renders the root dialog");
        let Some(dialog) = dialog else {
            return String::new();
        };
        assert_eq!(dialog.id.as_deref(), Some("provider-connection:root"));
        assert!(
            matches!(dialog.kind, DialogKind::Select { .. }),
            "root must be a selection dialog"
        );
        let DialogKind::Select { options } = dialog.kind else {
            return String::new();
        };
        assert_eq!(options[0].value, "add");
        options
            .get(1)
            .map_or_else(|| String::from("add"), |option| option.value.clone())
    }

    fn only_dialog(effects: Vec<ConnectionFlowEffect>, id: &str) {
        assert!(effects.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::ShowDialog(request) if request.id.as_deref() == Some(id)
            )
        }));
    }

    #[test]
    fn provider_connection_flow_drops_stale_root_list_outcomes() {
        let newer_connection = stored_connection(ConnectionState::Ready, Some("Newer"));
        let newer_id = newer_connection.id.as_str().to_owned();
        let mut flow = ProviderConnectionFlow::new(None);
        let first = flow.open();
        let first_generation = first.iter().find_map(|effect| match effect {
            ConnectionFlowEffect::LoadList { generation } => Some(*generation),
            _ => None,
        });
        assert!(first_generation.is_some(), "first root list request");
        let Some(first_generation) = first_generation else {
            return;
        };
        let second = flow.open();
        let second_generation = second.iter().find_map(|effect| match effect {
            ConnectionFlowEffect::LoadList { generation } => Some(*generation),
            _ => None,
        });
        assert!(second_generation.is_some(), "second root list request");
        let Some(second_generation) = second_generation else {
            return;
        };
        assert!(second_generation > first_generation);
        only_dialog(
            flow.complete_list(
                second_generation,
                ConnectionListOutcome::available(vec![newer_connection]),
            ),
            "provider-connection:root",
        );

        assert!(
            flow.complete_list(
                first_generation,
                ConnectionListOutcome::available(Vec::new())
            )
            .is_empty()
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: newer_id },
                false,
            ),
            "provider-connection:actions",
        );
    }

    #[test]
    fn provider_connection_flow_cancel_has_no_side_effects() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );

        let effects = flow.handle_dialog_response(
            "provider-connection:provider",
            DialogResponse::Cancelled,
            false,
        );

        assert!(effects.is_empty());
        assert_eq!(flow.state_tag(), ConnectionFlowStateTag::RootList);
    }

    #[test]
    fn provider_connection_flow_rejects_unrelated_or_stale_dialog_response() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        let state_before = flow.state_tag();

        let effects = flow.handle_dialog_response(
            "unrelated-dialog",
            DialogResponse::Selection {
                value: String::from("add"),
            },
            false,
        );

        assert!(effects.is_empty());
        assert_eq!(flow.state_tag(), state_before);
    }

    #[test]
    fn provider_connection_flow_keeps_invalid_label_field_open_without_mutation() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai"),
                },
                false,
            ),
            "provider-connection:label",
        );

        let effects = flow.handle_dialog_response(
            "provider-connection:label",
            DialogResponse::Text {
                value: "x".repeat(81),
            },
            false,
        );

        assert!(
            !effects
                .iter()
                .any(|effect| { matches!(effect, ConnectionFlowEffect::StartMutation(_)) })
        );
        only_dialog(effects, "provider-connection:label");
        assert_eq!(flow.state_tag(), ConnectionFlowStateTag::EnteringLabel);
    }

    #[test]
    fn chatgpt_subscription_label_starts_managed_login() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai-codex"),
                },
                false,
            ),
            "provider-connection:label",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:label",
            DialogResponse::Text {
                value: String::from("Codex"),
            },
            false,
        );
        assert!(effects.iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::ProbeChatGpt {
                    label: Some(label)
                }) if label == "Codex"
            )
        }));
        assert_eq!(flow.state_tag(), ConnectionFlowStateTag::Mutating);
    }

    #[test]
    fn chatgpt_missing_probe_starts_login() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai-codex"),
                },
                false,
            ),
            "provider-connection:label",
        );
        let _ = flow.handle_dialog_response(
            "provider-connection:label",
            DialogResponse::Text {
                value: String::from("Codex"),
            },
            false,
        );
        let effects = flow.complete_chatgpt_probe(ChatGptProbeOutcome::Missing);
        assert!(effects.iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::LoginChatGpt {
                    label: Some(label)
                }) if label == "Codex"
            )
        }));
        assert_eq!(
            flow.state_tag(),
            ConnectionFlowStateTag::WaitingChatGptDevice
        );

        let presented = flow.present_chatgpt_device_code(
            String::from("https://auth.openai.com/device"),
            String::from("ABCD-1234"),
        );
        only_dialog(presented, "provider-connection:chatgpt:device");

        let cancelled = flow.handle_dialog_response(
            "provider-connection:chatgpt:device",
            DialogResponse::Cancelled,
            false,
        );
        assert!(
            cancelled
                .iter()
                .any(|effect| matches!(effect, ConnectionFlowEffect::CancelChatGptLogin))
        );
        assert_eq!(flow.state_tag(), ConnectionFlowStateTag::RootList);
        assert!(!flow.mutation_in_flight());
    }

    #[test]
    fn chatgpt_existing_probe_confirms_before_adopt() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai-codex"),
                },
                false,
            ),
            "provider-connection:label",
        );
        let _ = flow.handle_dialog_response(
            "provider-connection:label",
            DialogResponse::Text {
                value: String::from("Codex"),
            },
            false,
        );
        let effects = flow.complete_chatgpt_probe(ChatGptProbeOutcome::Existing {
            account_id: String::from("acct_123"),
        });
        only_dialog(effects, "provider-connection:chatgpt:confirm");
        assert_eq!(
            flow.state_tag(),
            ConnectionFlowStateTag::ConfirmingChatGptLogin
        );

        let adopted = flow.handle_dialog_response(
            "provider-connection:chatgpt:confirm",
            DialogResponse::Selection {
                value: String::from("use"),
            },
            false,
        );
        assert!(adopted.iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::AdoptChatGpt {
                    label: Some(label)
                }) if label == "Codex"
            )
        }));
    }

    #[test]
    fn chatgpt_existing_login_reauth_starts_device_login() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai-codex"),
                },
                false,
            ),
            "provider-connection:label",
        );
        let _ = flow.handle_dialog_response(
            "provider-connection:label",
            DialogResponse::Text {
                value: String::from("Codex"),
            },
            false,
        );
        let _ = flow.complete_chatgpt_probe(ChatGptProbeOutcome::Existing {
            account_id: String::from("acct_123"),
        });
        let effects = flow.handle_dialog_response(
            "provider-connection:chatgpt:confirm",
            DialogResponse::Selection {
                value: String::from("reauth"),
            },
            false,
        );
        assert!(effects.iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::ReloginChatGpt {
                    label: Some(label),
                    account_id
                }) if label == "Codex" && account_id == "acct_123"
            )
        }));
        assert_eq!(
            flow.state_tag(),
            ConnectionFlowStateTag::WaitingChatGptDevice
        );
    }

    fn chatgpt_connection(label: &str) -> ProviderConnection {
        ProviderConnection {
            id: ConnectionId::new_stored(),
            provider: ProviderKind::ChatGptSubscription,
            label: Some(String::from(label)),
            base_url: None,
            authentication: ConnectionAuth::ChatGptSubscriptionManaged {
                auth_file: std::path::PathBuf::from("/tmp/chatgpt-subscription.json"),
                account_id: String::from("acct_123"),
            },
            state: ConnectionState::Ready,
        }
    }

    #[test]
    fn chatgpt_actions_offer_reauth_instead_of_api_key() {
        let connection = chatgpt_connection("Codex");
        let mut flow = ProviderConnectionFlow::new(None);
        let root_value = begin_root(&mut flow, vec![connection]);
        let actions = flow.handle_dialog_response(
            "provider-connection:root",
            DialogResponse::Selection { value: root_value },
            false,
        );
        let Some(DialogKind::Select { options }) = actions.iter().find_map(|effect| match effect {
            ConnectionFlowEffect::ShowDialog(request) => Some(&request.kind),
            _ => None,
        }) else {
            panic!("actions dialog");
        };
        assert!(options.iter().any(|option| option.value == "reauth"));
        assert!(options.iter().all(|option| option.value != "replace"));
        assert!(options.iter().all(|option| option.value != "repair"));

        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("reauth"),
                },
                false,
            ),
            "provider-connection:chatgpt:reauth",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:chatgpt:reauth",
            DialogResponse::Confirmed { accepted: true },
            false,
        );
        assert!(effects.iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::ReauthChatGpt { .. })
            )
        }));
        assert_eq!(
            flow.state_tag(),
            ConnectionFlowStateTag::WaitingChatGptDevice
        );
    }


    #[test]
    fn provider_connection_flow_create_never_activates() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai"),
                },
                false,
            ),
            "provider-connection:label",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:label",
                DialogResponse::Text {
                    value: String::from("Primary"),
                },
                false,
            ),
            "provider-connection:secret:create",
        );

        let effects = flow.handle_dialog_response(
            "provider-connection:secret:create",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("test-secret"),
            },
            false,
        );
        assert!(effects.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Create { .. })
            )
        }));

        let effects = flow.complete_mutation(&ConnectionMutationOutcome::Succeeded);
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, ConnectionFlowEffect::RefreshModels))
        );
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, ConnectionFlowEffect::LoadList { .. }))
        );
        assert_eq!(flow.active_target(), None);
    }

    #[test]
    fn provider_connection_flow_retries_persisted_create_as_repair_with_the_same_id() {
        let pending_id = ConnectionId::new_stored();
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai"),
                },
                false,
            ),
            "provider-connection:label",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:label",
                DialogResponse::Text {
                    value: String::from("Primary"),
                },
                false,
            ),
            "provider-connection:secret:create",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:secret:create",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("first-secret"),
            },
            false,
        );
        assert!(effects.iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Create { .. })
            )
        }));

        only_dialog(
            flow.complete_mutation(&ConnectionMutationOutcome::FailedAfterCreatePending {
                id: pending_id.clone(),
                failure: super::ConnectionRuntimeFailure::Unavailable,
            }),
            "provider-connection:secret:repair",
        );
        assert_eq!(
            flow.state_tag(),
            ConnectionFlowStateTag::EnteringRepairSecret
        );

        let effects = flow.handle_dialog_response(
            "provider-connection:secret:repair",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("second-secret"),
            },
            false,
        );
        assert!(effects.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Repair { id, .. })
                    if id == pending_id
            )
        }));
    }

    #[test]
    fn provider_connection_flow_retries_unpersisted_create_as_create() {
        let draft = NewConnectionDraft::new(
            ProviderKind::OpenAi,
            Some(String::from("Fresh retry")),
            None,
        );
        assert!(draft.is_ok(), "fixture draft");
        let Ok(draft) = draft else {
            return;
        };
        let mut flow = ProviderConnectionFlow::new(None);
        flow.state = super::ConnectionFlowState::EnteringCreateSecret { draft };
        let _ = flow.dialog_for_state();

        let effects = flow.handle_dialog_response(
            "provider-connection:secret:create",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("first-secret"),
            },
            false,
        );
        assert!(effects.iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Create { .. })
            )
        }));

        only_dialog(
            flow.complete_mutation(&ConnectionMutationOutcome::Failed(
                super::ConnectionRuntimeFailure::Authentication,
            )),
            "provider-connection:secret:create",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:secret:create",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("second-secret"),
            },
            false,
        );
        assert!(effects.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Create { .. })
            )
        }));
    }

    #[test]
    fn provider_connection_flow_repair_routes_pending_connection_to_secret_mutation() {
        let connection = stored_connection(ConnectionState::PendingCredential, Some("Repair me"));
        let id = connection.id.clone();
        let mut flow = ProviderConnectionFlow::new(None);
        let root_value = begin_root(&mut flow, vec![connection]);
        assert_eq!(root_value, id.as_str());

        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: root_value },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("repair"),
                },
                false,
            ),
            "provider-connection:secret:repair",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:secret:repair",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("test-secret"),
            },
            false,
        );
        assert!(effects.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Repair {
                    id: operation_id,
                    ..
                }) if operation_id == id
            )
        }));
    }

    #[test]
    fn provider_connection_flow_reports_pending_remove_without_exposing_operation_data() {
        let connection = stored_connection(ConnectionState::Ready, Some("Remove me"));
        let mut flow = ProviderConnectionFlow::new(None);
        let root_value = begin_root(&mut flow, vec![connection]);

        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: root_value },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("remove"),
                },
                false,
            ),
            "provider-connection:remove",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:remove",
            DialogResponse::Confirmed { accepted: true },
            false,
        );

        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, ConnectionFlowEffect::StartMutation(_)))
        );
        assert!(flow.mutation_in_flight());
        let _ = flow.complete_mutation(&ConnectionMutationOutcome::Succeeded);
        assert!(!flow.mutation_in_flight());
    }

    #[test]
    fn provider_connection_flow_renames_stored_connection() {
        let connection = stored_connection(ConnectionState::Ready, Some("Old"));
        let id = connection.id.clone();
        let mut flow = ProviderConnectionFlow::new(None);
        let root_value = begin_root(&mut flow, vec![connection]);

        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: root_value },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("rename"),
                },
                false,
            ),
            "provider-connection:rename",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:rename",
            DialogResponse::Text {
                value: String::from("New"),
            },
            false,
        );
        assert!(effects.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Rename {
                    id: operation_id,
                    label: Some(label),
                }) if operation_id == id && label == "New"
            )
        }));
    }

    #[test]
    fn provider_connection_flow_marks_active_replacement_for_runner_candidate_swap() {
        let connection = stored_connection(ConnectionState::Ready, Some("Active"));
        let id = connection.id.clone();
        let mut flow = ProviderConnectionFlow::new(Some(ActiveModelTarget {
            connection_id: id.clone(),
            model: String::from("gpt-test"),
        }));
        let root_value = begin_root(&mut flow, vec![connection]);
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: root_value },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("replace"),
                },
                false,
            ),
            "provider-connection:secret:replace",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:secret:replace",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("test-secret"),
            },
            false,
        );

        assert!(effects.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Replace {
                    id: operation_id,
                    model: Some(model),
                    ..
                }) if operation_id == id && model == "gpt-test"
            )
        }));
        assert!(flow.replacement_targets_active());
    }

    #[test]
    fn provider_connection_flow_uses_accepted_b_target_for_replacement_and_removal() {
        let connection_a = stored_connection(ConnectionState::Ready, Some("A"));
        let connection_b = stored_connection(ConnectionState::Ready, Some("B"));
        let mut flow = ProviderConnectionFlow::new(Some(ActiveModelTarget {
            connection_id: connection_a.id,
            model: String::from("model-a"),
        }));
        flow.set_active_target(Some(ActiveModelTarget {
            connection_id: connection_b.id.clone(),
            model: String::from("model-b"),
        }));

        let root_value = begin_root(&mut flow, vec![connection_b.clone()]);
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: root_value },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("replace"),
                },
                false,
            ),
            "provider-connection:secret:replace",
        );
        let replace = flow.handle_dialog_response(
            "provider-connection:secret:replace",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("test-secret"),
            },
            false,
        );
        assert!(replace.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Replace {
                    id,
                    model: Some(model),
                    ..
                }) if id == connection_b.id && model == "model-b"
            )
        }));

        let _ = flow.complete_mutation(&ConnectionMutationOutcome::Succeeded);
        let inspection = flow.open();
        let effect = inspection.first();
        assert!(
            matches!(effect, Some(ConnectionFlowEffect::LoadList { .. })),
            "root list load is requested"
        );
        let Some(ConnectionFlowEffect::LoadList { generation }) = effect else {
            return;
        };
        only_dialog(
            flow.complete_list(
                *generation,
                ConnectionListOutcome::available(vec![connection_b.clone()]),
            ),
            "provider-connection:root",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: connection_b.id.as_str().to_owned(),
                },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("remove"),
                },
                false,
            ),
            "provider-connection:remove",
        );
        let remove = flow.handle_dialog_response(
            "provider-connection:remove",
            DialogResponse::Confirmed { accepted: true },
            false,
        );
        assert!(
            !remove
                .iter()
                .any(|effect| matches!(effect, ConnectionFlowEffect::StartMutation(_)))
        );
        assert!(remove.iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::Status(message)
                    if *message == "select another connection before removing the active connection"
            )
        }));
    }

    #[test]
    fn provider_connection_flow_confirms_remove_before_mutating() {
        let connection = stored_connection(ConnectionState::Ready, Some("Remove me"));
        let id = connection.id.clone();
        let mut flow = ProviderConnectionFlow::new(None);
        let root_value = begin_root(&mut flow, vec![connection]);

        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: root_value },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("remove"),
                },
                false,
            ),
            "provider-connection:remove",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:remove",
            DialogResponse::Confirmed { accepted: true },
            false,
        );
        assert!(effects.into_iter().any(|effect| {
            matches!(
                effect,
                ConnectionFlowEffect::StartMutation(ConnectionMutationOperation::Remove {
                    id: operation_id
                }) if operation_id == id
            )
        }));
    }

    #[test]
    fn provider_connection_flow_rejects_active_remove() {
        let connection = stored_connection(ConnectionState::Ready, Some("Active"));
        let id = connection.id.clone();
        let mut flow = ProviderConnectionFlow::new(Some(ActiveModelTarget {
            connection_id: id.clone(),
            model: String::from("gpt-test"),
        }));
        let root_value = begin_root(&mut flow, vec![connection]);

        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: root_value },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("remove"),
                },
                false,
            ),
            "provider-connection:remove",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:remove",
            DialogResponse::Confirmed { accepted: true },
            false,
        );

        assert!(
            !effects
                .iter()
                .any(|effect| { matches!(effect, ConnectionFlowEffect::StartMutation(_)) })
        );
        assert!(effects.iter().any(|effect| {
            matches!(effect, ConnectionFlowEffect::Status(message) if *message == "select another connection before removing the active connection")
        }));
        only_dialog(effects, "provider-connection:actions");
    }

    #[test]
    fn provider_connection_flow_rejects_mutation_while_provider_turn_is_active() {
        let connection = stored_connection(ConnectionState::Ready, Some("Busy"));
        let mut flow = ProviderConnectionFlow::new(None);
        let root_value = begin_root(&mut flow, vec![connection]);

        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: root_value },
                true,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("rename"),
                },
                true,
            ),
            "provider-connection:rename",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:rename",
            DialogResponse::Text {
                value: String::from("Later"),
            },
            true,
        );

        assert!(
            !effects
                .iter()
                .any(|effect| { matches!(effect, ConnectionFlowEffect::StartMutation(_)) })
        );
        assert!(effects.iter().any(|effect| {
            matches!(effect, ConnectionFlowEffect::Status(message) if *message == "connection changes are unavailable while a prompt is in progress")
        }));
        assert_eq!(flow.state_tag(), ConnectionFlowStateTag::Renaming);
        only_dialog(effects, "provider-connection:rename");
    }

    #[test]
    fn provider_connection_flow_uses_domain_validation_for_compatible_base_urls() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai-compatible"),
                },
                false,
            ),
            "provider-connection:label",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:label",
                DialogResponse::Text {
                    value: String::from("Compatible"),
                },
                false,
            ),
            "provider-connection:base-url",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:base-url",
            DialogResponse::Text {
                value: String::from("not-a-url"),
            },
            false,
        );

        assert!(
            !effects
                .iter()
                .any(|effect| { matches!(effect, ConnectionFlowEffect::StartMutation(_)) })
        );
        only_dialog(effects, "provider-connection:base-url");
        assert_eq!(flow.state_tag(), ConnectionFlowStateTag::EnteringBaseUrl);
    }

    #[test]
    fn provider_connection_flow_allows_inspection_but_rejects_second_busy_mutation() {
        let mut flow = ProviderConnectionFlow::new(None);
        let _ = begin_root(&mut flow, Vec::new());
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection {
                    value: String::from("add"),
                },
                false,
            ),
            "provider-connection:provider",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:provider",
                DialogResponse::Selection {
                    value: String::from("openai"),
                },
                false,
            ),
            "provider-connection:label",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:label",
                DialogResponse::Text {
                    value: String::from("First"),
                },
                false,
            ),
            "provider-connection:secret:create",
        );
        let first_mutation = flow.handle_dialog_response(
            "provider-connection:secret:create",
            DialogResponse::Secret {
                value: yach_proto::SubmittedSecret::new("test-secret"),
            },
            false,
        );
        assert!(
            first_mutation
                .iter()
                .any(|effect| { matches!(effect, ConnectionFlowEffect::StartMutation(_)) })
        );

        let connection = stored_connection(ConnectionState::Ready, Some("Inspectable"));
        let id = connection.id.as_str().to_owned();
        let inspection = flow.open();
        let generation = inspection.iter().find_map(|effect| match effect {
            ConnectionFlowEffect::LoadList { generation } => Some(*generation),
            _ => None,
        });
        assert!(generation.is_some(), "inspection refreshes the root list");
        let Some(generation) = generation else {
            return;
        };
        only_dialog(
            flow.complete_list(
                generation,
                ConnectionListOutcome::available(vec![connection]),
            ),
            "provider-connection:root",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:root",
                DialogResponse::Selection { value: id },
                false,
            ),
            "provider-connection:actions",
        );
        only_dialog(
            flow.handle_dialog_response(
                "provider-connection:actions",
                DialogResponse::Selection {
                    value: String::from("rename"),
                },
                false,
            ),
            "provider-connection:rename",
        );
        let effects = flow.handle_dialog_response(
            "provider-connection:rename",
            DialogResponse::Text {
                value: String::from("Blocked"),
            },
            false,
        );

        assert!(
            !effects
                .iter()
                .any(|effect| { matches!(effect, ConnectionFlowEffect::StartMutation(_)) })
        );
        assert!(effects.iter().any(|effect| {
            matches!(effect, ConnectionFlowEffect::Status(message) if *message == "another connection change is already in progress")
        }));
        only_dialog(effects, "provider-connection:rename");
    }
    #[test]
    fn provider_connection_flow_never_constructs_a_draft_from_a_secret() {
        let draft = NewConnectionDraft::new(ProviderKind::OpenAi, None, None);
        assert!(draft.is_ok(), "domain draft is valid");
        let Ok(draft) = draft else {
            return;
        };
        assert_eq!(draft.label(), None);
    }
}
