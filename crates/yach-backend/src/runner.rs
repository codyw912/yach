use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use yach_proto::{
    BackendEvent, BackendState, Capability, ClientEvent, Handshake, LocalEditDecision, ModelInfo,
    PromptOutcome, ServerEvent, ToolResult, ToolReviewPayload,
};

use crate::agent_edit_tools::{
    AgentEditToolContext, AgentEditToolPrepared, PendingAgentEditToolReview,
    apply_agent_edit_tool_review, prepare_agent_edit_tool_request, reject_agent_edit_tool_review,
};
use crate::rig_adapter::{
    RigProviderAdapterConfig, RigProviderConfig, run_provider_request_with_approved_tools,
};
use crate::{
    DurationMetric, EditAccess, EditPolicy, EditPreviewId, EditTraceId, EditTraceOutcome,
    EditTracePhase, EditTraceRecord, EditTraceSource, EntryId, ExtensionStaticContextFile,
    JsonlSessionStore, MetricAttribute, PendingToolRequest, PermissionDecisionId, PermissionPolicy,
    ProjectReadOnlyToolExecutor, ProviderContinuationMappingError, ProviderContinuationRequest,
    ProviderContinuationValidationPolicy, ProviderError, ProviderErrorKind, ProviderFinishReason,
    ProviderMessage, ProviderMetadata, ProviderModel, ProviderRequest, ProviderStreamEvent,
    ProviderToolAdvertisingError, ProviderToolCall, ProviderToolResult, ProviderToolResultBlock,
    ProviderUsage, ResolvedToolCatalog, ResourceRoot, Role, SessionEvent, SessionEventSink,
    SessionId, SessionLog, StaticContextBundle, StaticContextItem, StaticContextPlacement,
    StaticContextPolicy, ToolContinuationError, ToolExecutionResult, ToolExecutor, ToolOutcome,
    ToolPayloadSummary, ToolPermissionPolicy, ToolRegistry, ToolRequestId, TurnId, TurnOutcome,
    assemble_project_static_context_with_extensions, build_provider_continuation_submission,
    build_provider_tool_advertising_extension, pending_tool_request_from_provider_call,
    record_native_tool_validation_with_resolved_catalog,
};
#[cfg(test)]
use crate::{ToolContinuationContext, ToolContinuationPolicy, ToolContinuationWorkflow};

mod extension_state;
mod local_edit;
mod session_state;

use extension_state::{
    ExtensionActivationSnapshotState, ExtensionManifestScanState,
    extension_activation_snapshot_from_state, extension_package_roots_for_scan,
    extension_static_context_files_from_scan_state,
    handle_native_extension_diagnostic_snapshot_request, handle_native_extension_lifecycle_request,
    schedule_extension_manifest_scan,
};
pub use extension_state::{ExtensionPackageRootLoader, StartupTraceMarker};
#[cfg(test)]
use local_edit::local_edit_error_message;
use local_edit::{
    LocalEditPrepareInput, handle_native_local_edit_decision, handle_native_local_edit_prepare,
    local_edit_preview_summary, local_edit_root,
};
#[cfg(test)]
use session_state::load_native_session_log_for_runner_with_loader;
use session_state::{
    load_native_session_log_for_runner, send_native_recent_sessions,
    send_native_session_messages_from_log, send_native_session_stats_from_log,
    send_native_session_stats_with_estimate, session_message_count, session_state_from_load_result,
};

/// Native runner configuration owned by the backend Module.
#[derive(Clone)]
pub struct RunnerConfig {
    pub session_path: PathBuf,
    pub project_root: Option<PathBuf>,
    pub provider: Option<ProviderConfig>,
    /// Why the native provider is unavailable, when the CLI could not build a
    /// provider config. Present only when `provider` is `None`; prompts fail
    /// with this message instead of falling back to fixture responses.
    pub provider_setup_error: Option<String>,
    pub extension_package_roots: Vec<crate::ExtensionPackageRoot>,
    pub extension_package_root_loader: Option<ExtensionPackageRootLoader>,
    pub startup_trace: Option<StartupTraceMarker>,
}

impl std::fmt::Debug for RunnerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunnerConfig")
            .field("session_path", &self.session_path)
            .field("project_root", &self.project_root)
            .field("provider", &self.provider)
            .field("provider_setup_error", &self.provider_setup_error)
            .field("extension_package_roots", &self.extension_package_roots)
            .field(
                "extension_package_root_loader",
                &self.extension_package_root_loader.is_some(),
            )
            .field("startup_trace", &self.startup_trace.is_some())
            .finish()
    }
}

/// Explicit native-provider settings supplied by the CLI Adapter.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub adapter: RigProviderAdapterConfig,
    pub model: String,
    pub test_delay_ms: Option<u64>,
}

impl ProviderConfig {
    #[must_use]
    pub fn provider_label(&self) -> &'static str {
        provider_label(&self.adapter.provider)
    }
}

const fn provider_label(provider: &RigProviderConfig) -> &'static str {
    match provider {
        RigProviderConfig::Anthropic { .. } => "anthropic",
        RigProviderConfig::ChatGptSubscription { .. } => "chatgpt-subscription",
        RigProviderConfig::OpenAiCompatible { .. } => "openai-compatible",
    }
}

#[derive(Debug)]
struct ActiveProviderTurn {
    handle: tokio::task::JoinHandle<SessionLog>,
    turn_id: TurnId,
    prompt_started: Instant,
    review_decision_tx: mpsc::UnboundedSender<AgentEditReviewDecision>,
}

async fn collect_finished_provider_turn(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    active_provider_turn: &mut Option<ActiveProviderTurn>,
    session_log: &mut SessionLog,
) {
    if !active_provider_turn
        .as_ref()
        .is_some_and(|turn| turn.handle.is_finished())
    {
        return;
    }

    let Some(active) = active_provider_turn.take() else {
        return;
    };
    match active.handle.await {
        Ok(updated_log) => *session_log = updated_log,
        Err(error) if error.is_cancelled() => {}
        Err(error) => {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("native provider: prompt task failed: {error}"),
            }));
        }
    }
}

#[derive(Clone)]
struct ProviderPromptProjectRuntime {
    project_context: Option<LaunchProjectContext>,
    extension_manifest_scan_state: ExtensionManifestScanState,
    extension_activation_state: ExtensionActivationSnapshotState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentEditReviewDecision {
    request_id: String,
    preview_id: String,
    permission_decision_id: String,
    decision: LocalEditDecision,
}

type AgentEditDecisionReceiver = mpsc::UnboundedReceiver<AgentEditReviewDecision>;
const EMPTY_ASSISTANT_RESPONSE_MESSAGE: &str = "assistant returned no text";

struct PromptSessionInput<'a> {
    current_session_id: &'a str,
    requested_session_id: String,
}

struct SessionSwitchState<'a> {
    current_session_path: &'a mut PathBuf,
    current_session_id: &'a mut String,
    store: &'a mut JsonlSessionStore,
    session_log: &'a mut SessionLog,
    turn_index: &'a mut u64,
    local_edit_index: &'a mut u64,
}

#[must_use]
pub fn session_log_dir() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".yach")
        .join("sessions")
}

#[must_use]
pub fn session_log_path(session_id: &str) -> PathBuf {
    session_log_dir().join(format!("{session_id}.jsonl"))
}

#[must_use]
pub fn fresh_session_id() -> String {
    let timestamp_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("session-{}-{timestamp_nanos}", std::process::id())
}

#[must_use]
pub fn session_id_from_log_path(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    if extension != "jsonl" {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_owned())
}

#[must_use]
pub fn latest_native_session_log_path() -> Option<PathBuf> {
    latest_native_session_log_path_in(&session_log_dir())
}

#[must_use]
pub fn latest_native_session_log_path_in(session_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(session_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| session_id_from_log_path(path).is_some())
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// Run the native backend event loop.
pub async fn run_native_loop(
    rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    config: RunnerConfig,
) {
    run_native_loop_with_requester_factory(rx, tx, config, |provider| RigProviderRequester {
        adapter: provider.adapter.clone(),
        approved_tools: provider_approved_tools(),
    })
    .await;
}

#[cfg(test)]
async fn run_native_loop_with_provider_requester<Requester>(
    rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    config: RunnerConfig,
    requester: Requester,
) where
    Requester: ProviderRequester + Send + 'static,
{
    let mut requester = Some(requester);
    run_native_loop_with_requester_factory(rx, tx, config, move |_| {
        let Some(requester) = requester.take() else {
            unreachable!("test provider requester can only be used once");
        };
        requester
    })
    .await;
}

async fn run_native_loop_with_requester_factory<MakeRequester, Requester>(
    mut rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    config: RunnerConfig,
    mut make_requester: MakeRequester,
) where
    MakeRequester: FnMut(&ProviderConfig) -> Requester,
    Requester: ProviderRequester + Send + 'static,
{
    let RunnerConfig {
        mut session_path,
        project_root,
        mut provider,
        provider_setup_error,
        extension_package_roots,
        extension_package_root_loader,
        startup_trace,
    } = config;
    let mut current_session_id =
        session_id_from_log_path(&session_path).unwrap_or_else(|| String::from("default"));
    let mut store = JsonlSessionStore::new(session_path.clone());
    let provider_project_context = project_root
        .as_ref()
        .and_then(launch_project_context_from_root);
    let edit_root = local_edit_root(project_root.clone());
    let mut edit_access = EditAccess::default();
    send_native_initial_state(
        &tx,
        &current_session_id,
        &session_path,
        provider.as_ref(),
        provider_setup_error.as_deref(),
    );
    for warning in crate::SensitivePathPolicy::load_for_project(project_root.as_deref()).1 {
        let message = match warning {
            crate::SensitivePathConfigWarning::InvalidConfig { path, .. } => {
                format!(
                    "sensitive_file_config: invalid config at {path}; built-in deny defaults remain in force"
                )
            }
            crate::SensitivePathConfigWarning::InvalidPattern { pattern } => {
                format!(
                    "sensitive_file_config: invalid pattern {pattern:?}; built-in deny defaults remain in force"
                )
            }
        };
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated { message }));
    }
    let mut session_log = load_native_session_log_for_runner(&tx, &store).await;
    // Push stats (and therefore the context meter) at startup; previously
    // the meter stayed empty until the first turn finished.
    send_native_session_stats_from_log(
        &tx,
        &session_log,
        context_budget(provider.as_ref(), project_root.as_deref()),
    );
    let mut turn_index = session_log.next_turn_index();
    let mut local_edit_index = turn_index;
    let mut active_provider_turn: Option<ActiveProviderTurn> = None;
    let mut extension_manifest_scan_scheduled = false;
    let extension_manifest_scan_state = Arc::new(AsyncMutex::new(None));
    let extension_activation_state = Arc::new(AsyncMutex::new(
        crate::ExtensionActivationSnapshot::default(),
    ));

    while let Some(event) = rx.recv().await {
        collect_finished_provider_turn(&tx, &mut active_provider_turn, &mut session_log).await;
        match event {
            ClientEvent::Initialize(_) => {
                send_native_initial_state(
                    &tx,
                    &current_session_id,
                    &session_path,
                    provider.as_ref(),
                    provider_setup_error.as_deref(),
                );
            }
            ClientEvent::FirstRenderCompleted => {
                let extension_package_roots = extension_package_roots_for_scan(
                    &extension_package_roots,
                    extension_package_root_loader.as_ref(),
                );
                schedule_extension_manifest_scan(
                    &tx,
                    extension_package_roots,
                    extension_manifest_scan_state.clone(),
                    extension_activation_state.clone(),
                    startup_trace.clone(),
                    &mut extension_manifest_scan_scheduled,
                );
            }
            ClientEvent::AvailableModelsRequested => {
                send_native_models(&tx, provider.as_ref(), provider_setup_error.as_deref());
            }
            ClientEvent::PromptCancelled { .. } => {
                if let Some(active) = active_provider_turn.take()
                    && !active.handle.is_finished()
                {
                    active.handle.abort();
                    let _ = active.handle.await;
                    persist_native_cancelled_turn(
                        &tx,
                        &store,
                        &mut session_log,
                        &SessionId(current_session_id.clone()),
                        active.turn_id,
                        active.prompt_started,
                        "native provider prompt cancelled",
                    );
                }
            }
            ClientEvent::RecentSessionsRequested => send_native_recent_sessions(&tx, &session_path),
            ClientEvent::SessionMessagesRequested => {
                send_native_session_messages_from_log(&tx, &session_log);
            }
            ClientEvent::SessionStatsRequested => {
                send_native_session_stats_from_log(
                    &tx,
                    &session_log,
                    context_budget(provider.as_ref(), project_root.as_deref()),
                );
            }
            ClientEvent::CompactionRequested { instructions, .. } => {
                if active_provider_turn
                    .as_ref()
                    .is_some_and(|active| !active.handle.is_finished())
                {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from(
                            "compaction unavailable while a prompt is in progress",
                        ),
                    }));
                    continue;
                }
                let Some(provider) = provider.clone() else {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("compaction requires a configured provider"),
                    }));
                    continue;
                };
                let compaction_config =
                    crate::CompactionConfig::load_for_project(project_root.as_deref());
                if crate::select_compaction_cut(&session_log, compaction_config.keep_recent_tokens)
                    .is_none()
                {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("nothing to compact yet"),
                    }));
                    continue;
                }
                let compaction_turn = TurnId(format!("turn-{turn_index}"));
                turn_index = turn_index.saturating_add(1);
                local_edit_index = local_edit_index.max(turn_index);
                let mut requester = make_requester(&provider);
                let model = ProviderModel {
                    provider: provider.provider_label().to_owned(),
                    model: provider.model.clone(),
                };
                let tokens_before = crate::estimate_current_context_tokens(&session_log);
                let mut pending_events = Vec::new();
                let result = run_compaction(
                    &mut requester,
                    CompactionRun {
                        session_id: &SessionId(current_session_id.clone()),
                        turn_id: &compaction_turn,
                        model: &model,
                        config: &compaction_config,
                        reason: crate::CompactionReason::Manual,
                        tokens_before,
                        focus_instructions: instructions,
                        log: &mut session_log,
                        pending_events: &mut pending_events,
                        tool_event_store: Some(&store),
                        review_tx: &tx,
                    },
                )
                .await;
                match result {
                    Ok(true) => {
                        send_native_session_messages_from_log(&tx, &session_log);
                        send_native_session_stats_from_log(
                            &tx,
                            &session_log,
                            context_budget(Some(&provider), project_root.as_deref()),
                        );
                    }
                    Ok(false) => {}
                    Err(error) => {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                            message: format!(
                                "manual compaction failed: {}",
                                provider_round_error_label(&error)
                            ),
                        }));
                    }
                }
            }
            ClientEvent::PromptSubmitted { session_id, prompt } => {
                if prompt.trim().is_empty() {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("empty prompt ignored"),
                    }));
                    continue;
                }
                let prompt_turn_index = turn_index;
                turn_index = turn_index.saturating_add(1);
                local_edit_index = local_edit_index.max(turn_index);
                if let Some(provider) = provider.clone() {
                    if active_provider_turn
                        .as_ref()
                        .is_some_and(|active| active.handle.is_finished())
                    {
                        active_provider_turn = None;
                    }
                    if active_provider_turn.is_some() {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                            message: String::from("native provider: prompt already in progress"),
                        }));
                        continue;
                    }
                    let prompt_started = Instant::now();
                    let Some(started_prompt) = start_native_prompt(
                        &tx,
                        &store,
                        &mut session_log,
                        PromptSessionInput {
                            current_session_id: &current_session_id,
                            requested_session_id: session_id,
                        },
                        prompt,
                        prompt_turn_index,
                        prompt_started,
                    ) else {
                        continue;
                    };
                    let turn_id = started_prompt.turn.clone();
                    let requester = make_requester(&provider);
                    let (review_decision_tx, review_decision_rx) = mpsc::unbounded_channel();
                    let handle = tokio::spawn(handle_started_native_provider_prompt(
                        tx.clone(),
                        store.clone(),
                        provider,
                        started_prompt,
                        requester,
                        ProviderPromptProjectRuntime {
                            project_context: provider_project_context.clone(),
                            extension_manifest_scan_state: extension_manifest_scan_state.clone(),
                            extension_activation_state: extension_activation_state.clone(),
                        },
                        review_decision_rx,
                    ));
                    active_provider_turn = Some(ActiveProviderTurn {
                        handle,
                        turn_id,
                        prompt_started,
                        review_decision_tx,
                    });
                } else if let Some(setup_error) = provider_setup_error.as_deref() {
                    handle_native_prompt_unconfigured_provider(
                        &tx,
                        &store,
                        &mut session_log,
                        PromptSessionInput {
                            current_session_id: &current_session_id,
                            requested_session_id: session_id,
                        },
                        &UnconfiguredProviderPrompt {
                            prompt: &prompt,
                            turn_index: prompt_turn_index,
                            prompt_started: Instant::now(),
                            setup_error,
                        },
                    );
                } else {
                    let prompt_started = Instant::now();
                    handle_native_prompt(
                        &tx,
                        &store,
                        &mut session_log,
                        PromptSessionInput {
                            current_session_id: &current_session_id,
                            requested_session_id: session_id,
                        },
                        &prompt,
                        prompt_turn_index,
                        prompt_started,
                    );
                }
            }
            ClientEvent::ModelSelected { model } => {
                apply_native_model_selection(
                    &tx,
                    &mut provider,
                    active_provider_turn.as_ref(),
                    None,
                    model,
                );
            }
            ClientEvent::ModelSelectedDetailed {
                provider: selected_provider,
                model_id,
            } => {
                apply_native_model_selection(
                    &tx,
                    &mut provider,
                    active_provider_turn.as_ref(),
                    Some(&selected_provider),
                    model_id,
                );
            }
            ClientEvent::SessionSelected { session_id } => {
                if active_provider_turn.is_some() {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("cannot switch sessions during an active prompt"),
                    }));
                    continue;
                }
                let selected_path = session_path_for_id_in_dir(session_path.parent(), &session_id);
                let Some(selected_path) = selected_path else {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: format!("unknown session {session_id}"),
                    }));
                    continue;
                };
                if !session_path_is_selectable(&selected_path, &session_path) {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: format!("unknown session {session_id}"),
                    }));
                    continue;
                }
                switch_native_session(
                    &tx,
                    selected_path,
                    SessionSwitchState {
                        current_session_path: &mut session_path,
                        current_session_id: &mut current_session_id,
                        store: &mut store,
                        session_log: &mut session_log,
                        turn_index: &mut turn_index,
                        local_edit_index: &mut local_edit_index,
                    },
                    context_budget(provider.as_ref(), project_root.as_deref()),
                )
                .await;
            }
            ClientEvent::SessionPathSelected {
                session_path: selected_session_path,
            } => {
                if active_provider_turn.is_some() {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("cannot switch sessions during an active prompt"),
                    }));
                    continue;
                }
                let selected_path = PathBuf::from(&selected_session_path);
                if !session_path_is_selectable(&selected_path, &session_path) {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: format!("unknown session path {selected_session_path}"),
                    }));
                    continue;
                }
                switch_native_session(
                    &tx,
                    selected_path,
                    SessionSwitchState {
                        current_session_path: &mut session_path,
                        current_session_id: &mut current_session_id,
                        store: &mut store,
                        session_log: &mut session_log,
                        turn_index: &mut turn_index,
                        local_edit_index: &mut local_edit_index,
                    },
                    context_budget(provider.as_ref(), project_root.as_deref()),
                )
                .await;
            }
            ClientEvent::ForkMessagesRequested | ClientEvent::SessionForkRequested { .. } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: String::from("fork/session tree UI is not available yet"),
                }));
            }
            ClientEvent::ThinkingLevelSelected { level } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!("thinking level {level} noted but not used yet"),
                }));
            }
            ClientEvent::LocalEditPrepareRequested {
                request_id,
                operation,
            } => {
                let edit_turn_index = local_edit_index.max(turn_index);
                local_edit_index = edit_turn_index.saturating_add(1);
                turn_index = turn_index.max(local_edit_index);
                handle_native_local_edit_prepare(
                    &tx,
                    &store,
                    &mut edit_access,
                    edit_root.as_ref(),
                    LocalEditPrepareInput {
                        session_id: SessionId(current_session_id.clone()),
                        request_id,
                        operation,
                        turn_index: edit_turn_index,
                    },
                );
            }
            ClientEvent::LocalEditDecisionSubmitted {
                preview_id,
                permission_decision_id,
                decision,
            } => {
                handle_native_local_edit_decision(
                    &tx,
                    &store,
                    &mut edit_access,
                    preview_id,
                    permission_decision_id,
                    decision,
                );
            }
            ClientEvent::ToolReviewDecisionSubmitted {
                request_id,
                preview_id,
                permission_decision_id,
                decision,
            } => {
                let decision = AgentEditReviewDecision {
                    request_id,
                    preview_id,
                    permission_decision_id,
                    decision,
                };
                let forwarded = active_provider_turn
                    .as_ref()
                    .is_some_and(|active| active.review_decision_tx.send(decision).is_ok());
                if !forwarded {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("native provider: stale tool review decision"),
                    }));
                }
            }
            ClientEvent::ExtensionLifecycleRequested {
                request_id,
                action,
                selector,
            } => {
                handle_native_extension_lifecycle_request(
                    &tx,
                    &extension_manifest_scan_state,
                    &extension_activation_state,
                    request_id,
                    action,
                    &selector,
                )
                .await;
            }
            ClientEvent::ExtensionDiagnosticSnapshotRequested {
                request_id,
                selector,
            } => {
                handle_native_extension_diagnostic_snapshot_request(
                    &tx,
                    &extension_activation_state,
                    request_id,
                    selector.as_deref(),
                )
                .await;
            }
            ClientEvent::DialogResolved { .. } | ClientEvent::WidgetCleared { .. } => {}
        }
    }
}

fn session_path_for_id_in_dir(session_dir: Option<&Path>, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() || session_id == "." || session_id == ".." {
        return None;
    }
    if session_id.contains('/') || session_id.contains('\\') {
        return None;
    }
    Some(session_dir?.join(format!("{session_id}.jsonl")))
}

fn session_path_is_selectable(selected_session_path: &Path, current_session_path: &Path) -> bool {
    session_id_from_log_path(selected_session_path).is_some()
        && selected_session_path.exists()
        && selected_session_path.parent() == current_session_path.parent()
}

async fn switch_native_session(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    selected_path: PathBuf,
    state: SessionSwitchState<'_>,
    context_budget: Option<crate::ContextBudget>,
) {
    let SessionSwitchState {
        current_session_path,
        current_session_id,
        store,
        session_log,
        turn_index,
        local_edit_index,
    } = state;
    let Some(selected_session_id) = session_id_from_log_path(&selected_path) else {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("unknown session path {}", selected_path.display()),
        }));
        return;
    };
    let selected_store = JsonlSessionStore::new(selected_path.clone());
    let load_store = selected_store.clone();
    let loaded = match tokio::task::spawn_blocking(move || load_store.load_with_warnings()).await {
        Ok(load_result) => session_state_from_load_result(tx, load_result),
        Err(error) => {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("failed to load session log: {error}"),
            }));
            return;
        }
    };
    *current_session_path = selected_path;
    current_session_id.clone_from(&selected_session_id);
    *store = selected_store;
    *session_log = loaded;
    *turn_index = session_log.next_turn_index();
    *local_edit_index = *turn_index;
    let _ = tx.send(BackendEvent::Server(ServerEvent::SessionChanged {
        session_id: selected_session_id,
    }));
    send_native_session_messages_from_log(tx, session_log);
    send_native_session_stats_from_log(tx, session_log, context_budget);
}

fn send_native_initial_state(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_id: &str,
    session_path: &Path,
    provider: Option<&ProviderConfig>,
    provider_setup_error: Option<&str>,
) {
    let session_file = Some(session_path.to_string_lossy().into_owned());
    let _ = tx.send(BackendEvent::Server(ServerEvent::Ready {
        handshake: Handshake::new(
            "yach-native",
            vec![
                Capability::PromptStreaming,
                Capability::PromptCancellation,
                Capability::LocalEdit,
                Capability::ExtensionLifecycle,
                Capability::FirstRenderEvents,
                Capability::ToolOutputStreaming,
            ],
        ),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::StateUpdated(
        BackendState {
            model_id: Some(active_model(provider, provider_setup_error).id),
            model_name: Some(active_model(provider, provider_setup_error).name),
            model_provider: Some(active_model(provider, provider_setup_error).provider),
            session_id: Some(session_id.to_owned()),
            session_file,
            thinking_level: Some(String::from("low")),
            is_streaming: false,
            is_compacting: false,
            message_count: session_message_count(session_path),
            pending_message_count: Some(0),
        },
    )));
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: backend_status_message(provider, provider_setup_error),
    }));
    send_native_models(tx, provider, provider_setup_error);
}

/// Switch the runner's provider model between turns. Refused while a
/// prompt is in progress (the active turn cloned the old config) and when
/// the selection names a different provider than the configured one.
fn apply_native_model_selection(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    provider: &mut Option<ProviderConfig>,
    active_provider_turn: Option<&ActiveProviderTurn>,
    selected_provider: Option<&str>,
    model: String,
) {
    if active_provider_turn.is_some_and(|active| !active.handle.is_finished()) {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: String::from("model change unavailable while a prompt is in progress"),
        }));
        return;
    }
    let Some(provider_config) = provider.as_mut() else {
        let _ = tx.send(BackendEvent::Server(ServerEvent::ModelChanged { model }));
        return;
    };
    if let Some(selected_provider) = selected_provider
        && selected_provider != provider_config.provider_label()
    {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!(
                "model change rejected: provider {selected_provider} is not the configured \
provider ({})",
                provider_config.provider_label()
            ),
        }));
        return;
    }
    provider_config.model.clone_from(&model);
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: format!("model changed to {model}"),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::ModelChanged { model }));
}

/// Curated model choices per provider. A stopgap list like the other
/// per-model constants: replaced by real model-catalog metadata in the
/// flagged revisit.
const ANTHROPIC_MODEL_CHOICES: &[(&str, &str)] = &[
    ("claude-sonnet-5", "Claude Sonnet 5"),
    ("claude-opus-4-8", "Claude Opus 4.8"),
    ("claude-haiku-4-5", "Claude Haiku 4.5"),
];

fn send_native_models(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    provider: Option<&ProviderConfig>,
    provider_setup_error: Option<&str>,
) {
    let active = active_model(provider, provider_setup_error);
    let mut models = vec![active.clone()];
    if provider.is_some_and(|provider| provider.provider_label() == "anthropic") {
        models.extend(
            ANTHROPIC_MODEL_CHOICES
                .iter()
                .filter(|(id, _)| *id != active.id)
                .map(|(id, name)| ModelInfo {
                    id: (*id).to_owned(),
                    name: (*name).to_owned(),
                    provider: String::from("anthropic"),
                }),
        );
    }
    let _ = tx.send(BackendEvent::Server(ServerEvent::AvailableModelsUpdated {
        models,
    }));
}

fn active_model(
    provider: Option<&ProviderConfig>,
    provider_setup_error: Option<&str>,
) -> ModelInfo {
    let Some(provider) = provider else {
        if provider_setup_error.is_some() {
            return ModelInfo {
                id: String::from("provider-unconfigured"),
                name: String::from("Provider Not Configured"),
                provider: String::from("native"),
            };
        }
        return ModelInfo {
            id: String::from("fixture-echo"),
            name: String::from("Fixture Echo"),
            provider: String::from("native"),
        };
    };
    let provider_label = provider.provider_label();
    let id = provider.model.clone();
    ModelInfo {
        name: id.clone(),
        id,
        provider: provider_label.to_owned(),
    }
}

fn backend_status_message(
    provider: Option<&ProviderConfig>,
    provider_setup_error: Option<&str>,
) -> String {
    if let Some(provider) = provider {
        let model = active_model(Some(provider), None);
        format!(
            "backend: {}/{}; read/search/list and exact/create edit tools available",
            model.provider, model.id
        )
    } else if let Some(setup_error) = provider_setup_error {
        format!("{setup_error}; set the provider environment and relaunch yach tui")
    } else {
        String::from(
            "backend: no provider configured; local read-only project inspection available",
        )
    }
}

fn handle_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &JsonlSessionStore,
    log: &mut SessionLog,
    session: PromptSessionInput<'_>,
    prompt: &str,
    turn_index: u64,
    prompt_started: Instant,
) {
    let session_id =
        if session.requested_session_id.is_empty() || session.requested_session_id == "default" {
            session.current_session_id.to_owned()
        } else {
            session.requested_session_id
        };
    if session_id != session.current_session_id {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("unknown session {session_id}"),
        }));
        return;
    }
    let typed_session_id = SessionId(session_id.clone());

    let turn_id = TurnId(format!("turn-{turn_index}"));
    let user_entry_id = EntryId(format!("entry-{turn_index}-user"));
    let assistant_entry_id = EntryId(format!("entry-{turn_index}-assistant"));
    let response = format!("fixture response: {prompt}");
    let fixture_outcome = fixture_outcome(prompt);
    let mut pending_events = Vec::new();
    push_native_session_event(
        log,
        &mut pending_events,
        SessionEvent::EntryAppended {
            session_id: typed_session_id.clone(),
            entry_id: user_entry_id.clone(),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: Role::User,
            text: prompt.to_owned(),
            provider: None,
        },
    );

    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("turn_start"),
    }));

    if let Err(error) = append_pending_native_session_events(store, &mut pending_events) {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("failed to persist session log: {error}"),
        }));
    }

    match fixture_outcome {
        FixtureOutcome::Completed => {
            for delta in response_chunks(&response) {
                if tx
                    .send(BackendEvent::Server(ServerEvent::PromptDelta {
                        session_id: session_id.clone(),
                        delta,
                    }))
                    .is_err()
                {
                    push_native_prompt_total_metric(
                        log,
                        &mut pending_events,
                        &typed_session_id,
                        &turn_id,
                        prompt_started,
                    );
                    push_native_session_event(
                        log,
                        &mut pending_events,
                        SessionEvent::TurnFinished {
                            session_id: typed_session_id.clone(),
                            turn_id,
                            outcome: TurnOutcome::Cancelled,
                            reason: Some(String::from("ui receiver dropped")),
                        },
                    );
                    let _ = append_pending_native_session_events(store, &mut pending_events);
                    return;
                }
            }
            push_native_prompt_total_metric(
                log,
                &mut pending_events,
                &typed_session_id,
                &turn_id,
                prompt_started,
            );
            push_native_session_event(
                log,
                &mut pending_events,
                SessionEvent::EntryAppended {
                    session_id: typed_session_id.clone(),
                    entry_id: assistant_entry_id,
                    parent_entry_id: Some(user_entry_id),
                    turn_id: turn_id.clone(),
                    role: Role::Assistant,
                    text: response,
                    provider: None,
                },
            );
            push_native_session_event(
                log,
                &mut pending_events,
                SessionEvent::TurnFinished {
                    session_id: typed_session_id.clone(),
                    turn_id,
                    outcome: TurnOutcome::Completed,
                    reason: None,
                },
            );
        }
        FixtureOutcome::Failed => {
            push_native_prompt_total_metric(
                log,
                &mut pending_events,
                &typed_session_id,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                log,
                &mut pending_events,
                &typed_session_id,
                turn_id,
                TurnOutcome::Failed,
                &ProviderError::fixture_failure(),
            );
        }
        FixtureOutcome::Malformed => {
            push_native_prompt_total_metric(
                log,
                &mut pending_events,
                &typed_session_id,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                log,
                &mut pending_events,
                &typed_session_id,
                turn_id,
                TurnOutcome::Failed,
                &ProviderError::malformed_stream("fixture malformed stream"),
            );
        }
        FixtureOutcome::Cancelled => {
            push_native_prompt_total_metric(
                log,
                &mut pending_events,
                &typed_session_id,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                log,
                &mut pending_events,
                &typed_session_id,
                turn_id,
                TurnOutcome::Cancelled,
                &ProviderError::cancelled("fixture cancellation"),
            );
        }
    }

    let status = match append_pending_native_session_events(store, &mut pending_events) {
        Ok(()) => fixture_outcome.status_message().to_owned(),
        Err(error) => format!("failed to persist session log: {error}"),
    };
    let outcome = fixture_outcome.prompt_outcome();
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: status.clone(),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::PromptFinished {
        session_id,
        outcome,
        message: Some(status),
    }));
    send_native_session_stats_from_log(tx, log, None);
}

/// Prompt details for a native session whose provider could not be configured.
struct UnconfiguredProviderPrompt<'a> {
    prompt: &'a str,
    turn_index: u64,
    prompt_started: Instant,
    setup_error: &'a str,
}

/// Fail a submitted prompt with the provider setup error instead of producing
/// fixture output, so an unconfigured launch stays honest and recoverable.
fn handle_native_prompt_unconfigured_provider(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &JsonlSessionStore,
    log: &mut SessionLog,
    session: PromptSessionInput<'_>,
    prompt: &UnconfiguredProviderPrompt<'_>,
) {
    let session_id =
        if session.requested_session_id.is_empty() || session.requested_session_id == "default" {
            session.current_session_id.to_owned()
        } else {
            session.requested_session_id
        };
    if session_id != session.current_session_id {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("unknown session {session_id}"),
        }));
        return;
    }
    let typed_session_id = SessionId(session_id);

    let turn_id = TurnId(format!("turn-{}", prompt.turn_index));
    let user_entry_id = EntryId(format!("entry-{}-user", prompt.turn_index));
    let mut pending_events = Vec::new();
    push_native_session_event(
        log,
        &mut pending_events,
        SessionEvent::EntryAppended {
            session_id: typed_session_id.clone(),
            entry_id: user_entry_id,
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: Role::User,
            text: prompt.prompt.to_owned(),
            provider: None,
        },
    );
    push_native_prompt_total_metric(
        log,
        &mut pending_events,
        &typed_session_id,
        &turn_id,
        prompt.prompt_started,
    );
    push_native_session_event(
        log,
        &mut pending_events,
        SessionEvent::TurnFinished {
            session_id: typed_session_id.clone(),
            turn_id,
            outcome: TurnOutcome::Failed,
            reason: Some(format!("provider_unconfigured {}", prompt.setup_error)),
        },
    );
    finish_native_prompt(
        tx,
        store,
        log,
        &mut pending_events,
        PromptCompletion {
            session_id: &typed_session_id.0,
            status: &format!(
                "{}; set the provider environment and relaunch yach tui",
                prompt.setup_error
            ),
            outcome: PromptOutcome::Failed,
            context_budget: None,
        },
    );
}

#[derive(Debug, Clone)]
struct StartedPrompt {
    session_id: SessionId,
    prompt: String,
    log: SessionLog,
    pending_events: Vec<SessionEvent>,
    turn: TurnId,
    user_entry: EntryId,
    assistant_entry: EntryId,
    prompt_started: Instant,
}

fn start_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &JsonlSessionStore,
    log: &mut SessionLog,
    session: PromptSessionInput<'_>,
    prompt: String,
    turn_index: u64,
    prompt_started: Instant,
) -> Option<StartedPrompt> {
    let session_id =
        if session.requested_session_id.is_empty() || session.requested_session_id == "default" {
            session.current_session_id.to_owned()
        } else {
            session.requested_session_id
        };
    if session_id != session.current_session_id {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("unknown session {session_id}"),
        }));
        return None;
    }
    let typed_session_id = SessionId(session_id);

    let turn = TurnId(format!("turn-{turn_index}"));
    let user_entry = EntryId(format!("entry-{turn_index}-user"));
    let assistant_entry = EntryId(format!("entry-{turn_index}-assistant"));
    let mut pending_events = Vec::new();
    push_native_session_event(
        log,
        &mut pending_events,
        SessionEvent::EntryAppended {
            session_id: typed_session_id.clone(),
            entry_id: user_entry.clone(),
            parent_entry_id: None,
            turn_id: turn.clone(),
            role: Role::User,
            text: prompt.clone(),
            provider: None,
        },
    );

    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("turn_start"),
    }));

    if let Err(error) = append_pending_native_session_events(store, &mut pending_events) {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("failed to persist session log: {error}"),
        }));
        return None;
    }

    Some(StartedPrompt {
        session_id: typed_session_id,
        prompt,
        log: log.clone(),
        pending_events,
        turn,
        user_entry,
        assistant_entry,
        prompt_started,
    })
}

fn push_native_session_event(
    log: &mut SessionLog,
    pending_events: &mut Vec<SessionEvent>,
    event: SessionEvent,
) {
    log.push(event.clone());
    pending_events.push(event);
}

fn push_native_prompt_total_metric(
    log: &mut SessionLog,
    pending_events: &mut Vec<SessionEvent>,
    session_id: &SessionId,
    turn_id: &TurnId,
    prompt_started: Instant,
) {
    push_native_session_event(
        log,
        pending_events,
        duration_metric_event(
            session_id.clone(),
            Some(turn_id.clone()),
            "prompt_total",
            prompt_started.elapsed(),
        ),
    );
}

fn duration_metric_event(
    session_id: SessionId,
    turn_id: Option<TurnId>,
    name: impl Into<String>,
    duration: Duration,
) -> SessionEvent {
    let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    SessionEvent::MetricRecorded {
        session_id,
        turn_id,
        metric: DurationMetric {
            name: name.into(),
            duration_ms,
            attributes: Vec::new(),
        },
    }
}

/// Compact one-line-per-message rendering of provider context for shape
/// assertions and diagnostics (the Codex snapshot pattern): `role:prefix`.
#[must_use]
pub fn provider_message_shapes(messages: &[ProviderMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|message| {
            let role = match message.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
                Role::System => "system",
            };
            let prefix: String = message
                .content
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(48)
                .collect();
            format!("{role}:{}", prefix.trim_end())
        })
        .collect()
}

/// Continuation frame wrapping a compaction summary in provider context.
fn compaction_summary_message(summary: &str) -> ProviderMessage {
    ProviderMessage::text(
        Role::System,
        format!(
            "Earlier work in this session was compacted. The summary below is \
authoritative for everything before the messages that follow it.\n\n{summary}"
        ),
    )
}

fn provider_messages_from_log(log: &SessionLog, current_turn_id: &TurnId) -> Vec<ProviderMessage> {
    let checkpoint = crate::compaction::newest_compaction_checkpoint(log);
    let kept_events = checkpoint.as_ref().map_or(&log.events[..], |view| {
        &log.events[view.kept_start_index.min(log.events.len())..]
    });

    let completed_turns = log
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TurnFinished {
                turn_id,
                outcome: TurnOutcome::Completed,
                ..
            } => Some(turn_id),
            SessionEvent::EntryAppended { .. }
            | SessionEvent::ToolRequestRecorded { .. }
            | SessionEvent::ToolExecutionFinished { .. }
            | SessionEvent::TurnFinished { .. }
            | SessionEvent::MetricRecorded { .. }
            | SessionEvent::StaticContextIncluded { .. }
            | SessionEvent::PermissionDecisionRecorded { .. }
            | SessionEvent::EditTraceRecorded { .. }
            | SessionEvent::EditTransactionPrepared { .. }
            | SessionEvent::EditTransactionFinished { .. }
            | SessionEvent::CompactionCheckpoint { .. } => None,
        })
        .collect::<std::collections::HashSet<_>>();

    // tool_request_id -> (tool name, argument json, call id)
    let mut tool_context_by_request_id: std::collections::HashMap<
        String,
        (String, Option<String>, String),
    > = std::collections::HashMap::new();
    let mut messages = checkpoint
        .map(|view| vec![compaction_summary_message(view.summary)])
        .unwrap_or_default();
    messages.extend(kept_events.iter().flat_map(|event| match event {
        SessionEvent::EntryAppended {
            turn_id,
            role,
            text,
            ..
        } if turn_id == current_turn_id || completed_turns.contains(turn_id) => {
            vec![ProviderMessage::text(*role, text.clone())]
        }
        SessionEvent::ToolRequestRecorded {
            turn_id,
            tool_request_id,
            tool_name,
            provider_call_id,
            argument_content,
            ..
        } if turn_id == current_turn_id || completed_turns.contains(turn_id) => {
            // The provider's own call id when the log has it; otherwise a
            // deterministic one derived from the request id. Determinism
            // matters for prompt-cache stability across rebuilds.
            let call_id = provider_call_id
                .clone()
                .unwrap_or_else(|| format!("yach-{}", tool_request_id.0));
            tool_context_by_request_id.insert(
                tool_request_id.0.clone(),
                (tool_name.clone(), argument_content.clone(), call_id),
            );
            Vec::new()
        }
        SessionEvent::ToolExecutionFinished {
            turn_id,
            tool_request_id,
            outcome,
            reason,
            result_summary,
            result_content,
            ..
        } if turn_id == current_turn_id || completed_turns.contains(turn_id) => {
            let (tool_name, arguments, call_id) = tool_context_by_request_id
                .get(&tool_request_id.0)
                .cloned()
                .unwrap_or_else(|| {
                    (
                        String::from("tool"),
                        None,
                        format!("yach-{}", tool_request_id.0),
                    )
                });
            // Native pairing needs the arguments the model actually sent.
            // Logs written before payload persistence do not have them, and
            // a tool_result without its tool_use is rejected outright — so
            // those fall back to the descriptive text form for both halves
            // rather than emitting an orphaned block.
            let parsed_arguments = arguments
                .as_deref()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok());
            match (parsed_arguments, result_content.as_deref()) {
                (Some(arguments_json), Some(result)) => vec![
                    ProviderMessage::assistant(
                        String::new(),
                        vec![ProviderToolCall {
                            call_id: call_id.clone(),
                            name: tool_name.clone(),
                            arguments_json,
                        }],
                    ),
                    ProviderMessage::tool_results(vec![ProviderToolResultBlock {
                        call_id,
                        content: result.to_owned(),
                    }]),
                ],
                _ => vec![provider_tool_activity_message(
                    &tool_name,
                    arguments.as_deref(),
                    *outcome,
                    reason.as_deref(),
                    result_summary.as_ref(),
                    result_content.as_deref(),
                )],
            }
        }
        SessionEvent::EntryAppended { .. }
        | SessionEvent::ToolRequestRecorded { .. }
        | SessionEvent::ToolExecutionFinished { .. }
        | SessionEvent::TurnFinished { .. }
        | SessionEvent::MetricRecorded { .. }
        | SessionEvent::StaticContextIncluded { .. }
        | SessionEvent::PermissionDecisionRecorded { .. }
        | SessionEvent::EditTraceRecorded { .. }
        | SessionEvent::EditTransactionPrepared { .. }
        | SessionEvent::EditTransactionFinished { .. }
        | SessionEvent::CompactionCheckpoint { .. } => Vec::new(),
    }));
    messages
}

/// Tool-role transcript message describing prior tool activity so provider
/// requests keep tool evidence across turns and resume. Uses persisted
/// payloads when present; logs written before payload persistence fall back
/// to the redacted summary marked as not retained.
fn provider_tool_activity_message(
    tool_name: &str,
    arguments: Option<&str>,
    outcome: ToolOutcome,
    reason: Option<&str>,
    result_summary: Option<&ToolPayloadSummary>,
    result_content: Option<&str>,
) -> ProviderMessage {
    let content = result_content.map_or_else(
        || {
            serde_json::Value::String(format!(
                "{} (output not retained)",
                result_summary.map_or("result unavailable", |summary| summary.summary.as_str())
            ))
        },
        |content| {
            serde_json::from_str::<serde_json::Value>(content)
                .unwrap_or_else(|_| serde_json::Value::String(content.to_owned()))
        },
    );
    let arguments = arguments.map_or(serde_json::Value::Null, |arguments| {
        serde_json::from_str::<serde_json::Value>(arguments)
            .unwrap_or_else(|_| serde_json::Value::String(arguments.to_owned()))
    });
    ProviderMessage::text(
        Role::Tool,
        serde_json::json!({
            "tool_name": tool_name,
            "arguments": arguments,
            "status": crate::rig_adapter::tool_outcome_label(outcome),
            "reason": reason,
            "content": content,
        })
        .to_string(),
    )
}

/// Baseline guardrails for every native-provider request, kept deliberately
/// small. Each sentence earned its place in dogfooding: without the first
/// two, cheap models assert filesystem state from stale in-conversation
/// memory and retry failed calls verbatim; without the last two, models
/// over-apply project instructions to conversational prompts (reading
/// orientation docs before answering "hello"). See
/// docs/project/records/2026-07-20-baseline-prompt-cohort-check.md.
const PROVIDER_BASELINE_GUIDANCE: &str = "You are a coding agent running in the yach harness. \
Files can change outside this conversation at any time: verify current state with \
a tool call before asserting or acting on remembered file contents. If a tool call \
fails because the target changed, already exists, or is missing, re-check the \
current state and adapt instead of repeating the call. Match effort to the \
request: answer greetings, small talk, and questions you can already answer \
directly, without tool calls. Project instructions in context describe how to \
carry out real work, not a checklist to run before every response.";

fn provider_baseline_guidance_message() -> ProviderMessage {
    ProviderMessage::text(Role::System, String::from(PROVIDER_BASELINE_GUIDANCE))
}

fn provider_messages_from_log_with_static_context(
    log: &SessionLog,
    current_turn_id: &TurnId,
    context: &StaticContextBundle,
) -> Vec<ProviderMessage> {
    let mut messages = vec![provider_baseline_guidance_message()];
    messages.extend(provider_messages_from_static_context(context));
    messages.extend(provider_messages_from_log(log, current_turn_id));
    messages
}

fn provider_messages_from_static_context(context: &StaticContextBundle) -> Vec<ProviderMessage> {
    if context.items.is_empty() {
        return Vec::new();
    }

    let system_content = render_static_context_items(context.items.iter().filter(|item| {
        matches!(
            item.placement,
            StaticContextPlacement::ProjectInstructions | StaticContextPlacement::AppendSystem
        )
    }));
    let background_content = render_static_context_items(
        context
            .items
            .iter()
            .filter(|item| item.placement == StaticContextPlacement::BackgroundContext),
    );

    let mut messages = Vec::new();
    if let Some(content) = system_content {
        messages.push(ProviderMessage::text(Role::System, content));
    }
    if let Some(content) = background_content {
        messages.push(ProviderMessage::text(Role::User, content));
    }
    messages
}

fn render_static_context_items<'a>(
    items: impl Iterator<Item = &'a StaticContextItem>,
) -> Option<String> {
    let content = items
        .map(|item| format!("# {}\n\n{}", item.title, item.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}

fn append_pending_native_session_events(
    store: &JsonlSessionStore,
    pending_events: &mut Vec<SessionEvent>,
) -> std::io::Result<()> {
    store.append_events(pending_events)?;
    pending_events.clear();
    Ok(())
}

fn log_has_finished_turn(log: &SessionLog, turn_id: &TurnId) -> bool {
    log.events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::TurnFinished {
                turn_id: finished_turn_id,
                ..
            } if finished_turn_id == turn_id
        )
    })
}

#[derive(Debug, Clone)]
struct ProviderTurnRefs {
    session_id: SessionId,
    turn: TurnId,
    user_entry: EntryId,
    assistant_entry: EntryId,
    prompt_started: Instant,
}

trait ProviderRequester {
    fn request(
        &mut self,
        request: ProviderRequest,
    ) -> BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>>;
}

struct RigProviderRequester {
    adapter: RigProviderAdapterConfig,
    approved_tools: Vec<String>,
}

impl ProviderRequester for RigProviderRequester {
    fn request(
        &mut self,
        request: ProviderRequest,
    ) -> BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>> {
        let adapter = self.adapter.clone();
        let approved_tools = self.approved_tools.clone();
        Box::pin(async move {
            run_provider_request_with_approved_tools(adapter, request, approved_tools).await
        })
    }
}

fn provider_approved_tools() -> Vec<String> {
    [
        "project_path_info",
        "read_text_file",
        "search_project",
        "list_project_paths",
        "edit_text_file",
        "create_text_file",
        "bash",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[expect(
    clippy::struct_field_names,
    reason = "limit fields intentionally share the same prefix for policy readability"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderToolLoopPolicy {
    max_tool_rounds: Option<usize>,
    max_tool_calls_per_round: usize,
    max_total_tool_calls: usize,
    max_result_bytes_per_tool: usize,
    max_total_result_bytes: usize,
}

impl ProviderToolLoopPolicy {
    // Runaway-loop backstops, not working budgets: real coding turns run
    // dozens of tool calls (the first sesh dogfood turn died at the old
    // total of 12 doing entirely legitimate work — the max_tokens=128 bug
    // class again). The cohort caps per-turn tool use only by context and
    // user cancellation; these bounds exist to stop a pathological loop,
    // so they sit far above any productive turn. Total result bytes stays
    // moderate because mid-turn context overflow has no recovery yet.
    const fn agent_default() -> Self {
        Self {
            max_tool_rounds: None,
            max_tool_calls_per_round: 16,
            max_total_tool_calls: 200,
            max_result_bytes_per_tool: 64 * 1024,
            max_total_result_bytes: 512 * 1024,
        }
    }

    #[cfg(test)]
    const fn with_max_tool_rounds(mut self, max_tool_rounds: usize) -> Self {
        self.max_tool_rounds = Some(max_tool_rounds);
        self
    }

    #[cfg(test)]
    const fn as_continuation_policy(self) -> ToolContinuationPolicy {
        ToolContinuationPolicy {
            max_tool_calls: self.max_tool_calls_per_round,
            max_result_bytes: self.max_result_bytes_per_tool,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderToolLoopBudget {
    policy: ProviderToolLoopPolicy,
    tool_rounds: usize,
    total_tool_calls: usize,
    total_result_bytes: usize,
}

impl ProviderToolLoopBudget {
    const fn new(policy: ProviderToolLoopPolicy) -> Self {
        Self {
            policy,
            tool_rounds: 0,
            total_tool_calls: 0,
            total_result_bytes: 0,
        }
    }

    fn begin_tool_round(&mut self, tool_call_count: usize) -> Result<(), ProviderRoundError> {
        if self
            .policy
            .max_tool_rounds
            .is_some_and(|max_tool_rounds| self.tool_rounds >= max_tool_rounds)
        {
            return Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_rounds",
            )));
        }
        if tool_call_count > self.policy.max_tool_calls_per_round {
            return Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_round_too_many_calls",
            )));
        }
        let next_total_tool_calls = self.total_tool_calls.saturating_add(tool_call_count);
        if next_total_tool_calls > self.policy.max_total_tool_calls {
            return Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_total_calls",
            )));
        }

        self.tool_rounds += 1;
        self.total_tool_calls = next_total_tool_calls;
        Ok(())
    }

    fn record_tool_result(
        &mut self,
        tool_request_id: &str,
        byte_count: usize,
    ) -> Result<(), ProviderRoundError> {
        if byte_count > self.policy.max_result_bytes_per_tool {
            return Err(ProviderRoundError::ToolContinuation(format!(
                "tool_result_too_large:{tool_request_id}"
            )));
        }
        let next_total_result_bytes = self.total_result_bytes.saturating_add(byte_count);
        if next_total_result_bytes > self.policy.max_total_result_bytes {
            return Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_loop_total_result_too_large",
            )));
        }

        self.total_result_bytes = next_total_result_bytes;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderRoundResult {
    text: String,
    provider_response_id: Option<String>,
    /// Provider-reported usage summed across the turn's rounds.
    usage: Option<ProviderUsage>,
    /// Text the model produced in earlier tool rounds of the same turn,
    /// already streamed to the UI as it happened; joined with the final
    /// text for the persisted assistant entry.
    mid_turn_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProviderRoundError {
    Provider(ProviderError),
    Cancelled(String),
    StreamEndedWithoutCompletion,
    ProjectRootUnavailable,
    ToolContinuation(String),
    ToolExecutionDenied {
        tool_request_id: String,
        tool_name: String,
        reason: String,
    },
    #[cfg(test)]
    SecondRoundToolCall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderFirstRound {
    text: String,
    provider_response_id: Option<String>,
    tool_calls: Vec<ProviderToolCall>,
    usage: Option<ProviderUsage>,
}

fn collect_native_provider_first_round(
    events: Vec<ProviderStreamEvent>,
) -> Result<ProviderFirstRound, ProviderRoundError> {
    let mut text = String::new();
    let mut completed = false;
    let mut finish_reason = None;
    let mut provider_response_id = None;
    let mut tool_calls = Vec::new();
    let mut usage = None;
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { delta, .. } => text.push_str(&delta),
            ProviderStreamEvent::ToolCallCompleted { tool_call, .. } => tool_calls.push(tool_call),
            ProviderStreamEvent::Completed {
                provider_response_id: response_id,
                finish_reason: reason,
                usage: round_usage,
                ..
            } => {
                completed = true;
                finish_reason = reason;
                provider_response_id = response_id;
                usage = round_usage.or(usage);
            }
            ProviderStreamEvent::Failed { error, .. } => {
                return Err(ProviderRoundError::Provider(error));
            }
            ProviderStreamEvent::Cancelled { reason, .. } => {
                return Err(ProviderRoundError::Cancelled(
                    reason.unwrap_or_else(|| String::from("native provider cancelled")),
                ));
            }
            ProviderStreamEvent::ToolCallStarted { .. }
            | ProviderStreamEvent::ToolCallDelta { .. }
            | ProviderStreamEvent::Started { .. } => {}
        }
    }
    if !completed {
        return Err(ProviderRoundError::StreamEndedWithoutCompletion);
    }
    if tool_calls.is_empty() && matches!(finish_reason, Some(ProviderFinishReason::ToolCalls)) {
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "provider_tool_call_incomplete",
        )));
    }
    Ok(ProviderFirstRound {
        text,
        provider_response_id,
        tool_calls,
        usage,
    })
}

/// Sum provider-reported usage across a turn's rounds; each request bills
/// its own input, so summing is the billing-correct turn total.
fn sum_provider_usage(
    total: Option<ProviderUsage>,
    round: Option<ProviderUsage>,
) -> Option<ProviderUsage> {
    match (total, round) {
        (None, round) => round,
        (total, None) => total,
        (Some(total), Some(round)) => {
            let add = |lhs: Option<u64>, rhs: Option<u64>| match (lhs, rhs) {
                (None, rhs) => rhs,
                (lhs, None) => lhs,
                (Some(lhs), Some(rhs)) => Some(lhs.saturating_add(rhs)),
            };
            Some(ProviderUsage {
                input_tokens: add(total.input_tokens, round.input_tokens),
                output_tokens: add(total.output_tokens, round.output_tokens),
                total_tokens: add(total.total_tokens, round.total_tokens),
            })
        }
    }
}

#[cfg(test)]
fn collect_native_provider_final_round(
    events: Vec<ProviderStreamEvent>,
) -> Result<ProviderRoundResult, ProviderRoundError> {
    let first_round = collect_native_provider_first_round(events)?;
    if !first_round.tool_calls.is_empty() {
        return Err(ProviderRoundError::SecondRoundToolCall);
    }
    Ok(ProviderRoundResult {
        text: first_round.text,
        provider_response_id: first_round.provider_response_id,
        usage: first_round.usage,
        mid_turn_text: String::new(),
    })
}

#[cfg(test)]
struct ProviderToolRoundContext<'a, Executor>
where
    Executor: ToolExecutor,
{
    model: ProviderModel,
    log: &'a mut SessionLog,
    pending_events: &'a mut Vec<SessionEvent>,
    turn_id: &'a TurnId,
    project_root: Option<ResourceRoot>,
    static_context_cwd: Option<PathBuf>,
    extension_static_context_files: Vec<ExtensionStaticContextFile>,
    tool_event_store: Option<&'a JsonlSessionStore>,
    registry: &'a ToolRegistry,
    permission_policy: &'a ToolPermissionPolicy,
    executor: &'a Executor,
    routable_tool_names: &'a [&'a str],
    require_project_root_for_tools: bool,
}

#[cfg(test)]
async fn run_native_provider_one_tool_round_with_registry<Provider, Executor>(
    requester: &mut Provider,
    context: ProviderToolRoundContext<'_, Executor>,
) -> Result<ProviderRoundResult, ProviderRoundError>
where
    Provider: ProviderRequester,
    Executor: ToolExecutor,
{
    let ProviderToolRoundContext {
        model,
        log,
        pending_events,
        turn_id,
        project_root,
        static_context_cwd,
        extension_static_context_files,
        tool_event_store,
        registry,
        permission_policy,
        executor,
        routable_tool_names,
        require_project_root_for_tools,
    } = context;
    let resolved_catalog = registry
        .resolve_provider_turn_catalog(permission_policy, routable_tool_names.iter().copied());
    let advertising_tools = resolved_catalog.provider_definitions();
    let mut extensions = Vec::new();
    if !advertising_tools.is_empty() {
        extensions.push(
            build_provider_tool_advertising_extension(&advertising_tools).map_err(|error| {
                ProviderRoundError::ToolContinuation(provider_tool_advertising_error_label(&error))
            })?,
        );
    }
    let static_context_assembly = project_root
        .as_ref()
        .map(|root| {
            assemble_project_static_context_with_extensions(
                root.canonical_path(),
                static_context_cwd
                    .as_deref()
                    .unwrap_or_else(|| root.canonical_path()),
                StaticContextPolicy::conservative(),
                extension_static_context_files,
            )
        })
        .unwrap_or_default();
    if !static_context_assembly.bundle.items.is_empty()
        || !static_context_assembly.omissions.is_empty()
    {
        log.record_static_context_included(
            SessionId(String::from("default")),
            turn_id.clone(),
            static_context_assembly.bundle.summary(),
            static_context_assembly.omissions.clone(),
        );
        if let Some(event) = log.events.last().cloned() {
            pending_events.push(event);
        }
        if let Some(store) = tool_event_store
            && append_pending_native_session_events(store, pending_events).is_err()
        {
            return Err(ProviderRoundError::ToolContinuation(String::from(
                "static_context_persist_failed",
            )));
        }
    }
    let initial_request = ProviderRequest {
        turn_id: turn_id.clone(),
        model,
        messages: provider_messages_from_log_with_static_context(
            log,
            turn_id,
            &static_context_assembly.bundle,
        ),
        extensions,
    };
    let first_events = requester
        .request(initial_request.clone())
        .await
        .map_err(ProviderRoundError::Provider)?;
    let first_round = collect_native_provider_first_round(first_events)?;
    if first_round.tool_calls.is_empty() {
        return Ok(ProviderRoundResult {
            text: first_round.text,
            provider_response_id: first_round.provider_response_id,
            usage: first_round.usage,
            mid_turn_text: String::new(),
        });
    }
    if require_project_root_for_tools && project_root.is_none() {
        return Err(ProviderRoundError::ProjectRootUnavailable);
    }
    let tool_event_start = log.events.len();
    let tool_results = match (ToolContinuationWorkflow {
        registry,
        permission_policy,
        executor,
        continuation_policy: ToolContinuationPolicy::fixture_default(),
    })
    .build_provider_tool_results(
        log,
        &ToolContinuationContext {
            session_id: SessionId(String::from("default")),
            turn_id: turn_id.clone(),
        },
        first_round.tool_calls,
    ) {
        Ok(results) => results,
        Err(error) => {
            pending_events.extend(log.events[tool_event_start..].iter().cloned());
            return Err(ProviderRoundError::ToolContinuation(
                tool_round_error_label(&error),
            ));
        }
    };
    pending_events.extend(log.events[tool_event_start..].iter().cloned());
    if let Some(store) = tool_event_store
        && append_pending_native_session_events(store, pending_events).is_err()
    {
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_event_persist_failed",
        )));
    }

    let continuation_request = ProviderContinuationRequest {
        turn_id: turn_id.clone(),
        model: initial_request.model.clone(),
        prior_messages: initial_request.messages,
        tool_results,
        extensions: crate::strip_provider_tool_advertising_extensions(initial_request.extensions),
    };
    let submission = build_provider_continuation_submission(
        &continuation_request,
        ProviderContinuationValidationPolicy::strict_tool_results(
            ToolContinuationPolicy::fixture_default().max_result_bytes,
        ),
    )
    .map_err(|error| ProviderRoundError::ToolContinuation(provider_mapping_error_label(&error)))?;
    let continuation_request =
        crate::rig_adapter::project_provider_continuation_request(submission);
    let continuation_events = requester
        .request(continuation_request)
        .await
        .map_err(ProviderRoundError::Provider)?;
    collect_native_provider_final_round(continuation_events)
}

#[cfg(test)]
async fn run_native_provider_one_readonly_tool_round(
    requester: &mut impl ProviderRequester,
    model: ProviderModel,
    log: &mut SessionLog,
    pending_events: &mut Vec<SessionEvent>,
    turn_id: &TurnId,
    project_context: Option<LaunchProjectContext>,
    tool_event_store: Option<&JsonlSessionStore>,
) -> Result<ProviderRoundResult, ProviderRoundError> {
    let registry = ToolRegistry::with_project_read_only_tools();
    let permission_policy = ToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
    let (project_root, static_context_cwd) = project_context.map_or((None, None), |context| {
        (Some(context.project_root), Some(context.cwd))
    });
    let executor = if let Some(root) = project_root.clone() {
        ProjectReadOnlyToolExecutor::new(root)
    } else {
        ProjectReadOnlyToolExecutor::unavailable_root()
    };
    let routable_tool_names = ["project_path_info"];

    run_native_provider_one_tool_round_with_registry(
        requester,
        ProviderToolRoundContext {
            model,
            log,
            pending_events,
            turn_id,
            project_root,
            static_context_cwd,
            extension_static_context_files: Vec::new(),
            tool_event_store,
            registry: &registry,
            permission_policy: &permission_policy,
            executor: &executor,
            routable_tool_names: &routable_tool_names,
            require_project_root_for_tools: true,
        },
    )
    .await
}

struct ProviderBufferedEventSink<'a> {
    store: Option<&'a JsonlSessionStore>,
    events: Mutex<Vec<SessionEvent>>,
}

impl<'a> ProviderBufferedEventSink<'a> {
    fn new(store: Option<&'a JsonlSessionStore>) -> Self {
        Self {
            store,
            events: Mutex::new(Vec::new()),
        }
    }

    fn drain_into(
        &self,
        log: &mut SessionLog,
        pending_events: &mut Vec<SessionEvent>,
    ) -> Result<(), ProviderRoundError> {
        let mut events = self.events.lock().map_err(|_| {
            ProviderRoundError::ToolContinuation(String::from("tool_event_buffer_poisoned"))
        })?;
        log.events.extend(events.iter().cloned());
        if self.store.is_none() {
            pending_events.extend(events.iter().cloned());
        }
        events.clear();
        Ok(())
    }
}

impl SessionEventSink for ProviderBufferedEventSink<'_> {
    fn append_event(&self, event: &SessionEvent) -> std::io::Result<()> {
        if let Some(store) = self.store {
            store.append_event(event)?;
        }
        let mut events = self
            .events
            .lock()
            .map_err(|_| std::io::Error::other("native provider event buffer poisoned"))?;
        events.push(event.clone());
        Ok(())
    }

    fn append_events(&self, events: &[SessionEvent]) -> std::io::Result<()> {
        if let Some(store) = self.store {
            store.append_events(events)?;
        }
        let mut buffered_events = self
            .events
            .lock()
            .map_err(|_| std::io::Error::other("native provider event buffer poisoned"))?;
        buffered_events.extend(events.iter().cloned());
        Ok(())
    }
}

struct ProviderAgentToolRound<'a> {
    session_id: &'a SessionId,
    model: ProviderModel,
    log: &'a mut SessionLog,
    pending_events: &'a mut Vec<SessionEvent>,
    turn_id: &'a TurnId,
    project_context: Option<LaunchProjectContext>,
    extension_static_context_files: Vec<ExtensionStaticContextFile>,
    extension_activation_snapshot: crate::ExtensionActivationSnapshot,
    tool_event_store: Option<&'a JsonlSessionStore>,
    review_tx: mpsc::UnboundedSender<BackendEvent>,
    review_decisions: AgentEditDecisionReceiver,
    /// Compaction accounting inputs (`usable = context_window −
    /// max_output_tokens − reserve`).
    context_window: u64,
    max_output_tokens: u64,
}

struct ProviderAgentToolBatch<'a> {
    session_id: SessionId,
    turn_id: TurnId,
    project_root: ResourceRoot,
    shell_policy: crate::ShellPolicy,
    registry: &'a ToolRegistry,
    resolved_catalog: &'a ResolvedToolCatalog,
    permission_policy: &'a ToolPermissionPolicy,
    read_only_executor: &'a ProjectReadOnlyToolExecutor,
    extension_executor: Option<&'a crate::ExtensionToolExecutorRouter>,
    edit_access: &'a mut EditAccess,
    edit_sink: &'a ProviderBufferedEventSink<'a>,
    review_tx: mpsc::UnboundedSender<BackendEvent>,
    review_decisions: &'a mut AgentEditDecisionReceiver,
    tool_event_store: Option<&'a JsonlSessionStore>,
    budget: &'a mut ProviderToolLoopBudget,
    tool_round_index: usize,
    edit_traces: &'a mut Vec<ProviderContinuationEditTrace>,
    log: &'a mut SessionLog,
    pending_events: &'a mut Vec<SessionEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderContinuationEditTrace {
    trace_id: EditTraceId,
    tool_name: String,
    tool_request_id: ToolRequestId,
    provider_call_id: Option<String>,
    preview_id: Option<EditPreviewId>,
    permission_decision_id: Option<PermissionDecisionId>,
}

#[derive(Clone, Copy)]
struct ProviderContinuationTraceInput<'a> {
    session_id: &'a SessionId,
    turn_id: &'a TurnId,
    edit_traces: &'a [ProviderContinuationEditTrace],
    started: Instant,
    outcome: EditTraceOutcome,
    reason_label: Option<&'a str>,
}

/// Per-turn cap on mid-turn threshold compactions (Claude Code ships a
/// 3-strike circuit breaker for the same summarize-refill loop).
const MID_TURN_COMPACTIONS_MAX: u32 = 3;

async fn run_native_provider_one_agent_tool_round(
    requester: &mut impl ProviderRequester,
    round: ProviderAgentToolRound<'_>,
) -> Result<ProviderRoundResult, ProviderRoundError> {
    let ProviderAgentToolRound {
        session_id,
        model,
        log,
        pending_events,
        turn_id,
        project_context,
        extension_static_context_files,
        extension_activation_snapshot,
        tool_event_store,
        review_tx,
        mut review_decisions,
        context_window,
        max_output_tokens,
    } = round;
    let registry = extension_activation_snapshot.registry.clone();
    let active_extension_tool_names = extension_activation_snapshot.active_tool_names();
    let mut metadata_tool_names = vec![String::from("project_path_info")];
    metadata_tool_names.extend(
        active_extension_tool_names
            .iter()
            .map(|name| (*name).to_owned()),
    );
    let permission_policy =
        ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
            metadata_tool_names,
            ["read_text_file", "search_project", "list_project_paths"],
            ["edit_text_file", "create_text_file"],
        )
        .with_process_tools(["bash"]);
    let mut routable_tool_names = vec![
        String::from("project_path_info"),
        String::from("read_text_file"),
        String::from("search_project"),
        String::from("list_project_paths"),
        String::from("edit_text_file"),
        String::from("create_text_file"),
        String::from("bash"),
    ];
    routable_tool_names.extend(
        active_extension_tool_names
            .iter()
            .map(|name| (*name).to_owned()),
    );
    let resolved_catalog = registry.resolve_provider_turn_catalog(
        &permission_policy,
        routable_tool_names.iter().map(String::as_str),
    );
    let advertising_tools = resolved_catalog.provider_definitions();
    let mut extensions = Vec::new();
    if !advertising_tools.is_empty() {
        extensions.push(
            build_provider_tool_advertising_extension(&advertising_tools).map_err(|error| {
                ProviderRoundError::ToolContinuation(provider_tool_advertising_error_label(&error))
            })?,
        );
    }

    let (project_root, static_context_cwd) = project_context.map_or((None, None), |context| {
        (Some(context.project_root), Some(context.cwd))
    });
    let static_context_assembly = project_root
        .as_ref()
        .map(|root| {
            assemble_project_static_context_with_extensions(
                root.canonical_path(),
                static_context_cwd
                    .as_deref()
                    .unwrap_or_else(|| root.canonical_path()),
                StaticContextPolicy::conservative(),
                extension_static_context_files,
            )
        })
        .unwrap_or_default();
    if !static_context_assembly.bundle.items.is_empty()
        || !static_context_assembly.omissions.is_empty()
    {
        log.record_static_context_included(
            session_id.clone(),
            turn_id.clone(),
            static_context_assembly.bundle.summary(),
            static_context_assembly.omissions.clone(),
        );
        if let Some(event) = log.events.last().cloned() {
            pending_events.push(event);
        }
        if let Some(store) = tool_event_store
            && append_pending_native_session_events(store, pending_events).is_err()
        {
            return Err(ProviderRoundError::ToolContinuation(String::from(
                "static_context_persist_failed",
            )));
        }
    }

    // Auto-compaction trigger, checked before the turn's first request.
    // Design: docs/superpowers/specs/2026-07-20-context-compaction-design.md.
    let compaction_config = crate::CompactionConfig::load_for_project(
        project_root.as_ref().map(ResourceRoot::canonical_path),
    );
    let compaction_budget = CompactionBudget {
        context_window,
        max_output_tokens,
        config: &compaction_config,
    };
    if compaction_config.enabled {
        let estimate =
            estimate_provider_messages_tokens(&provider_messages_from_log_with_static_context(
                log,
                turn_id,
                &static_context_assembly.bundle,
            ));
        if compaction_budget.over_threshold(estimate) {
            let compacted = run_compaction(
                requester,
                CompactionRun {
                    session_id,
                    turn_id,
                    model: &model,
                    config: &compaction_config,
                    reason: crate::CompactionReason::Threshold,
                    tokens_before: estimate,
                    focus_instructions: None,
                    log,
                    pending_events,
                    tool_event_store,
                    review_tx: &review_tx,
                },
            )
            .await?;
            if compacted {
                let refilled = estimate_provider_messages_tokens(
                    &provider_messages_from_log_with_static_context(
                        log,
                        turn_id,
                        &static_context_assembly.bundle,
                    ),
                );
                // Thrash guard: fail only when the context cannot fit even
                // after compaction. Still-above-threshold-but-fits means
                // compaction made what progress it could; the turn proceeds
                // and the next turn compacts again.
                if refilled > compaction_budget.usable_tokens() {
                    let _ = review_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from(
                            "context exceeds the usable window even after compaction; \
narrow the request or start a fresh session",
                        ),
                    }));
                    return Err(ProviderRoundError::ToolContinuation(String::from(
                        "context_overflow_after_compaction",
                    )));
                }
                if compaction_budget.over_threshold(refilled) {
                    let _ = review_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from(
                            "context still above the compaction threshold; continuing",
                        ),
                    }));
                }
            }
        }
    }

    let initial_request = ProviderRequest {
        turn_id: turn_id.clone(),
        model,
        messages: provider_messages_from_log_with_static_context(
            log,
            turn_id,
            &static_context_assembly.bundle,
        ),
        extensions,
    };
    let read_only_executor = project_root
        .as_ref()
        .map(|project_root| ProjectReadOnlyToolExecutor::new(project_root.clone()));
    let shell_policy = crate::ShellPolicy::load_for_project(
        project_root.as_ref().map(ResourceRoot::canonical_path),
    );
    let mut edit_access = EditAccess::default();
    let edit_sink = ProviderBufferedEventSink::new(tool_event_store);
    let mut provider_continuation_edit_traces = Vec::new();
    let loop_policy = ProviderToolLoopPolicy::agent_default();
    let mut loop_budget = ProviderToolLoopBudget::new(loop_policy);
    let mut next_request = initial_request.clone();
    let mut prior_messages = initial_request.messages.clone();
    let mut pending_continuation_trace: Option<(Instant, Vec<ProviderContinuationEditTrace>)> =
        None;
    let mut is_initial_request = true;
    let mut overflow_compaction_used = false;
    let mut mid_turn_text = String::new();
    let mut turn_usage: Option<ProviderUsage> = None;
    let mut empty_response_nudged = false;
    // Mid-turn compaction churn guards: a hard per-turn cap, and only
    // re-compacting once the context has grown past the previous
    // post-compaction refill — a giant kept tail would otherwise trigger
    // a doomed summarize every round.
    let mut mid_turn_compactions: u32 = 0;
    let mut last_mid_turn_refill: Option<u64> = None;
    loop {
        let provider_events =
            match provider_request_with_retry(requester, &next_request, &review_tx).await {
                Ok(events) => events,
                Err(error) => {
                    // Overflow recovery (design: reason=overflow): a context
                    // overflow on the turn's first request compacts once and
                    // retries; a second overflow, or overflow mid-tool-loop,
                    // fails the turn.
                    if error.kind == crate::ProviderErrorKind::ContextLength
                        && is_initial_request
                        && compaction_config.enabled
                        && !overflow_compaction_used
                    {
                        overflow_compaction_used = true;
                        let compacted = run_compaction(
                            requester,
                            CompactionRun {
                                session_id,
                                turn_id,
                                model: &initial_request.model,
                                config: &compaction_config,
                                reason: crate::CompactionReason::Overflow,
                                tokens_before: estimate_provider_messages_tokens(
                                    &next_request.messages,
                                ),
                                focus_instructions: None,
                                log,
                                pending_events,
                                tool_event_store,
                                review_tx: &review_tx,
                            },
                        )
                        .await?;
                        if compacted {
                            let messages = provider_messages_from_log_with_static_context(
                                log,
                                turn_id,
                                &static_context_assembly.bundle,
                            );
                            prior_messages.clone_from(&messages);
                            next_request = ProviderRequest {
                                turn_id: turn_id.clone(),
                                model: initial_request.model.clone(),
                                messages,
                                extensions: initial_request.extensions.clone(),
                            };
                            continue;
                        }
                    }
                    if let Some((started, edit_traces)) = pending_continuation_trace.take() {
                        record_provider_continuation_trace_records(
                            log,
                            pending_events,
                            tool_event_store,
                            ProviderContinuationTraceInput {
                                session_id,
                                turn_id,
                                edit_traces: &edit_traces,
                                started,
                                outcome: EditTraceOutcome::Failed,
                                reason_label: Some("provider_request_failed"),
                            },
                        );
                    }
                    return Err(ProviderRoundError::Provider(error));
                }
            };
        let round = match collect_native_provider_first_round(provider_events) {
            Ok(round) => {
                turn_usage = sum_provider_usage(turn_usage, round.usage);
                round
            }
            Err(error) => {
                if let Some((started, edit_traces)) = pending_continuation_trace.take() {
                    let reason = provider_round_error_label(&error);
                    record_provider_continuation_trace_records(
                        log,
                        pending_events,
                        tool_event_store,
                        ProviderContinuationTraceInput {
                            session_id,
                            turn_id,
                            edit_traces: &edit_traces,
                            started,
                            outcome: EditTraceOutcome::Failed,
                            reason_label: Some(reason.as_str()),
                        },
                    );
                }
                return Err(error);
            }
        };
        if let Some((started, edit_traces)) = pending_continuation_trace.take() {
            record_provider_continuation_trace_records(
                log,
                pending_events,
                tool_event_store,
                ProviderContinuationTraceInput {
                    session_id,
                    turn_id,
                    edit_traces: &edit_traces,
                    started,
                    outcome: EditTraceOutcome::Completed,
                    reason_label: None,
                },
            );
        }
        if round.tool_calls.is_empty() {
            // A contentless final response (no text, no calls — seen from
            // thinking-heavy models after long tool sequences) gets one
            // nudge for the actual answer instead of ending the turn with
            // "assistant returned no text".
            if round.text.trim().is_empty()
                && mid_turn_text.trim().is_empty()
                && !empty_response_nudged
            {
                empty_response_nudged = true;
                let _ = review_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: String::from(
                        "assistant returned an empty response; requesting the final answer",
                    ),
                }));
                prior_messages.push(ProviderMessage::text(
                    Role::User,
                    String::from(
                        "Your previous response contained no text. Reply with your final \
answer now, or call tools if more work is needed.",
                    ),
                ));
                next_request = ProviderRequest {
                    turn_id: turn_id.clone(),
                    model: initial_request.model.clone(),
                    messages: prior_messages.clone(),
                    extensions: initial_request.extensions.clone(),
                };
                is_initial_request = false;
                continue;
            }
            return Ok(ProviderRoundResult {
                text: round.text,
                provider_response_id: round.provider_response_id,
                usage: turn_usage,
                mid_turn_text: mid_turn_text.clone(),
            });
        }

        // The model's round output must survive the round: stream any text
        // to the UI now (mid-turn commentary was previously invisible), and
        // echo text + requested calls back into the continuation context as
        // an assistant message. Without that self-narrative the model
        // cannot see what it already did or planned, and it loops — the
        // sesh dogfood ran 161 identical reads into the call backstop.
        let assistant_round_message = assistant_round_message(&round.text, &round.tool_calls);
        if !round.text.trim().is_empty() {
            let _ = review_tx.send(BackendEvent::Server(ServerEvent::PromptDelta {
                session_id: session_id.0.clone(),
                delta: format!("{}\n\n", round.text.trim_end()),
            }));
            mid_turn_text.push_str(round.text.trim_end());
            mid_turn_text.push_str("\n\n");
        }

        let Some(project_root) = project_root.clone() else {
            return Err(ProviderRoundError::ProjectRootUnavailable);
        };
        let Some(read_only_executor) = read_only_executor.as_ref() else {
            return Err(ProviderRoundError::ProjectRootUnavailable);
        };
        let tool_round_index = loop_budget.tool_rounds + 1;
        let edit_trace_start = provider_continuation_edit_traces.len();
        let tool_results = execute_native_provider_agent_tool_batch(
            ProviderAgentToolBatch {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                project_root,
                shell_policy: shell_policy.clone(),
                registry: &registry,
                resolved_catalog: &resolved_catalog,
                permission_policy: &permission_policy,
                read_only_executor,
                extension_executor: Some(&extension_activation_snapshot.executor),
                edit_access: &mut edit_access,
                edit_sink: &edit_sink,
                review_tx: review_tx.clone(),
                review_decisions: &mut review_decisions,
                tool_event_store,
                budget: &mut loop_budget,
                tool_round_index,
                edit_traces: &mut provider_continuation_edit_traces,
                log,
                pending_events,
            },
            round.tool_calls,
        )
        .await?;
        let continuation_edit_traces =
            provider_continuation_edit_traces[edit_trace_start..].to_vec();
        let provider_continuation_started = Instant::now();
        prior_messages.push(assistant_round_message);
        next_request = match build_native_provider_tool_continuation_request(
            &initial_request,
            &prior_messages,
            tool_results,
        ) {
            Ok(request) => request,
            Err(ProviderRoundError::ToolContinuation(reason)) => {
                record_provider_continuation_trace_records(
                    log,
                    pending_events,
                    tool_event_store,
                    ProviderContinuationTraceInput {
                        session_id,
                        turn_id,
                        edit_traces: &continuation_edit_traces,
                        started: provider_continuation_started,
                        outcome: EditTraceOutcome::Failed,
                        reason_label: Some(reason.as_str()),
                    },
                );
                return Err(ProviderRoundError::ToolContinuation(reason));
            }
            Err(error) => return Err(error),
        };
        prior_messages.clone_from(&next_request.messages);
        let mut continuation_estimate = estimate_provider_messages_tokens(&prior_messages);
        // Mid-turn threshold check (dogfood finding 2026-07-24: a single
        // milestone turn accumulated to 126% of the usable window because
        // the trigger only ran at turn start). Tool request and result
        // events are already in the in-memory log, so the continuation
        // rebuilds through the same path resume and pre-turn compaction
        // use; the model's round narrative lives only in `mid_turn_text`
        // and is re-appended so the model keeps its self-narrative
        // (losing it is the 161-identical-reads loop).
        if compaction_config.enabled
            && compaction_budget.over_threshold(continuation_estimate)
            && mid_turn_compactions < MID_TURN_COMPACTIONS_MAX
            && last_mid_turn_refill.is_none_or(|refill| continuation_estimate > refill)
        {
            let compacted = run_compaction(
                requester,
                CompactionRun {
                    session_id,
                    turn_id,
                    model: &initial_request.model,
                    config: &compaction_config,
                    reason: crate::CompactionReason::Threshold,
                    tokens_before: continuation_estimate,
                    focus_instructions: None,
                    log,
                    pending_events,
                    tool_event_store,
                    review_tx: &review_tx,
                },
            )
            .await?;
            if compacted {
                mid_turn_compactions += 1;
                let mut rebuilt = provider_messages_from_log_with_static_context(
                    log,
                    turn_id,
                    &static_context_assembly.bundle,
                );
                if !mid_turn_text.trim().is_empty() {
                    rebuilt.push(ProviderMessage::text(
                        Role::Assistant,
                        mid_turn_text.trim_end().to_string(),
                    ));
                }
                let refilled = estimate_provider_messages_tokens(&rebuilt);
                if refilled > compaction_budget.usable_tokens() {
                    let _ = review_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from(
                            "context exceeds the usable window even after compaction; \
narrow the request or start a fresh session",
                        ),
                    }));
                    return Err(ProviderRoundError::ToolContinuation(String::from(
                        "context_overflow_after_compaction",
                    )));
                }
                if compaction_budget.over_threshold(refilled) {
                    let _ = review_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from(
                            "context still above the compaction threshold; continuing",
                        ),
                    }));
                }
                last_mid_turn_refill = Some(refilled);
                continuation_estimate = refilled;
                next_request = ProviderRequest {
                    turn_id: turn_id.clone(),
                    model: initial_request.model.clone(),
                    messages: rebuilt.clone(),
                    extensions: initial_request.extensions.clone(),
                };
                prior_messages = rebuilt;
            }
        }
        // The context meter otherwise freezes at the pre-turn value for the
        // whole turn (dogfood finding 2026-07-22): refresh it once per round
        // from the assembled continuation context, which carries everything
        // the round accumulated — round text, tool-call echoes, and tool
        // results.
        send_native_session_stats_with_estimate(
            &review_tx,
            log,
            Some(crate::ContextBudget {
                context_window,
                max_output_tokens,
                reserve_tokens: compaction_config.reserve_tokens,
            }),
            Some(continuation_estimate),
        );
        pending_continuation_trace =
            Some((provider_continuation_started, continuation_edit_traces));
        is_initial_request = false;
    }
}

/// Assistant-role message for one tool round: the model's text plus the
/// calls it requested, carried as structure the adapter maps onto the
/// provider's native tool-call blocks.
///
/// This previously rendered the calls as prose
/// (`[requested tool calls: ...]`) because the request was flattened into
/// a single string and there was nowhere else to put them. That format
/// then taught models to write it — a failing run reproduced it verbatim,
/// tool-result blobs included, while calling no tool at all
/// (`records/2026-07-28-tool-call-baseline.md`). Structure removes the
/// surface rather than policing it.
fn assistant_round_message(round_text: &str, tool_calls: &[ProviderToolCall]) -> ProviderMessage {
    ProviderMessage::assistant(round_text.trim(), tool_calls.to_vec())
}

fn build_native_provider_tool_continuation_request(
    initial_request: &ProviderRequest,
    prior_messages: &[ProviderMessage],
    tool_results: Vec<ProviderToolResult>,
) -> Result<ProviderRequest, ProviderRoundError> {
    let continuation_request = ProviderContinuationRequest {
        turn_id: initial_request.turn_id.clone(),
        model: initial_request.model.clone(),
        prior_messages: prior_messages.to_vec(),
        tool_results,
        extensions: initial_request.extensions.clone(),
    };
    let submission = build_provider_continuation_submission(
        &continuation_request,
        ProviderContinuationValidationPolicy::agent_tool_results(
            ProviderToolLoopPolicy::agent_default().max_result_bytes_per_tool,
        ),
    )
    .map_err(|error| ProviderRoundError::ToolContinuation(provider_mapping_error_label(&error)))?;
    Ok(crate::rig_adapter::project_provider_continuation_request(
        submission,
    ))
}

/// Transient provider failures worth retrying in place: a stream timeout,
/// network blip, rate limit, or provider-side 5xx can interrupt a turn
/// that is otherwise fine, and failing the turn discards every completed
/// tool round (first observed in sesh dogfood: a 120s stream stall after
/// 17 productive rounds). Request-shaped errors are never retried.
const fn provider_error_is_transient(kind: crate::ProviderErrorKind) -> bool {
    matches!(
        kind,
        crate::ProviderErrorKind::Timeout
            | crate::ProviderErrorKind::Network
            | crate::ProviderErrorKind::RateLimited
            | crate::ProviderErrorKind::ProviderInternal
    )
}

const PROVIDER_RETRY_DELAYS_MS: [u64; 2] = [1_000, 5_000];

/// Issue a provider request, retrying transient failures with backoff and
/// a visible status per attempt. Non-transient errors return immediately.
async fn provider_request_with_retry<Requester>(
    requester: &mut Requester,
    request: &ProviderRequest,
    review_tx: &mpsc::UnboundedSender<BackendEvent>,
) -> Result<Vec<ProviderStreamEvent>, ProviderError>
where
    Requester: ProviderRequester,
{
    let mut attempt = 0;
    loop {
        match requester.request(request.clone()).await {
            Ok(events) => return Ok(events),
            Err(error)
                if attempt < PROVIDER_RETRY_DELAYS_MS.len()
                    && provider_error_is_transient(error.kind) =>
            {
                let delay_ms = PROVIDER_RETRY_DELAYS_MS[attempt];
                attempt += 1;
                let _ = review_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!(
                        "provider {}; retrying in {}s (attempt {attempt} of {})",
                        provider_error_kind_label(error.kind),
                        delay_ms / 1_000,
                        PROVIDER_RETRY_DELAYS_MS.len(),
                    ),
                }));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Compaction accounting: `usable = context_window − max_output_tokens −
/// reserve`; the trigger fires above the configured percent of usable.
struct CompactionBudget<'a> {
    context_window: u64,
    max_output_tokens: u64,
    config: &'a crate::CompactionConfig,
}

impl CompactionBudget<'_> {
    fn usable_tokens(&self) -> u64 {
        crate::ContextBudget {
            context_window: self.context_window,
            max_output_tokens: self.max_output_tokens,
            reserve_tokens: self.config.reserve_tokens,
        }
        .usable_tokens()
    }

    fn threshold_tokens(&self) -> u64 {
        let percent_tokens = self
            .usable_tokens()
            .saturating_mul(u64::from(self.config.auto_threshold_percent_clamped()))
            / 100;
        // A threshold below the kept-tail budget can never be satisfied
        // after compaction, so a low percent would demand the impossible;
        // clamp to the floor keep_recent_tokens implies.
        percent_tokens.max(self.config.keep_recent_tokens)
    }

    fn over_threshold(&self, estimated_tokens: u64) -> bool {
        estimated_tokens > self.threshold_tokens()
    }
}

/// Context-meter budget from the active provider config plus the
/// compaction reserve; `None` without a configured provider.
fn context_budget(
    provider: Option<&ProviderConfig>,
    project_root: Option<&Path>,
) -> Option<crate::ContextBudget> {
    let provider = provider?;
    let config = crate::CompactionConfig::load_for_project(project_root);
    Some(crate::ContextBudget {
        context_window: provider.adapter.context_window,
        max_output_tokens: provider.adapter.max_tokens,
        reserve_tokens: config.reserve_tokens,
    })
}

fn estimate_provider_messages_tokens(messages: &[ProviderMessage]) -> u64 {
    messages
        .iter()
        .map(|message| {
            // Tool arguments and results are part of what the provider is
            // sent, so they count. Before native tool blocks they were
            // already inside `content` as prose and counted for free;
            // omitting them here would quietly shrink the meter and delay
            // compaction on exactly the turns that need it most.
            let calls: u64 = message
                .tool_calls
                .iter()
                .map(|call| {
                    crate::estimate_text_tokens(&call.name)
                        + crate::estimate_text_tokens(&call.arguments_json.to_string())
                })
                .sum();
            let results: u64 = message
                .tool_results
                .iter()
                .map(|result| crate::estimate_text_tokens(&result.content))
                .sum();
            crate::estimate_text_tokens(&message.content) + calls + results
        })
        .sum()
}

struct CompactionRun<'a> {
    session_id: &'a SessionId,
    turn_id: &'a TurnId,
    model: &'a ProviderModel,
    config: &'a crate::CompactionConfig,
    reason: crate::CompactionReason,
    tokens_before: u64,
    focus_instructions: Option<String>,
    log: &'a mut SessionLog,
    pending_events: &'a mut Vec<SessionEvent>,
    tool_event_store: Option<&'a JsonlSessionStore>,
    review_tx: &'a mpsc::UnboundedSender<BackendEvent>,
}

/// Run one compaction: select the cut, produce the summary via the
/// provider, and append the checkpoint. Returns false (leaving the session
/// uncompacted, with a visible status) when there is nothing to fold or
/// the summary call fails; the caller decides what that means for the
/// turn. Only checkpoint persistence failures are hard errors.
async fn run_compaction<Requester>(
    requester: &mut Requester,
    run: CompactionRun<'_>,
) -> Result<bool, ProviderRoundError>
where
    Requester: ProviderRequester,
{
    if run.config.compactor != "summary" {
        let _ = run
            .review_tx
            .send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!(
                    "compaction skipped: unknown compaction.compactor {:?}; only \"summary\" \
exists today",
                    run.config.compactor
                ),
            }));
        return Ok(false);
    }
    let Some(cut) = crate::select_compaction_cut(run.log, run.config.keep_recent_tokens) else {
        return Ok(false);
    };
    let previous = crate::newest_compaction_checkpoint(run.log);
    let preparation = crate::CompactionPreparation {
        serialized_conversation: crate::serialize_events_for_summary(
            &run.log.events[cut.fold_range.clone()],
        ),
        previous_summary: previous.as_ref().map(|view| view.summary.to_owned()),
        previous_details: previous.as_ref().map(|view| view.details.clone()),
        first_kept_entry_id: cut.first_kept_entry_id.clone(),
        tokens_before: run.tokens_before,
        reason: run.reason,
        focus_instructions: run.focus_instructions.clone(),
    };
    let details = crate::merge_compaction_file_details(
        previous.as_ref().map(|view| view.details),
        &run.log.events[cut.fold_range.clone()],
    );
    let _ = run
        .review_tx
        .send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: String::from("compacting context..."),
        }));
    let summary_request = ProviderRequest {
        turn_id: run.turn_id.clone(),
        model: run.model.clone(),
        messages: vec![ProviderMessage::text(
            Role::User,
            crate::build_summary_prompt(&preparation),
        )],
        extensions: Vec::new(),
    };
    let summary =
        match provider_request_with_retry(requester, &summary_request, run.review_tx).await {
            Ok(events) => match collect_native_provider_first_round(events) {
                Ok(round) if !round.text.trim().is_empty() => round.text,
                Ok(_) | Err(_) => {
                    let _ = run
                        .review_tx
                        .send(BackendEvent::Server(ServerEvent::StatusUpdated {
                            message: String::from(
                                "compaction failed: summarizer returned no usable summary; \
continuing uncompacted",
                            ),
                        }));
                    return Ok(false);
                }
            },
            Err(error) => {
                let _ = run
                    .review_tx
                    .send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: format!(
                            "compaction failed: {}; continuing uncompacted",
                            error.message
                        ),
                    }));
                return Ok(false);
            }
        };

    let kept_tail_tokens: u64 = run.log.events[cut.kept_start_index..]
        .iter()
        .map(crate::estimate_event_tokens)
        .sum();
    let tokens_after_estimate =
        crate::estimate_text_tokens(&summary).saturating_add(kept_tail_tokens);
    let checkpoint_index = run
        .log
        .events
        .iter()
        .filter(|event| matches!(event, SessionEvent::CompactionCheckpoint { .. }))
        .count()
        + 1;
    push_native_session_event(
        run.log,
        run.pending_events,
        SessionEvent::CompactionCheckpoint {
            session_id: run.session_id.clone(),
            turn_id: run.turn_id.clone(),
            checkpoint_id: crate::CompactionCheckpointId(format!("compaction-{checkpoint_index}")),
            summary,
            first_kept_entry_id: cut.first_kept_entry_id,
            tokens_before: run.tokens_before,
            tokens_after_estimate,
            reason: run.reason,
            compactor: run.config.compactor.clone(),
            details,
        },
    );
    if let Some(store) = run.tool_event_store
        && append_pending_native_session_events(store, run.pending_events).is_err()
    {
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "compaction_persist_failed",
        )));
    }
    let _ = run
        .review_tx
        .send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!(
                "compacted context: ~{}K -> ~{}K tokens",
                run.tokens_before / 1_000,
                tokens_after_estimate / 1_000
            ),
        }));
    Ok(true)
}

fn provider_tool_batch_result_budget_failure(
    error: ProviderRoundError,
) -> (ProviderRoundError, String) {
    match error {
        ProviderRoundError::ToolContinuation(label)
            if label.starts_with("tool_result_too_large:") =>
        {
            let reason = String::from("tool_round_result_too_large");
            (ProviderRoundError::ToolContinuation(reason.clone()), reason)
        }
        ProviderRoundError::ToolContinuation(label) => {
            (ProviderRoundError::ToolContinuation(label.clone()), label)
        }
        other => (other, String::from("tool_round_result_too_large")),
    }
}

/// Failed-but-continuable tool result for sensitive-file denies, following
/// the recoverable edit-failure shape: categorical error plus explicit
/// next-step guidance.
fn sensitive_denied_tool_result(request: &PendingToolRequest) -> ProviderToolResult {
    let content = crate::tool_text::verdict_with_guidance(
        "error: sensitive_path_denied",
        "This path matches the sensitive-file deny list, so its contents are \
    not available to tools. If access is intended, ask the user to allow the path under \
    files.allow in .yach/config.json and retry.",
    );
    ProviderToolResult {
        tool_request_id: request.request_id.clone(),
        provider_call_id: request.provider_call_id.clone(),
        status: ToolOutcome::Failed,
        byte_count: content.len(),
        content,
        redacted: true,
        truncated: false,
        reason: Some(String::from("sensitive_path_denied")),
    }
}

/// Categorical reason + guidance for read-only tool failures the model can
/// recover from. `None` means harness-integrity failure: abort the turn.
fn recoverable_readonly_failure(
    error: &crate::ToolExecutionError,
) -> Option<(&'static str, &'static str)> {
    match error {
        crate::ToolExecutionError::ResourceReadTooLarge => Some((
            "resource_read_too_large",
            "The file exceeds the read_text_file size limit (32KB). Use the bash tool to \
sample it instead (for example `head -c 20000 <path>`, `wc -l <path>`, or `sed -n '1,50p' \
<path>`), or read a smaller file.",
        )),
        crate::ToolExecutionError::ResourceReadNotUtf8 => Some((
            "resource_read_not_utf8",
            "The file is not valid UTF-8 text. Use the bash tool to inspect it (for example \
`file <path>` or `head -c 200 <path> | xxd`), or skip it.",
        )),
        crate::ToolExecutionError::ResourcePath { error } => match error {
            crate::ResourcePathError::Missing => Some((
                "path_missing",
                "The path does not exist. Use list_project_paths to inspect the project layout.",
            )),
            crate::ResourcePathError::EscapesRoot => Some((
                "path_outside_project",
                "Paths must stay inside the project root. Use project-relative paths.",
            )),
            crate::ResourcePathError::ExpectedFile => Some((
                "expected_file",
                "The path is a directory. Use list_project_paths to browse it, or name a file.",
            )),
            crate::ResourcePathError::ExpectedDirectory => Some((
                "expected_directory",
                "The path is a file, not a directory. Use read_text_file for file contents.",
            )),
            crate::ResourcePathError::RootUnavailable
            | crate::ResourcePathError::SensitiveDenied => None,
        },
        crate::ToolExecutionError::UnknownTool
        | crate::ToolExecutionError::PermissionDenied
        | crate::ToolExecutionError::UnsupportedTool
        | crate::ToolExecutionError::MalformedResult
        | crate::ToolExecutionError::ExtensionHost { .. } => None,
    }
}

fn execute_native_provider_readonly_tool_request(
    batch: &mut ProviderAgentToolBatch<'_>,
    request: PendingToolRequest,
) -> Result<ProviderToolResult, ProviderRoundError> {
    let tool_event_start = batch.log.events.len();
    let Ok(validation) = record_native_tool_validation_with_resolved_catalog(
        batch.log,
        batch.session_id.clone(),
        &request,
        batch.registry,
        batch.permission_policy,
        batch.resolved_catalog,
    ) else {
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_round_validation_failed",
        )));
    };
    let execution = match batch
        .read_only_executor
        .execute(batch.registry, &request, &validation)
    {
        Ok(execution) => execution,
        Err(crate::ToolExecutionError::ResourcePath {
            error: crate::ResourcePathError::SensitiveDenied,
        }) => {
            // Recoverable: the model asked for a path on the sensitive-file
            // deny list. Fail the tool call with guidance and continue the
            // loop instead of aborting the turn.
            let result = sensitive_denied_tool_result(&request);
            batch.log.push(SessionEvent::ToolExecutionFinished {
                session_id: batch.session_id.clone(),
                turn_id: batch.turn_id.clone(),
                tool_request_id: ToolRequestId(request.request_id.clone()),
                outcome: ToolOutcome::Failed,
                reason: Some(String::from("sensitive_path_denied")),
                result_summary: Some(ToolPayloadSummary {
                    summary: String::from("sensitive_path_denied"),
                    byte_count: result.byte_count,
                    redacted: true,
                    truncated: false,
                }),
                result_content: Some(result.content.clone()),
            });
            batch
                .pending_events
                .extend(batch.log.events[tool_event_start..].iter().cloned());
            return Ok(result);
        }
        Err(error) => {
            // Model-recoverable failures (oversized file, non-UTF-8, bad
            // path) fail the tool call with guidance and continue the loop;
            // only harness-integrity errors abort the turn.
            if let Some((reason, guidance)) = recoverable_readonly_failure(&error) {
                let result = failed_tool_result(&request, reason, guidance);
                batch.log.push(SessionEvent::ToolExecutionFinished {
                    session_id: batch.session_id.clone(),
                    turn_id: batch.turn_id.clone(),
                    tool_request_id: ToolRequestId(request.request_id.clone()),
                    outcome: ToolOutcome::Failed,
                    reason: Some(String::from(reason)),
                    result_summary: Some(ToolPayloadSummary {
                        summary: String::from(reason),
                        byte_count: result.byte_count,
                        redacted: true,
                        truncated: false,
                    }),
                    result_content: Some(result.content.clone()),
                });
                batch
                    .pending_events
                    .extend(batch.log.events[tool_event_start..].iter().cloned());
                return Ok(result);
            }
            batch.log.push(SessionEvent::ToolExecutionFinished {
                session_id: batch.session_id.clone(),
                turn_id: batch.turn_id.clone(),
                tool_request_id: ToolRequestId(request.request_id.clone()),
                outcome: ToolOutcome::Failed,
                reason: Some(String::from("tool_round_execution_failed")),
                result_summary: None,
                result_content: None,
            });
            batch
                .pending_events
                .extend(batch.log.events[tool_event_start..].iter().cloned());
            return Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_round_execution_failed",
            )));
        }
    };
    if let Err(error) = batch
        .budget
        .record_tool_result(&request.request_id, execution.byte_count)
    {
        let (error, reason) = provider_tool_batch_result_budget_failure(error);
        batch.log.push(SessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: ToolRequestId(request.request_id.clone()),
            outcome: ToolOutcome::Failed,
            reason: Some(reason),
            result_summary: None,
            result_content: None,
        });
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(error);
    }
    let result_summary = provider_readonly_tool_result_summary(&request.tool_name, &execution);
    batch.log.push(SessionEvent::ToolExecutionFinished {
        session_id: batch.session_id.clone(),
        turn_id: batch.turn_id.clone(),
        tool_request_id: ToolRequestId(request.request_id.clone()),
        outcome: ToolOutcome::Completed,
        reason: None,
        result_summary: Some(result_summary),
        result_content: Some(execution.summary.clone()),
    });
    batch
        .pending_events
        .extend(batch.log.events[tool_event_start..].iter().cloned());
    Ok(ProviderToolResult {
        tool_request_id: request.request_id,
        provider_call_id: request.provider_call_id,
        status: ToolOutcome::Completed,
        content: execution.summary,
        byte_count: execution.byte_count,
        redacted: execution.redacted,
        truncated: execution.truncated,
        reason: None,
    })
}

fn execute_native_provider_extension_tool_request(
    batch: &mut ProviderAgentToolBatch<'_>,
    request: PendingToolRequest,
    implementation_name: &str,
) -> Result<ProviderToolResult, ProviderRoundError> {
    let tool_event_start = batch.log.events.len();
    let Ok(validation) = record_native_tool_validation_with_resolved_catalog(
        batch.log,
        batch.session_id.clone(),
        &request,
        batch.registry,
        batch.permission_policy,
        batch.resolved_catalog,
    ) else {
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_round_validation_failed",
        )));
    };
    let Some(extension_executor) = batch.extension_executor else {
        batch.log.push(SessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: ToolRequestId(request.request_id.clone()),
            outcome: ToolOutcome::Failed,
            reason: Some(String::from("tool_round_execution_failed")),
            result_summary: None,
            result_content: None,
        });
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_round_execution_failed",
        )));
    };
    let mut implementation_request = request.clone();
    implementation_request.tool_name = String::from(implementation_name);
    let Ok(execution) =
        extension_executor.execute(batch.registry, &implementation_request, &validation)
    else {
        batch.log.push(SessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: ToolRequestId(request.request_id.clone()),
            outcome: ToolOutcome::Failed,
            reason: Some(String::from("tool_round_execution_failed")),
            result_summary: None,
            result_content: None,
        });
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_round_execution_failed",
        )));
    };
    if let Err(error) = batch
        .budget
        .record_tool_result(&request.request_id, execution.byte_count)
    {
        let (error, reason) = provider_tool_batch_result_budget_failure(error);
        batch.log.push(SessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: ToolRequestId(request.request_id.clone()),
            outcome: ToolOutcome::Failed,
            reason: Some(reason),
            result_summary: None,
            result_content: None,
        });
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(error);
    }
    let result_summary = provider_readonly_tool_result_summary(&request.tool_name, &execution);
    batch.log.push(SessionEvent::ToolExecutionFinished {
        session_id: batch.session_id.clone(),
        turn_id: batch.turn_id.clone(),
        tool_request_id: ToolRequestId(request.request_id.clone()),
        outcome: ToolOutcome::Completed,
        reason: None,
        result_summary: Some(result_summary),
        result_content: Some(execution.summary.clone()),
    });
    batch
        .pending_events
        .extend(batch.log.events[tool_event_start..].iter().cloned());
    Ok(ProviderToolResult {
        tool_request_id: request.request_id,
        provider_call_id: request.provider_call_id,
        status: ToolOutcome::Completed,
        content: execution.summary,
        byte_count: execution.byte_count,
        redacted: execution.redacted,
        truncated: execution.truncated,
        reason: None,
    })
}

async fn execute_native_provider_edit_tool_request(
    batch: &mut ProviderAgentToolBatch<'_>,
    request: PendingToolRequest,
) -> Result<ProviderToolResult, ProviderRoundError> {
    if let Some(store) = batch.tool_event_store
        && append_pending_native_session_events(store, batch.pending_events).is_err()
    {
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_event_persist_failed",
        )));
    }
    let tool_name = request.tool_name.clone();
    let prepared = prepare_agent_edit_tool_request(
        batch.registry,
        &batch.project_root,
        batch.edit_access,
        batch.edit_sink,
        AgentEditToolContext {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            permission_policy: PermissionPolicy::default_local_edit(),
            edit_policy: EditPolicy::conservative(),
        },
        request,
    );
    batch
        .edit_sink
        .drain_into(batch.log, batch.pending_events)?;
    let prepared = prepared
        .map_err(|error| ProviderRoundError::ToolContinuation(tool_round_error_label(&error)))?;
    let result = match prepared {
        AgentEditToolPrepared::Completed { trace_id, result }
        | AgentEditToolPrepared::Failed { trace_id, result } => {
            batch.edit_traces.push(ProviderContinuationEditTrace {
                trace_id,
                tool_name,
                tool_request_id: ToolRequestId(result.tool_request_id.clone()),
                provider_call_id: result.provider_call_id.clone(),
                preview_id: None,
                permission_decision_id: None,
            });
            result
        }
        AgentEditToolPrepared::Denied { result, .. } => {
            return Err(ProviderRoundError::ToolExecutionDenied {
                tool_request_id: result.tool_request_id,
                tool_name,
                reason: result.reason.unwrap_or_else(|| String::from("denied")),
            });
        }
        AgentEditToolPrepared::NeedsUserReview {
            trace_id,
            request_id,
            provider_call_id,
            preview,
            path,
            operation,
        } => {
            let pending = PendingAgentEditToolReview {
                trace_id,
                session_id: batch.session_id.clone(),
                turn_id: batch.turn_id.clone(),
                request_id: request_id.clone(),
                provider_call_id,
                preview_id: preview.preview_id.clone(),
                permission_decision_id: preview.permission_decision_id.clone(),
                path: path.clone(),
                operation: operation.clone(),
            };
            let continuation_trace = ProviderContinuationEditTrace {
                trace_id: pending.trace_id.clone(),
                tool_name: tool_name.clone(),
                tool_request_id: ToolRequestId(pending.request_id.clone()),
                provider_call_id: Some(pending.provider_call_id.clone()),
                preview_id: Some(pending.preview_id.clone()),
                permission_decision_id: Some(pending.permission_decision_id.clone()),
            };
            let preview_summary = local_edit_preview_summary(preview, path, operation);
            if batch
                .review_tx
                .send(BackendEvent::Server(ServerEvent::ToolReviewRequested {
                    request_id: request_id.clone(),
                    tool_name,
                    payload: ToolReviewPayload::LocalEdit {
                        preview: preview_summary,
                    },
                }))
                .is_err()
            {
                return Err(ProviderRoundError::Cancelled(String::from(
                    "ui receiver dropped during tool review",
                )));
            }
            let review_wait_started = Instant::now();
            let decision_result =
                wait_for_agent_edit_review_decision(batch.review_decisions, &pending).await;
            match &decision_result {
                Ok(LocalEditDecision::Apply) => record_review_wait_trace(
                    batch.edit_sink,
                    &pending,
                    review_wait_started,
                    EditTraceOutcome::Completed,
                    None,
                ),
                Ok(LocalEditDecision::Reject) => record_review_wait_trace(
                    batch.edit_sink,
                    &pending,
                    review_wait_started,
                    EditTraceOutcome::Rejected,
                    None,
                ),
                Err(error) => record_review_wait_trace(
                    batch.edit_sink,
                    &pending,
                    review_wait_started,
                    review_wait_error_outcome(error),
                    Some(provider_round_error_label(error)),
                ),
            }
            let decision = decision_result?;
            let reviewed = match decision {
                LocalEditDecision::Apply => {
                    apply_agent_edit_tool_review(batch.edit_access, batch.edit_sink, pending)
                }
                LocalEditDecision::Reject => {
                    reject_agent_edit_tool_review(batch.edit_access, batch.edit_sink, pending)
                }
            };
            batch
                .edit_sink
                .drain_into(batch.log, batch.pending_events)?;
            let result = reviewed.map_err(|error| {
                ProviderRoundError::ToolContinuation(tool_round_error_label(&error))
            })?;
            batch.edit_traces.push(continuation_trace);
            result
        }
    };
    batch
        .budget
        .record_tool_result(&result.tool_request_id, result.byte_count)
        .map_err(|error| provider_tool_batch_result_budget_failure(error).0)?;
    Ok(result)
}

static COMMAND_REVIEW_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_command_review_ids() -> (String, String) {
    let next = COMMAND_REVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    (
        format!("command-review-{next}"),
        format!("command-permission-{next}"),
    )
}

/// Failed-but-continuable tool result with actionable guidance (the
/// recoverable-failure shape: categorical error plus explicit next step).
fn failed_tool_result(
    request: &PendingToolRequest,
    reason: &str,
    guidance: &str,
) -> ProviderToolResult {
    let content = crate::tool_text::verdict_with_guidance(&format!("error: {reason}"), guidance);
    ProviderToolResult {
        tool_request_id: request.request_id.clone(),
        provider_call_id: request.provider_call_id.clone(),
        status: ToolOutcome::Failed,
        byte_count: content.len(),
        content,
        redacted: true,
        truncated: false,
        reason: Some(reason.to_owned()),
    }
}

/// `"unknown"` when the process was killed before it could report a code.
fn exit_code_label(code: Option<i32>) -> String {
    code.map_or_else(|| String::from("unknown"), |code| code.to_string())
}

fn record_native_bash_finished_event(
    batch: &mut ProviderAgentToolBatch<'_>,
    request_id: &str,
    outcome: ToolOutcome,
    reason: Option<String>,
    result: &ProviderToolResult,
) {
    batch.log.push(SessionEvent::ToolExecutionFinished {
        session_id: batch.session_id.clone(),
        turn_id: batch.turn_id.clone(),
        tool_request_id: ToolRequestId(request_id.to_owned()),
        outcome,
        reason,
        result_summary: Some(ToolPayloadSummary {
            summary: format!("bash outcome={outcome:?}"),
            byte_count: result.byte_count,
            redacted: true,
            truncated: result.truncated,
        }),
        result_content: Some(result.content.clone()),
    });
}

async fn wait_for_command_review_decision(
    review_decisions: &mut AgentEditDecisionReceiver,
    request_id: &str,
    review_id: &str,
    permission_decision_id: &str,
) -> Result<LocalEditDecision, ProviderRoundError> {
    let Some(decision) = review_decisions.recv().await else {
        return Err(ProviderRoundError::Cancelled(String::from(
            "tool review decision channel closed",
        )));
    };
    if decision.request_id == request_id
        && decision.preview_id == review_id
        && decision.permission_decision_id == permission_decision_id
    {
        return Ok(decision.decision);
    }
    Err(ProviderRoundError::ToolContinuation(String::from(
        "stale_tool_review_decision",
    )))
}

async fn execute_native_provider_bash_tool_request(
    batch: &mut ProviderAgentToolBatch<'_>,
    request: PendingToolRequest,
) -> Result<ProviderToolResult, ProviderRoundError> {
    let tool_event_start = batch.log.events.len();
    let Ok(_validation) = record_native_tool_validation_with_resolved_catalog(
        batch.log,
        batch.session_id.clone(),
        &request,
        batch.registry,
        batch.permission_policy,
        batch.resolved_catalog,
    ) else {
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_round_validation_failed",
        )));
    };

    let arguments = &request.arguments;
    let command = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let workdir = arguments
        .get("workdir")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let requested_timeout = arguments.get("timeout").and_then(serde_json::Value::as_u64);

    let finish_failed = |batch: &mut ProviderAgentToolBatch<'_>,
                         reason: &str,
                         guidance: &str|
     -> Result<ProviderToolResult, ProviderRoundError> {
        let result = failed_tool_result(&request, reason, guidance);
        record_native_bash_finished_event(
            batch,
            &request.request_id,
            ToolOutcome::Failed,
            Some(reason.to_owned()),
            &result,
        );
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        Ok(result)
    };

    // Resolve the working directory inside the project root.
    let root_path = batch.project_root.canonical_path().to_path_buf();
    let cwd = match &workdir {
        None => root_path.clone(),
        Some(dir) => {
            let joined = root_path.join(dir);
            match joined.canonicalize() {
                Ok(resolved) if resolved.starts_with(&root_path) && resolved.is_dir() => resolved,
                _ => {
                    return finish_failed(
                        batch,
                        "workdir_invalid",
                        "workdir must be an existing directory inside the project root. \
Use list_project_paths to inspect the project layout.",
                    );
                }
            }
        }
    };

    let shell_policy = &batch.shell_policy;
    if shell_policy.config.executor != "host" {
        return finish_failed(
            batch,
            "unknown_shell_executor",
            "The configured shell.executor is not available in this build; only \"host\" \
exists today. Ask the user to fix .yach/config.json.",
        );
    }

    let prepared = crate::PreparedCommand {
        command: command.clone(),
        cwd,
        env: crate::build_command_env(&shell_policy.config.env_allow),
        timeout: std::time::Duration::from_millis(shell_policy.clamp_timeout_ms(requested_timeout)),
    };

    // Approval: allowlisted commands auto-run; everything else waits for
    // the user's review decision.
    let _approved_by = if shell_policy.auto_run_eligible(&command) {
        "allowlist"
    } else {
        let (review_id, permission_decision_id) = next_command_review_ids();
        if batch
            .review_tx
            .send(BackendEvent::Server(ServerEvent::ToolReviewRequested {
                request_id: request.request_id.clone(),
                tool_name: String::from("bash"),
                payload: ToolReviewPayload::Command {
                    command: yach_proto::CommandReviewSummary {
                        review_id: review_id.clone(),
                        permission_decision_id: permission_decision_id.clone(),
                        command: command.clone(),
                        workdir: workdir.clone(),
                        timeout_ms: prepared.timeout.as_millis().try_into().unwrap_or(u64::MAX),
                    },
                },
            }))
            .is_err()
        {
            return Err(ProviderRoundError::Cancelled(String::from(
                "ui receiver dropped during tool review",
            )));
        }
        let decision = wait_for_command_review_decision(
            batch.review_decisions,
            &request.request_id,
            &review_id,
            &permission_decision_id,
        )
        .await?;
        match decision {
            LocalEditDecision::Apply => "user",
            LocalEditDecision::Reject => {
                return finish_failed(
                    batch,
                    "user_rejected",
                    "The user declined to run this command. Ask the user how to proceed \
or take a different approach.",
                );
            }
        }
    };

    // Live output: executor chunks forward to the UI as ToolCallOutput
    // while the command runs. join! polls both on this task; the forwarder
    // drains until the executor drops its sender at command end.
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
    let run = crate::CommandExecutor::run(&crate::HostCommandExecutor, prepared, Some(chunk_tx));
    let forward = async {
        while let Some(chunk) = chunk_rx.recv().await {
            let _ = batch
                .review_tx
                .send(BackendEvent::Server(ServerEvent::ToolCallOutput {
                    tool_call_id: request.request_id.clone(),
                    chunk,
                }));
        }
    };
    let (run_result, ()) = tokio::join!(run, forward);
    let outcome = match run_result {
        Ok(outcome) => outcome,
        Err(crate::CommandSpawnError::Spawn(error)) => {
            return finish_failed(
                batch,
                "spawn_failed",
                &format!("The command could not be started: {error}."),
            );
        }
    };

    if outcome.timed_out {
        return finish_failed(
            batch,
            "timeout",
            "The command exceeded its timeout and was killed. Retry with a larger timeout \
argument, or run a narrower command.",
        );
    }

    let mut notices = Vec::new();
    if outcome.output.is_empty() {
        notices.push(crate::tool_text::notice(&format!(
            "no output; exit code {}",
            exit_code_label(outcome.exit_code)
        )));
    } else if outcome.exit_code != Some(0) {
        notices.push(crate::tool_text::notice(&format!(
            "exit code {}",
            exit_code_label(outcome.exit_code)
        )));
    }
    if outcome.truncated {
        notices.push(crate::tool_text::notice(&format!(
            "truncated: kept {} of {} output bytes",
            outcome.output.len(),
            outcome.output_bytes_total
        )));
    }
    let content = crate::tool_text::append_notices(&outcome.output, &notices);
    let result = ProviderToolResult {
        tool_request_id: request.request_id.clone(),
        provider_call_id: request.provider_call_id.clone(),
        status: ToolOutcome::Completed,
        byte_count: content.len(),
        content,
        redacted: true,
        truncated: outcome.truncated,
        reason: None,
    };
    record_native_bash_finished_event(
        batch,
        &request.request_id,
        ToolOutcome::Completed,
        None,
        &result,
    );
    batch
        .pending_events
        .extend(batch.log.events[tool_event_start..].iter().cloned());
    if let Some(store) = batch.tool_event_store
        && append_pending_native_session_events(store, batch.pending_events).is_err()
    {
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_event_persist_failed",
        )));
    }
    batch
        .budget
        .record_tool_result(&request.request_id, result.byte_count)
        .map_err(|error| provider_tool_batch_result_budget_failure(error).0)?;
    Ok(result)
}

async fn execute_native_provider_agent_tool_batch(
    mut batch: ProviderAgentToolBatch<'_>,
    tool_calls: Vec<ProviderToolCall>,
) -> Result<Vec<ProviderToolResult>, ProviderRoundError> {
    batch.budget.begin_tool_round(tool_calls.len())?;
    let mut tool_results = Vec::with_capacity(tool_calls.len());
    for (index, tool_call) in tool_calls.into_iter().enumerate() {
        let request = pending_tool_request_from_provider_call(
            format!("tool-request-{}-{}", batch.tool_round_index, index + 1),
            batch.turn_id.clone(),
            tool_call,
        );
        emit_native_provider_tool_call_started(&batch.review_tx, &request)?;
        let request_id = request.request_id.clone();
        let tool_name = request.tool_name.clone();
        let implementation_name = batch
            .resolved_catalog
            .implementation_name_for_provider_tool(&request.tool_name)
            .map(str::to_owned);
        let result = match implementation_name.as_deref() {
            Some(
                "project_path_info" | "read_text_file" | "search_project" | "list_project_paths",
            ) => execute_native_provider_readonly_tool_request(&mut batch, request),
            Some("edit_text_file" | "create_text_file") => {
                execute_native_provider_edit_tool_request(&mut batch, request).await
            }
            Some("bash") => execute_native_provider_bash_tool_request(&mut batch, request).await,
            Some(implementation_name)
                if batch
                    .registry
                    .get(implementation_name)
                    .is_some_and(|definition| {
                        matches!(definition.owner, crate::ToolOwner::Extension { .. })
                    }) =>
            {
                execute_native_provider_extension_tool_request(
                    &mut batch,
                    request,
                    implementation_name,
                )
            }
            _ => {
                let tool_event_start = batch.log.events.len();
                let _ = record_native_tool_validation_with_resolved_catalog(
                    batch.log,
                    batch.session_id.clone(),
                    &request,
                    batch.registry,
                    batch.permission_policy,
                    batch.resolved_catalog,
                );
                batch
                    .pending_events
                    .extend(batch.log.events[tool_event_start..].iter().cloned());
                emit_native_provider_tool_call_error(
                    &batch.review_tx,
                    Some(request_id),
                    tool_name,
                    "tool_round_validation_failed",
                )?;
                return Err(ProviderRoundError::ToolContinuation(String::from(
                    "tool_round_validation_failed",
                )));
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let reason = provider_round_error_label(&error);
                emit_native_provider_tool_call_error(
                    &batch.review_tx,
                    Some(request_id),
                    tool_name,
                    &reason,
                )?;
                return Err(error);
            }
        };
        emit_native_provider_tool_call_finished(&batch.review_tx, &tool_name, &result)?;
        tool_results.push(result);
    }
    if let Some(store) = batch.tool_event_store
        && append_pending_native_session_events(store, batch.pending_events).is_err()
    {
        return Err(ProviderRoundError::ToolContinuation(String::from(
            "tool_event_persist_failed",
        )));
    }
    Ok(tool_results)
}

fn emit_native_provider_tool_call_started(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    request: &PendingToolRequest,
) -> Result<(), ProviderRoundError> {
    tx.send(BackendEvent::Server(ServerEvent::ToolCallStarted {
        tool_call_id: Some(request.request_id.clone()),
        tool_name: request.tool_name.clone(),
        preview: provider_tool_call_preview(&request.tool_name, &request.arguments),
    }))
    .map_err(|_| {
        ProviderRoundError::Cancelled(String::from("ui receiver dropped during tool progress"))
    })
}

fn emit_native_provider_tool_call_finished(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    tool_name: &str,
    result: &ProviderToolResult,
) -> Result<(), ProviderRoundError> {
    let is_error = result.status != ToolOutcome::Completed;
    emit_native_provider_tool_call_result(
        tx,
        Some(result.tool_request_id.clone()),
        tool_name.to_owned(),
        provider_tool_progress_output(tool_name, result),
        is_error,
    )
}

fn emit_native_provider_tool_call_error(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    tool_call_id: Option<String>,
    tool_name: String,
    reason: &str,
) -> Result<(), ProviderRoundError> {
    emit_native_provider_tool_call_result(
        tx,
        tool_call_id,
        tool_name,
        format!("failed: {reason}"),
        true,
    )
}

fn emit_native_provider_tool_call_result(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    tool_call_id: Option<String>,
    tool_name: String,
    output: String,
    is_error: bool,
) -> Result<(), ProviderRoundError> {
    tx.send(BackendEvent::Server(ServerEvent::ToolCallFinished(
        ToolResult {
            tool_call_id,
            tool_name,
            output,
            is_error,
        },
    )))
    .map_err(|_| {
        ProviderRoundError::Cancelled(String::from("ui receiver dropped during tool progress"))
    })
}

fn provider_tool_progress_output(tool_name: &str, result: &ProviderToolResult) -> String {
    tool_result_display(
        tool_name,
        result.status,
        Some(&result.content),
        result.byte_count,
        result.truncated,
        result.reason.as_deref(),
    )
}

/// Shared tool-result display shaping for live progress and resumed
/// transcript hydration, so both render identical rows from the same
/// provider-visible payload.
pub(super) fn tool_result_display(
    tool_name: &str,
    status: ToolOutcome,
    content: Option<&str>,
    byte_count: usize,
    truncated: bool,
    reason: Option<&str>,
) -> String {
    let status_label = match status {
        ToolOutcome::Completed => "completed",
        ToolOutcome::Failed => "failed",
        ToolOutcome::Denied => "denied",
        ToolOutcome::Cancelled => "cancelled",
        ToolOutcome::ValidationFailed => "validation_failed",
    };
    if status == ToolOutcome::Completed
        && let Some(content) = content
        && let Some(display) = provider_visible_tool_progress_output(tool_name, content)
    {
        return display;
    }
    if status == ToolOutcome::Failed
        && let Some(content) = content
        && let Some(display) = provider_visible_failed_progress(content)
    {
        return display;
    }
    let mut output =
        format!("{status_label}; bytes={byte_count}; content=redacted; truncated={truncated}");
    if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
        output.push_str("; reason=");
        output.push_str(reason);
    }
    output
}

fn provider_visible_tool_progress_output(tool_name: &str, content: &str) -> Option<String> {
    match tool_name {
        "read_text_file" => Some(read_progress_line(content)),
        "search_project" | "list_project_paths" => Some(head_lines_progress(content, 8)),
        "bash" => Some(tail_lines_progress(content, BASH_PROGRESS_TAIL_LINES)),
        "project_path_info" | "edit_text_file" | "create_text_file" => Some(format!(
            "completed: {}",
            content.lines().next().unwrap_or_default()
        )),
        _ => None,
    }
}

/// Reads are byte-exact file text; the row shows its size, not its body.
fn read_progress_line(content: &str) -> String {
    let line_count = content.lines().count().max(1);
    let line_label = if line_count == 1 { "line" } else { "lines" };
    format!(
        "completed: {line_count} {line_label}, {} bytes",
        content.len()
    )
}

/// First lines of a line-oriented result (search matches, listing
/// entries), with an elision marker. Notice lines count like any other
/// line — they are part of what the model saw.
fn head_lines_progress(content: &str, keep: usize) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut out = vec![format!("completed: {} lines", lines.len())];
    out.extend(lines.iter().take(keep).map(|line| (*line).to_owned()));
    if lines.len() > keep {
        out.push(format!("... {} more lines", lines.len() - keep));
    }
    out.join("\n")
}

/// Trailing lines of a command capture, so the evidence survives the
/// live stream being replaced by the finished row.
fn tail_lines_progress(content: &str, keep: usize) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut out = vec![format!("completed; {} bytes", content.len())];
    if lines.len() > keep {
        out.push(format!("... {} earlier lines", lines.len() - keep));
    }
    out.extend(
        lines
            .iter()
            .rev()
            .take(keep)
            .rev()
            .map(|line| (*line).to_owned()),
    );
    out.join("\n")
}

/// Failed contents are already `[error: ...]` + guidance — show them as-is.
fn provider_visible_failed_progress(content: &str) -> Option<String> {
    if content.starts_with('[') {
        Some(content.to_owned())
    } else {
        None
    }
}

/// Finished bash rows keep this many trailing output lines visible, so the
/// command's evidence survives the live stream (which the finished summary
/// replaces) and reappears on resume through the shared shaping path.
const BASH_PROGRESS_TAIL_LINES: usize = 8;

const MAX_TOOL_CALL_PREVIEW_CHARS: usize = 80;

/// Short argument-derived preview shown next to the tool name in the TUI
/// (the reviewed argument for review-gated tools; the primary target for
/// read-only tools), so users can see what a tool call touched.
fn provider_tool_call_preview(tool_name: &str, arguments: &serde_json::Value) -> Option<String> {
    let argument_name = match tool_name {
        "read_text_file" | "project_path_info" | "list_project_paths" | "edit_text_file"
        | "create_text_file" => "path",
        "search_project" => "query",
        "bash" => "command",
        _ => return None,
    };
    let value = arguments.get(argument_name)?.as_str()?;
    let first_line = value.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return None;
    }
    let mut preview: String = first_line
        .chars()
        .take(MAX_TOOL_CALL_PREVIEW_CHARS)
        .collect();
    if value.trim().chars().count() > preview.chars().count() {
        preview.push_str("...");
    }
    Some(preview)
}

async fn wait_for_agent_edit_review_decision(
    review_decisions: &mut AgentEditDecisionReceiver,
    pending: &PendingAgentEditToolReview,
) -> Result<LocalEditDecision, ProviderRoundError> {
    let Some(decision) = review_decisions.recv().await else {
        return Err(ProviderRoundError::Cancelled(String::from(
            "tool review decision channel closed",
        )));
    };
    if decision.request_id == pending.request_id
        && decision.preview_id == pending.preview_id.0
        && decision.permission_decision_id == pending.permission_decision_id.0
    {
        return Ok(decision.decision);
    }
    Err(ProviderRoundError::ToolContinuation(String::from(
        "stale_tool_review_decision",
    )))
}

fn provider_readonly_tool_result_summary(
    tool_name: &str,
    execution: &ToolExecutionResult,
) -> ToolPayloadSummary {
    let summary = match tool_name {
        "read_text_file" => String::from("read_text_file result redacted"),
        "search_project" => crate::tool_text::content_line_count_summary(
            "search_project",
            "matches",
            &execution.summary,
            execution.truncated,
        ),
        "list_project_paths" => crate::tool_text::content_line_count_summary(
            "list_project_paths",
            "entries",
            &execution.summary,
            execution.truncated,
        ),
        _ => execution.summary.clone(),
    };
    ToolPayloadSummary {
        summary,
        byte_count: execution.byte_count,
        redacted: matches!(
            tool_name,
            "read_text_file" | "search_project" | "list_project_paths"
        ),
        truncated: execution.truncated,
    }
}

fn record_review_wait_trace(
    sink: &impl SessionEventSink,
    pending: &PendingAgentEditToolReview,
    started: Instant,
    outcome: EditTraceOutcome,
    reason_label: Option<String>,
) {
    let mut log = SessionLog::default();
    log.record_edit_trace(
        pending.session_id.clone(),
        pending.turn_id.clone(),
        EditTraceRecord {
            trace_id: pending.trace_id.clone(),
            phase: EditTracePhase::ReviewWait,
            source: EditTraceSource::ProviderTool,
            tool_name: Some(pending.operation.clone()),
            tool_request_id: Some(ToolRequestId(pending.request_id.clone())),
            provider_call_id: Some(pending.provider_call_id.clone()),
            preview_id: Some(pending.preview_id.clone()),
            permission_decision_id: Some(pending.permission_decision_id.clone()),
            transaction_id: None,
            outcome,
            duration_ms: elapsed_ms(started),
            reason_label,
            attributes: vec![trace_attribute("operation", pending.operation.clone())],
        },
    );
    if let Some(event) = log.events.last() {
        let _ = sink.append_event(event);
    }
}

fn record_provider_continuation_trace_records(
    log: &mut SessionLog,
    pending_events: &mut Vec<SessionEvent>,
    store: Option<&JsonlSessionStore>,
    input: ProviderContinuationTraceInput<'_>,
) {
    for edit_trace in input.edit_traces {
        log.record_edit_trace(
            input.session_id.clone(),
            input.turn_id.clone(),
            EditTraceRecord {
                trace_id: edit_trace.trace_id.clone(),
                phase: EditTracePhase::ProviderContinuation,
                source: EditTraceSource::ProviderTool,
                tool_name: Some(edit_trace.tool_name.clone()),
                tool_request_id: Some(edit_trace.tool_request_id.clone()),
                provider_call_id: edit_trace.provider_call_id.clone(),
                preview_id: edit_trace.preview_id.clone(),
                permission_decision_id: edit_trace.permission_decision_id.clone(),
                transaction_id: None,
                outcome: input.outcome,
                duration_ms: elapsed_ms(input.started),
                reason_label: input.reason_label.map(str::to_owned),
                attributes: vec![trace_attribute("operation", edit_trace.tool_name.clone())],
            },
        );
        let Some(event) = log.events.last().cloned() else {
            continue;
        };
        if let Some(store) = store {
            let _ = store.append_event(&event);
        } else {
            pending_events.push(event);
        }
    }
}

fn trace_attribute(key: &str, value: impl Into<String>) -> MetricAttribute {
    MetricAttribute {
        key: key.to_owned(),
        value: value.into(),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn provider_round_error_label(error: &ProviderRoundError) -> String {
    match error {
        ProviderRoundError::Provider(_) => String::from("provider_failed"),
        ProviderRoundError::Cancelled(_) => String::from("provider_cancelled"),
        ProviderRoundError::StreamEndedWithoutCompletion => {
            String::from("stream_ended_without_completion")
        }
        ProviderRoundError::ProjectRootUnavailable => String::from("project_root_unavailable"),
        ProviderRoundError::ToolContinuation(reason) => reason.clone(),
        ProviderRoundError::ToolExecutionDenied { .. } => String::from("tool_execution_denied"),
        #[cfg(test)]
        ProviderRoundError::SecondRoundToolCall => String::from("unexpected_tool_call"),
    }
}

fn review_wait_error_outcome(error: &ProviderRoundError) -> EditTraceOutcome {
    match error {
        ProviderRoundError::Cancelled(_) => EditTraceOutcome::Cancelled,
        ProviderRoundError::Provider(_)
        | ProviderRoundError::StreamEndedWithoutCompletion
        | ProviderRoundError::ProjectRootUnavailable
        | ProviderRoundError::ToolContinuation(_)
        | ProviderRoundError::ToolExecutionDenied { .. } => EditTraceOutcome::Failed,
        #[cfg(test)]
        ProviderRoundError::SecondRoundToolCall => EditTraceOutcome::Failed,
    }
}

fn tool_round_error_label(error: &ToolContinuationError) -> String {
    match error {
        ToolContinuationError::TooManyToolCalls { .. } => String::from("tool_round_too_many_calls"),
        ToolContinuationError::Validation(_) => String::from("tool_round_validation_failed"),
        ToolContinuationError::Execution(_) => String::from("tool_round_execution_failed"),
        ToolContinuationError::ResultTooLarge { .. } => String::from("tool_round_result_too_large"),
    }
}

fn provider_mapping_error_label(error: &ProviderContinuationMappingError) -> String {
    match error {
        ProviderContinuationMappingError::Validation(_) => {
            String::from("tool_continuation_validation_failed")
        }
        ProviderContinuationMappingError::EmptyToolResults => {
            String::from("tool_continuation_empty_results")
        }
        ProviderContinuationMappingError::UnsupportedToolResultStatus { .. } => {
            String::from("tool_continuation_unsupported_status")
        }
    }
}

fn provider_tool_loop_stop_message(reason: &str) -> &'static str {
    match reason {
        "tool_loop_too_many_rounds"
        | "tool_loop_too_many_total_calls"
        | "tool_loop_total_result_too_large" => {
            "Native provider tool loop stopped before completion"
        }
        "context_overflow_after_compaction" => {
            "Context exceeds the usable window even after compaction; narrow \
the request or start a fresh session"
        }
        _ => "Native provider tool continuation failed",
    }
}

fn provider_tool_advertising_error_label(error: &ProviderToolAdvertisingError) -> String {
    match error {
        ProviderToolAdvertisingError::Malformed => {
            String::from("provider_tool_advertising_malformed")
        }
        ProviderToolAdvertisingError::EmptyTools => {
            String::from("provider_tool_advertising_empty_tools")
        }
        ProviderToolAdvertisingError::DuplicateExtension => {
            String::from("provider_tool_advertising_duplicate_extension")
        }
        ProviderToolAdvertisingError::DuplicateToolName { .. } => {
            String::from("provider_tool_advertising_duplicate_tool_name")
        }
        ProviderToolAdvertisingError::UnsupportedTool { .. } => {
            String::from("provider_tool_advertising_unsupported_tool")
        }
        ProviderToolAdvertisingError::UnsupportedRisk { .. } => {
            String::from("provider_tool_advertising_unsupported_risk")
        }
        ProviderToolAdvertisingError::UnsupportedSchema { .. } => {
            String::from("provider_tool_advertising_unsupported_schema")
        }
    }
}

fn provider_round_error_to_provider_error(error: &ProviderRoundError) -> ProviderError {
    match error {
        ProviderRoundError::Provider(error) => error.clone(),
        ProviderRoundError::Cancelled(reason) => ProviderError::cancelled(reason.clone()),
        ProviderRoundError::StreamEndedWithoutCompletion => ProviderError {
            kind: ProviderErrorKind::MalformedStream,
            message: String::from("Native provider stream ended without completion"),
            redacted_debug: None,
        },
        ProviderRoundError::ProjectRootUnavailable => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider project root unavailable"),
            redacted_debug: None,
        },
        ProviderRoundError::ToolContinuation(reason) => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from(provider_tool_loop_stop_message(reason)),
            redacted_debug: Some(reason.clone()),
        },
        ProviderRoundError::ToolExecutionDenied {
            tool_request_id,
            tool_name,
            reason,
        } => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider tool execution denied"),
            redacted_debug: Some(format!(
                "tool_execution_denied:{tool_name}:{tool_request_id}:{reason}"
            )),
        },
        #[cfg(test)]
        ProviderRoundError::SecondRoundToolCall => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider tool continuation failed"),
            redacted_debug: Some(String::from("unexpected_tool_call")),
        },
    }
}

#[derive(Debug, Clone)]
struct LaunchProjectContext {
    project_root: ResourceRoot,
    cwd: PathBuf,
}

#[cfg(test)]
impl LaunchProjectContext {
    fn from_project_root(project_root: ResourceRoot) -> Self {
        let cwd = project_root.canonical_path().to_path_buf();
        Self { project_root, cwd }
    }
}

fn launch_project_context(launch_cwd: impl AsRef<Path>) -> Option<LaunchProjectContext> {
    let cwd = launch_cwd.as_ref().canonicalize().ok()?;
    let project_root_path = nearest_project_marker_root(&cwd).unwrap_or_else(|| cwd.clone());
    let project_root = configured_project_root(project_root_path)?;
    Some(LaunchProjectContext { project_root, cwd })
}

fn launch_project_context_from_root(
    project_root: impl AsRef<Path>,
) -> Option<LaunchProjectContext> {
    let project_root = configured_project_root(project_root)?;
    let cwd = project_root.canonical_path().to_path_buf();
    Some(LaunchProjectContext { project_root, cwd })
}

/// Project resource root with the config-resolved sensitive-file policy
/// applied. Config load failures fail closed to the built-in defaults;
/// warnings surface separately at runner startup.
fn configured_project_root(project_root: impl AsRef<Path>) -> Option<ResourceRoot> {
    let root = ResourceRoot::project(project_root).ok()?;
    let (policy, _warnings) =
        crate::SensitivePathPolicy::load_for_project(Some(root.canonical_path()));
    Some(root.with_sensitive_policy(policy))
}

fn nearest_project_marker_root(cwd: &Path) -> Option<PathBuf> {
    if let Some(directory) = cwd
        .ancestors()
        .find(|directory| directory.join(".git").exists())
    {
        return Some(directory.to_path_buf());
    }

    for directory in cwd.ancestors() {
        if directory.join(".yach/APPEND_SYSTEM.md").exists() {
            return Some(directory.to_path_buf());
        }
    }
    None
}

async fn handle_started_native_provider_prompt<Requester>(
    tx: mpsc::UnboundedSender<BackendEvent>,
    store: JsonlSessionStore,
    provider: ProviderConfig,
    started_prompt: StartedPrompt,
    mut requester: Requester,
    project_runtime: ProviderPromptProjectRuntime,
    review_decisions: AgentEditDecisionReceiver,
) -> SessionLog
where
    Requester: ProviderRequester,
{
    let StartedPrompt {
        session_id,
        prompt,
        mut log,
        mut pending_events,
        turn,
        user_entry,
        assistant_entry,
        prompt_started,
    } = started_prompt;
    let ProviderPromptProjectRuntime {
        project_context,
        extension_manifest_scan_state,
        extension_activation_state,
    } = project_runtime;

    handle_native_provider_prompt(ProviderPromptRequest {
        tx: &tx,
        store: &store,
        _prompt: &prompt,
        provider,
        requester: &mut requester,
        log: &mut log,
        pending_events: &mut pending_events,
        ids: ProviderTurnRefs {
            session_id,
            turn,
            user_entry,
            assistant_entry,
            prompt_started,
        },
        project_context,
        extension_static_context_files: extension_static_context_files_from_scan_state(
            &extension_manifest_scan_state,
        )
        .await,
        extension_activation_snapshot: extension_activation_snapshot_from_state(
            &extension_activation_state,
        )
        .await,
        review_decisions,
    })
    .await;
    log
}

struct ProviderPromptRequest<'a, Requester> {
    tx: &'a mpsc::UnboundedSender<BackendEvent>,
    store: &'a JsonlSessionStore,
    _prompt: &'a str,
    provider: ProviderConfig,
    requester: &'a mut Requester,
    log: &'a mut SessionLog,
    pending_events: &'a mut Vec<SessionEvent>,
    ids: ProviderTurnRefs,
    project_context: Option<LaunchProjectContext>,
    extension_static_context_files: Vec<ExtensionStaticContextFile>,
    extension_activation_snapshot: crate::ExtensionActivationSnapshot,
    review_decisions: AgentEditDecisionReceiver,
}

async fn handle_native_provider_prompt<Requester>(request: ProviderPromptRequest<'_, Requester>)
where
    Requester: ProviderRequester,
{
    let ProviderPromptRequest {
        tx,
        store,
        _prompt: _,
        provider,
        requester,
        log,
        pending_events,
        ids,
        project_context,
        extension_static_context_files,
        extension_activation_snapshot,
        review_decisions,
    } = request;
    let provider_name = provider.provider_label();
    let model_id = provider.model.clone();
    if let Some(delay_ms) = provider.test_delay_ms {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native provider test delay: {delay_ms}ms"),
        }));
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    let project_context = project_context.or_else(|| {
        launch_project_context(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    });
    let context_budget = context_budget(
        Some(&provider),
        project_context
            .as_ref()
            .map(|context| context.project_root.canonical_path()),
    );
    let result = run_native_provider_one_agent_tool_round(
        requester,
        ProviderAgentToolRound {
            session_id: &ids.session_id,
            model: ProviderModel {
                provider: provider_name.to_owned(),
                model: model_id.clone(),
            },
            log,
            pending_events,
            turn_id: &ids.turn,
            project_context,
            extension_static_context_files,
            extension_activation_snapshot,
            tool_event_store: Some(store),
            review_tx: tx.clone(),
            review_decisions,
            context_window: provider.adapter.context_window,
            max_output_tokens: provider.adapter.max_tokens,
        },
    )
    .await;
    match result {
        Ok(round) => {
            // Mid-turn round text already streamed live; the persisted
            // assistant entry carries the full narrative (mid-turn + final)
            // so resume matches what the live transcript showed.
            let persisted_text = if round.mid_turn_text.is_empty() {
                round.text.clone()
            } else {
                format!("{}{}", round.mid_turn_text, round.text)
            };
            let response_chunks =
                if round.text.trim().is_empty() && round.mid_turn_text.trim().is_empty() {
                    vec![String::from(EMPTY_ASSISTANT_RESPONSE_MESSAGE)]
                } else if round.text.trim().is_empty() {
                    Vec::new()
                } else {
                    response_chunks(&round.text)
                };
            for delta in response_chunks {
                if tx
                    .send(BackendEvent::Server(ServerEvent::PromptDelta {
                        session_id: ids.session_id.0.clone(),
                        delta,
                    }))
                    .is_err()
                {
                    push_native_prompt_total_metric(
                        log,
                        pending_events,
                        &ids.session_id,
                        &ids.turn,
                        ids.prompt_started,
                    );
                    push_native_session_event(
                        log,
                        pending_events,
                        SessionEvent::TurnFinished {
                            session_id: ids.session_id.clone(),
                            turn_id: ids.turn,
                            outcome: TurnOutcome::Cancelled,
                            reason: Some(String::from("ui receiver dropped")),
                        },
                    );
                    let _ = append_pending_native_session_events(store, pending_events);
                    return;
                }
            }
            push_native_prompt_total_metric(
                log,
                pending_events,
                &ids.session_id,
                &ids.turn,
                ids.prompt_started,
            );
            push_native_session_event(
                log,
                pending_events,
                SessionEvent::EntryAppended {
                    session_id: ids.session_id.clone(),
                    entry_id: ids.assistant_entry,
                    parent_entry_id: Some(ids.user_entry),
                    turn_id: ids.turn.clone(),
                    role: Role::Assistant,
                    text: persisted_text,
                    provider: Some(ProviderMetadata {
                        provider: provider_name.to_owned(),
                        model: model_id,
                        response_id: round.provider_response_id,
                        usage: round.usage,
                    }),
                },
            );
            push_native_session_event(
                log,
                pending_events,
                SessionEvent::TurnFinished {
                    session_id: ids.session_id.clone(),
                    turn_id: ids.turn,
                    outcome: TurnOutcome::Completed,
                    reason: None,
                },
            );
            finish_native_prompt(
                tx,
                store,
                log,
                pending_events,
                PromptCompletion {
                    session_id: &ids.session_id.0,
                    status: "turn_end provider",
                    outcome: PromptOutcome::Completed,
                    context_budget,
                },
            );
        }
        Err(error) => {
            let provider_error = provider_round_error_to_provider_error(&error);
            let (turn_outcome, prompt_outcome, status) =
                if matches!(error, ProviderRoundError::Cancelled(_)) {
                    (
                        TurnOutcome::Cancelled,
                        PromptOutcome::Cancelled,
                        "turn_end provider cancelled",
                    )
                } else {
                    (
                        TurnOutcome::Failed,
                        PromptOutcome::Failed,
                        "turn_end provider failed",
                    )
                };
            push_native_prompt_total_metric(
                log,
                pending_events,
                &ids.session_id,
                &ids.turn,
                ids.prompt_started,
            );
            persist_native_fixture_error(
                tx,
                log,
                pending_events,
                &ids.session_id,
                ids.turn,
                turn_outcome,
                &provider_error,
            );
            finish_native_prompt(
                tx,
                store,
                log,
                pending_events,
                PromptCompletion {
                    session_id: &ids.session_id.0,
                    status,
                    outcome: prompt_outcome,
                    context_budget,
                },
            );
        }
    }
}

#[derive(Clone, Copy)]
struct PromptCompletion<'a> {
    session_id: &'a str,
    status: &'a str,
    outcome: PromptOutcome,
    context_budget: Option<crate::ContextBudget>,
}

fn finish_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &JsonlSessionStore,
    log: &SessionLog,
    pending_events: &mut Vec<SessionEvent>,
    completion: PromptCompletion<'_>,
) {
    let status = match append_pending_native_session_events(store, pending_events) {
        Ok(()) => completion.status.to_owned(),
        Err(error) => format!("failed to persist session log: {error}"),
    };
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: status.clone(),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::PromptFinished {
        session_id: completion.session_id.to_owned(),
        outcome: completion.outcome,
        message: Some(status),
    }));
    send_native_session_stats_from_log(tx, log, completion.context_budget);
}

fn persist_native_cancelled_turn(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &JsonlSessionStore,
    log: &mut SessionLog,
    session_id: &SessionId,
    turn_id: TurnId,
    prompt_started: Instant,
    reason: &str,
) {
    if log_has_finished_turn(log, &turn_id) {
        return;
    }

    let mut pending_events = Vec::new();
    push_native_prompt_total_metric(
        log,
        &mut pending_events,
        session_id,
        &turn_id,
        prompt_started,
    );
    push_native_session_event(
        log,
        &mut pending_events,
        SessionEvent::TurnFinished {
            session_id: session_id.clone(),
            turn_id,
            outcome: TurnOutcome::Cancelled,
            reason: Some(reason.to_owned()),
        },
    );
    finish_native_prompt(
        tx,
        store,
        log,
        &mut pending_events,
        PromptCompletion {
            session_id: &session_id.0,
            status: "turn_end provider cancelled",
            outcome: PromptOutcome::Cancelled,
            context_budget: None,
        },
    );
}

fn persist_native_fixture_error(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &mut SessionLog,
    pending_events: &mut Vec<SessionEvent>,
    session_id: &SessionId,
    turn_id: TurnId,
    outcome: TurnOutcome,
    error: &ProviderError,
) {
    let reason = provider_error_reason(error);
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: provider_failure_status(error),
    }));
    push_native_session_event(
        log,
        pending_events,
        SessionEvent::TurnFinished {
            session_id: session_id.clone(),
            turn_id,
            outcome,
            reason: Some(reason),
        },
    );
}

fn provider_error_reason(error: &ProviderError) -> String {
    match error.redacted_debug.as_deref() {
        Some(debug) if !debug.is_empty() => {
            format!(
                "provider_error kind={} message={} debug={debug}",
                provider_error_kind_label(error.kind),
                error.message
            )
        }
        _ => format!(
            "provider_error kind={} message={}",
            provider_error_kind_label(error.kind),
            error.message
        ),
    }
}

fn provider_failure_status(error: &ProviderError) -> String {
    let hint = match error.kind {
        ProviderErrorKind::Authentication => {
            "check provider credentials and required YACH_RIG_* env vars"
        }
        ProviderErrorKind::UnavailableModel => {
            "provider model is unavailable or unsupported; check YACH_RIG_*_MODEL"
        }
        ProviderErrorKind::Timeout => {
            "provider stream timed out; try again or increase YACH_RIG_PROVIDER_TIMEOUT_SECS"
        }
        ProviderErrorKind::Network => {
            "provider network error; check connectivity and provider endpoint"
        }
        ProviderErrorKind::RateLimited => {
            "provider rate limit reached; wait or switch provider/model"
        }
        ProviderErrorKind::InvalidRequest => {
            "provider rejected the request; inspect prompt/model setup"
        }
        ProviderErrorKind::ContextLength => "prompt is too large for the selected provider/model",
        ProviderErrorKind::Cancelled => "native provider cancelled",
        _ => error.message.as_str(),
    };
    format!(
        "provider failed ({}): {hint}",
        provider_error_kind_label(error.kind)
    )
}

const fn provider_error_kind_label(kind: ProviderErrorKind) -> &'static str {
    match kind {
        ProviderErrorKind::Authentication => "authentication",
        ProviderErrorKind::RateLimited => "rate_limited",
        ProviderErrorKind::InvalidRequest => "invalid_request",
        ProviderErrorKind::ContextLength => "context_length",
        ProviderErrorKind::UnavailableModel => "unavailable_model",
        ProviderErrorKind::Timeout => "timeout",
        ProviderErrorKind::Network => "network",
        ProviderErrorKind::ProviderInternal => "provider_internal",
        ProviderErrorKind::SafetyRefusal => "safety_refusal",
        ProviderErrorKind::MalformedStream => "malformed_stream",
        ProviderErrorKind::Backpressure => "backpressure",
        ProviderErrorKind::Cancelled => "cancelled",
        ProviderErrorKind::Unknown => "unknown",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureOutcome {
    Completed,
    Failed,
    Malformed,
    Cancelled,
}

impl FixtureOutcome {
    const fn status_message(self) -> &'static str {
        match self {
            Self::Completed => "turn_end",
            Self::Failed => "turn_end failed",
            Self::Malformed => "turn_end malformed",
            Self::Cancelled => "turn_end cancelled",
        }
    }

    const fn prompt_outcome(self) -> PromptOutcome {
        match self {
            Self::Completed => PromptOutcome::Completed,
            Self::Failed | Self::Malformed => PromptOutcome::Failed,
            Self::Cancelled => PromptOutcome::Cancelled,
        }
    }
}

fn fixture_outcome(prompt: &str) -> FixtureOutcome {
    if prompt.contains("/native-fixture-fail") {
        FixtureOutcome::Failed
    } else if prompt.contains("/native-fixture-malformed") {
        FixtureOutcome::Malformed
    } else if prompt.contains("/native-fixture-cancel") {
        FixtureOutcome::Cancelled
    } else {
        FixtureOutcome::Completed
    }
}

fn response_chunks(response: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in response.chars() {
        current.push(character);
        if current.len() >= 16 {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::{
        AgentEditReviewDecision, EMPTY_ASSISTANT_RESPONSE_MESSAGE,
        ExtensionActivationSnapshotState, ExtensionManifestScanState, FixtureOutcome,
        LaunchProjectContext, MAX_TOOL_CALL_PREVIEW_CHARS, ProviderAgentToolBatch,
        ProviderAgentToolRound, ProviderBufferedEventSink, ProviderConfig, ProviderRequester,
        ProviderRoundError, ProviderRoundResult, ProviderToolLoopBudget, ProviderToolLoopPolicy,
        ProviderToolRoundContext, backend_status_message, collect_native_provider_first_round,
        execute_native_provider_agent_tool_batch, fixture_outcome,
        handle_native_extension_diagnostic_snapshot_request,
        handle_native_extension_lifecycle_request, launch_project_context,
        load_native_session_log_for_runner, load_native_session_log_for_runner_with_loader,
        local_edit_error_message, log_has_finished_turn, provider_messages_from_log,
        provider_messages_from_log_with_static_context, provider_round_error_label,
        provider_round_error_to_provider_error, provider_tool_call_preview,
        provider_tool_progress_output, record_provider_continuation_trace_records, response_chunks,
        run_native_provider_one_agent_tool_round, run_native_provider_one_readonly_tool_round,
        run_native_provider_one_tool_round_with_registry, send_native_initial_state,
        send_native_session_messages_from_log, tool_result_display,
    };
    use crate::rig_adapter::{RigProviderAdapterConfig, RigProviderConfig};
    use crate::{
        EditAccess, EditAccessError, EditError, EditEvidenceOutcome, EditEvidenceSummary,
        EditOperationEvidence, EditPreviewId, EditTraceId, EditTraceOutcome, EditTracePhase,
        EditTraceRecord, EditTransactionId, EntryId, ExtensionActivationDiagnostic,
        ExtensionActivationSnapshot, ExtensionActivationState, ExtensionInstallScope,
        ExtensionManifestIndex, ExtensionPackageRoot, ExtensionToolExecutorRouter,
        ExtensionToolHandler, JsonlSessionStore, PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY,
        PermissionDecisionId, PermissionDecisionOutcome, ProjectReadOnlyToolExecutor,
        ProviderError, ProviderErrorKind, ProviderFinishReason, ProviderMessage, ProviderModel,
        ProviderRequest, ProviderStreamEvent, ProviderToolCall, ProviderToolResult,
        ProviderToolVisibility, ResourceRoot, Role, SessionEvent, SessionEventSink, SessionId,
        SessionLoadResult, SessionLog, StaticContextBundle, StaticContextItem,
        StaticContextPlacement, StaticContextPriority, StaticContextSource, ToolContinuationPolicy,
        ToolDefinition, ToolInputSchema, ToolOutcome, ToolPayloadSummary, ToolPermissionPolicy,
        ToolPermissionState, ToolRegistry, ToolReplacementPolicy, ToolReplacementRule,
        ToolReplacementSource, ToolRequestId, ToolResolutionMode, TurnId, TurnOutcome,
        completed_text_exchange, parse_provider_tool_advertising_extensions, sha256_hex_for_test,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;
    use yach_proto::{
        BackendEvent, Capability, ClientEvent, ExtensionDiagnosticSnapshotOutcome,
        ExtensionLifecycleAction, ExtensionLifecycleOutcome, LocalEditDecision,
        LocalEditFinishedOutcome, LocalEditOperationInput, LocalEditPreviewSummary,
        LocalEditReviewState, PromptOutcome, ServerEvent, ToolReviewPayload,
    };

    static TEMP_PROJECT_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn assert_async_extension_state_mutexes(
        _: &ExtensionManifestScanState,
        _: &ExtensionActivationSnapshotState,
    ) {
    }

    #[test]
    fn extension_shared_runner_state_uses_async_mutexes() {
        let scan_state: Arc<tokio::sync::Mutex<Option<ExtensionManifestIndex>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let activation_state: Arc<tokio::sync::Mutex<ExtensionActivationSnapshot>> = Arc::new(
            tokio::sync::Mutex::new(ExtensionActivationSnapshot::default()),
        );

        assert_async_extension_state_mutexes(&scan_state, &activation_state);
    }

    #[test]
    fn provider_tool_loop_policy_matches_design_limits() {
        let policy = ProviderToolLoopPolicy::agent_default();

        assert_eq!(policy.max_tool_rounds, None);
        assert_eq!(policy.max_tool_calls_per_round, 16);
        assert_eq!(policy.max_total_tool_calls, 200);
        assert_eq!(policy.max_result_bytes_per_tool, 64 * 1024);
        assert_eq!(policy.max_total_result_bytes, 512 * 1024);

        let continuation_policy = policy.as_continuation_policy();
        assert_eq!(
            continuation_policy,
            ToolContinuationPolicy {
                max_tool_calls: 16,
                max_result_bytes: 64 * 1024,
            }
        );
    }

    #[test]
    fn extension_lifecycle_request_stops_active_snapshot_tool() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let mut registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
            assert!(
                registry
                    .register_extension_tool(ToolDefinition::extension_metadata_tool(
                        "example.toy-tools",
                        "toy_tool",
                        "toy metadata",
                        ToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512,),
                        ProviderToolVisibility::Visible,
                    ))
                    .is_ok()
            );
            let activation_state = Arc::new(tokio::sync::Mutex::new(ExtensionActivationSnapshot {
                registry,
                executor: ExtensionToolExecutorRouter::from_handlers([(
                    "toy_tool",
                    ExtensionToolHandler::static_metadata(
                        "example.toy-tools",
                        "{\"kind\":\"toy\"}",
                    ),
                )]),
                diagnostics: vec![ExtensionActivationDiagnostic {
                    extension_id: Some(String::from("example.toy-tools")),
                    version: Some(String::from("0.1.0")),
                    scope: ExtensionInstallScope::User,
                    source_ref: Some(String::from("test-package-root")),
                    install_source: None,
                    package_root: PathBuf::from("/tmp/yach-extension"),
                    manifest_path: Some(PathBuf::from("/tmp/yach-extension/yach.extension.json")),
                    activation_state: ExtensionActivationState::Active,
                    generation: 1,
                    last_error_kind: None,
                    last_error_summary: None,
                    registered_tools: vec![String::from("toy_tool")],
                    provider_visible_tools: vec![String::from("toy_tool")],
                }],
                host_start_count: 1,
            }));
            let (tx, mut rx) = mpsc::unbounded_channel();

            handle_native_extension_lifecycle_request(
                &tx,
                &Arc::new(tokio::sync::Mutex::new(None)),
                &activation_state,
                String::from("extension-lifecycle-request-1"),
                ExtensionLifecycleAction::Stop,
                "example.toy-tools",
            )
            .await;

            assert_eq!(
                rx.try_recv(),
                Ok(BackendEvent::Server(
                    ServerEvent::ExtensionLifecycleFinished {
                        request_id: String::from("extension-lifecycle-request-1"),
                        action: ExtensionLifecycleAction::Stop,
                        selector: String::from("example.toy-tools"),
                        outcome: ExtensionLifecycleOutcome::Completed,
                        message: String::from("extension stopped: example.toy-tools"),
                    },
                ))
            );
            let snapshot = activation_state.lock().await;
            assert_eq!(snapshot.active_tool_names(), Vec::<&str>::new());
            assert!(snapshot.registry.get("toy_tool").is_none());
            assert_eq!(snapshot.executor.handler_count(), 0);
        });
    }

    #[test]
    fn extension_diagnostic_snapshot_request_reports_live_activation_state() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let activation_state = Arc::new(tokio::sync::Mutex::new(ExtensionActivationSnapshot {
                registry: ToolRegistry::with_project_read_only_and_agent_edit_tools(),
                executor: ExtensionToolExecutorRouter::default(),
                diagnostics: vec![ExtensionActivationDiagnostic {
                    extension_id: Some(String::from("example.toy-tools")),
                    version: Some(String::from("0.1.0")),
                    scope: ExtensionInstallScope::User,
                    source_ref: Some(String::from("test-package-root")),
                    install_source: None,
                    package_root: PathBuf::from("/tmp/yach-extension"),
                    manifest_path: Some(PathBuf::from("/tmp/yach-extension/yach.extension.json")),
                    activation_state: ExtensionActivationState::Stopped,
                    generation: 2,
                    last_error_kind: None,
                    last_error_summary: None,
                    registered_tools: Vec::new(),
                    provider_visible_tools: Vec::new(),
                }],
                host_start_count: 1,
            }));
            let (tx, mut rx) = mpsc::unbounded_channel();

            handle_native_extension_diagnostic_snapshot_request(
                &tx,
                &activation_state,
                String::from("extension-diagnostic-request-1"),
                Some("example.toy-tools"),
            )
            .await;

            let event = rx.try_recv();
            assert!(matches!(
                event,
                Ok(BackendEvent::Server(
                    ServerEvent::ExtensionDiagnosticSnapshotUpdated { .. }
                ))
            ));
            let Ok(BackendEvent::Server(ServerEvent::ExtensionDiagnosticSnapshotUpdated {
                request_id,
                outcome,
                records,
                message,
            })) = event
            else {
                return;
            };
            assert_eq!(request_id, "extension-diagnostic-request-1");
            assert_eq!(outcome, ExtensionDiagnosticSnapshotOutcome::Completed);
            assert_eq!(message, None);
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].id.as_deref(), Some("example.toy-tools"));
            assert_eq!(records[0].scope, "user");
            assert_eq!(records[0].activation_state, "stopped");
            assert_eq!(records[0].generation, 2);
            assert_eq!(records[0].package_root, "/tmp/yach-extension");
        });
    }

    #[test]
    fn extension_reload_request_requires_discovered_manifest_record() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let activation_state = Arc::new(tokio::sync::Mutex::new(
                ExtensionActivationSnapshot::default(),
            ));
            let scan_state = Arc::new(tokio::sync::Mutex::new(None));
            let (tx, mut rx) = mpsc::unbounded_channel();

            handle_native_extension_lifecycle_request(
                &tx,
                &scan_state,
                &activation_state,
                String::from("extension-lifecycle-request-1"),
                ExtensionLifecycleAction::Reload,
                "example.toy-tools",
            )
            .await;

            assert_eq!(
                rx.try_recv(),
                Ok(BackendEvent::Server(
                    ServerEvent::ExtensionLifecycleFinished {
                        request_id: String::from("extension-lifecycle-request-1"),
                        action: ExtensionLifecycleAction::Reload,
                        selector: String::from("example.toy-tools"),
                        outcome: ExtensionLifecycleOutcome::NotFound,
                        message: String::from("extension not discovered: example.toy-tools"),
                    },
                ))
            );
        });
    }

    #[test]
    fn provider_tool_loop_budget_rejects_round_call_and_byte_overages() {
        let policy = ProviderToolLoopPolicy {
            max_tool_rounds: Some(1),
            max_tool_calls_per_round: 2,
            max_total_tool_calls: 3,
            max_result_bytes_per_tool: 8,
            max_total_result_bytes: 12,
        };

        assert_eq!(
            ProviderToolLoopBudget::new(policy).begin_tool_round(3),
            Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_round_too_many_calls"
            )))
        );

        let mut budget = ProviderToolLoopBudget::new(policy);
        assert_eq!(budget.begin_tool_round(1), Ok(()));
        assert_eq!(
            budget.begin_tool_round(1),
            Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_rounds"
            )))
        );

        let total_call_policy = ProviderToolLoopPolicy {
            max_tool_rounds: None,
            ..policy
        };
        let mut budget = ProviderToolLoopBudget::new(total_call_policy);
        assert_eq!(budget.begin_tool_round(2), Ok(()));
        assert_eq!(
            budget.begin_tool_round(2),
            Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_total_calls"
            )))
        );

        assert_eq!(
            ProviderToolLoopBudget::new(policy).record_tool_result("call-too-large", 9),
            Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_result_too_large:call-too-large"
            )))
        );

        let mut budget = ProviderToolLoopBudget::new(policy);
        assert_eq!(budget.record_tool_result("call-a", 8), Ok(()));
        assert_eq!(
            budget.record_tool_result("call-b", 5),
            Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_loop_total_result_too_large"
            )))
        );
    }

    #[test]
    fn provider_agent_loop_limit_maps_to_redacted_provider_error() {
        let error = ProviderRoundError::ToolContinuation(String::from("tool_loop_too_many_rounds"));

        let provider_error = provider_round_error_to_provider_error(&error);

        assert_eq!(provider_error.kind, ProviderErrorKind::InvalidRequest);
        assert_eq!(
            provider_error.message,
            "Native provider tool loop stopped before completion"
        );
        assert_eq!(
            provider_error.redacted_debug,
            Some(String::from("tool_loop_too_many_rounds"))
        );
    }

    #[test]
    fn provider_round_error_label_maps_second_round_helper_to_unexpected_tool_call() {
        assert_eq!(
            provider_round_error_label(&ProviderRoundError::SecondRoundToolCall),
            "unexpected_tool_call"
        );
    }

    #[test]
    fn provider_agent_tool_batch_executes_read_tool_results() {
        let root = TempProject::new("native-provider-agent-tool-batch-read");
        root.write("src/lib.rs", "alpha\n");
        let project_root = ResourceRoot::project(root.root());
        assert!(project_root.is_ok());
        let Ok(project_root) = project_root else {
            return;
        };
        let registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
        let permission_policy =
            ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
                ["project_path_info"],
                ["read_text_file", "search_project", "list_project_paths"],
                ["edit_text_file", "create_text_file"],
            );
        let resolved_catalog = registry.resolve_provider_turn_catalog(
            &permission_policy,
            [
                "project_path_info",
                "read_text_file",
                "search_project",
                "list_project_paths",
                "edit_text_file",
                "create_text_file",
            ],
        );
        let read_only_executor = ProjectReadOnlyToolExecutor::new(project_root.clone());
        let mut edit_access = EditAccess::default();
        let edit_sink = ProviderBufferedEventSink::new(None);
        let (review_tx, _review_rx) = mpsc::unbounded_channel();
        let (_decision_tx, mut review_decisions) = mpsc::unbounded_channel();
        let mut budget = ProviderToolLoopBudget::new(ProviderToolLoopPolicy::agent_default());
        let mut edit_traces = Vec::new();
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = TurnId(String::from("turn-1"));

        let results = futures::executor::block_on(execute_native_provider_agent_tool_batch(
            ProviderAgentToolBatch {
                session_id: SessionId(String::from("default")),
                shell_policy: crate::ShellPolicy::default(),
                turn_id: turn_id.clone(),
                project_root,
                registry: &registry,
                resolved_catalog: &resolved_catalog,
                permission_policy: &permission_policy,
                read_only_executor: &read_only_executor,
                extension_executor: None,
                edit_access: &mut edit_access,
                edit_sink: &edit_sink,
                review_tx,
                review_decisions: &mut review_decisions,
                tool_event_store: None,
                budget: &mut budget,
                tool_round_index: 1,
                edit_traces: &mut edit_traces,
                log: &mut log,
                pending_events: &mut pending_events,
            },
            vec![ProviderToolCall {
                call_id: String::from("call-read-1"),
                name: String::from("read_text_file"),
                arguments_json: serde_json::json!({"path": "src/lib.rs"}),
            }],
        ));

        assert!(results.is_ok());
        let Ok(results) = results else {
            return;
        };
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].provider_call_id,
            Some(String::from("call-read-1"))
        );
        assert_eq!(results[0].content, "alpha\n");
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolExecutionFinished {
                tool_request_id,
                outcome: ToolOutcome::Completed,
                ..
            } if tool_request_id == &ToolRequestId(String::from("tool-request-1-1"))
        )));
    }

    #[test]
    fn provider_agent_tool_batch_executes_extension_metadata_tool_results() {
        let root = TempProject::new("native-provider-agent-tool-batch-extension");
        let project_root = ResourceRoot::project(root.root());
        assert!(project_root.is_ok());
        let Ok(project_root) = project_root else {
            return;
        };
        let mut registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
        assert_eq!(
            registry.register_extension_tool(ToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                ToolInputSchema::string_object(
                    std::iter::empty::<&str>(),
                    std::iter::empty::<&str>(),
                    512,
                ),
                ProviderToolVisibility::Visible,
            )),
            Ok(())
        );
        let permission_policy =
            ToolPermissionPolicy::allow_project_metadata_tools(["project_path_info", "toy_tool"]);
        let resolved_catalog =
            registry.resolve_provider_turn_catalog(&permission_policy, ["toy_tool"]);
        let read_only_executor = ProjectReadOnlyToolExecutor::new(project_root.clone());
        let extension_executor = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata("example.toy-tools", "{\"ok\":true}"),
        )]);
        let mut edit_access = EditAccess::default();
        let edit_sink = ProviderBufferedEventSink::new(None);
        let (review_tx, _review_rx) = mpsc::unbounded_channel();
        let (_decision_tx, mut review_decisions) = mpsc::unbounded_channel();
        let mut budget = ProviderToolLoopBudget::new(ProviderToolLoopPolicy::agent_default());
        let mut edit_traces = Vec::new();
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = TurnId(String::from("turn-1"));

        let results = futures::executor::block_on(execute_native_provider_agent_tool_batch(
            ProviderAgentToolBatch {
                session_id: SessionId(String::from("default")),
                shell_policy: crate::ShellPolicy::default(),
                turn_id: turn_id.clone(),
                project_root,
                registry: &registry,
                resolved_catalog: &resolved_catalog,
                permission_policy: &permission_policy,
                read_only_executor: &read_only_executor,
                extension_executor: Some(&extension_executor),
                edit_access: &mut edit_access,
                edit_sink: &edit_sink,
                review_tx,
                review_decisions: &mut review_decisions,
                tool_event_store: None,
                budget: &mut budget,
                tool_round_index: 1,
                edit_traces: &mut edit_traces,
                log: &mut log,
                pending_events: &mut pending_events,
            },
            vec![ProviderToolCall {
                call_id: String::from("call-toy-1"),
                name: String::from("toy_tool"),
                arguments_json: serde_json::json!({}),
            }],
        ));

        assert!(results.is_ok());
        let Ok(results) = results else {
            return;
        };
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].provider_call_id.as_deref(), Some("call-toy-1"));
        assert_eq!(results[0].content, "{\"ok\":true}");
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolExecutionFinished {
                tool_request_id,
                outcome: ToolOutcome::Completed,
                ..
            } if tool_request_id == &ToolRequestId(String::from("tool-request-1-1"))
        )));
    }

    #[test]
    fn provider_agent_tool_batch_records_replacement_provenance_evidence() {
        let root = TempProject::new("native-provider-agent-tool-batch-replacement");
        let project_root = ResourceRoot::project(root.root());
        assert!(project_root.is_ok());
        let Ok(project_root) = project_root else {
            return;
        };
        let mut registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
        assert_eq!(
            registry.register_extension_tool(ToolDefinition::extension_metadata_tool_with_version(
                "example.toy-tools",
                Some("1.2.3"),
                "toy_path_info",
                "Replacement path metadata implementation.",
                ToolDefinition::project_path_info().input_schema,
                ProviderToolVisibility::Visible,
            )),
            Ok(())
        );
        let permission_policy = ToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_path_info",
        ]);
        let replacement_policy = ToolReplacementPolicy::from_rules([ToolReplacementRule {
            builtin_name: String::from("project_path_info"),
            extension_id: String::from("example.toy-tools"),
            extension_tool: String::from("toy_path_info"),
            mode: ToolResolutionMode::ReplaceBuiltin,
            source: ToolReplacementSource::Profile,
        }]);
        let resolved_catalog = registry.resolve_provider_turn_catalog_with_replacements(
            &permission_policy,
            ["project_path_info", "toy_path_info"],
            &replacement_policy,
        );
        assert!(resolved_catalog.is_ok());
        let Ok(resolved_catalog) = resolved_catalog else {
            return;
        };
        let read_only_executor = ProjectReadOnlyToolExecutor::new(project_root.clone());
        let extension_executor = ExtensionToolExecutorRouter::from_handlers([(
            "toy_path_info",
            ExtensionToolHandler::static_metadata("example.toy-tools", "{\"ok\":true}"),
        )]);
        let mut edit_access = EditAccess::default();
        let edit_sink = ProviderBufferedEventSink::new(None);
        let (review_tx, _review_rx) = mpsc::unbounded_channel();
        let (_decision_tx, mut review_decisions) = mpsc::unbounded_channel();
        let mut budget = ProviderToolLoopBudget::new(ProviderToolLoopPolicy::agent_default());
        let mut edit_traces = Vec::new();
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = TurnId(String::from("turn-1"));

        let results = futures::executor::block_on(execute_native_provider_agent_tool_batch(
            ProviderAgentToolBatch {
                session_id: SessionId(String::from("default")),
                shell_policy: crate::ShellPolicy::default(),
                turn_id: turn_id.clone(),
                project_root,
                registry: &registry,
                resolved_catalog: &resolved_catalog,
                permission_policy: &permission_policy,
                read_only_executor: &read_only_executor,
                extension_executor: Some(&extension_executor),
                edit_access: &mut edit_access,
                edit_sink: &edit_sink,
                review_tx,
                review_decisions: &mut review_decisions,
                tool_event_store: None,
                budget: &mut budget,
                tool_round_index: 1,
                edit_traces: &mut edit_traces,
                log: &mut log,
                pending_events: &mut pending_events,
            },
            vec![ProviderToolCall {
                call_id: String::from("call-replaced-1"),
                name: String::from("project_path_info"),
                arguments_json: serde_json::json!({"path": "README.md"}),
            }],
        ));

        assert!(results.is_ok());
        let request_event = pending_events.iter().find_map(|event| match event {
            SessionEvent::ToolRequestRecorded {
                argument_summary, ..
            } => Some(argument_summary),
            _ => None,
        });
        assert!(request_event.is_some());
        let Some(argument_summary) = request_event else {
            return;
        };
        assert!(argument_summary.redacted);
        assert!(
            argument_summary
                .summary
                .contains("resolved_tool=extension_replacement")
        );
        assert!(
            argument_summary
                .summary
                .contains("extension_id=example.toy-tools")
        );
        assert!(argument_summary.summary.contains("extension_version=1.2.3"));
        assert!(
            argument_summary
                .summary
                .contains("provider_name=project_path_info")
        );
        assert!(
            argument_summary
                .summary
                .contains("implementation=toy_path_info")
        );
        assert!(
            argument_summary
                .summary
                .contains("replacement_source=profile")
        );
    }

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(name: &str) -> Self {
            let sequence = TEMP_PROJECT_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "yach-native-runner-{name}-{}-{sequence}",
                std::process::id()
            ));
            assert!(
                std::fs::create_dir_all(&root).is_ok(),
                "failed to create temp project at {}",
                root.display()
            );
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn write(&self, relative_path: &str, content: &str) {
            let path = self.root.join(relative_path);
            if let Some(parent) = path.parent() {
                assert!(
                    std::fs::create_dir_all(parent).is_ok(),
                    "failed to create parent directory at {}",
                    parent.display()
                );
            }
            assert!(
                std::fs::write(&path, content).is_ok(),
                "failed to write file at {}",
                path.display()
            );
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn extension_manifest_scan_package_root(root: &TempProject) -> ExtensionPackageRoot {
        ExtensionPackageRoot {
            root: root.root().to_path_buf(),
            scope: ExtensionInstallScope::Project,
            source_ref: Some(String::from("test-package-root")),
        }
    }

    fn extension_manifest_scan_manifest_json() -> &'static str {
        r#"{
  "schema": "yach.extension.v1",
  "id": "example.scan-toy-tools",
  "version": "0.1.0",
  "main": {
    "command": "node",
    "args": ["./extension.js"]
  },
  "activation": {
    "events": ["onCommand:yach.extensions.activate.example.scan-toy-tools"]
  },
  "contributes": {
    "tools": [{
      "name": "scan_toy_tool",
      "description": "Return static fixture metadata when activated.",
      "risk": "reads_local_metadata",
      "provider_visible": false
    }]
  }
}"#
    }

    fn extension_static_context_manifest_json() -> &'static str {
        r#"{
  "schema": "yach.extension.v1",
  "id": "example.context-pack",
  "version": "0.1.0",
  "main": {
    "command": "node",
    "args": ["./extension.js"]
  },
  "activation": {
    "events": ["onCommand:yach.extensions.activate.example.context-pack"]
  },
  "contributes": {
    "static_context": [{
      "id": "rust-style-guide",
      "title": "Rust style guide",
      "source": {
        "type": "extension_file",
        "path": "context/rust.md"
      },
      "placement": "background_context",
      "max_bytes": 1024
    }]
  }
}"#
    }

    async fn collect_extension_manifest_scan_statuses(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
        terminal_prefix: &str,
    ) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut statuses = Vec::new();
        while tokio::time::Instant::now() < deadline {
            let event = tokio::time::timeout_at(deadline, backend_rx.recv()).await;
            let Ok(Some(BackendEvent::Server(ServerEvent::StatusUpdated { message }))) = event
            else {
                continue;
            };
            if message.starts_with("extension_manifest_scan_")
                || message.starts_with("extension_background_activation_")
            {
                let done = message.starts_with(terminal_prefix);
                statuses.push(message);
                if done {
                    break;
                }
            }
        }
        statuses
    }

    #[test]
    fn extension_manifest_scan_waits_until_first_render_completed() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("extension-manifest-scan-order");
            root.write(
                "yach.extension.json",
                extension_manifest_scan_manifest_json(),
            );
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let trace_labels = Arc::new(Mutex::new(Vec::new()));
            let marker_labels = trace_labels.clone();
            let marker = super::StartupTraceMarker::new(move |label| {
                if let Ok(mut labels) = marker_labels.lock() {
                    labels.push(label.to_owned());
                }
            });
            let handle = tokio::spawn(super::run_native_loop(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path,
                    project_root: Some(root.root().to_path_buf()),
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: vec![extension_manifest_scan_package_root(&root)],
                    extension_package_root_loader: None,
                    startup_trace: Some(marker),
                },
            ));

            let first = backend_rx.recv().await;
            assert!(matches!(
                first,
                Some(BackendEvent::Server(ServerEvent::Ready { .. }))
            ));
            let second = backend_rx.recv().await;
            assert!(matches!(
                second,
                Some(BackendEvent::Server(ServerEvent::StateUpdated(_)))
            ));
            while let Ok(event) = backend_rx.try_recv() {
                if let BackendEvent::Server(ServerEvent::StatusUpdated { message }) = event {
                    assert!(!message.starts_with("extension_manifest_scan_"));
                }
            }

            assert!(client_tx.send(ClientEvent::FirstRenderCompleted).is_ok());
            let statuses = collect_extension_manifest_scan_statuses(
                &mut backend_rx,
                "extension_background_activation_finished",
            )
            .await;

            assert_eq!(
                statuses,
                vec![
                    String::from("extension_manifest_scan_scheduled"),
                    String::from("extension_manifest_scan_started"),
                    String::from(
                        "extension_manifest_scan_finished extension_count=1 host_start_count=0"
                    ),
                    String::from("extension_background_activation_scheduled"),
                    String::from("extension_background_activation_started"),
                    String::from(
                        "extension_background_activation_finished active_extension_count=0 registered_tool_count=0 host_start_count=0"
                    ),
                ]
            );
            let labels = trace_labels.lock().map(|labels| labels.clone());
            assert!(labels.is_ok());
            assert_eq!(
                labels.unwrap_or_default(),
                vec![
                    String::from("extension_manifest_scan_scheduled"),
                    String::from("extension_manifest_scan_started"),
                    String::from("extension_manifest_scan_finished"),
                    String::from("extension_background_activation_scheduled"),
                    String::from("extension_background_activation_started"),
                    String::from("extension_background_activation_finished"),
                ]
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn extension_manifest_scan_reports_redacted_failure() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("extension-manifest-scan-failure");
            root.write(
                "package.json",
                r#"{"yach":{"manifests":["missing/yach.extension.json"]}}"#,
            );
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(super::run_native_loop(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path,
                    project_root: Some(root.root().to_path_buf()),
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: vec![extension_manifest_scan_package_root(&root)],
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
            ));

            assert!(client_tx.send(ClientEvent::FirstRenderCompleted).is_ok());
            let statuses = collect_extension_manifest_scan_statuses(
                &mut backend_rx,
                "extension_manifest_scan_failed",
            )
            .await;

            let failure = statuses.last();
            assert_eq!(
                failure,
                Some(&String::from(
                    "extension_manifest_scan_failed reason=missing_manifest_file"
                ))
            );
            assert!(
                failure
                    .is_none_or(|message| !message.contains(root.root().to_string_lossy().as_ref()))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    fn edit_trace_records(log: &SessionLog) -> Vec<EditTraceRecord> {
        log.events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::EditTraceRecorded { trace, .. } => Some(trace.clone()),
                SessionEvent::EntryAppended { .. }
                | SessionEvent::ToolRequestRecorded { .. }
                | SessionEvent::ToolExecutionFinished { .. }
                | SessionEvent::TurnFinished { .. }
                | SessionEvent::MetricRecorded { .. }
                | SessionEvent::StaticContextIncluded { .. }
                | SessionEvent::PermissionDecisionRecorded { .. }
                | SessionEvent::EditTransactionPrepared { .. }
                | SessionEvent::EditTransactionFinished { .. }
                | SessionEvent::CompactionCheckpoint { .. } => None,
            })
            .collect()
    }

    #[derive(Debug, Default)]
    struct FakeProviderRequester {
        requests: Vec<ProviderRequest>,
        responses: std::collections::VecDeque<Result<Vec<ProviderStreamEvent>, ProviderError>>,
    }

    impl FakeProviderRequester {
        fn with_responses(
            responses: impl IntoIterator<Item = Result<Vec<ProviderStreamEvent>, ProviderError>>,
        ) -> Self {
            Self {
                requests: Vec::new(),
                responses: responses.into_iter().collect(),
            }
        }
    }

    impl ProviderRequester for FakeProviderRequester {
        fn request(
            &mut self,
            request: ProviderRequest,
        ) -> futures::future::BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>>
        {
            self.requests.push(request);
            let response = self.responses.pop_front().unwrap_or_else(|| {
                Err(ProviderError {
                    kind: ProviderErrorKind::InvalidRequest,
                    message: String::from("missing fake provider response"),
                    redacted_debug: None,
                })
            });
            Box::pin(async move { response })
        }
    }

    type RecordingProviderResponses = std::sync::Arc<
        std::sync::Mutex<
            std::collections::VecDeque<Result<Vec<ProviderStreamEvent>, ProviderError>>,
        >,
    >;

    #[derive(Debug, Clone)]
    struct RecordingProviderRequester {
        requests: std::sync::Arc<std::sync::Mutex<Vec<ProviderRequest>>>,
        responses: RecordingProviderResponses,
    }

    impl RecordingProviderRequester {
        fn with_responses(
            responses: impl IntoIterator<Item = Result<Vec<ProviderStreamEvent>, ProviderError>>,
        ) -> (Self, std::sync::Arc<std::sync::Mutex<Vec<ProviderRequest>>>) {
            let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    requests: requests.clone(),
                    responses: std::sync::Arc::new(std::sync::Mutex::new(
                        responses.into_iter().collect(),
                    )),
                },
                requests,
            )
        }
    }

    impl ProviderRequester for RecordingProviderRequester {
        fn request(
            &mut self,
            request: ProviderRequest,
        ) -> futures::future::BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>>
        {
            {
                let requests = self.requests.lock();
                assert!(requests.is_ok());
                let Ok(mut requests) = requests else {
                    return Box::pin(async {
                        Err(ProviderError {
                            kind: ProviderErrorKind::Unknown,
                            message: String::from("recording requester lock poisoned"),
                            redacted_debug: None,
                        })
                    });
                };
                requests.push(request);
            }
            let response = {
                let responses = self.responses.lock();
                assert!(responses.is_ok());
                let Ok(mut responses) = responses else {
                    return Box::pin(async {
                        Err(ProviderError {
                            kind: ProviderErrorKind::Unknown,
                            message: String::from("recording requester response lock poisoned"),
                            redacted_debug: None,
                        })
                    });
                };
                responses.pop_front().unwrap_or_else(|| {
                    Err(ProviderError {
                        kind: ProviderErrorKind::InvalidRequest,
                        message: String::from("missing fake provider response"),
                        redacted_debug: None,
                    })
                })
            };
            Box::pin(async move { response })
        }
    }

    #[test]
    fn response_chunks_preserve_unicode() {
        let chunks = response_chunks("hello 🙂 native runner");

        assert_eq!(chunks.concat(), "hello 🙂 native runner");
        assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
    }

    #[test]
    fn fixture_outcome_uses_explicit_markers() {
        assert_eq!(fixture_outcome("hello"), FixtureOutcome::Completed);
        assert_eq!(
            fixture_outcome("/native-fixture-fail"),
            FixtureOutcome::Failed
        );
        assert_eq!(
            fixture_outcome("/native-fixture-malformed"),
            FixtureOutcome::Malformed
        );
        assert_eq!(
            fixture_outcome("/native-fixture-cancel"),
            FixtureOutcome::Cancelled
        );
    }

    #[test]
    fn status_reports_local_read_only_resources_available() {
        let status = backend_status_message(None, None);

        assert_eq!(
            status,
            "backend: no provider configured; local read-only project inspection available"
        );
    }

    #[test]
    fn unconfigured_provider_status_reports_setup_error_and_recovery() {
        let status = backend_status_message(
            None,
            Some("provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY"),
        );

        assert_eq!(
            status,
            "provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY; set the provider environment and relaunch yach tui"
        );
    }

    #[test]
    fn provider_status_reports_agent_tools_available() {
        let config = provider_test_config();
        let status = backend_status_message(Some(&config), None);

        assert_eq!(
            status,
            "backend: anthropic/fixture-model; read/search/list and exact/create edit tools available"
        );
    }

    #[test]
    fn launch_project_context_discovers_marker_root_from_nested_cwd() {
        let root = TempProject::new("launch-marker-root");
        assert!(std::fs::create_dir_all(root.root().join(".git")).is_ok());
        let nested_cwd = root.root().join("crates/yach-backend/src");
        assert!(std::fs::create_dir_all(&nested_cwd).is_ok());

        let context = launch_project_context(&nested_cwd);

        assert!(context.is_some());
        let Some(context) = context else {
            return;
        };
        let root_canonicalized = root.root().canonicalize();
        assert!(root_canonicalized.is_ok());
        let Ok(root_canonicalized) = root_canonicalized else {
            return;
        };
        let nested_cwd_canonicalized = nested_cwd.canonicalize();
        assert!(nested_cwd_canonicalized.is_ok());
        let Ok(nested_cwd_canonicalized) = nested_cwd_canonicalized else {
            return;
        };
        assert_eq!(context.project_root.canonical_path(), root_canonicalized);
        assert_eq!(context.cwd, nested_cwd_canonicalized);
    }

    #[test]
    fn launch_project_context_prefers_parent_git_over_nested_session_yach() {
        let root = TempProject::new("launch-git-over-session-yach");
        assert!(std::fs::create_dir_all(root.root().join(".git")).is_ok());
        let nested_cwd = root.root().join("crates/yach-backend/src");
        assert!(std::fs::create_dir_all(nested_cwd.join(".yach/sessions")).is_ok());

        let context = launch_project_context(&nested_cwd);

        assert!(context.is_some());
        let Some(context) = context else {
            return;
        };
        let root_canonicalized = root.root().canonicalize();
        assert!(root_canonicalized.is_ok());
        let Ok(root_canonicalized) = root_canonicalized else {
            return;
        };
        let nested_cwd_canonicalized = nested_cwd.canonicalize();
        assert!(nested_cwd_canonicalized.is_ok());
        let Ok(nested_cwd_canonicalized) = nested_cwd_canonicalized else {
            return;
        };
        assert_eq!(context.project_root.canonical_path(), root_canonicalized);
        assert_eq!(context.cwd, nested_cwd_canonicalized);
    }

    #[test]
    fn launch_project_context_discovers_yach_append_system_marker_without_git() {
        let root = TempProject::new("launch-yach-append-system-marker");
        root.write(".yach/APPEND_SYSTEM.md", "project system rules");
        let nested_cwd = root.root().join("nested/cwd");
        assert!(std::fs::create_dir_all(&nested_cwd).is_ok());

        let context = launch_project_context(&nested_cwd);

        assert!(context.is_some());
        let Some(context) = context else {
            return;
        };
        let root_canonicalized = root.root().canonicalize();
        assert!(root_canonicalized.is_ok());
        let Ok(root_canonicalized) = root_canonicalized else {
            return;
        };
        let nested_cwd_canonicalized = nested_cwd.canonicalize();
        assert!(nested_cwd_canonicalized.is_ok());
        let Ok(nested_cwd_canonicalized) = nested_cwd_canonicalized else {
            return;
        };
        assert_eq!(context.project_root.canonical_path(), root_canonicalized);
        assert_eq!(context.cwd, nested_cwd_canonicalized);
    }

    #[test]
    fn launch_project_context_falls_back_to_cwd_without_project_marker() {
        let root = TempProject::new("launch-no-marker");
        root.write("AGENTS.md", "parent rules should not be discovered");
        let nested_cwd = root.root().join("nested/cwd");
        assert!(std::fs::create_dir_all(&nested_cwd).is_ok());

        let context = launch_project_context(&nested_cwd);

        assert!(context.is_some());
        let Some(context) = context else {
            return;
        };
        let nested_cwd_canonicalized = nested_cwd.canonicalize();
        assert!(nested_cwd_canonicalized.is_ok());
        let Ok(nested_cwd_canonicalized) = nested_cwd_canonicalized else {
            return;
        };
        assert_eq!(
            context.project_root.canonical_path(),
            nested_cwd_canonicalized
        );
        assert_eq!(context.cwd, nested_cwd_canonicalized);
    }

    #[test]
    fn session_log_has_finished_turn_detects_terminal_event() {
        let turn_id = TurnId(String::from("turn-7"));
        let mut log = SessionLog::default();

        assert!(!log_has_finished_turn(&log, &turn_id));

        log.push(SessionEvent::TurnFinished {
            session_id: SessionId(String::from("default")),
            turn_id: turn_id.clone(),
            outcome: TurnOutcome::Completed,
            reason: None,
        });

        assert!(log_has_finished_turn(&log, &turn_id));
        assert!(!log_has_finished_turn(
            &log,
            &TurnId(String::from("turn-8"))
        ));
    }

    #[test]
    fn provider_messages_include_resumed_transcript() {
        let session_id = SessionId(String::from("default"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-0",
            "entry-0-user",
            Role::User,
            "first prompt",
        );
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-0",
            "entry-0-assistant",
            Role::Assistant,
            "first answer",
        );
        finish_native_provider_test_turn(&mut log, &session_id, "turn-0", TurnOutcome::Completed);
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "second prompt",
        );

        assert_eq!(
            provider_messages_from_log(&log, &TurnId(String::from("turn-1"))),
            vec![
                ProviderMessage::text(Role::User, String::from("first prompt")),
                ProviderMessage::text(Role::Assistant, String::from("first answer")),
                ProviderMessage::text(Role::User, String::from("second prompt")),
            ]
        );
    }

    #[test]
    fn provider_messages_ignore_local_edit_evidence() {
        let session_id = SessionId(String::from("default"));
        let turn_id = TurnId(String::from("turn-1"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "current prompt",
        );

        let summary = EditEvidenceSummary {
            operation_count: 1,
            operations: vec![EditOperationEvidence::CreateTextFile {
                relative_path: String::from("notes.txt"),
                after_sha256: String::from("after"),
                after_bytes: 4,
                bytes_written: Some(4),
            }],
            diff_summary: ToolPayloadSummary {
                summary: String::from("+new\n"),
                byte_count: 5,
                redacted: false,
                truncated: false,
            },
        };
        log.push(SessionEvent::EditTransactionPrepared {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: Some(ToolRequestId(String::from("tool-request-1"))),
            transaction_id: EditTransactionId(String::from("edit-1")),
            summary: summary.clone(),
        });
        log.push(SessionEvent::EditTransactionFinished {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: Some(ToolRequestId(String::from("tool-request-1"))),
            transaction_id: Some(EditTransactionId(String::from("edit-1"))),
            outcome: EditEvidenceOutcome::Completed,
            reason: None,
            summary: Some(summary),
        });

        assert_eq!(
            provider_messages_from_log(&log, &turn_id),
            vec![ProviderMessage::text(
                Role::User,
                String::from("current prompt")
            )]
        );
    }

    #[test]
    fn provider_messages_ignore_agent_edit_evidence() {
        let session_id = SessionId(String::from("default"));
        let turn_id = TurnId(String::from("turn-1"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "current prompt",
        );
        log.push(SessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: ToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            validation: Ok(()),
            permission: ToolPermissionState::Allowed,
            argument_summary: ToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 42,
                redacted: true,
                truncated: false,
            },
            argument_content: None,
        });
        log.push(SessionEvent::EditTransactionPrepared {
            session_id,
            turn_id: turn_id.clone(),
            tool_request_id: Some(ToolRequestId(String::from("tool-request-1"))),
            transaction_id: EditTransactionId(String::from("edit-1")),
            summary: EditEvidenceSummary {
                operation_count: 1,
                operations: vec![EditOperationEvidence::ModifyTextFile {
                    relative_path: String::from("src/lib.rs"),
                    before_sha256: String::from("before"),
                    after_sha256: String::from("after"),
                    before_bytes: 12,
                    after_bytes: 14,
                    hunk_count: 1,
                    bytes_written: None,
                }],
                diff_summary: ToolPayloadSummary {
                    summary: String::from("tool payload redacted"),
                    byte_count: 42,
                    redacted: true,
                    truncated: false,
                },
            },
        });

        let messages = provider_messages_from_log(&log, &turn_id);
        assert_eq!(
            messages,
            vec![ProviderMessage::text(
                Role::User,
                String::from("current prompt")
            )]
        );
        let rendered = format!("{messages:?}");
        assert!(!rendered.contains("edit_text_file"));
        assert!(!rendered.contains("call-edit-1"));
        assert!(!rendered.contains("tool-request-1"));
    }

    #[test]
    fn provider_messages_include_tool_activity_with_persisted_payloads() {
        let session_id = SessionId(String::from("default"));
        let prior_turn = TurnId(String::from("turn-1"));
        let current_turn = TurnId(String::from("turn-2"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "list src",
        );
        log.push(SessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: prior_turn.clone(),
            tool_request_id: ToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("list_project_paths"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: ToolPermissionState::Allowed,
            argument_summary: ToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 15,
                redacted: true,
                truncated: false,
            },
            argument_content: Some(String::from("{\"path\":\"src\"}")),
        });
        log.push(SessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id: prior_turn.clone(),
            tool_request_id: ToolRequestId(String::from("tool-request-1")),
            outcome: ToolOutcome::Completed,
            reason: None,
            result_summary: Some(ToolPayloadSummary {
                summary: String::from("list_project_paths entries=1 truncated=false"),
                byte_count: 64,
                redacted: true,
                truncated: false,
            }),
            result_content: Some(String::from(
                "{\"outcome\":\"list\",\"entries\":[{\"path\":\"src/lib.rs\",\"kind\":\"file\"}],\"truncated\":false}",
            )),
        });
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-assistant",
            Role::Assistant,
            "listed",
        );
        finish_native_provider_test_turn(&mut log, &session_id, "turn-1", TurnOutcome::Completed);
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-2",
            "entry-2-user",
            Role::User,
            "current prompt",
        );

        let messages = provider_messages_from_log(&log, &current_turn);

        // Persisted payloads rebuild as a native pair: the assistant turn
        // that requested the call, then the result bound to it by id. It
        // is two messages rather than one descriptive blob, so the model
        // resumes into the same structure a live round produces.
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[1].role, Role::Assistant);
        assert_eq!(messages[1].tool_calls.len(), 1);
        assert_eq!(messages[1].tool_calls[0].name, "list_project_paths");
        assert_eq!(messages[1].tool_calls[0].arguments_json["path"], "src");

        assert_eq!(messages[2].role, Role::Tool);
        assert_eq!(messages[2].tool_results.len(), 1);
        assert_eq!(
            messages[2].tool_results[0].call_id, messages[1].tool_calls[0].call_id,
            "result must bind to the call it answers"
        );
        let result =
            serde_json::from_str::<serde_json::Value>(&messages[2].tool_results[0].content);
        assert!(result.is_ok(), "result payload should be json");
        let Ok(result) = result else {
            return;
        };
        assert_eq!(result["entries"][0]["path"], "src/lib.rs");
    }

    #[test]
    fn provider_messages_mark_pre_persistence_tool_activity_as_not_retained() {
        let session_id = SessionId(String::from("default"));
        let turn_id = TurnId(String::from("turn-1"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "search",
        );
        log.push(SessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: ToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("search_project"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: ToolPermissionState::Allowed,
            argument_summary: ToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 20,
                redacted: true,
                truncated: false,
            },
            argument_content: None,
        });
        log.push(SessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: ToolRequestId(String::from("tool-request-1")),
            outcome: ToolOutcome::Completed,
            reason: None,
            result_summary: Some(ToolPayloadSummary {
                summary: String::from("search_project matches=2 truncated=false"),
                byte_count: 64,
                redacted: true,
                truncated: false,
            }),
            result_content: None,
        });

        let messages = provider_messages_from_log(&log, &turn_id);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, Role::Tool);
        assert!(messages[1].content.contains("output not retained"));
        assert!(
            messages[1]
                .content
                .contains("search_project matches=2 truncated=false")
        );
    }

    #[test]
    fn provider_messages_exclude_tool_activity_from_unfinished_prior_turns() {
        let session_id = SessionId(String::from("default"));
        let prior_turn = TurnId(String::from("turn-1"));
        let current_turn = TurnId(String::from("turn-2"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "failed prompt",
        );
        log.push(SessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id: prior_turn,
            tool_request_id: ToolRequestId(String::from("tool-request-1")),
            outcome: ToolOutcome::Completed,
            reason: None,
            result_summary: None,
            result_content: Some(String::from("{\"outcome\":\"list\"}")),
        });
        finish_native_provider_test_turn(&mut log, &session_id, "turn-1", TurnOutcome::Failed);
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-2",
            "entry-2-user",
            Role::User,
            "current prompt",
        );

        let messages = provider_messages_from_log(&log, &current_turn);

        assert!(messages.iter().all(|message| message.role != Role::Tool));
    }

    #[test]
    fn provider_messages_rebuild_from_newest_compaction_checkpoint() {
        let session_id = SessionId(String::from("default"));
        let current_turn = TurnId(String::from("turn-3"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "old work that was folded",
        );
        finish_native_provider_test_turn(&mut log, &session_id, "turn-1", TurnOutcome::Completed);
        log.push(SessionEvent::CompactionCheckpoint {
            session_id: session_id.clone(),
            turn_id: TurnId(String::from("turn-2")),
            checkpoint_id: crate::CompactionCheckpointId(String::from("checkpoint-1")),
            summary: String::from("anchored summary of the folded work"),
            first_kept_entry_id: EntryId(String::from("entry-2-user")),
            tokens_before: 90_000,
            tokens_after_estimate: 12_000,
            reason: crate::CompactionReason::Threshold,
            compactor: String::from("summary"),
            details: serde_json::json!({}),
        });
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-2",
            "entry-2-user",
            Role::User,
            "kept turn prompt",
        );
        finish_native_provider_test_turn(&mut log, &session_id, "turn-2", TurnOutcome::Completed);
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-3",
            "entry-3-user",
            Role::User,
            "current prompt",
        );

        let messages = provider_messages_from_log(&log, &current_turn);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::System);
        assert!(messages[0].content.contains("was compacted"));
        assert!(
            messages[0]
                .content
                .contains("anchored summary of the folded work")
        );
        assert_eq!(messages[1].content, "kept turn prompt");
        assert_eq!(messages[2].content, "current prompt");
        assert!(
            messages
                .iter()
                .all(|message| !message.content.contains("old work that was folded"))
        );

        // Golden shape of the rebuilt context (the Codex snapshot pattern):
        // one summary system message, then the verbatim kept tail.
        assert_eq!(
            super::provider_message_shapes(&messages),
            vec![
                String::from("system:Earlier work in this session was compacted. The"),
                String::from("user:kept turn prompt"),
                String::from("user:current prompt"),
            ]
        );
    }

    #[test]
    fn provider_messages_prepend_static_context_before_transcript() {
        let mut log = SessionLog::default();
        let session_id = SessionId(String::from("session-static-context"));
        let turn_id = TurnId(String::from("turn-static-context"));
        log.push(SessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: EntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: Role::User,
            text: String::from("hello"),
            provider: None,
        });
        let context = StaticContextBundle {
            items: vec![StaticContextItem {
                source: StaticContextSource::AgentsMd,
                relative_path: String::from("AGENTS.md"),
                placement: StaticContextPlacement::ProjectInstructions,
                title: String::from("AGENTS.md instructions for ."),
                content: String::from("root rules"),
                byte_count: "root rules".len(),
                priority: StaticContextPriority::ProjectInstructions,
            }],
            total_bytes: "root rules".len(),
        };

        let messages = provider_messages_from_log_with_static_context(&log, &turn_id, &context);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::System);
        assert!(
            messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert!(messages[0].content.contains("Match effort to the request"));
        assert_eq!(messages[1].role, Role::System);
        assert!(
            messages[1]
                .content
                .contains("# AGENTS.md instructions for .")
        );
        assert!(messages[1].content.contains("root rules"));
        assert_eq!(messages[2].role, Role::User);
        assert_eq!(messages[2].content, "hello");
    }

    #[test]
    fn provider_messages_render_extension_background_as_non_system_context() {
        let mut log = SessionLog::default();
        let session_id = SessionId(String::from("session-extension-background-context"));
        let turn_id = TurnId(String::from("turn-extension-background-context"));
        log.push(SessionEvent::EntryAppended {
            session_id,
            entry_id: EntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: Role::User,
            text: String::from("hello"),
            provider: None,
        });
        let context = StaticContextBundle {
            items: vec![
                StaticContextItem {
                    source: StaticContextSource::AgentsMd,
                    relative_path: String::from("AGENTS.md"),
                    placement: StaticContextPlacement::ProjectInstructions,
                    title: String::from("AGENTS.md instructions for ."),
                    content: String::from("root rules"),
                    byte_count: "root rules".len(),
                    priority: StaticContextPriority::ProjectInstructions,
                },
                StaticContextItem {
                    source: StaticContextSource::ExtensionFile {
                        extension_id: String::from("example.context-pack"),
                        item_id: String::from("rust-style-guide"),
                    },
                    relative_path: String::from("context/rust.md"),
                    placement: StaticContextPlacement::BackgroundContext,
                    title: String::from("Extension background context: Rust style guide"),
                    content: String::from("extension guidance"),
                    byte_count: "extension guidance".len(),
                    priority: StaticContextPriority::ExtensionBackground,
                },
            ],
            total_bytes: "root rulesextension guidance".len(),
        };

        let messages = provider_messages_from_log_with_static_context(&log, &turn_id, &context);

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, Role::System);
        assert!(
            messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(messages[1].role, Role::System);
        assert!(messages[1].content.contains("root rules"));
        assert!(!messages[1].content.contains("extension guidance"));
        assert_eq!(messages[2].role, Role::User);
        assert!(
            messages[2]
                .content
                .contains("# Extension background context: Rust style guide")
        );
        assert!(messages[2].content.contains("extension guidance"));
        assert_eq!(messages[3].role, Role::User);
        assert_eq!(messages[3].content, "hello");
    }

    #[test]
    fn provider_request_includes_project_static_context_and_records_evidence() {
        let root = TempProject::new("provider-static-context");
        root.write("AGENTS.md", "root rules");
        root.write(".yach/APPEND_SYSTEM.md", "system rules");
        let project_root = ResourceRoot::project(root.root()).ok();
        let executor_root = ResourceRoot::project(root.root());
        assert!(executor_root.is_ok());
        let Some(executor_root) = executor_root.ok() else {
            return;
        };
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = TurnId(String::from("turn-static-context-provider"));
        log.push(SessionEvent::EntryAppended {
            session_id: SessionId(String::from("default")),
            entry_id: EntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: Role::User,
            text: String::from("hello"),
            provider: None,
        });
        let model = ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: turn_id.clone(),
                model: model.clone(),
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn_id.clone(),
                delta: String::from("ok"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn_id.clone(),
                finish_reason: Some(crate::ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
            },
        ])]);
        let registry = ToolRegistry::with_project_read_only_tools();
        let permission_policy =
            ToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let executor = ProjectReadOnlyToolExecutor::new(executor_root);
        let routable_tool_names = ["project_path_info"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            ProviderToolRoundContext {
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_root,
                static_context_cwd: Some(root.root().to_path_buf()),
                extension_static_context_files: Vec::new(),
                tool_event_store: None,
                registry: &registry,
                permission_policy: &permission_policy,
                executor: &executor,
                routable_tool_names: &routable_tool_names,
                require_project_root_for_tools: true,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("ok"),
                provider_response_id: None,
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 1);
        let Some(request) = requester.requests.first() else {
            return;
        };
        assert_eq!(request.messages[0].role, Role::System);
        assert!(
            request.messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(request.messages[1].role, Role::System);
        assert!(request.messages[1].content.contains("root rules"));
        assert!(request.messages[1].content.contains("system rules"));
        assert!(pending_events.iter().any(|event| {
            matches!(event, SessionEvent::StaticContextIncluded { summary, .. }
                if summary.items.len() == 2)
        }));
    }

    #[test]
    fn provider_messages_do_not_include_extension_static_context_before_manifest_scan() {
        let root = TempProject::new("provider-extension-static-context-before-scan");
        let package = TempProject::new("provider-extension-static-context-package-before");
        package.write(
            "yach.extension.json",
            extension_static_context_manifest_json(),
        );
        package.write("context/rust.md", "extension context should wait for scan");
        let project_root = ResourceRoot::project(root.root()).ok();
        let executor_root = ResourceRoot::project(root.root());
        assert!(executor_root.is_ok());
        let Some(executor_root) = executor_root.ok() else {
            return;
        };
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = TurnId(String::from("turn-extension-static-context-before"));
        log.push(SessionEvent::EntryAppended {
            session_id: SessionId(String::from("default")),
            entry_id: EntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: Role::User,
            text: String::from("hello"),
            provider: None,
        });
        let model = ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::TextDelta {
                turn_id: turn_id.clone(),
                delta: String::from("ok"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn_id.clone(),
                finish_reason: Some(crate::ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
            },
        ])]);
        let registry = ToolRegistry::with_project_read_only_tools();
        let permission_policy =
            ToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let executor = ProjectReadOnlyToolExecutor::new(executor_root);
        let routable_tool_names = ["project_path_info"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            ProviderToolRoundContext {
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_root,
                static_context_cwd: Some(root.root().to_path_buf()),
                extension_static_context_files: Vec::new(),
                tool_event_store: None,
                registry: &registry,
                permission_policy: &permission_policy,
                executor: &executor,
                routable_tool_names: &routable_tool_names,
                require_project_root_for_tools: true,
            },
        ));

        assert!(matches!(result, Ok(ProviderRoundResult { .. })));
        let Some(request) = requester.requests.first() else {
            return;
        };
        assert_eq!(request.messages[0].role, Role::System);
        assert!(
            request.messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(request.messages[1].role, Role::User);
        assert!(request.messages.iter().all(|message| {
            !message
                .content
                .contains("extension context should wait for scan")
        }));
        assert!(
            !pending_events
                .iter()
                .any(|event| { matches!(event, SessionEvent::StaticContextIncluded { .. }) })
        );
    }

    #[test]
    fn provider_messages_include_extension_static_context_after_manifest_scan() {
        let root = TempProject::new("provider-extension-static-context-after-scan");
        let package = TempProject::new("provider-extension-static-context-package-after");
        package.write(
            "yach.extension.json",
            extension_static_context_manifest_json(),
        );
        package.write("context/rust.md", "extension context after scan");
        let index =
            ExtensionManifestIndex::from_package_roots([extension_manifest_scan_package_root(
                &package,
            )]);
        assert!(index.is_ok());
        let Ok(index) = index else {
            return;
        };
        let project_root = ResourceRoot::project(root.root()).ok();
        let executor_root = ResourceRoot::project(root.root());
        assert!(executor_root.is_ok());
        let Some(executor_root) = executor_root.ok() else {
            return;
        };
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = TurnId(String::from("turn-extension-static-context-after"));
        log.push(SessionEvent::EntryAppended {
            session_id: SessionId(String::from("default")),
            entry_id: EntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: Role::User,
            text: String::from("hello"),
            provider: None,
        });
        let model = ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::TextDelta {
                turn_id: turn_id.clone(),
                delta: String::from("ok"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn_id.clone(),
                finish_reason: Some(crate::ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
            },
        ])]);
        let registry = ToolRegistry::with_project_read_only_tools();
        let permission_policy =
            ToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let executor = ProjectReadOnlyToolExecutor::new(executor_root);
        let routable_tool_names = ["project_path_info"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            ProviderToolRoundContext {
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_root,
                static_context_cwd: Some(root.root().to_path_buf()),
                extension_static_context_files: index.static_context_files(),
                tool_event_store: None,
                registry: &registry,
                permission_policy: &permission_policy,
                executor: &executor,
                routable_tool_names: &routable_tool_names,
                require_project_root_for_tools: true,
            },
        ));

        assert!(matches!(result, Ok(ProviderRoundResult { .. })));
        let Some(request) = requester.requests.first() else {
            return;
        };
        assert_eq!(request.messages[0].role, Role::System);
        assert!(
            request.messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(request.messages[1].role, Role::User);
        assert!(
            request.messages[1]
                .content
                .contains("# Extension background context: Rust style guide")
        );
        assert!(
            request.messages[1]
                .content
                .contains("extension context after scan")
        );
        assert_eq!(request.messages[2].role, Role::User);
        assert_eq!(request.messages[2].content, "hello");
        assert!(request.messages.iter().all(|message| {
            message.role != Role::System
                || !message.content.contains("extension context after scan")
        }));
        assert!(pending_events.iter().any(|event| {
            matches!(event, SessionEvent::StaticContextIncluded { summary, omissions, .. }
            if omissions.is_empty()
                && summary.items == vec![crate::StaticContextItemSummary {
                    source: StaticContextSource::ExtensionFile {
                        extension_id: String::from("example.context-pack"),
                        item_id: String::from("rust-style-guide"),
                    },
                    relative_path: String::from("context/rust.md"),
                    placement: StaticContextPlacement::BackgroundContext,
                    title: String::from("Extension background context: Rust style guide"),
                    byte_count: "extension context after scan".len(),
                }])
        }));
    }

    #[test]
    fn static_context_persist_failure_prevents_provider_request() {
        let root = TempProject::new("static-context-persist-failure");
        root.write("AGENTS.md", "root rules");
        let blocked_parent = root.root().join("session-parent");
        assert!(std::fs::write(&blocked_parent, "not a directory").is_ok());
        let store = JsonlSessionStore::new(blocked_parent.join("session.jsonl"));
        let project_root = ResourceRoot::project(root.root()).ok();
        assert!(project_root.is_some());
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = TurnId(String::from("turn-static-context-persist-failure"));
        log.push(SessionEvent::EntryAppended {
            session_id: SessionId(String::from("default")),
            entry_id: EntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: Role::User,
            text: String::from("hello"),
            provider: None,
        });
        let model = ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::TextDelta {
                turn_id: turn_id.clone(),
                delta: String::from("should not be requested"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn_id.clone(),
                finish_reason: Some(crate::ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
            },
        ])]);
        let registry = ToolRegistry::with_project_read_only_tools();
        let permission_policy =
            ToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let Some(project_root_for_executor) = project_root.clone() else {
            return;
        };
        let executor = ProjectReadOnlyToolExecutor::new(project_root_for_executor);
        let routable_tool_names = ["project_path_info"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            ProviderToolRoundContext {
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_root,
                static_context_cwd: Some(root.root().to_path_buf()),
                extension_static_context_files: Vec::new(),
                tool_event_store: Some(&store),
                registry: &registry,
                permission_policy: &permission_policy,
                executor: &executor,
                routable_tool_names: &routable_tool_names,
                require_project_root_for_tools: true,
            },
        ));

        assert_eq!(
            result,
            Err(ProviderRoundError::ToolContinuation(String::from(
                "static_context_persist_failed"
            )))
        );
        assert!(requester.requests.is_empty());
        assert!(pending_events.iter().any(|event| {
            matches!(event, SessionEvent::StaticContextIncluded { summary, .. }
                if summary.items.len() == 1)
        }));
    }

    #[test]
    fn provider_request_from_nested_cwd_includes_root_and_nested_agents_md() {
        let root = TempProject::new("provider-nested-static-context");
        assert!(std::fs::create_dir_all(root.root().join(".git")).is_ok());
        root.write("AGENTS.md", "root rules");
        root.write("crates/yach-backend/AGENTS.md", "backend rules");
        let nested_cwd = root.root().join("crates/yach-backend/src");
        assert!(std::fs::create_dir_all(&nested_cwd).is_ok());
        let context = launch_project_context(&nested_cwd);
        assert!(context.is_some());
        let Some(context) = context else {
            return;
        };
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "hello",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::TextDelta {
                turn_id: turn.clone(),
                delta: String::from("ok"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
            },
        ])]);

        let result = futures::executor::block_on(run_native_provider_one_readonly_tool_round(
            &mut requester,
            model,
            &mut log,
            &mut pending_events,
            &turn,
            Some(context),
            None,
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("ok"),
                provider_response_id: None,
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 1);
        let guidance_message = &requester.requests[0].messages[0];
        assert_eq!(guidance_message.role, Role::System);
        assert!(
            guidance_message
                .content
                .contains("coding agent running in the yach harness")
        );
        let system_message = &requester.requests[0].messages[1];
        assert_eq!(system_message.role, Role::System);
        assert!(system_message.content.contains("root rules"));
        assert!(system_message.content.contains("backend rules"));
        assert!(pending_events.iter().any(|event| {
            matches!(event, SessionEvent::StaticContextIncluded { summary, .. }
                if summary.items.iter().any(|item| item.relative_path == "AGENTS.md")
                    && summary
                        .items
                        .iter()
                        .any(|item| item.relative_path == "crates/yach-backend/AGENTS.md"))
        }));
    }

    #[test]
    fn provider_one_round_without_tools_preserves_one_shot_response() {
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: turn.clone(),
                model: model.clone(),
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn.clone(),
                delta: String::from("plain answer"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: Some(String::from("response-1")),
            },
        ])]);

        let result = futures::executor::block_on(run_native_provider_one_readonly_tool_round(
            &mut requester,
            model,
            &mut log,
            &mut pending_events,
            &turn,
            None,
            None,
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("plain answer"),
                provider_response_id: Some(String::from("response-1")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 1);
        let Ok(Some(advertising)) =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
        else {
            return;
        };
        assert_eq!(advertising.tools.len(), 1);
        assert_eq!(advertising.tools[0].name, "project_path_info");
        assert!(
            !advertising
                .tools
                .iter()
                .any(|tool| matches!(tool.name.as_str(), "edit" | "write"))
        );
        assert!(pending_events.is_empty());
    }

    #[test]
    fn provider_initial_request_advertises_registered_extension_tool_for_future_turn() {
        let mut registry = ToolRegistry::with_project_read_only_tools();
        let Ok(()) = registry.register_extension_tool(ToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            ToolInputSchema::string_object(
                std::iter::empty::<&str>(),
                std::iter::empty::<&str>(),
                1024,
            ),
            ProviderToolVisibility::Visible,
        )) else {
            return;
        };
        let policy =
            ToolPermissionPolicy::allow_project_metadata_tools(["project_path_info", "toy_tool"]);
        let executor = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata("example.toy-tools", "{\"ok\":true}"),
        )]);
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect toy metadata",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: turn.clone(),
                model: model.clone(),
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn.clone(),
                delta: String::from("done"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: Some(String::from("response-1")),
            },
        ])]);
        let routable_tool_names = ["toy_tool"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            ProviderToolRoundContext {
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn,
                project_root: None,
                static_context_cwd: None,
                extension_static_context_files: Vec::new(),
                tool_event_store: None,
                registry: &registry,
                permission_policy: &policy,
                executor: &executor,
                routable_tool_names: &routable_tool_names,
                require_project_root_for_tools: false,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("done"),
                provider_response_id: Some(String::from("response-1")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 1);
        let Ok(Some(advertising)) =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
        else {
            return;
        };
        assert!(advertising.tools.iter().any(|tool| tool.name == "toy_tool"));
        assert!(
            !advertising
                .tools
                .iter()
                .any(|tool| tool.name == "project_path_info")
        );
    }

    #[test]
    fn provider_extension_tool_continuation_does_not_require_project_root() {
        let mut registry = ToolRegistry::with_project_read_only_tools();
        let Ok(()) = registry.register_extension_tool(ToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            ToolInputSchema::string_object(
                std::iter::empty::<&str>(),
                std::iter::empty::<&str>(),
                1024,
            ),
            ProviderToolVisibility::Visible,
        )) else {
            return;
        };
        let policy = ToolPermissionPolicy::allow_project_metadata_tools(["toy_tool"]);
        let executor = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata("example.toy-tools", "{\"ok\":true}"),
        )]);
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect toy metadata",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-1"),
                        name: String::from("toy_tool"),
                        arguments_json: serde_json::json!({}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: Some(String::from("response-1")),
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn.clone(),
                    delta: String::from("done"),
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: Some(String::from("response-2")),
                },
            ]),
        ]);
        let routable_tool_names = ["toy_tool"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            ProviderToolRoundContext {
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn,
                project_root: None,
                static_context_cwd: None,
                extension_static_context_files: Vec::new(),
                tool_event_store: None,
                registry: &registry,
                permission_policy: &policy,
                executor: &executor,
                routable_tool_names: &routable_tool_names,
                require_project_root_for_tools: false,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("done"),
                provider_response_id: Some(String::from("response-2")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 2);
        assert!(
            !requester.requests[1]
                .extensions
                .iter()
                .any(|extension| extension.key == PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY)
        );
    }

    #[test]
    fn provider_one_round_rejects_incomplete_tool_call_stream() {
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: turn.clone(),
                model: model.clone(),
            },
            ProviderStreamEvent::ToolCallStarted {
                turn_id: turn.clone(),
                call_id: String::from("provider-call-1"),
                name: String::from("project_path_info"),
            },
            ProviderStreamEvent::ToolCallDelta {
                turn_id: turn.clone(),
                call_id: String::from("provider-call-1"),
                arguments_delta: String::from(r#"{"path":"Cargo.toml"}"#),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: Some(String::from("response-1")),
            },
        ])]);

        let result = futures::executor::block_on(run_native_provider_one_readonly_tool_round(
            &mut requester,
            model,
            &mut log,
            &mut pending_events,
            &turn,
            None,
            None,
        ));

        assert_eq!(
            result,
            Err(ProviderRoundError::ToolContinuation(String::from(
                "provider_tool_call_incomplete"
            )))
        );
        assert_eq!(requester.requests.len(), 1);
        assert!(pending_events.is_empty());
    }

    #[test]
    fn initial_state_handshake_advertises_local_edit() {
        let root_guard = temp_native_provider_root("native-initial-state-handshake");
        let (tx, mut rx) = mpsc::unbounded_channel();

        send_native_initial_state(&tx, "initial-state", root_guard.path(), None, None);

        let ready = rx.try_recv().ok();
        assert!(matches!(
            ready,
            Some(BackendEvent::Server(ServerEvent::Ready { handshake }))
                if handshake.capabilities == vec![
                    Capability::PromptStreaming,
                    Capability::PromptCancellation,
                    Capability::LocalEdit,
                    Capability::ExtensionLifecycle,
                    Capability::FirstRenderEvents,
                    Capability::ToolOutputStreaming,
                ]
        ));
    }

    #[test]
    fn provider_initial_request_advertises_content_tools_for_agent_edit_context() {
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: TurnId(String::from("turn-1")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
            },
            ProviderStreamEvent::TextDelta {
                turn_id: TurnId(String::from("turn-1")),
                delta: String::from("done"),
            },
            ProviderStreamEvent::Completed {
                turn_id: TurnId(String::from("turn-1")),
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: Some(String::from("response-1")),
            },
        ])]);
        let root_guard = temp_native_provider_root("agent-content-advertising");
        let resource_root = ResourceRoot::project(root_guard.path());
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-1",
            "entry-1-user",
            Role::User,
            "inspect project",
        );
        let turn_id = TurnId(String::from("turn-1"));
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("done"),
                provider_response_id: Some(String::from("response-1")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 1);
        let Ok(Some(advertising)) =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
        else {
            return;
        };
        let names = advertising
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "project_path_info",
                "read_text_file",
                "search_project",
                "list_project_paths",
                "edit_text_file",
                "create_text_file",
                "bash",
            ]
        );
        for name in ["read_text_file", "search_project", "list_project_paths"] {
            let schema = advertising
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .map(|tool| &tool.parameters);
            assert!(schema.is_some(), "missing schema for {name}");
            assert!(schema.is_some_and(serde_json::Value::is_object));
        }
    }

    #[test]
    fn provider_agent_rounds_echo_assistant_narrative_into_continuation() {
        let root = TempProject::new("native-provider-round-narrative");
        root.write("note.txt", "note body here\n");
        let resource_root = ResourceRoot::project(root.root());
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let turn_id = TurnId(String::from("turn-1"));
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("I'll read the note first."),
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-read-1"),
                        name: String::from("read_text_file"),
                        arguments_json: serde_json::json!({ "path": "note.txt" }),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(provider_text_response("all done")),
        ]);
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-1",
            "entry-1-user",
            Role::User,
            "read the note",
        );
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("all done"),
                provider_response_id: None,
                mid_turn_text: String::from("I'll read the note first.\n\n"),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 2);
        // The continuation request carries the model's own round output —
        // text plus requested calls — but the calls are carried as
        // structure, not rendered into the text. The prose form is what
        // models learned to imitate; nothing here should reproduce it.
        let assistant = requester.requests[1]
            .messages
            .iter()
            .find(|message| message.role == Role::Assistant);
        assert!(assistant.is_some_and(|message| {
            message.content.contains("I'll read the note first.")
                && message.tool_calls.len() == 1
                && message.tool_calls[0].name == "read_text_file"
        }));
        assert!(
            requester.requests[1]
                .messages
                .iter()
                .all(|message| !message.content.contains("[requested tool calls:"))
        );
        // The result is bound to the call by id rather than described.
        let call_id = assistant
            .map(|message| message.tool_calls[0].call_id.clone())
            .unwrap_or_default();
        assert!(requester.requests[1].messages.iter().any(|message| {
            message.tool_results.iter().any(|result| {
                result.call_id == call_id && result.content.contains("note body here")
            })
        }));
    }

    #[test]
    fn provider_agent_rounds_echo_survives_hydrated_session_log() {
        // Repro harness for the 2026-07-28 eval-gate session-continuation
        // finding (the model repeated one edit five times on a resumed
        // `yach run --session` session): a turn running over a log
        // hydrated from a prior invocation must still see the hydrated
        // context exactly once and its own in-turn rounds — a missing
        // round echo or tool result here is the repetition failure mode.
        let root = TempProject::new("native-provider-hydrated-round-narrative");
        root.write("note.txt", "note body here\n");
        let resource_root = ResourceRoot::project(root.root());
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let turn_id = TurnId(String::from("turn-1"));
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("I'll read the note back."),
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-read-1"),
                        name: String::from("read_text_file"),
                        arguments_json: serde_json::json!({ "path": "note.txt" }),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(provider_text_response("all done")),
        ]);
        // The state a second `yach run --session` invocation starts from:
        // a completed prior exchange already in the log, then the new
        // turn's user entry.
        let mut log = crate::completed_text_exchange(
            SessionId(String::from("default")),
            EntryId(String::from("entry-0-user")),
            EntryId(String::from("entry-0-assistant")),
            TurnId(String::from("turn-0")),
            String::from("create the note"),
            String::from("created note.txt with the note body"),
        );
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-1",
            "entry-1-user",
            Role::User,
            "read the note back",
        );
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("all done"),
                provider_response_id: None,
                mid_turn_text: String::from("I'll read the note back.\n\n"),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 2);
        // The hydrated prior-turn context appears exactly once in the
        // initial request (duplication would invite re-execution).
        let initial_prior_count = requester.requests[0]
            .messages
            .iter()
            .filter(|message| {
                message
                    .content
                    .contains("created note.txt with the note body")
            })
            .count();
        assert_eq!(initial_prior_count, 1);
        // The continuation request carries the round's calls and their
        // results as structure — the model must see what it already did,
        // bound by call id rather than described in prose.
        let assistant = requester.requests[1].messages.iter().find(|message| {
            message.role == Role::Assistant
                && message
                    .tool_calls
                    .iter()
                    .any(|call| call.name == "read_text_file")
        });
        assert!(assistant.is_some());
        let tool_result = requester.requests[1].messages.iter().find(|message| {
            message
                .tool_results
                .iter()
                .any(|result| result.content.contains("note body here"))
        });
        assert!(tool_result.is_some());
        // And the hydrated prior turn is still present exactly once.
        let continuation_prior_count = requester.requests[1]
            .messages
            .iter()
            .filter(|message| {
                message
                    .content
                    .contains("created note.txt with the note body")
            })
            .count();
        assert_eq!(continuation_prior_count, 1);
    }

    #[test]
    fn provider_agent_round_advertises_and_executes_active_extension_tool() {
        let mut registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
        assert_eq!(
            registry.register_extension_tool(ToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                ToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512,),
                ProviderToolVisibility::Visible,
            )),
            Ok(())
        );
        let extension_activation_snapshot = ExtensionActivationSnapshot {
            registry,
            executor: ExtensionToolExecutorRouter::from_handlers([(
                "toy_tool",
                ExtensionToolHandler::static_metadata(
                    "example.toy-tools",
                    "{\"kind\":\"toy\",\"label\":\"fixture\"}",
                ),
            )]),
            diagnostics: vec![ExtensionActivationDiagnostic {
                extension_id: Some(String::from("example.toy-tools")),
                version: Some(String::from("0.1.0")),
                scope: ExtensionInstallScope::User,
                source_ref: Some(String::from("test-package-root")),
                install_source: None,
                package_root: PathBuf::from("/tmp/yach-extension"),
                manifest_path: Some(PathBuf::from("/tmp/yach-extension/yach.extension.json")),
                activation_state: ExtensionActivationState::Active,
                generation: 1,
                last_error_kind: None,
                last_error_summary: None,
                registered_tools: vec![String::from("toy_tool")],
                provider_visible_tools: vec![String::from("toy_tool")],
            }],
            host_start_count: 1,
        };
        let turn_id = TurnId(String::from("turn-active-extension"));
        let model = ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-1"),
                        name: String::from("toy_tool"),
                        arguments_json: serde_json::json!({"label":"fixture"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: Some(String::from("response-1")),
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("done"),
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: Some(String::from("response-2")),
                },
            ]),
        ]);
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-active-extension",
            "entry-active-extension-user",
            Role::User,
            "inspect toy metadata",
        );
        let root_guard = temp_native_provider_root("active-extension-agent-round");
        let resource_root = ResourceRoot::project(root_guard.path());
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot,
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("done"),
                provider_response_id: Some(String::from("response-2")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 2);
        let Ok(Some(advertising)) =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
        else {
            return;
        };
        assert!(advertising.tools.iter().any(|tool| tool.name == "toy_tool"));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolExecutionFinished {
                outcome: ToolOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn provider_agent_continuation_preserves_tool_advertising() {
        let root_guard = temp_native_provider_root("agent-continuation-tool-advertising");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("README.md"), "tool advertising\n").is_ok());
        let resource_root = ResourceRoot::project(root_path);
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-1",
            "entry-1-user",
            Role::User,
            "read README",
        );
        let turn_id = TurnId(String::from("turn-1"));
        let model = ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-read-1"),
                        name: String::from("read_text_file"),
                        arguments_json: serde_json::json!({"path": "README.md"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: Some(String::from("response-1")),
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("read complete"),
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: Some(String::from("response-2")),
                },
            ]),
        ]);
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("read complete"),
                provider_response_id: Some(String::from("response-2")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 2);
        let initial_advertising =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions);
        assert!(
            initial_advertising.is_ok(),
            "advertising: {initial_advertising:?}"
        );
        let Ok(initial_advertising) = initial_advertising else {
            return;
        };
        assert!(initial_advertising.is_some());
        let Some(initial_advertising) = initial_advertising else {
            return;
        };
        let continuation_advertising =
            parse_provider_tool_advertising_extensions(&requester.requests[1].extensions);
        assert!(continuation_advertising.is_ok());
        let Ok(continuation_advertising) = continuation_advertising else {
            return;
        };
        assert!(continuation_advertising.is_some());
        let Some(continuation_advertising) = continuation_advertising else {
            return;
        };
        let initial_names = initial_advertising
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        let continuation_names = continuation_advertising
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(continuation_names, initial_names);
    }

    #[test]
    fn provider_agent_loop_reads_then_edits_in_later_round() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root_guard = temp_native_provider_root("agent-loop-read-then-edit");
            let root_path = root_guard.path();
            assert!(std::fs::write(root_path.join("note.txt"), "native provider edit ok").is_ok());
            let resource_root = ResourceRoot::project(root_path);
            assert!(resource_root.is_ok());
            let Ok(resource_root) = resource_root else {
                return;
            };
            let mut log = SessionLog::default();
            let mut pending_events = Vec::new();
            append_native_provider_test_entry(
                &mut log,
                &SessionId(String::from("default")),
                "turn-1",
                "entry-1-user",
                Role::User,
                "read and update note",
            );
            let turn_id = TurnId(String::from("turn-1"));
            let model = ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            };
            let mut requester = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: turn_id.clone(),
                        model: model.clone(),
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: turn_id.clone(),
                        delta: String::from("Reading note.txt."),
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: turn_id.clone(),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({"path": "note.txt"}),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn_id.clone(),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: Some(String::from("response-1")),
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: turn_id.clone(),
                        model: model.clone(),
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: turn_id.clone(),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-edit-1"),
                            name: String::from("edit_text_file"),
                            arguments_json: serde_json::json!({
                                "path": "note.txt",
                                "find": "ok",
                                "replace": "passed"
                            }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn_id.clone(),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: Some(String::from("response-2")),
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: turn_id.clone(),
                        model: model.clone(),
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: turn_id.clone(),
                        delta: String::from("Updated note.txt."),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn_id.clone(),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: Some(String::from("response-3")),
                    },
                ]),
            ]);
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let (decision_tx, review_rx) = mpsc::unbounded_channel();
            let session_id = SessionId(String::from("default"));
            let run = run_native_provider_one_agent_tool_round(
                &mut requester,
                ProviderAgentToolRound {
                    session_id: &session_id,
                    model,
                    log: &mut log,
                    pending_events: &mut pending_events,
                    turn_id: &turn_id,
                    project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                    extension_static_context_files: Vec::new(),
                    extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                    tool_event_store: None,
                    review_tx: backend_tx,
                    review_decisions: review_rx,
                    context_window: 200_000,
                    max_output_tokens: 1_000,
                },
            );
            let review = async {
                let review = recv_tool_review(&mut backend_rx).await;
                let Some(ReceivedToolReview {
                    request_id,
                    tool_name,
                    payload,
                }) = review
                else {
                    return;
                };
                assert_eq!(tool_name, "edit_text_file");
                let ToolReviewPayload::LocalEdit { preview } = payload else {
                    unreachable!("edit review payload expected");
                };
                assert!(
                    decision_tx
                        .send(AgentEditReviewDecision {
                            request_id,
                            preview_id: preview.preview_id,
                            permission_decision_id: preview.permission_decision_id,
                            decision: LocalEditDecision::Apply,
                        })
                        .is_ok()
                );
            };
            let (result, ()) = futures::future::join(run, review).await;
            assert_eq!(requester.requests.len(), 3);
            let edited = std::fs::read_to_string(root_path.join("note.txt"));
            assert!(edited.is_ok());
            let Ok(edited) = edited else {
                return;
            };
            assert_eq!(edited, "native provider edit passed");
            assert!(result.is_ok());
            let Ok(result) = result else {
                return;
            };
            assert_eq!(result.text, "Updated note.txt.");
        });
    }

    #[test]
    fn provider_agent_loop_emits_tool_progress_before_final_answer() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root_guard = temp_native_provider_root("agent-loop-tool-progress");
            let root_path = root_guard.path();
            assert!(std::fs::write(root_path.join("note.txt"), "progress visible").is_ok());
            let resource_root = ResourceRoot::project(root_path);
            assert!(resource_root.is_ok());
            let Ok(resource_root) = resource_root else {
                return;
            };
            let mut log = SessionLog::default();
            let mut pending_events = Vec::new();
            append_native_provider_test_entry(
                &mut log,
                &SessionId(String::from("default")),
                "turn-1",
                "entry-1-user",
                Role::User,
                "read note",
            );
            let turn_id = TurnId(String::from("turn-1"));
            let model = ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            };
            let mut requester = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: turn_id.clone(),
                        model: model.clone(),
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: turn_id.clone(),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({"path": "note.txt"}),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn_id.clone(),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: Some(String::from("response-1")),
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: turn_id.clone(),
                        model: model.clone(),
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: turn_id.clone(),
                        delta: String::from("Read note.txt."),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn_id.clone(),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: Some(String::from("response-2")),
                    },
                ]),
            ]);
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let (_decision_tx, review_rx) = mpsc::unbounded_channel();
            let result = run_native_provider_one_agent_tool_round(
                &mut requester,
                ProviderAgentToolRound {
                    session_id: &SessionId(String::from("default")),
                    model,
                    log: &mut log,
                    pending_events: &mut pending_events,
                    turn_id: &turn_id,
                    project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                    extension_static_context_files: Vec::new(),
                    extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                    tool_event_store: None,
                    review_tx: backend_tx,
                    review_decisions: review_rx,
                    context_window: 200_000,
                    max_output_tokens: 1_000,
                },
            )
            .await;

            assert!(result.is_ok());
            let Ok(result) = result else {
                return;
            };
            assert_eq!(result.text, "Read note.txt.");

            let mut progress_events = Vec::new();
            while let Ok(event) = backend_rx.try_recv() {
                progress_events.push(event);
            }
            assert!(progress_events.iter().any(|event| matches!(
                event,
                BackendEvent::Server(ServerEvent::ToolCallStarted {
                    tool_call_id,
                    tool_name,
                    preview: Some(preview),
                }) if tool_call_id.as_deref() == Some("tool-request-1-1")
                    && tool_name == "read_text_file"
                    && preview == "note.txt"
            )));
            assert!(
                progress_events.iter().any(|event| matches!(
                event,
                BackendEvent::Server(ServerEvent::ToolCallFinished(result))
                    if result.tool_call_id.as_deref() == Some("tool-request-1-1")
                    && result.tool_name == "read_text_file"
                    && !result.is_error
                    && result.output == "completed: 1 line, 16 bytes"
                )),
                "{progress_events:#?}"
            );
        });
    }

    #[test]
    fn provider_tool_call_preview_targets_primary_argument() {
        assert_eq!(
            provider_tool_call_preview(
                "read_text_file",
                &serde_json::json!({"path": "docs/project/state.md"})
            ),
            Some(String::from("docs/project/state.md"))
        );
        assert_eq!(
            provider_tool_call_preview("search_project", &serde_json::json!({"query": "needle"})),
            Some(String::from("needle"))
        );
        assert_eq!(
            provider_tool_call_preview(
                "bash",
                &serde_json::json!({"command": "cargo test\n--workspace"})
            ),
            Some(String::from("cargo test..."))
        );
        let long_command = "x".repeat(MAX_TOOL_CALL_PREVIEW_CHARS + 1);
        assert_eq!(
            provider_tool_call_preview("bash", &serde_json::json!({"command": long_command})),
            Some(format!("{}...", "x".repeat(MAX_TOOL_CALL_PREVIEW_CHARS)))
        );
        assert_eq!(
            provider_tool_call_preview("edit_text_file", &serde_json::json!({"path": "a.rs"})),
            Some(String::from("a.rs"))
        );
        assert_eq!(
            provider_tool_call_preview("read_text_file", &serde_json::json!({})),
            None
        );
    }

    #[test]
    fn tool_result_display_shapes_read_text_file_with_line_and_byte_counts() {
        let content = "alpha line\nneedle evidence line\n";
        assert_eq!(
            tool_result_display(
                "read_text_file",
                ToolOutcome::Completed,
                Some(content),
                content.len(),
                false,
                None,
            ),
            "completed: 2 lines, 32 bytes"
        );
        // Non-completed statuses fall back to the redacted summary line.
        assert_eq!(
            tool_result_display(
                "read_text_file",
                ToolOutcome::Denied,
                Some("denied content"),
                14,
                false,
                Some("policy"),
            ),
            "denied; bytes=14; content=redacted; truncated=false; reason=policy"
        );
    }

    #[test]
    fn tool_result_display_shapes_project_path_info() {
        let content = "testdata/sample-session.jsonl: file, 31744 bytes";
        assert_eq!(
            tool_result_display(
                "project_path_info",
                ToolOutcome::Completed,
                Some(content),
                content.len(),
                false,
                None,
            ),
            "completed: testdata/sample-session.jsonl: file, 31744 bytes"
        );
        // Directories report no byte size; shape without one.
        let directory_content = ".: directory";
        assert_eq!(
            tool_result_display(
                "project_path_info",
                ToolOutcome::Completed,
                Some(directory_content),
                directory_content.len(),
                false,
                None,
            ),
            "completed: .: directory"
        );
    }

    #[test]
    fn tool_result_display_shapes_bash_with_exit_and_output_tail() {
        let content = "line-1\nline-2\n";
        assert_eq!(
            tool_result_display(
                "bash",
                ToolOutcome::Completed,
                Some(content),
                content.len(),
                false,
                None,
            ),
            "completed; 14 bytes\nline-1\nline-2"
        );
    }

    #[test]
    fn tool_result_display_bounds_bash_output_tail_and_reports_nonzero_exit() {
        let output = (0..20)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!("{output}\n[exit code 101]");
        let display = tool_result_display(
            "bash",
            ToolOutcome::Completed,
            Some(&content),
            content.len(),
            false,
            None,
        );
        assert!(display.starts_with(&format!("completed; {} bytes", content.len())));
        assert!(display.contains("... 13 earlier lines"));
        assert!(!display.contains("line-12\n"));
        assert!(display.contains("line-13"));
        assert!(display.ends_with("[exit code 101]"));
    }

    #[test]
    fn provider_agent_loop_records_read_and_edit_evidence_before_final_answer() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root_guard = temp_native_provider_root("agent-loop-read-edit-evidence");
            let root_path = root_guard.path();
            assert!(std::fs::write(root_path.join("note.txt"), "native provider edit ok").is_ok());
            let resource_root = ResourceRoot::project(root_path);
            assert!(resource_root.is_ok());
            let Ok(resource_root) = resource_root else {
                return;
            };
            let mut log = SessionLog::default();
            let mut pending_events = Vec::new();
            append_native_provider_test_entry(
                &mut log,
                &SessionId(String::from("default")),
                "turn-1",
                "entry-1-user",
                Role::User,
                "read and update note",
            );
            let turn_id = TurnId(String::from("turn-1"));
            let model = ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            };
            let mut requester = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: turn_id.clone(),
                        model: model.clone(),
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: turn_id.clone(),
                        delta: String::from("Reading note.txt."),
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: turn_id.clone(),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({"path": "note.txt"}),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn_id.clone(),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: Some(String::from("response-1")),
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: turn_id.clone(),
                        model: model.clone(),
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: turn_id.clone(),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-edit-1"),
                            name: String::from("edit_text_file"),
                            arguments_json: serde_json::json!({
                                "path": "note.txt",
                                "find": "ok",
                                "replace": "passed"
                            }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn_id.clone(),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: Some(String::from("response-2")),
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: turn_id.clone(),
                        model: model.clone(),
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: turn_id.clone(),
                        delta: String::from("Updated note.txt."),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn_id.clone(),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: Some(String::from("response-3")),
                    },
                ]),
            ]);
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let (decision_tx, review_rx) = mpsc::unbounded_channel();
            let session_id = SessionId(String::from("default"));
            let run = run_native_provider_one_agent_tool_round(
                &mut requester,
                ProviderAgentToolRound {
                    session_id: &session_id,
                    model,
                    log: &mut log,
                    pending_events: &mut pending_events,
                    turn_id: &turn_id,
                    project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                    extension_static_context_files: Vec::new(),
                    extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                    tool_event_store: None,
                    review_tx: backend_tx,
                    review_decisions: review_rx,
                    context_window: 200_000,
                    max_output_tokens: 1_000,
                },
            );
            let review = async {
                let review = recv_tool_review(&mut backend_rx).await;
                let Some(ReceivedToolReview {
                    request_id,
                    tool_name,
                    payload,
                }) = review
                else {
                    return;
                };
                assert_eq!(tool_name, "edit_text_file");
                let ToolReviewPayload::LocalEdit { preview } = payload else {
                    unreachable!("edit review payload expected");
                };
                assert!(
                    decision_tx
                        .send(AgentEditReviewDecision {
                            request_id,
                            preview_id: preview.preview_id,
                            permission_decision_id: preview.permission_decision_id,
                            decision: LocalEditDecision::Apply,
                        })
                        .is_ok()
                );
            };
            let (result, ()) = futures::future::join(run, review).await;

            assert!(result.is_ok());
            let Ok(result) = result else {
                return;
            };
            assert_eq!(result.text, "Updated note.txt.");
            assert_eq!(requester.requests.len(), 3);
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Completed,
                    ..
                }
            )));
            let traces = edit_trace_records(&log);
            assert!(traces.iter().any(|trace| {
                trace.phase == EditTracePhase::ProviderContinuation
                    && trace.outcome == EditTraceOutcome::Completed
                    && trace.tool_name.as_deref() == Some("edit_text_file")
            }));
            for request in &requester.requests {
                let advertising = parse_provider_tool_advertising_extensions(&request.extensions);
                assert!(advertising.is_ok());
                let Ok(advertising) = advertising else {
                    return;
                };
                assert!(advertising.is_some());
                let Some(advertising) = advertising else {
                    return;
                };
                assert!(
                    advertising
                        .tools
                        .iter()
                        .any(|tool| tool.name == "read_text_file")
                );
                assert!(
                    advertising
                        .tools
                        .iter()
                        .any(|tool| tool.name == "edit_text_file")
                );
            }
        });
    }

    #[test]
    fn provider_one_round_executes_read_search_list_and_continues_with_persisted_evidence() {
        let root_guard = temp_native_provider_root("agent-content-round");
        let root_path = root_guard.path();
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        assert!(
            std::fs::write(
                root_path.join("src/lib.rs"),
                "alpha line\nneedle evidence line\n"
            )
            .is_ok()
        );
        assert!(std::fs::write(root_path.join("src/main.rs"), "main file\n").is_ok());
        let resource_root = ResourceRoot::project(root_path);
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-1",
            "entry-1-user",
            Role::User,
            "inspect content",
        );
        let turn_id = TurnId(String::from("turn-1"));
        let model = ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-read-1"),
                        name: String::from("read_text_file"),
                        arguments_json: serde_json::json!({"path": "src/lib.rs"}),
                    },
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-search-1"),
                        name: String::from("search_project"),
                        arguments_json: serde_json::json!({"query": "needle"}),
                    },
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-list-1"),
                        name: String::from("list_project_paths"),
                        arguments_json: serde_json::json!({"path": "src"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: Some(String::from("response-1")),
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("content inspected"),
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: Some(String::from("response-2")),
                },
            ]),
        ]);
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("content inspected"),
                provider_response_id: Some(String::from("response-2")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        let mut progress_outputs = Vec::new();
        while let Ok(event) = backend_rx.try_recv() {
            if let BackendEvent::Server(ServerEvent::ToolCallFinished(result)) = event {
                progress_outputs.push((result.tool_name, result.output));
            }
        }
        assert!(progress_outputs.iter().any(|(tool_name, output)| {
            tool_name == "read_text_file" && output == "completed: 2 lines, 32 bytes"
        }));
        assert!(progress_outputs.iter().any(|(tool_name, output)| {
            tool_name == "search_project"
                && output.contains("completed: 1 lines")
                && output.contains("src/lib.rs:2: needle evidence line")
        }));
        assert!(progress_outputs.iter().any(|(tool_name, output)| {
            tool_name == "list_project_paths"
                && output.contains("completed: 2 lines")
                && output.contains("src/lib.rs  32 bytes")
                && output.contains("src/main.rs  10 bytes")
        }));
        assert_eq!(requester.requests.len(), 2);
        // One tool message carrying a block per result, not one message
        // per result.
        let tool_messages = requester.requests[1]
            .messages
            .iter()
            .filter(|message| message.role == Role::Tool)
            .collect::<Vec<_>>();
        assert_eq!(tool_messages.len(), 1);
        assert_eq!(tool_messages[0].tool_results.len(), 3);
        // Call ids live on the block, which is what binds a result to its
        // call — they are no longer repeated inside the payload.
        let call_ids = tool_messages[0]
            .tool_results
            .iter()
            .map(|result| result.call_id.as_str())
            .collect::<Vec<_>>();
        assert!(call_ids.contains(&"call-read-1"));
        assert!(call_ids.contains(&"call-search-1"));
        assert!(call_ids.contains(&"call-list-1"));
        // The payload is the tool's own result, passed through directly.
        let tool_contents = tool_messages[0]
            .tool_results
            .iter()
            .map(|result| result.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(tool_contents.len(), 3);
        assert!(tool_contents.contains(&"alpha line\nneedle evidence line\n"));
        assert!(
            tool_contents
                .iter()
                .any(|content| content.contains("needle evidence line"))
        );
        assert!(
            tool_contents.iter().any(|content| {
                content.contains("src/lib.rs") && content.contains("src/main.rs")
            })
        );

        let finished_summaries = pending_events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Completed,
                    result_summary: Some(summary),
                    ..
                } => Some(summary),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(finished_summaries.len(), 3);
        assert!(finished_summaries.iter().all(|summary| summary.redacted));
        let raw_events = serde_json::to_string(&pending_events);
        assert!(raw_events.is_ok());
        let Some(raw_events) = raw_events.ok() else {
            return;
        };
        assert!(raw_events.contains("read_text_file result redacted"));
        assert!(raw_events.contains("search_project matches=1 truncated=false"));
        assert!(raw_events.contains("list_project_paths entries=2 truncated=false"));
        assert!(raw_events.contains("alpha line"));
        assert!(raw_events.contains("needle evidence line"));
        assert!(raw_events.contains("src/lib.rs"));
        assert!(raw_events.contains("src/main.rs"));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolRequestRecorded {
                tool_name,
                argument_content: Some(content),
                ..
            } if tool_name == "search_project" && content.contains("needle")
        )));
    }

    #[test]
    fn provider_one_round_allows_read_text_results_above_metadata_fixture_limit() {
        let root_guard = temp_native_provider_root("agent-content-large-read");
        let root_path = root_guard.path();
        let large_readme = "native provider content\n".repeat(32);
        assert!(large_readme.len() > ToolContinuationPolicy::fixture_default().max_result_bytes);
        assert!(std::fs::write(root_path.join("README.md"), &large_readme).is_ok());
        let resource_root = ResourceRoot::project(root_path);
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-large-read",
            "entry-large-read-user",
            Role::User,
            "read README",
        );
        let turn_id = TurnId(String::from("turn-large-read"));
        let model = ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-read-large"),
                        name: String::from("read_text_file"),
                        arguments_json: serde_json::json!({"path": "README.md"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: Some(String::from("response-1")),
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("read complete"),
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: Some(String::from("response-2")),
                },
            ]),
        ]);
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("read complete"),
                provider_response_id: Some(String::from("response-2")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 2);
        assert!(requester.requests[1].messages.iter().any(|message| {
            message.role == Role::Tool
                && message
                    .tool_results
                    .iter()
                    .any(|result| result.content.contains("native provider content"))
        }));
    }

    #[test]
    fn runner_prepares_and_applies_local_edit() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("local-edit-apply");
            root.write("notes.txt", "alpha\n");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(super::run_native_loop(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
            ));

            assert!(
                client_tx
                    .send(ClientEvent::LocalEditPrepareRequested {
                        request_id: String::from("edit-request-1"),
                        operation: LocalEditOperationInput::ModifyTextFile {
                            path: String::from("notes.txt"),
                            expected_sha256: sha256_hex_for_test("alpha\n"),
                            find: String::from("alpha"),
                            replace: String::from("beta"),
                        },
                    })
                    .is_ok()
            );

            let preview = recv_local_edit_preview(&mut backend_rx).await;
            assert!(preview.is_some());
            let Some(preview) = preview else {
                return;
            };
            assert_eq!(preview.path, "notes.txt");
            assert_eq!(preview.operation, "modify_text_file");
            assert_eq!(
                preview.review_state,
                LocalEditReviewState::NeedsUserApproval
            );
            assert!(
                client_tx
                    .send(ClientEvent::LocalEditDecisionSubmitted {
                        preview_id: preview.preview_id.clone(),
                        permission_decision_id: preview.permission_decision_id.clone(),
                        decision: LocalEditDecision::Apply,
                    })
                    .is_ok()
            );

            let finished = recv_local_edit_finished(&mut backend_rx).await;
            assert!(finished.is_some());
            let Some(finished) = finished else {
                return;
            };
            assert_eq!(finished.0, Some(preview.preview_id));
            assert_eq!(finished.1, LocalEditFinishedOutcome::Applied);
            let edited_text = std::fs::read_to_string(root.root().join("notes.txt"));
            assert!(edited_text.is_ok());
            let Ok(edited_text) = edited_text else {
                return;
            };
            assert_eq!(edited_text, "beta\n");
            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            let permission_summaries = log.events.iter().filter_map(|event| match event {
                SessionEvent::PermissionDecisionRecorded { summary, .. } => Some(summary),
                _ => None,
            });
            assert!(permission_summaries.clone().any(|summary| {
                summary.outcome == PermissionDecisionOutcome::NeedsUserReview
                    && !summary.user_override
            }));
            assert!(permission_summaries.clone().any(|summary| {
                summary.outcome == PermissionDecisionOutcome::Allowed
                    && summary.reason == "user_approved"
                    && summary.user_override
            }));
            assert!(
                log.events
                    .iter()
                    .any(|event| matches!(event, SessionEvent::EditTransactionPrepared { .. }))
            );
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::EditTransactionFinished {
                    outcome: EditEvidenceOutcome::ApplyStarted,
                    reason: Some(reason),
                    ..
                } if reason == "apply_started"
            )));
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::EditTransactionFinished {
                    outcome: EditEvidenceOutcome::Completed,
                    summary: Some(summary),
                    ..
                } if summary.operations.iter().all(|operation| matches!(
                    operation,
                    EditOperationEvidence::ModifyTextFile {
                        bytes_written: Some(_),
                        ..
                    } | EditOperationEvidence::CreateTextFile {
                        bytes_written: Some(_),
                        ..
                    }
                ))
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn local_edit_error_messages_are_categorical() {
        let message =
            local_edit_error_message(&EditAccessError::Preview(EditError::HashMismatch {
                path: String::from("/private/project/secrets.txt"),
                expected_sha256: String::from("expected-secret-hash"),
                actual_sha256: String::from("actual-secret-hash"),
            }));

        assert!(message.contains("hash_mismatch"));
        assert!(!message.contains("/private/project"));
        assert!(!message.contains("expected-secret-hash"));
        assert!(!message.contains("actual-secret-hash"));
    }

    #[test]
    fn runner_rejects_stale_local_edit_decision() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("local-edit-stale");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(super::run_native_loop(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
            ));

            assert!(
                client_tx
                    .send(ClientEvent::LocalEditDecisionSubmitted {
                        preview_id: String::from("missing"),
                        permission_decision_id: String::from("permission-decision-stale"),
                        decision: LocalEditDecision::Apply,
                    })
                    .is_ok()
            );

            let finished = recv_local_edit_finished(&mut backend_rx).await;
            assert!(finished.is_some());
            let Some(finished) = finished else {
                return;
            };
            assert_eq!(finished.0, Some(String::from("missing")));
            assert_eq!(finished.1, LocalEditFinishedOutcome::Failed);
            assert!(finished.2.contains("stale local edit preview"));
            let log = JsonlSessionStore::new(session_path)
                .load()
                .unwrap_or_default();
            assert!(log.events.is_empty());

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn runner_unconfigured_provider_prompt_fails_with_setup_error() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("unconfigured-provider-prompt");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let setup_error =
                "provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY";
            let handle = tokio::spawn(super::run_native_loop(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: None,
                    provider_setup_error: Some(setup_error.to_owned()),
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
            ));

            let first = backend_rx.recv().await;
            assert!(matches!(
                first,
                Some(BackendEvent::Server(ServerEvent::Ready { .. }))
            ));
            let second = backend_rx.recv().await;
            assert!(matches!(
                &second,
                Some(BackendEvent::Server(ServerEvent::StateUpdated(state)))
                    if state.model_id.as_deref() == Some("provider-unconfigured")
            ));
            let third = backend_rx.recv().await;
            assert!(matches!(
                &third,
                Some(BackendEvent::Server(ServerEvent::StatusUpdated { message }))
                    if message.contains(setup_error) && message.contains("relaunch")
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("hello without provider config"),
                    })
                    .is_ok()
            );

            let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                let mut deltas = Vec::new();
                loop {
                    match backend_rx.recv().await {
                        Some(BackendEvent::Server(ServerEvent::PromptDelta { delta, .. })) => {
                            deltas.push(delta);
                        }
                        Some(BackendEvent::Server(ServerEvent::PromptFinished {
                            outcome,
                            message,
                            ..
                        })) => {
                            return (deltas, Some((outcome, message)));
                        }
                        Some(_) => {}
                        None => return (deltas, None),
                    }
                }
            })
            .await;
            assert!(result.is_ok(), "timed out waiting for prompt finish");
            let (deltas, finished) = result.unwrap_or_default();
            assert!(
                deltas.is_empty(),
                "unconfigured provider must not stream fixture text: {deltas:?}"
            );
            assert!(
                finished.is_some(),
                "backend channel closed before prompt finish"
            );
            let Some((outcome, message)) = finished else {
                return;
            };
            assert_eq!(outcome, PromptOutcome::Failed);
            let message = message.unwrap_or_default();
            assert!(message.contains(setup_error));
            assert!(message.contains("relaunch"));

            let log = JsonlSessionStore::new(session_path)
                .load()
                .unwrap_or_default();
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::EntryAppended {
                    role: Role::User,
                    text,
                    ..
                } if text == "hello without provider config"
            )));
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::TurnFinished {
                    outcome: TurnOutcome::Failed,
                    reason: Some(reason),
                    ..
                } if reason.starts_with("provider_unconfigured ") && reason.contains(setup_error)
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn runner_does_not_apply_when_local_edit_evidence_preflight_fails() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("local-edit-evidence-preflight");
            root.write("notes.txt", "alpha\n");
            let session_dir = root.root().join("logs");
            let session_path = session_dir.join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(super::run_native_loop(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
            ));

            assert!(
                client_tx
                    .send(ClientEvent::LocalEditPrepareRequested {
                        request_id: String::from("edit-request-1"),
                        operation: LocalEditOperationInput::ModifyTextFile {
                            path: String::from("notes.txt"),
                            expected_sha256: sha256_hex_for_test("alpha\n"),
                            find: String::from("alpha"),
                            replace: String::from("beta"),
                        },
                    })
                    .is_ok()
            );
            let preview = recv_local_edit_preview(&mut backend_rx).await;
            assert!(preview.is_some());
            let Some(preview) = preview else {
                return;
            };

            assert!(std::fs::remove_file(&session_path).is_ok());
            assert!(std::fs::remove_dir(&session_dir).is_ok());
            assert!(std::fs::write(&session_dir, "not a directory").is_ok());

            assert!(
                client_tx
                    .send(ClientEvent::LocalEditDecisionSubmitted {
                        preview_id: preview.preview_id,
                        permission_decision_id: preview.permission_decision_id,
                        decision: LocalEditDecision::Apply,
                    })
                    .is_ok()
            );

            let finished = recv_local_edit_finished(&mut backend_rx).await;
            assert!(finished.is_some());
            let Some(finished) = finished else {
                return;
            };
            assert_eq!(finished.1, LocalEditFinishedOutcome::Failed);
            assert!(finished.2.contains("failed to persist local edit evidence"));
            let unedited_text = std::fs::read_to_string(root.root().join("notes.txt"));
            assert!(unedited_text.is_ok());
            let Ok(unedited_text) = unedited_text else {
                return;
            };
            assert_eq!(unedited_text, "alpha\n");

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_edit_tool_pauses_for_user_review_and_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-agent-edit-review");
            root.write("notes.txt", "alpha\n");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-edit-1"),
                            name: String::from("edit_text_file"),
                            arguments_json: serde_json::json!({
                                "path": "notes.txt",
                                "find": "alpha",
                                "replace": "beta"
                            }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: TurnId(String::from("turn-1")),
                        delta: String::from("edit applied"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("change alpha to beta"),
                    })
                    .is_ok()
            );

            let review = recv_tool_review(&mut backend_rx).await;
            assert!(review.is_some());
            let Some(review) = review else {
                return;
            };
            assert_eq!(review.tool_name, "edit_text_file");
            let ToolReviewPayload::LocalEdit { preview } = review.payload else {
                unreachable!("edit review payload expected");
            };
            assert!(
                client_tx
                    .send(ClientEvent::ToolReviewDecisionSubmitted {
                        request_id: review.request_id,
                        preview_id: preview.preview_id,
                        permission_decision_id: preview.permission_decision_id,
                        decision: LocalEditDecision::Apply,
                    })
                    .is_ok()
            );

            let finished = recv_prompt_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Completed));
            let edited_text = std::fs::read_to_string(root.root().join("notes.txt"));
            assert!(edited_text.is_ok());
            let Ok(edited_text) = edited_text else {
                return;
            };
            assert_eq!(edited_text, "beta\n");

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolRequestRecorded {
                    provider_call_id: Some(id),
                    tool_name,
                    ..
                } if id == "call-edit-1" && tool_name == "edit_text_file"
            )));
            let traces = edit_trace_records(&log);
            let preview_trace = traces.iter().find(|trace| {
                trace.phase == EditTracePhase::Preview
                    && trace.provider_call_id.as_deref() == Some("call-edit-1")
            });
            assert!(preview_trace.is_some());
            let Some(preview_trace) = preview_trace else {
                return;
            };
            let trace_id = preview_trace.trace_id.clone();
            assert!(traces.iter().any(|trace| {
                trace.trace_id == trace_id
                    && trace.phase == EditTracePhase::ReviewWait
                    && trace.outcome == EditTraceOutcome::Completed
                    && trace.provider_call_id.as_deref() == Some("call-edit-1")
                    && trace.tool_request_id
                        == Some(ToolRequestId(String::from("tool-request-1-1")))
                    && trace.preview_id.is_some()
                    && trace.permission_decision_id.is_some()
            }));
            assert!(traces.iter().any(|trace| {
                trace.trace_id == trace_id
                    && trace.phase == EditTracePhase::ProviderContinuation
                    && trace.outcome == EditTraceOutcome::Completed
                    && trace.provider_call_id.as_deref() == Some("call-edit-1")
                    && trace.tool_request_id
                        == Some(ToolRequestId(String::from("tool-request-1-1")))
            }));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_edit_continuation_records_each_edit_trace() {
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let traces = vec![
            super::ProviderContinuationEditTrace {
                trace_id: EditTraceId(String::from("edit-trace-1")),
                tool_name: String::from("edit_text_file"),
                tool_request_id: ToolRequestId(String::from("tool-request-1")),
                provider_call_id: Some(String::from("call-edit-1")),
                preview_id: Some(EditPreviewId(String::from("edit-preview-1"))),
                permission_decision_id: Some(PermissionDecisionId(String::from(
                    "permission-decision-1",
                ))),
            },
            super::ProviderContinuationEditTrace {
                trace_id: EditTraceId(String::from("edit-trace-2")),
                tool_name: String::from("create_text_file"),
                tool_request_id: ToolRequestId(String::from("tool-request-2")),
                provider_call_id: Some(String::from("call-edit-2")),
                preview_id: Some(EditPreviewId(String::from("edit-preview-2"))),
                permission_decision_id: Some(PermissionDecisionId(String::from(
                    "permission-decision-2",
                ))),
            },
        ];

        record_provider_continuation_trace_records(
            &mut log,
            &mut pending_events,
            None,
            super::ProviderContinuationTraceInput {
                session_id: &SessionId(String::from("default")),
                turn_id: &TurnId(String::from("turn-1")),
                edit_traces: &traces,
                started: std::time::Instant::now(),
                outcome: EditTraceOutcome::Completed,
                reason_label: None,
            },
        );

        let continuation_traces = edit_trace_records(&log)
            .into_iter()
            .filter(|trace| trace.phase == EditTracePhase::ProviderContinuation)
            .collect::<Vec<_>>();
        assert_eq!(continuation_traces.len(), 2);
        assert!(continuation_traces.iter().any(|trace| {
            trace.trace_id == EditTraceId(String::from("edit-trace-1"))
                && trace.tool_name.as_deref() == Some("edit_text_file")
                && trace.tool_request_id == Some(ToolRequestId(String::from("tool-request-1")))
                && trace.provider_call_id.as_deref() == Some("call-edit-1")
                && trace.outcome == EditTraceOutcome::Completed
        }));
        assert!(continuation_traces.iter().any(|trace| {
            trace.trace_id == EditTraceId(String::from("edit-trace-2"))
                && trace.tool_name.as_deref() == Some("create_text_file")
                && trace.tool_request_id == Some(ToolRequestId(String::from("tool-request-2")))
                && trace.provider_call_id.as_deref() == Some("call-edit-2")
                && trace.outcome == EditTraceOutcome::Completed
        }));
        assert_eq!(pending_events.len(), 2);
    }

    #[test]
    fn provider_agent_edit_tool_denial_does_not_continue_provider_round() {
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: TurnId(String::from("turn-1")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
            },
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: TurnId(String::from("turn-1")),
                tool_call: ProviderToolCall {
                    call_id: String::from("call-edit-1"),
                    name: String::from("edit_text_file"),
                    arguments_json: serde_json::json!({
                        "path": "./.yach/APPEND_SYSTEM.md",
                        "find": "old",
                        "replace": "new"
                    }),
                },
            },
            ProviderStreamEvent::Completed {
                turn_id: TurnId(String::from("turn-1")),
                finish_reason: Some(ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ])]);
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        let root_guard = temp_native_provider_root("agent-edit-denied");
        let resource_root = ResourceRoot::project(root_guard.path());
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let (_review_tx, review_rx) = mpsc::unbounded_channel();
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let turn_id = TurnId(String::from("turn-1"));

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(LaunchProjectContext::from_project_root(resource_root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert!(matches!(
            result,
            Err(ProviderRoundError::ToolExecutionDenied { .. })
        ));
        assert_eq!(requester.requests.len(), 1);
    }

    #[test]
    fn provider_agent_duplicate_create_fails_tool_and_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-agent-duplicate-create");
            root.write("notes.txt", "existing content\n");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-create-1"),
                            name: String::from("create_text_file"),
                            arguments_json: serde_json::json!({
                                "path": "notes.txt",
                                "content": "hello"
                            }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: TurnId(String::from("turn-1")),
                        delta: String::from("the file already exists"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("create notes.txt"),
                    })
                    .is_ok()
            );

            let (deltas, finished) = recv_prompt_deltas_until_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Completed));
            assert!(deltas.join("").contains("the file already exists"));
            assert_eq!(
                std::fs::read_to_string(root.root().join("notes.txt")).ok(),
                Some(String::from("existing content\n"))
            );

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Failed,
                    reason: Some(reason),
                    ..
                } if reason == "target_exists"
            )));
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::TurnFinished {
                    outcome: TurnOutcome::Completed,
                    ..
                }
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_sensitive_read_fails_tool_and_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-agent-sensitive-read");
            root.write(".env", "API_KEY=super-secret\n");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({"path": ".env"}),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: TurnId(String::from("turn-1")),
                        delta: String::from("that file is protected"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("read .env"),
                    })
                    .is_ok()
            );

            let (deltas, finished) = recv_prompt_deltas_until_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Completed));
            assert!(deltas.join("").contains("that file is protected"));

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Failed,
                    reason: Some(reason),
                    ..
                } if reason == "sensitive_path_denied"
            )));
            let raw = serde_json::to_string(&log.events);
            assert!(raw.is_ok());
            assert!(!raw.unwrap_or_default().contains("super-secret"));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    fn bash_tool_round_responses(
        command: &str,
        final_text: &str,
    ) -> [Result<Vec<ProviderStreamEvent>, ProviderError>; 2] {
        [
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: TurnId(String::from("turn-1")),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: TurnId(String::from("turn-1")),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-bash-1"),
                        name: String::from("bash"),
                        arguments_json: serde_json::json!({ "command": command }),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: TurnId(String::from("turn-1")),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: TurnId(String::from("turn-1")),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: TurnId(String::from("turn-1")),
                    delta: String::from(final_text),
                },
                ProviderStreamEvent::Completed {
                    turn_id: TurnId(String::from("turn-1")),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
        ]
    }

    #[test]
    fn provider_agent_bash_review_approval_runs_command() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-bash-approve");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses(bash_tool_round_responses(
                "printf run-evidence && exit 4",
                "the command exited with 4",
            ));

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("run the probe"),
                    })
                    .is_ok()
            );

            let review = recv_tool_review(&mut backend_rx).await;
            assert!(review.is_some());
            let Some(review) = review else {
                return;
            };
            assert_eq!(review.tool_name, "bash");
            let ToolReviewPayload::Command { command } = review.payload else {
                unreachable!("command review payload expected");
            };
            assert_eq!(command.command, "printf run-evidence && exit 4");
            assert!(
                client_tx
                    .send(ClientEvent::ToolReviewDecisionSubmitted {
                        request_id: review.request_id,
                        preview_id: command.review_id,
                        permission_decision_id: command.permission_decision_id,
                        decision: LocalEditDecision::Apply,
                    })
                    .is_ok()
            );

            let (deltas, finished) = recv_prompt_deltas_until_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Completed));
            assert!(deltas.join("").contains("the command exited with 4"));

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Completed,
                    result_content: Some(content),
                    ..
                } if content.contains("run-evidence") && content.contains("[exit code 4]")
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_bash_review_approval_with_empty_output_reports_no_output_notice() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-bash-empty-output");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses(bash_tool_round_responses(
                "true",
                "ran it, no output",
            ));

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("run the silent probe"),
                    })
                    .is_ok()
            );

            let review = recv_tool_review(&mut backend_rx).await;
            assert!(review.is_some());
            let Some(review) = review else {
                return;
            };
            let ToolReviewPayload::Command { command } = review.payload else {
                unreachable!("command review payload expected");
            };
            assert_eq!(command.command, "true");
            assert!(
                client_tx
                    .send(ClientEvent::ToolReviewDecisionSubmitted {
                        request_id: review.request_id,
                        preview_id: command.review_id,
                        permission_decision_id: command.permission_decision_id,
                        decision: LocalEditDecision::Apply,
                    })
                    .is_ok()
            );

            let (deltas, finished) = recv_prompt_deltas_until_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Completed));
            assert!(deltas.join("").contains("ran it, no output"));

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            // Empty output with a clean exit renders as exactly the "no
            // output" notice -- not the raw empty string a whitespace-trim
            // synthesis check elsewhere could otherwise confuse with
            // "nothing was captured at all".
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Completed,
                    result_content: Some(content),
                    ..
                } if content == "[no output; exit code 0]"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_bash_review_rejection_fails_tool_and_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-bash-reject");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses(bash_tool_round_responses(
                "rm -rf /tmp/precious",
                "understood, not running it",
            ));

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("clean up"),
                    })
                    .is_ok()
            );

            let review = recv_tool_review(&mut backend_rx).await;
            assert!(review.is_some());
            let Some(review) = review else {
                return;
            };
            let ToolReviewPayload::Command { command } = review.payload else {
                unreachable!("command review payload expected");
            };
            assert!(
                client_tx
                    .send(ClientEvent::ToolReviewDecisionSubmitted {
                        request_id: review.request_id,
                        preview_id: command.review_id,
                        permission_decision_id: command.permission_decision_id,
                        decision: LocalEditDecision::Reject,
                    })
                    .is_ok()
            );

            let (_, finished) = recv_prompt_deltas_until_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Completed));

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Failed,
                    reason: Some(reason),
                    ..
                } if reason == "user_rejected"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_bash_allowlist_auto_runs_without_review() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-bash-allowlist");
            root.write(".yach/config.json", r#"{"shell":{"allow":["printf"]}}"#);
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses(bash_tool_round_responses(
                "printf allowlist-evidence",
                "printed",
            ));

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("print the probe"),
                    })
                    .is_ok()
            );

            // No review request may appear: the prompt must complete with
            // only prompt deltas and tool progress events.
            let (deltas, streamed, finished) =
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let mut deltas = Vec::new();
                    let mut streamed = Vec::new();
                    loop {
                        match backend_rx.recv().await {
                            Some(BackendEvent::Server(ServerEvent::ToolReviewRequested {
                                ..
                            })) => {
                                unreachable!("allowlisted command must not request review");
                            }
                            Some(BackendEvent::Server(ServerEvent::PromptDelta {
                                delta, ..
                            })) => deltas.push(delta),
                            Some(BackendEvent::Server(ServerEvent::ToolCallOutput {
                                tool_call_id,
                                chunk,
                            })) => streamed.push((tool_call_id, chunk)),
                            Some(BackendEvent::Server(ServerEvent::PromptFinished {
                                outcome,
                                ..
                            })) => return (deltas, streamed, Some(outcome)),
                            Some(_) => {}
                            None => return (deltas, streamed, None),
                        }
                    }
                })
                .await
                .unwrap_or_default();
            assert_eq!(finished, Some(PromptOutcome::Completed));
            assert!(deltas.join("").contains("printed"));
            // Live output streamed as ToolCallOutput while the command ran,
            // keyed to the tool call id the started event announced.
            assert!(
                streamed
                    .iter()
                    .map(|(_, chunk)| chunk.as_str())
                    .collect::<String>()
                    .contains("allowlist-evidence")
            );
            assert!(
                streamed
                    .iter()
                    .all(|(tool_call_id, _)| tool_call_id.starts_with("tool-request-"))
            );

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Completed,
                    result_content: Some(content),
                    ..
                } if content == "allowlist-evidence"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    fn seed_completed_turn(session_path: &Path, turn: &str, user_text: &str) {
        let store = JsonlSessionStore::new(session_path.to_path_buf());
        let session_id = SessionId(String::from("default"));
        let mut events = vec![
            SessionEvent::EntryAppended {
                session_id: session_id.clone(),
                entry_id: EntryId(format!("{turn}-user")),
                parent_entry_id: None,
                turn_id: TurnId(String::from(turn)),
                role: Role::User,
                text: String::from(user_text),
                provider: None,
            },
            SessionEvent::EntryAppended {
                session_id: session_id.clone(),
                entry_id: EntryId(format!("{turn}-assistant")),
                parent_entry_id: None,
                turn_id: TurnId(String::from(turn)),
                role: Role::Assistant,
                text: String::from("acknowledged"),
                provider: None,
            },
            SessionEvent::TurnFinished {
                session_id,
                turn_id: TurnId(String::from(turn)),
                outcome: TurnOutcome::Completed,
                reason: None,
            },
        ];
        assert!(super::append_pending_native_session_events(&store, &mut events).is_ok());
    }

    fn provider_text_response(text: &str) -> Vec<ProviderStreamEvent> {
        vec![
            ProviderStreamEvent::Started {
                turn_id: TurnId(String::from("turn-1")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
            },
            ProviderStreamEvent::TextDelta {
                turn_id: TurnId(String::from("turn-1")),
                delta: String::from(text),
            },
            ProviderStreamEvent::Completed {
                turn_id: TurnId(String::from("turn-1")),
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
            },
        ]
    }

    async fn collect_prompt_outcome(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    ) -> (
        Vec<String>,
        Vec<String>,
        Option<(PromptOutcome, Option<String>)>,
    ) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut deltas = Vec::new();
            let mut statuses = Vec::new();
            loop {
                match backend_rx.recv().await {
                    Some(BackendEvent::Server(ServerEvent::PromptDelta { delta, .. })) => {
                        deltas.push(delta);
                    }
                    Some(BackendEvent::Server(ServerEvent::StatusUpdated { message })) => {
                        statuses.push(message);
                    }
                    Some(BackendEvent::Server(ServerEvent::PromptFinished {
                        outcome,
                        message,
                        ..
                    })) => return (deltas, statuses, Some((outcome, message))),
                    Some(_) => {}
                    None => return (deltas, statuses, None),
                }
            }
        })
        .await
        .unwrap_or_default()
    }

    #[test]
    fn provider_agent_threshold_compaction_checkpoints_and_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-compaction-threshold");
            root.write(
                ".yach/config.json",
                r#"{"compaction":{"auto_threshold_percent":10,"keep_recent_tokens":200}}"#,
            );
            let session_path = root.root().join("session.jsonl");
            // ~120KB of prior-turn text (~30K estimated tokens) exceeds the
            // 10% threshold of the usable window.
            seed_completed_turn(&session_path, "turn-0", &"legacy work ".repeat(10_000));
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(provider_text_response(
                    "anchored summary of the legacy work",
                )),
                Ok(provider_text_response("post-compaction reply")),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("fresh prompt after big history"),
                    })
                    .is_ok()
            );

            let (deltas, statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            assert_eq!(
                finished.map(|(outcome, _)| outcome),
                Some(PromptOutcome::Completed)
            );
            assert!(deltas.join("").contains("post-compaction reply"));
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("compacted context"))
            );

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::CompactionCheckpoint {
                    reason: crate::CompactionReason::Threshold,
                    summary,
                    compactor,
                    ..
                } if summary == "anchored summary of the legacy work" && compactor == "summary"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn manual_compaction_checkpoints_and_refreshes_session_views() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-manual-compaction");
            root.write(
                ".yach/config.json",
                r#"{"compaction":{"keep_recent_tokens":100}}"#,
            );
            let session_path = root.root().join("session.jsonl");
            seed_completed_turn(&session_path, "turn-0", &"prior context ".repeat(600));
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([Ok(provider_text_response(
                "manual anchored summary",
            ))]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::CompactionRequested {
                        session_id: String::from("default"),
                        instructions: Some(String::from("keep the prior context goals")),
                    })
                    .is_ok()
            );

            let (statuses, marker_seen, stats_percent) =
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let mut statuses = Vec::new();
                    let mut marker_seen = false;
                    loop {
                        match backend_rx.recv().await {
                            Some(BackendEvent::Server(ServerEvent::StatusUpdated { message })) => {
                                statuses.push(message);
                            }
                            Some(BackendEvent::Server(ServerEvent::SessionMessagesUpdated {
                                messages,
                            })) => {
                                marker_seen = messages.iter().any(|message| {
                                    message.role == "system"
                                        && message.text.contains("— compacted:")
                                        && message.text.contains("manual anchored summary")
                                });
                            }
                            Some(BackendEvent::Server(ServerEvent::SessionStatsUpdated(stats))) => {
                                // Startup pushes stats too; wait for the
                                // post-compaction refresh (after the marker).
                                if marker_seen {
                                    return (statuses, marker_seen, stats.context_used_percent);
                                }
                            }
                            Some(_) => {}
                            None => return (statuses, marker_seen, None),
                        }
                    }
                })
                .await
                .unwrap_or((Vec::new(), false, None));
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("compacted context"))
            );
            assert!(marker_seen);
            assert!(stats_percent.is_some());

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::CompactionCheckpoint {
                    reason: crate::CompactionReason::Manual,
                    summary,
                    ..
                } if summary == "manual anchored summary"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn model_selection_switches_next_prompt_and_rejects_other_providers() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-model-selection");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([Ok(provider_text_response(
                "reply from the switched model",
            ))]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::ModelSelectedDetailed {
                        provider: String::from("openai"),
                        model_id: String::from("gpt-5.3"),
                    })
                    .is_ok()
            );
            assert!(
                client_tx
                    .send(ClientEvent::ModelSelectedDetailed {
                        provider: String::from("anthropic"),
                        model_id: String::from("claude-opus-4-8"),
                    })
                    .is_ok()
            );
            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("use the new model"),
                    })
                    .is_ok()
            );

            let (_deltas, statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            assert_eq!(
                finished.map(|(outcome, _)| outcome),
                Some(PromptOutcome::Completed)
            );
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("model change rejected: provider openai"))
            );
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("model changed to claude-opus-4-8"))
            );

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            // The assistant entry's provider metadata records the switched
            // model, proving the request went out with it.
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::EntryAppended {
                    role: Role::Assistant,
                    provider: Some(metadata),
                    ..
                } if metadata.model == "claude-opus-4-8"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn transient_provider_errors_retry_and_request_errors_do_not() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-transient-retry");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            // A stream timeout mid-session recovers on retry (the sesh
            // dogfood failure: a 120s stall after 17 productive rounds was
            // turn-fatal).
            let provider = FakeProviderRequester::with_responses([
                Err(ProviderError {
                    kind: crate::ProviderErrorKind::Timeout,
                    message: String::from("Rig provider stream timed out"),
                    redacted_debug: None,
                }),
                Ok(provider_text_response("recovered after the timeout")),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("survive the stream stall"),
                    })
                    .is_ok()
            );

            let (deltas, statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            assert_eq!(
                finished.map(|(outcome, _)| outcome),
                Some(PromptOutcome::Completed)
            );
            assert!(deltas.join("").contains("recovered after the timeout"));
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("provider timeout; retrying in 1s"))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn empty_final_response_gets_one_nudge_for_the_answer() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-empty-response-nudge");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(provider_text_response("the actual final answer")),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("answer me"),
                    })
                    .is_ok()
            );

            let (deltas, statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            assert_eq!(
                finished.map(|(outcome, _)| outcome),
                Some(PromptOutcome::Completed)
            );
            assert!(deltas.join("").contains("the actual final answer"));
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("empty response; requesting the final answer"))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn truncated_bash_output_continues_the_turn() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-truncated-bash");
            root.write(".yach/config.json", r#"{"shell":{"allow":["seq"]}}"#);
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            // seq to 200k prints far past the 48KB bounded capture, so the
            // result comes back truncated=true; the turn must continue (the
            // capture is the designed shape, not an error).
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-bash-1"),
                            name: String::from("bash"),
                            arguments_json: serde_json::json!({ "command": "seq 1 200000" }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(provider_text_response("summarized the long output")),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("run the long command"),
                    })
                    .is_ok()
            );

            let (deltas, _statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            assert_eq!(
                finished.map(|(outcome, _)| outcome),
                Some(PromptOutcome::Completed)
            );
            assert!(deltas.join("").contains("summarized the long output"));

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Completed,
                    result_content: Some(content),
                    ..
                } if content.contains("[truncated: kept ")
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn oversized_read_fails_recoverably_and_turn_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-oversized-read");
            root.write("big.jsonl", &"x".repeat(64 * 1024));
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({ "path": "big.jsonl" }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(provider_text_response(
                    "the file is too large; sampling with bash instead",
                )),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("read the big file"),
                    })
                    .is_ok()
            );

            let (deltas, _statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            // The oversized read must not abort the turn: the model gets a
            // failed-with-guidance tool result and finishes normally.
            assert_eq!(
                finished.map(|(outcome, _)| outcome),
                Some(PromptOutcome::Completed)
            );
            assert!(deltas.join("").contains("sampling with bash"));

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::ToolExecutionFinished {
                    outcome: ToolOutcome::Failed,
                    reason: Some(reason),
                    result_content: Some(content),
                    ..
                } if reason == "resource_read_too_large"
                    && content.contains("bash tool to sample")
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn agent_tool_rounds_refresh_session_stats_mid_turn() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-mid-turn-stats");
            root.write("note.txt", "hello from the note");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({ "path": "note.txt" }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(provider_text_response("done reading the note")),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("read the note"),
                    })
                    .is_ok()
            );

            // A stats push with the user entry counted but no assistant
            // entry yet can only come from inside the turn: the startup
            // push has no entries and the completion push already counts
            // the assistant reply.
            let mid_turn_stats_seen =
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        match backend_rx.recv().await {
                            Some(BackendEvent::Server(ServerEvent::SessionStatsUpdated(stats)))
                                if stats.user_message_count == Some(1)
                                    && stats.assistant_message_count == Some(0) =>
                            {
                                return stats.context_used_percent.is_some();
                            }
                            Some(BackendEvent::Server(ServerEvent::PromptFinished { .. }))
                            | None => return false,
                            Some(_) => {}
                        }
                    }
                })
                .await
                .unwrap_or(false);
            assert!(mid_turn_stats_seen);

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn manual_compaction_reports_nothing_to_fold() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-manual-compaction-empty");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::CompactionRequested {
                        session_id: String::from("default"),
                        instructions: None,
                    })
                    .is_ok()
            );

            let status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    match backend_rx.recv().await {
                        Some(BackendEvent::Server(ServerEvent::StatusUpdated { message }))
                            if message.contains("compact") =>
                        {
                            return Some(message);
                        }
                        Some(_) => {}
                        None => return None,
                    }
                }
            })
            .await
            .unwrap_or_default();
            assert_eq!(status.as_deref(), Some("nothing to compact yet"));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_overflow_error_compacts_and_retries() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-compaction-overflow");
            root.write(
                ".yach/config.json",
                r#"{"compaction":{"keep_recent_tokens":100}}"#,
            );
            let session_path = root.root().join("session.jsonl");
            seed_completed_turn(&session_path, "turn-0", &"prior context ".repeat(600));
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Err(ProviderError {
                    kind: crate::ProviderErrorKind::ContextLength,
                    message: String::from("prompt is too long"),
                    redacted_debug: None,
                }),
                Ok(provider_text_response("overflow recovery summary")),
                Ok(provider_text_response("reply after overflow recovery")),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("prompt that overflows the provider"),
                    })
                    .is_ok()
            );

            let (deltas, _statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            assert_eq!(
                finished.map(|(outcome, _)| outcome),
                Some(PromptOutcome::Completed)
            );
            assert!(deltas.join("").contains("reply after overflow recovery"));

            let log = JsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                SessionEvent::CompactionCheckpoint {
                    reason: crate::CompactionReason::Overflow,
                    summary,
                    ..
                } if summary == "overflow recovery summary"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_compaction_over_threshold_but_fitting_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-compaction-over-threshold");
            root.write(
                ".yach/config.json",
                r#"{"compaction":{"auto_threshold_percent":10,"keep_recent_tokens":200}}"#,
            );
            let session_path = root.root().join("session.jsonl");
            seed_completed_turn(&session_path, "turn-0", &"legacy work ".repeat(10_000));
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            // The current prompt alone stays above the threshold after
            // compaction, but it fits the usable window, so the turn
            // proceeds to the second (answer) response.
            let provider = FakeProviderRequester::with_responses([
                Ok(provider_text_response("summary of the legacy work")),
                Ok(provider_text_response("answer after compaction")),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: "x".repeat(120_000),
                    })
                    .is_ok()
            );

            let (deltas, statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            let Some((outcome, _message)) = finished else {
                unreachable!("prompt must finish");
            };
            assert_eq!(outcome, PromptOutcome::Completed);
            assert!(deltas.join("").contains("answer after compaction"));
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("still above the compaction threshold"))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_mid_turn_threshold_compaction_checkpoints_and_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-mid-turn-compaction");
            root.write(
                ".yach/config.json",
                r#"{"compaction":{"auto_threshold_percent":10,"keep_recent_tokens":200}}"#,
            );
            // The seeded turn (~15K tokens) keeps the pre-turn estimate
            // under the ~18K threshold; the tool result (~8K tokens, under
            // the 32KB read_text_file bound) pushes the continuation over
            // it mid-turn, where no trigger existed before (dogfood
            // finding 2026-07-24: a turn reached 126% of usable with the
            // trigger only running at turn start).
            root.write("big-notes.txt", &"note content ".repeat(2_400));
            let session_path = root.root().join("session.jsonl");
            seed_completed_turn(&session_path, "turn-0", &"legacy work ".repeat(5_000));
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-big"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({ "path": "big-notes.txt" }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(provider_text_response(
                    "mid-turn summary of the legacy work",
                )),
                Ok(provider_text_response("answer after mid-turn compaction")),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("read the big notes"),
                    })
                    .is_ok()
            );

            let (deltas, statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            let Some((outcome, _message)) = finished else {
                unreachable!("prompt must finish");
            };
            assert_eq!(outcome, PromptOutcome::Completed);
            assert!(deltas.join("").contains("answer after mid-turn compaction"));
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("compacting context"))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());

            // The checkpoint must land mid-turn: after the tool result was
            // recorded, not before the turn's first request (which is
            // where the pre-turn trigger writes it).
            let raw = std::fs::read_to_string(&session_path);
            assert!(raw.is_ok());
            let Ok(raw) = raw else {
                return;
            };
            let lines: Vec<&str> = raw.lines().collect();
            // The single tool execution in this turn; ToolExecutionFinished
            // carries the native request id, not the provider call id.
            let tool_finished_index = lines
                .iter()
                .position(|line| line.contains("\"type\":\"tool_execution_finished\""));
            let checkpoint_index = lines
                .iter()
                .position(|line| line.contains("\"type\":\"compaction_checkpoint\""));
            let (Some(tool_finished_index), Some(checkpoint_index)) =
                (tool_finished_index, checkpoint_index)
            else {
                unreachable!("tool result and checkpoint must both be persisted");
            };
            assert!(checkpoint_index > tool_finished_index);
            assert!(lines[checkpoint_index].contains("\"turn_id\":\"turn-1\""));
            assert!(lines[checkpoint_index].contains("\"reason\":\"threshold\""));
        });
    }

    #[test]
    fn compaction_threshold_clamps_to_keep_recent_floor() {
        let config = crate::CompactionConfig {
            keep_recent_tokens: 20_000,
            auto_threshold_percent: 10,
            ..crate::CompactionConfig::default()
        };
        let budget = super::CompactionBudget {
            context_window: 200_000,
            max_output_tokens: 32_768,
            config: &config,
        };
        // 10% of usable (~15K) sits below the 20K kept-tail floor.
        assert_eq!(budget.threshold_tokens(), 20_000);
        assert!(!budget.over_threshold(20_000));
        assert!(budget.over_threshold(20_001));
    }

    #[test]
    fn provider_agent_compaction_overflow_guard_fails_turn_visibly() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-compaction-thrash");
            // reserve_tokens shrinks the usable window to 9000 tokens so
            // the 30K-token prompt cannot fit even after compaction.
            root.write(
                ".yach/config.json",
                r#"{"compaction":{"auto_threshold_percent":10,"keep_recent_tokens":200,"reserve_tokens":190000}}"#,
            );
            let session_path = root.root().join("session.jsonl");
            seed_completed_turn(&session_path, "turn-0", &"legacy work ".repeat(10_000));
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            // Only the summary response: the current prompt alone overflows
            // the usable window, so the turn must fail before any further
            // provider request.
            let provider = FakeProviderRequester::with_responses([Ok(provider_text_response(
                "summary that cannot save this turn",
            ))]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: "x".repeat(120_000),
                    })
                    .is_ok()
            );

            let (_deltas, statuses, finished) = collect_prompt_outcome(&mut backend_rx).await;
            let Some((outcome, _message)) = finished else {
                unreachable!("prompt must finish");
            };
            assert_eq!(outcome, PromptOutcome::Failed);
            assert!(
                statuses
                    .iter()
                    .any(|status| status.contains("exceeds the usable window even after compaction"))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_agent_edit_tool_long_path_result_stays_bounded_after_apply() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-agent-edit-long-path");
            let path = format!("{}/notes.txt", "a".repeat(180));
            root.write(&path, "alpha\n");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-edit-1"),
                            name: String::from("edit_text_file"),
                            arguments_json: serde_json::json!({
                                "path": path,
                                "find": "alpha",
                                "replace": "beta"
                            }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: TurnId(String::from("turn-1")),
                        delta: String::from("edit applied"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));
            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("change alpha to beta"),
                    })
                    .is_ok()
            );
            let review = recv_tool_review(&mut backend_rx).await;
            assert!(review.is_some());
            let Some(review) = review else {
                return;
            };
            let ToolReviewPayload::LocalEdit { preview } = review.payload else {
                unreachable!("edit review payload expected");
            };
            assert!(
                client_tx
                    .send(ClientEvent::ToolReviewDecisionSubmitted {
                        request_id: review.request_id,
                        preview_id: preview.preview_id,
                        permission_decision_id: preview.permission_decision_id,
                        decision: LocalEditDecision::Apply,
                    })
                    .is_ok()
            );

            let finished = recv_prompt_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Completed));
            assert_eq!(
                std::fs::read_to_string(root.root().join(path)).ok(),
                Some(String::from("beta\n"))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_prompt_uses_in_memory_log_after_startup() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-in-memory-log");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let (provider, requests) = RecordingProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-0")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: TurnId(String::from("turn-0")),
                        delta: String::from("first answer"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-0")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: TurnId(String::from("turn-1")),
                        delta: String::from("second answer"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_requester_factory(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                move |_| provider.clone(),
            ));
            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("first prompt"),
                    })
                    .is_ok()
            );
            assert_eq!(
                recv_prompt_finished(&mut backend_rx).await,
                Some(PromptOutcome::Completed)
            );

            let injected_log = completed_text_exchange(
                SessionId(String::from("default")),
                EntryId(String::from("entry-injected-user")),
                EntryId(String::from("entry-injected-assistant")),
                TurnId(String::from("turn-injected")),
                String::from("injected disk prompt"),
                String::from("injected disk answer"),
            );
            let store = JsonlSessionStore::new(session_path);
            assert!(store.append_events(&injected_log.events).is_ok());

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("second prompt"),
                    })
                    .is_ok()
            );
            assert_eq!(
                recv_prompt_finished(&mut backend_rx).await,
                Some(PromptOutcome::Completed)
            );

            let second_request_text = {
                let requests = requests.lock();
                assert!(requests.is_ok());
                let Ok(requests) = requests else {
                    return;
                };
                assert_eq!(requests.len(), 2);
                requests[1]
                    .messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            assert!(second_request_text.contains("first prompt"));
            assert!(second_request_text.contains("first answer"));
            assert!(second_request_text.contains("second prompt"));
            assert!(!second_request_text.contains("injected disk prompt"));
            assert!(!second_request_text.contains("injected disk answer"));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn loop_switches_to_selected_session_path() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };

        runtime.block_on(async {
            let root = TempProject::new("native-session-switch");
            let session_a_path = root.root().join("session-a.jsonl");
            let session_b_path = root.root().join("session-b.jsonl");
            let session_a_log = completed_text_exchange(
                SessionId(String::from("session-a")),
                EntryId(String::from("entry-a-user")),
                EntryId(String::from("entry-a-assistant")),
                TurnId(String::from("turn-a")),
                String::from("prompt from session a"),
                String::from("answer from session a"),
            );
            let session_b_log = completed_text_exchange(
                SessionId(String::from("session-b")),
                EntryId(String::from("entry-b-user")),
                EntryId(String::from("entry-b-assistant")),
                TurnId(String::from("turn-b")),
                String::from("prompt from session b"),
                String::from("answer from session b"),
            );
            assert!(
                JsonlSessionStore::new(session_a_path.clone())
                    .append_events(&session_a_log.events)
                    .is_ok()
            );
            assert!(
                JsonlSessionStore::new(session_b_path.clone())
                    .append_events(&session_b_log.events)
                    .is_ok()
            );

            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(super::run_native_loop(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_a_path,
                    project_root: None,
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
            ));
            assert!(
                client_tx
                    .send(ClientEvent::SessionPathSelected {
                        session_path: session_b_path.to_string_lossy().into_owned(),
                    })
                    .is_ok()
            );

            let mut saw_session_changed = false;
            let mut saw_selected_messages = false;
            for _ in 0..32 {
                let event =
                    tokio::time::timeout(std::time::Duration::from_secs(1), backend_rx.recv())
                        .await;
                match event {
                    Ok(Some(BackendEvent::Server(ServerEvent::SessionChanged { session_id })))
                        if session_id == "session-b" =>
                    {
                        saw_session_changed = true;
                    }
                    Ok(Some(BackendEvent::Server(ServerEvent::SessionMessagesUpdated {
                        messages,
                    }))) => {
                        let text = messages
                            .iter()
                            .map(|message| message.text.as_str())
                            .collect::<Vec<_>>();
                        if text == ["prompt from session b", "answer from session b"] {
                            saw_selected_messages = true;
                            break;
                        }
                    }
                    Ok(Some(_)) => {}
                    _ => break,
                }
            }

            drop(client_tx);
            assert!(handle.await.is_ok());
            assert!(saw_session_changed);
            assert!(saw_selected_messages);
        });
    }

    #[test]
    fn provider_agent_edit_tool_mismatched_review_decision_finishes_failed() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-agent-edit-stale-review");
            root.write("notes.txt", "alpha\n");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: TurnId(String::from("turn-1")),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: TurnId(String::from("turn-1")),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-edit-1"),
                        name: String::from("edit_text_file"),
                        arguments_json: serde_json::json!({
                            "path": "notes.txt",
                            "find": "alpha",
                            "replace": "beta"
                        }),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: TurnId(String::from("turn-1")),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ])]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path,
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));
            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("change alpha to beta"),
                    })
                    .is_ok()
            );
            let review = recv_tool_review(&mut backend_rx).await;
            assert!(review.is_some());
            let Some(review) = review else {
                return;
            };
            let ToolReviewPayload::LocalEdit { preview } = review.payload else {
                unreachable!("edit review payload expected");
            };
            assert!(
                client_tx
                    .send(ClientEvent::ToolReviewDecisionSubmitted {
                        request_id: String::from("stale-request"),
                        preview_id: preview.preview_id,
                        permission_decision_id: preview.permission_decision_id,
                        decision: LocalEditDecision::Apply,
                    })
                    .is_ok()
            );

            let finished = recv_prompt_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Failed));
            assert_eq!(
                std::fs::read_to_string(root.root().join("notes.txt")).ok(),
                Some(String::from("alpha\n"))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn provider_empty_tool_continuation_emits_no_text_marker() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-empty-tool-continuation");
            root.write("notes.txt", "alpha\n");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: TurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({"path": "notes.txt"}),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                // The empty final response is nudged once; a second empty
                // response is accepted and shows the no-text marker.
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: TurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: TurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path,
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
                provider,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("read notes"),
                    })
                    .is_ok()
            );

            let (deltas, outcome) = recv_prompt_deltas_until_finished(&mut backend_rx).await;
            assert_eq!(outcome, Some(PromptOutcome::Completed));
            assert_eq!(deltas, vec![String::from(EMPTY_ASSISTANT_RESPONSE_MESSAGE)]);

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    fn provider_test_config() -> ProviderConfig {
        ProviderConfig {
            adapter: RigProviderAdapterConfig {
                provider: RigProviderConfig::Anthropic {
                    api_key: String::from("test-key"),
                    base_url: None,
                },
                timeout: std::time::Duration::from_secs(30),
                max_tokens: 1000,
                context_window: 200_000,
                max_tokens_param: crate::rig_adapter::MaxTokensParam::default(),
            },
            model: String::from("fixture-model"),
            test_delay_ms: None,
        }
    }

    struct ReceivedToolReview {
        request_id: String,
        tool_name: String,
        payload: ToolReviewPayload,
    }

    async fn recv_tool_review(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    ) -> Option<ReceivedToolReview> {
        let review = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match backend_rx.recv().await {
                    Some(BackendEvent::Server(ServerEvent::ToolReviewRequested {
                        request_id,
                        tool_name,
                        payload,
                    })) => {
                        return Some(ReceivedToolReview {
                            request_id,
                            tool_name,
                            payload,
                        });
                    }
                    Some(_) => {}
                    None => {
                        assert!(
                            !backend_rx.is_closed(),
                            "backend channel closed before tool review"
                        );
                        return None;
                    }
                }
            }
        })
        .await;
        assert!(review.is_ok(), "timed out waiting for tool review");
        let Ok(review) = review else {
            return None;
        };
        review
    }

    async fn recv_prompt_finished(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    ) -> Option<PromptOutcome> {
        let finished = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match backend_rx.recv().await {
                    Some(BackendEvent::Server(ServerEvent::PromptFinished { outcome, .. })) => {
                        return Some(outcome);
                    }
                    Some(_) => {}
                    None => {
                        assert!(
                            !backend_rx.is_closed(),
                            "backend channel closed before prompt finish"
                        );
                        return None;
                    }
                }
            }
        })
        .await;
        assert!(finished.is_ok(), "timed out waiting for prompt finish");
        let Ok(finished) = finished else {
            return None;
        };
        finished
    }

    async fn recv_prompt_deltas_until_finished(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    ) -> (Vec<String>, Option<PromptOutcome>) {
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let mut deltas = Vec::new();
            loop {
                match backend_rx.recv().await {
                    Some(BackendEvent::Server(ServerEvent::PromptDelta { delta, .. })) => {
                        deltas.push(delta);
                    }
                    Some(BackendEvent::Server(ServerEvent::PromptFinished { outcome, .. })) => {
                        return (deltas, Some(outcome));
                    }
                    Some(_) => {}
                    None => {
                        assert!(
                            !backend_rx.is_closed(),
                            "backend channel closed before prompt finish"
                        );
                        return (deltas, None);
                    }
                }
            }
        })
        .await;
        assert!(result.is_ok(), "timed out waiting for prompt finish");
        result.unwrap_or_default()
    }

    async fn recv_local_edit_preview(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    ) -> Option<LocalEditPreviewSummary> {
        let preview = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match backend_rx.recv().await {
                    Some(BackendEvent::Server(ServerEvent::LocalEditPreviewReady {
                        preview,
                        ..
                    })) => return Some(preview),
                    Some(_) => {}
                    None => {
                        assert!(
                            !backend_rx.is_closed(),
                            "backend channel closed before local edit preview"
                        );
                        return None;
                    }
                }
            }
        })
        .await;
        assert!(preview.is_ok(), "timed out waiting for local edit preview");
        let Ok(preview) = preview else {
            return None;
        };
        preview
    }

    async fn recv_local_edit_finished(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    ) -> Option<(Option<String>, LocalEditFinishedOutcome, String)> {
        let finished = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match backend_rx.recv().await {
                    Some(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id,
                        outcome,
                        message,
                    })) => return Some((preview_id, outcome, message)),
                    Some(_) => {}
                    None => {
                        assert!(
                            !backend_rx.is_closed(),
                            "backend channel closed before local edit finish"
                        );
                        return None;
                    }
                }
            }
        })
        .await;
        assert!(finished.is_ok(), "timed out waiting for local edit finish");
        let Ok(finished) = finished else {
            return None;
        };
        finished
    }

    #[test]
    fn provider_one_round_executes_project_path_info_and_continues() {
        let root_guard = temp_native_provider_root("native-provider-one-round-success");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = ResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn.clone(),
                    delta: String::from("I will inspect that."),
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-1"),
                        name: String::from("project_path_info"),
                        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: Some(String::from("response-1")),
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn.clone(),
                    delta: String::from("Cargo.toml is a file."),
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: Some(String::from("response-2")),
                },
            ]),
        ]);

        let Some(root) = root else {
            return;
        };
        let result = futures::executor::block_on(run_native_provider_one_readonly_tool_round(
            &mut requester,
            model,
            &mut log,
            &mut pending_events,
            &turn,
            Some(LaunchProjectContext::from_project_root(root)),
            None,
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("Cargo.toml is a file."),
                provider_response_id: Some(String::from("response-2")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 2);
        let Ok(Some(advertising)) =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
        else {
            return;
        };
        assert_eq!(advertising.tools.len(), 1);
        assert_eq!(advertising.tools[0].name, "project_path_info");
        assert!(
            !requester.requests[1]
                .extensions
                .iter()
                .any(|extension| extension.key == PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY)
        );
        assert_eq!(
            parse_provider_tool_advertising_extensions(&requester.requests[1].extensions),
            Ok(None)
        );
        let guard_message = requester.requests[1]
            .messages
            .iter()
            .rev()
            .find(|message| message.role == Role::System);
        assert!(guard_message.is_some());
        let Some(guard_message) = guard_message else {
            return;
        };
        assert!(
            guard_message
                .content
                .contains("You may call more advertised tools")
        );
        assert!(guard_message.content.contains("Do not claim"));
        assert_eq!(requester.requests[1].messages.len(), 4);
        assert_eq!(requester.requests[1].messages[3].role, Role::Tool);
        assert_eq!(requester.requests[1].messages[3].tool_results.len(), 1);
        let tool_message_content = &requester.requests[1].messages[3].tool_results[0].content;
        assert!(!tool_message_content.contains(root_path.to_string_lossy().as_ref()));
        // The call id binds the result to its call on the block itself,
        // and the payload is the tool's own metadata passed through as text.
        assert_eq!(
            requester.requests[1].messages[3].tool_results[0].call_id,
            "provider-call-1"
        );
        assert_eq!(tool_message_content, "Cargo.toml: file, 10 bytes");
        // project_path_info returns metadata only; the file body never
        // reaches the model.
        assert!(!tool_message_content.contains("[package]"));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
        )));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolExecutionFinished {
                outcome: ToolOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn provider_one_round_persists_tool_events_before_continuation_request() {
        let root_guard = temp_native_provider_root("native-provider-tool-event-flush");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let store = JsonlSessionStore::new(root_path.join("session.jsonl"));
        let root = ResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = StoreCheckingProviderRequester {
            requests: Vec::new(),
            responses: [
                Ok(vec![
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: turn.clone(),
                        tool_call: ProviderToolCall {
                            call_id: String::from("provider-call-1"),
                            name: String::from("project_path_info"),
                            arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn.clone(),
                        finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::TextDelta {
                        turn_id: turn.clone(),
                        delta: String::from("Cargo.toml is a file."),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: turn.clone(),
                        finish_reason: Some(crate::ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: Some(String::from("response-2")),
                    },
                ]),
            ]
            .into_iter()
            .collect(),
            store: store.clone(),
        };

        let Some(root) = root else {
            return;
        };
        let result = futures::executor::block_on(run_native_provider_one_readonly_tool_round(
            &mut requester,
            model,
            &mut log,
            &mut pending_events,
            &turn,
            Some(LaunchProjectContext::from_project_root(root)),
            Some(&store),
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("Cargo.toml is a file."),
                provider_response_id: Some(String::from("response-2")),
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 2);
        assert!(pending_events.is_empty());
    }

    #[test]
    fn provider_one_round_keeps_pending_tool_events_when_flush_fails() {
        let root_guard = temp_native_provider_root("native-provider-tool-event-flush-failure");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let blocked_parent = root_path.join("session-parent");
        assert!(std::fs::write(&blocked_parent, "not a directory").is_ok());
        let store = JsonlSessionStore::new(blocked_parent.join("session.jsonl"));
        let root = ResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn.clone(),
                tool_call: ProviderToolCall {
                    call_id: String::from("provider-call-1"),
                    name: String::from("project_path_info"),
                    arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                },
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ])]);

        let Some(root) = root else {
            return;
        };
        let result = futures::executor::block_on(run_native_provider_one_readonly_tool_round(
            &mut requester,
            model,
            &mut log,
            &mut pending_events,
            &turn,
            Some(LaunchProjectContext::from_project_root(root)),
            Some(&store),
        ));

        assert_eq!(
            result,
            Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_event_persist_failed"
            )))
        );
        assert_eq!(requester.requests.len(), 1);
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
        )));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolExecutionFinished {
                outcome: ToolOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn provider_tool_batch_configured_round_limit_stops_before_next_provider_request() {
        let root_guard = temp_native_provider_root("native-provider-tool-loop-limit");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = ResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn.clone(),
                tool_call: ProviderToolCall {
                    call_id: String::from("provider-call-3"),
                    name: String::from("project_path_info"),
                    arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                },
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ])]);
        let registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
        let permission_policy =
            ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
                ["project_path_info"],
                ["read_text_file", "search_project", "list_project_paths"],
                ["edit_text_file", "create_text_file"],
            );
        let resolved_catalog = registry.resolve_provider_turn_catalog(
            &permission_policy,
            [
                "project_path_info",
                "read_text_file",
                "search_project",
                "list_project_paths",
                "edit_text_file",
                "create_text_file",
            ],
        );
        let Some(root) = root else {
            return;
        };
        let read_only_executor = ProjectReadOnlyToolExecutor::new(root.clone());
        let mut edit_access = EditAccess::default();
        let edit_sink = ProviderBufferedEventSink::new(None);
        let (review_tx, _review_rx) = mpsc::unbounded_channel();
        let (_decision_tx, mut review_decisions) = mpsc::unbounded_channel();
        let mut budget = ProviderToolLoopBudget::new(
            ProviderToolLoopPolicy::agent_default().with_max_tool_rounds(2),
        );
        assert_eq!(budget.begin_tool_round(1), Ok(()));
        assert_eq!(budget.begin_tool_round(1), Ok(()));
        let mut edit_traces = Vec::new();

        let result = futures::executor::block_on(async {
            let round = requester
                .request(ProviderRequest {
                    turn_id: turn.clone(),
                    model,
                    messages: Vec::new(),
                    extensions: Vec::new(),
                })
                .await
                .and_then(|events| {
                    collect_native_provider_first_round(events)
                        .map_err(|error| provider_round_error_to_provider_error(&error))
                });
            assert!(round.is_ok());
            let Ok(round) = round else {
                return Ok(());
            };
            execute_native_provider_agent_tool_batch(
                ProviderAgentToolBatch {
                    session_id: SessionId(String::from("default")),
                    shell_policy: crate::ShellPolicy::default(),
                    turn_id: turn.clone(),
                    project_root: root,
                    registry: &registry,
                    resolved_catalog: &resolved_catalog,
                    permission_policy: &permission_policy,
                    read_only_executor: &read_only_executor,
                    extension_executor: None,
                    edit_access: &mut edit_access,
                    edit_sink: &edit_sink,
                    review_tx,
                    review_decisions: &mut review_decisions,
                    tool_event_store: None,
                    budget: &mut budget,
                    tool_round_index: 3,
                    edit_traces: &mut edit_traces,
                    log: &mut log,
                    pending_events: &mut pending_events,
                },
                round.tool_calls,
            )
            .await
            .map(|_| ())
        });

        assert_eq!(
            result,
            Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_rounds"
            )))
        );
        assert_eq!(requester.requests.len(), 1);
        assert!(pending_events.is_empty());
    }

    #[test]
    fn provider_agent_default_loop_has_no_round_limit() {
        let root_guard = temp_native_provider_root("native-provider-default-tool-loop");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = ResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo repeatedly",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-1"),
                        name: String::from("project_path_info"),
                        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-2"),
                        name: String::from("project_path_info"),
                        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-3"),
                        name: String::from("project_path_info"),
                        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-4"),
                        name: String::from("project_path_info"),
                        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-5"),
                        name: String::from("project_path_info"),
                        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: turn.clone(),
                    model: model.clone(),
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: turn.clone(),
                    delta: String::from("done after five rounds"),
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
        ]);

        let Some(root) = root else {
            return;
        };
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            ProviderAgentToolRound {
                session_id: &SessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn,
                project_context: Some(LaunchProjectContext::from_project_root(root)),
                extension_static_context_files: Vec::new(),
                extension_activation_snapshot: crate::ExtensionActivationSnapshot::default(),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
                context_window: 200_000,
                max_output_tokens: 1_000,
            },
        ));

        assert_eq!(
            result,
            Ok(ProviderRoundResult {
                text: String::from("done after five rounds"),
                provider_response_id: None,
                mid_turn_text: String::new(),
                usage: None,
            })
        );
        assert_eq!(requester.requests.len(), 6);
        let Ok(Some(advertising)) =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
        else {
            return;
        };
        assert!(
            advertising
                .tools
                .iter()
                .any(|tool| tool.name == "project_path_info")
        );
        assert!(requester.requests.iter().skip(1).all(|request| {
            request
                .extensions
                .iter()
                .any(|extension| extension.key == PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY)
        }));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
        )));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolExecutionFinished {
                outcome: ToolOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn provider_one_round_rejects_unknown_tool_before_second_request() {
        let root_guard = temp_native_provider_root("native-provider-unknown-tool");
        let root = ResourceRoot::project(root_guard.path()).ok();
        assert!(root.is_some());
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn.clone(),
                tool_call: ProviderToolCall {
                    call_id: String::from("provider-call-1"),
                    name: String::from("read"),
                    arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                },
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ])]);

        let Some(root) = root else {
            return;
        };
        let result = futures::executor::block_on(run_native_provider_one_readonly_tool_round(
            &mut requester,
            model,
            &mut log,
            &mut pending_events,
            &turn,
            Some(LaunchProjectContext::from_project_root(root)),
            None,
        ));

        assert_eq!(
            result,
            Err(ProviderRoundError::ToolContinuation(String::from(
                "tool_round_validation_failed"
            )))
        );
        assert_eq!(requester.requests.len(), 1);
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolExecutionFinished {
                outcome: ToolOutcome::ValidationFailed,
                ..
            }
        )));
    }

    #[test]
    fn provider_one_round_maps_second_provider_failure() {
        let root_guard = temp_native_provider_root("native-provider-second-failure");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = ResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = SessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &SessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            Role::User,
            "inspect cargo",
        );
        let turn = TurnId(String::from("turn-0"));
        let model = ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: ProviderToolCall {
                        call_id: String::from("provider-call-1"),
                        name: String::from("project_path_info"),
                        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Err(ProviderError::malformed_stream(
                "second provider request failed",
            )),
        ]);

        let Some(root) = root else {
            return;
        };
        let result = futures::executor::block_on(run_native_provider_one_readonly_tool_round(
            &mut requester,
            model,
            &mut log,
            &mut pending_events,
            &turn,
            Some(LaunchProjectContext::from_project_root(root)),
            None,
        ));

        assert_eq!(
            result,
            Err(ProviderRoundError::Provider(
                ProviderError::malformed_stream("second provider request failed")
            ))
        );
        assert_eq!(requester.requests.len(), 2);
        assert_eq!(requester.requests[1].messages.len(), 4);
        assert_eq!(requester.requests[1].messages[0].role, Role::System);
        assert!(
            requester.requests[1].messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(requester.requests[1].messages[2].role, Role::System);
        assert!(
            requester.requests[1].messages[2]
                .content
                .contains("You may call more advertised tools")
        );
        assert_eq!(requester.requests[1].messages[3].role, Role::Tool);
        assert!(
            requester.requests[1].messages[3]
                .tool_results
                .iter()
                .any(|result| result.call_id == "provider-call-1")
        );
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
        )));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            SessionEvent::ToolExecutionFinished {
                outcome: ToolOutcome::Completed,
                ..
            }
        )));
    }

    struct StoreCheckingProviderRequester {
        requests: Vec<ProviderRequest>,
        responses: std::collections::VecDeque<Result<Vec<ProviderStreamEvent>, ProviderError>>,
        store: JsonlSessionStore,
    }

    impl ProviderRequester for StoreCheckingProviderRequester {
        fn request(
            &mut self,
            request: ProviderRequest,
        ) -> futures::future::BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>>
        {
            if self.requests.len() == 1 {
                let stored_log = self.store.load();
                assert!(
                    stored_log.is_ok(),
                    "tool events should be durable before continuation request: {stored_log:?}"
                );
                let Ok(stored_log) = stored_log else {
                    unreachable!("checked above");
                };
                assert!(stored_log.events.iter().any(|event| matches!(
                    event,
                    SessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
                )));
                assert!(stored_log.events.iter().any(|event| matches!(
                    event,
                    SessionEvent::ToolExecutionFinished {
                        outcome: ToolOutcome::Completed,
                        ..
                    }
                )));
            }
            self.requests.push(request);
            let response = self.responses.pop_front().unwrap_or_else(|| {
                Err(ProviderError {
                    kind: ProviderErrorKind::InvalidRequest,
                    message: String::from("missing fake provider response"),
                    redacted_debug: None,
                })
            });
            Box::pin(async move { response })
        }
    }

    #[test]
    fn provider_messages_exclude_failed_prior_turns() {
        let session_id = SessionId(String::from("default"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-0",
            "entry-0-user",
            Role::User,
            "failed prompt",
        );
        finish_native_provider_test_turn(&mut log, &session_id, "turn-0", TurnOutcome::Failed);
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "current prompt",
        );

        assert_eq!(
            provider_messages_from_log(&log, &TurnId(String::from("turn-1"))),
            vec![ProviderMessage::text(
                Role::User,
                String::from("current prompt")
            )]
        );
    }

    #[test]
    fn provider_messages_exclude_cancelled_prior_turns() {
        let session_id = SessionId(String::from("default"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-0",
            "entry-0-user",
            Role::User,
            "cancelled prompt",
        );
        finish_native_provider_test_turn(&mut log, &session_id, "turn-0", TurnOutcome::Cancelled);
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "current prompt",
        );

        assert_eq!(
            provider_messages_from_log(&log, &TurnId(String::from("turn-1"))),
            vec![ProviderMessage::text(
                Role::User,
                String::from("current prompt")
            )]
        );
    }

    #[test]
    fn session_load_warning_status_preserves_valid_events() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        let root = TempProject::new("session-load-warning");
        let path = root.root().join("session.jsonl");
        let log = completed_text_exchange(
            SessionId(String::from("default")),
            EntryId(String::from("entry-user-0")),
            EntryId(String::from("entry-assistant-0")),
            TurnId(String::from("turn-0")),
            String::from("hello"),
            String::from("hi"),
        );
        let raw = log
            .events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap_or_default())
            .chain([String::from("{bad json")])
            .collect::<Vec<_>>()
            .join("\n");
        assert!(std::fs::write(&path, format!("{raw}\n")).is_ok());
        let store = JsonlSessionStore::new(path);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let loaded = runtime.block_on(load_native_session_log_for_runner(&tx, &store));
        let status = rx.try_recv();

        assert_eq!(loaded, log);
        assert!(matches!(
            status,
            Ok(BackendEvent::Server(ServerEvent::StatusUpdated { message }))
                if message.contains("skipped corrupt session log line 4")
                    && !message.contains("bad json")
        ));
    }

    #[test]
    fn session_startup_load_runs_on_blocking_thread() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        let reactor_thread_id = std::thread::current().id();
        let load_thread_id = Arc::new(Mutex::new(None));
        let load_thread_id_for_loader = load_thread_id.clone();

        runtime.block_on(async move {
            let (tx, _rx) = mpsc::unbounded_channel();
            let loaded = load_native_session_log_for_runner_with_loader(&tx, move || {
                let recorded = load_thread_id_for_loader.lock();
                assert!(recorded.is_ok());
                let Ok(mut recorded) = recorded else {
                    return Err(std::io::Error::other("load thread lock poisoned"));
                };
                *recorded = Some(std::thread::current().id());
                Ok(SessionLoadResult {
                    log: SessionLog::default(),
                    warnings: Vec::new(),
                })
            })
            .await;

            assert!(loaded.events.is_empty());
        });

        let recorded = load_thread_id.lock();
        assert!(recorded.is_ok());
        let Ok(recorded) = recorded else {
            return;
        };
        assert!(recorded.is_some());
        assert_ne!(*recorded, Some(reactor_thread_id));
    }

    #[test]
    fn session_messages_include_tool_execution_results() {
        let session_id = SessionId(String::from("default"));
        let turn_id = TurnId(String::from("turn-1"));
        let tool_request_id = ToolRequestId(String::from("tool-request-1"));
        let mut log = SessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            Role::User,
            "read file",
        );
        log.push(SessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("read_text_file"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: ToolPermissionState::Allowed,
            argument_summary: ToolPayloadSummary {
                summary: String::from("path=README.md"),
                byte_count: 16,
                redacted: false,
                truncated: false,
            },
            argument_content: None,
        });
        log.push(SessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id,
            tool_request_id,
            outcome: ToolOutcome::Completed,
            reason: None,
            result_summary: Some(ToolPayloadSummary {
                summary: String::from("read_text_file result redacted"),
                byte_count: 6,
                redacted: true,
                truncated: false,
            }),
            result_content: Some(String::from("hello\n")),
        });
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-assistant",
            Role::Assistant,
            "summary",
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        send_native_session_messages_from_log(&tx, &log);
        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert!(matches!(
            event,
            BackendEvent::Server(ServerEvent::SessionMessagesUpdated { .. })
        ));
        let BackendEvent::Server(ServerEvent::SessionMessagesUpdated { messages }) = event else {
            return;
        };

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].role, "tool");
        assert_eq!(messages[1].tool_name.as_deref(), Some("read_text_file"));
        assert_eq!(messages[1].is_error, Some(false));
        assert_eq!(messages[1].text, "completed: 1 line, 6 bytes");
    }

    #[test]
    fn session_messages_render_persisted_tool_content_like_live_progress() {
        let session_id = SessionId(String::from("default"));
        let turn_id = TurnId(String::from("turn-1"));
        let tool_request_id = ToolRequestId(String::from("tool-request-1"));
        let list_content = String::from("src/lib.rs\nsrc/main.rs");
        let live_result = ProviderToolResult {
            tool_request_id: tool_request_id.0.clone(),
            provider_call_id: Some(String::from("call-1")),
            status: ToolOutcome::Completed,
            byte_count: list_content.len(),
            content: list_content.clone(),
            redacted: true,
            truncated: false,
            reason: None,
        };
        let live_display = provider_tool_progress_output("list_project_paths", &live_result);

        let mut log = SessionLog::default();
        log.push(SessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("list_project_paths"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: ToolPermissionState::Allowed,
            argument_summary: ToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 15,
                redacted: true,
                truncated: false,
            },
            argument_content: Some(String::from("{\"path\":\"src\"}")),
        });
        log.push(SessionEvent::ToolExecutionFinished {
            session_id,
            turn_id,
            tool_request_id,
            outcome: ToolOutcome::Completed,
            reason: None,
            result_summary: Some(ToolPayloadSummary {
                summary: String::from("list_project_paths entries=2 truncated=false"),
                byte_count: list_content.len(),
                redacted: true,
                truncated: false,
            }),
            result_content: Some(list_content),
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        send_native_session_messages_from_log(&tx, &log);
        let Ok(BackendEvent::Server(ServerEvent::SessionMessagesUpdated { messages })) =
            rx.try_recv()
        else {
            unreachable!("session messages event expected");
        };

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, live_display);
        assert!(messages[0].text.contains("completed: 2 lines"));
        assert!(messages[0].text.contains("src/lib.rs"));
        assert!(messages[0].text.contains("src/main.rs"));
    }

    #[test]
    fn session_messages_note_missing_content_for_pre_persistence_logs() {
        let session_id = SessionId(String::from("default"));
        let turn_id = TurnId(String::from("turn-1"));
        let tool_request_id = ToolRequestId(String::from("tool-request-1"));
        let mut log = SessionLog::default();
        log.push(SessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("search_project"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: ToolPermissionState::Allowed,
            argument_summary: ToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 20,
                redacted: true,
                truncated: false,
            },
            argument_content: None,
        });
        log.push(SessionEvent::ToolExecutionFinished {
            session_id,
            turn_id,
            tool_request_id,
            outcome: ToolOutcome::Completed,
            reason: None,
            result_summary: Some(ToolPayloadSummary {
                summary: String::from("search_project matches=2 truncated=false"),
                byte_count: 64,
                redacted: true,
                truncated: false,
            }),
            result_content: None,
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        send_native_session_messages_from_log(&tx, &log);
        let Ok(BackendEvent::Server(ServerEvent::SessionMessagesUpdated { messages })) =
            rx.try_recv()
        else {
            unreachable!("session messages event expected");
        };

        assert_eq!(messages.len(), 1);
        assert!(messages[0].text.contains("output not retained"));
    }

    fn append_native_provider_test_entry(
        log: &mut SessionLog,
        session_id: &SessionId,
        turn_id: &str,
        entry_id: &str,
        role: Role,
        text: &str,
    ) {
        log.push(SessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: EntryId(entry_id.to_owned()),
            parent_entry_id: None,
            turn_id: TurnId(turn_id.to_owned()),
            role,
            text: text.to_owned(),
            provider: None,
        });
    }

    fn finish_native_provider_test_turn(
        log: &mut SessionLog,
        session_id: &SessionId,
        turn_id: &str,
        outcome: TurnOutcome,
    ) {
        log.push(SessionEvent::TurnFinished {
            session_id: session_id.clone(),
            turn_id: TurnId(turn_id.to_owned()),
            outcome,
            reason: None,
        });
    }

    struct ProviderTempRoot {
        path: std::path::PathBuf,
    }

    impl ProviderTempRoot {
        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for ProviderTempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn temp_native_provider_root(label: &str) -> ProviderTempRoot {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "yach-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        assert!(std::fs::create_dir_all(&path).is_ok());
        ProviderTempRoot { path }
    }
}
