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
    NativeAgentEditToolContext, NativeAgentEditToolPrepared, PendingAgentEditToolReview,
    apply_agent_edit_tool_review, prepare_agent_edit_tool_request, reject_agent_edit_tool_review,
};
use crate::rig_adapter::{
    RigProviderAdapterConfig, RigProviderConfig, run_provider_request_with_approved_tools,
};
use crate::{
    NativeDurationMetric, NativeEditAccess, NativeEditPolicy, NativeEditPreviewId,
    NativeEditTraceId, NativeEditTraceOutcome, NativeEditTracePhase, NativeEditTraceRecord,
    NativeEditTraceSource, NativeEntryId, NativeExtensionStaticContextFile,
    NativeJsonlSessionStore, NativeMetricAttribute, NativePermissionDecisionId,
    NativePermissionPolicy, NativeProviderToolResult, NativeResourceRoot, NativeRole,
    NativeSessionEvent, NativeSessionEventSink, NativeSessionId, NativeSessionLog,
    NativeStaticContextBundle, NativeStaticContextItem, NativeStaticContextPlacement,
    NativeStaticContextPolicy, NativeToolContinuationError, NativeToolExecutionResult,
    NativeToolExecutor, NativeToolOutcome, NativeToolPayloadSummary, NativeToolPermissionPolicy,
    NativeToolRegistry, NativeToolRequestId, NativeTurnId, NativeTurnOutcome,
    PendingNativeToolRequest, ProjectReadOnlyToolExecutor, ProviderContinuationMappingError,
    ProviderContinuationRequest, ProviderContinuationValidationPolicy, ProviderError,
    ProviderErrorKind, ProviderFinishReason, ProviderMessage, ProviderMetadata, ProviderModel,
    ProviderRequest, ProviderStreamEvent, ProviderToolAdvertisingError, ProviderToolCall,
    ResolvedNativeToolCatalog, assemble_project_static_context_with_extensions,
    build_provider_continuation_submission, build_provider_tool_advertising_extension,
    pending_tool_request_from_provider_call, record_native_tool_validation_with_resolved_catalog,
};
#[cfg(test)]
use crate::{
    NativeToolContinuationContext, NativeToolContinuationPolicy, NativeToolContinuationWorkflow,
};

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
pub use extension_state::{NativeExtensionPackageRootLoader, NativeStartupTraceMarker};
#[cfg(test)]
use local_edit::native_local_edit_error_message;
use local_edit::{
    NativeLocalEditPrepareInput, handle_native_local_edit_decision,
    handle_native_local_edit_prepare, native_local_edit_preview_summary, native_local_edit_root,
};
#[cfg(test)]
use session_state::load_native_session_log_for_runner_with_loader;
use session_state::{
    load_native_session_log_for_runner, native_session_message_count,
    native_session_state_from_load_result, send_native_recent_sessions,
    send_native_session_messages_from_log, send_native_session_stats_from_log,
};

/// Native dogfood runner configuration owned by the backend Module.
#[derive(Clone)]
pub struct NativeDogfoodRunnerConfig {
    pub session_path: PathBuf,
    pub project_root: Option<PathBuf>,
    pub provider: Option<NativeProviderDogfoodConfig>,
    /// Why the native provider is unavailable, when the CLI could not build a
    /// provider config. Present only when `provider` is `None`; prompts fail
    /// with this message instead of falling back to fixture responses.
    pub provider_setup_error: Option<String>,
    pub extension_package_roots: Vec<crate::ExtensionPackageRoot>,
    pub extension_package_root_loader: Option<NativeExtensionPackageRootLoader>,
    pub startup_trace: Option<NativeStartupTraceMarker>,
}

impl std::fmt::Debug for NativeDogfoodRunnerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeDogfoodRunnerConfig")
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

/// Explicit native-provider dogfood settings supplied by the CLI Adapter.
#[derive(Debug, Clone)]
pub struct NativeProviderDogfoodConfig {
    pub adapter: RigProviderAdapterConfig,
    pub model: String,
    pub test_delay_ms: Option<u64>,
}

impl NativeProviderDogfoodConfig {
    #[must_use]
    pub fn provider_label(&self) -> &'static str {
        native_provider_label(&self.adapter.provider)
    }
}

const fn native_provider_label(provider: &RigProviderConfig) -> &'static str {
    match provider {
        RigProviderConfig::Anthropic { .. } => "anthropic",
        RigProviderConfig::ChatGptSubscription { .. } => "chatgpt-subscription",
    }
}

#[derive(Debug)]
struct ActiveProviderTurn {
    handle: tokio::task::JoinHandle<NativeSessionLog>,
    turn_id: NativeTurnId,
    prompt_started: Instant,
    review_decision_tx: mpsc::UnboundedSender<AgentEditReviewDecision>,
}

async fn collect_finished_provider_turn(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    active_provider_turn: &mut Option<ActiveProviderTurn>,
    session_log: &mut NativeSessionLog,
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
struct NativeProviderPromptProjectRuntime {
    project_context: Option<NativeLaunchProjectContext>,
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

struct NativePromptSessionInput<'a> {
    current_session_id: &'a str,
    requested_session_id: String,
}

struct NativeSessionSwitchState<'a> {
    current_session_path: &'a mut PathBuf,
    current_session_id: &'a mut String,
    store: &'a mut NativeJsonlSessionStore,
    session_log: &'a mut NativeSessionLog,
    turn_index: &'a mut u64,
    local_edit_index: &'a mut u64,
}

#[must_use]
pub fn native_session_log_dir() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".yach")
        .join("native-sessions")
}

#[must_use]
pub fn native_session_log_path(session_id: &str) -> PathBuf {
    native_session_log_dir().join(format!("{session_id}.jsonl"))
}

#[must_use]
pub fn native_fresh_session_id() -> String {
    let timestamp_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("session-{}-{timestamp_nanos}", std::process::id())
}

#[must_use]
pub fn native_session_id_from_log_path(path: &Path) -> Option<String> {
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
    latest_native_session_log_path_in(&native_session_log_dir())
}

#[must_use]
pub fn latest_native_session_log_path_in(session_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(session_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| native_session_id_from_log_path(path).is_some())
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// Run the constrained native dogfood backend event loop.
pub async fn run_native_dogfood_loop(
    rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    config: NativeDogfoodRunnerConfig,
) {
    run_native_dogfood_loop_with_requester_factory(rx, tx, config, |provider| {
        RigProviderRequester {
            adapter: provider.adapter.clone(),
            approved_tools: native_provider_approved_tools(),
        }
    })
    .await;
}

#[cfg(test)]
async fn run_native_dogfood_loop_with_provider_requester<Requester>(
    rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    config: NativeDogfoodRunnerConfig,
    requester: Requester,
) where
    Requester: ProviderRequester + Send + 'static,
{
    let mut requester = Some(requester);
    run_native_dogfood_loop_with_requester_factory(rx, tx, config, move |_| {
        let Some(requester) = requester.take() else {
            unreachable!("test provider requester can only be used once");
        };
        requester
    })
    .await;
}

async fn run_native_dogfood_loop_with_requester_factory<MakeRequester, Requester>(
    mut rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    config: NativeDogfoodRunnerConfig,
    mut make_requester: MakeRequester,
) where
    MakeRequester: FnMut(&NativeProviderDogfoodConfig) -> Requester,
    Requester: ProviderRequester + Send + 'static,
{
    let NativeDogfoodRunnerConfig {
        mut session_path,
        project_root,
        mut provider,
        provider_setup_error,
        extension_package_roots,
        extension_package_root_loader,
        startup_trace,
    } = config;
    let mut current_session_id =
        native_session_id_from_log_path(&session_path).unwrap_or_else(|| String::from("default"));
    let mut store = NativeJsonlSessionStore::new(session_path.clone());
    let provider_project_context = project_root
        .as_ref()
        .and_then(native_launch_project_context_from_root);
    let edit_root = native_local_edit_root(project_root.clone());
    let mut edit_access = NativeEditAccess::default();
    send_native_initial_state(
        &tx,
        &current_session_id,
        &session_path,
        provider.as_ref(),
        provider_setup_error.as_deref(),
    );
    for warning in crate::NativeSensitivePathPolicy::load_for_project(project_root.as_deref()).1 {
        let message = match warning {
            crate::NativeSensitivePathConfigWarning::InvalidConfig { path, .. } => {
                format!(
                    "sensitive_file_config: invalid config at {path}; built-in deny defaults remain in force"
                )
            }
            crate::NativeSensitivePathConfigWarning::InvalidPattern { pattern } => {
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
        native_context_budget(provider.as_ref(), project_root.as_deref()),
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
                        &NativeSessionId(current_session_id.clone()),
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
                    native_context_budget(provider.as_ref(), project_root.as_deref()),
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
                    crate::NativeCompactionConfig::load_for_project(project_root.as_deref());
                if crate::select_compaction_cut(&session_log, compaction_config.keep_recent_tokens)
                    .is_none()
                {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("nothing to compact yet"),
                    }));
                    continue;
                }
                let compaction_turn = NativeTurnId(format!("turn-{turn_index}"));
                turn_index = turn_index.saturating_add(1);
                local_edit_index = local_edit_index.max(turn_index);
                let mut requester = make_requester(&provider);
                let model = ProviderModel {
                    provider: provider.provider_label().to_owned(),
                    model: provider.model.clone(),
                };
                let tokens_before = crate::estimate_current_context_tokens(&session_log);
                let mut pending_events = Vec::new();
                let result = native_run_compaction(
                    &mut requester,
                    NativeCompactionRun {
                        session_id: &NativeSessionId(current_session_id.clone()),
                        turn_id: &compaction_turn,
                        model: &model,
                        config: &compaction_config,
                        reason: crate::NativeCompactionReason::Manual,
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
                            native_context_budget(Some(&provider), project_root.as_deref()),
                        );
                    }
                    Ok(false) => {}
                    Err(error) => {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                            message: format!(
                                "manual compaction failed: {}",
                                native_provider_round_error_label(&error)
                            ),
                        }));
                    }
                }
            }
            ClientEvent::PromptSubmitted { session_id, prompt } => {
                if prompt.trim().is_empty() {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("native dogfood: empty prompt ignored"),
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
                        NativePromptSessionInput {
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
                        NativeProviderPromptProjectRuntime {
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
                        NativePromptSessionInput {
                            current_session_id: &current_session_id,
                            requested_session_id: session_id,
                        },
                        &NativeUnconfiguredProviderPrompt {
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
                        NativePromptSessionInput {
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
                        message: String::from(
                            "native dogfood: cannot switch sessions during an active prompt",
                        ),
                    }));
                    continue;
                }
                let selected_path =
                    native_session_path_for_id_in_dir(session_path.parent(), &session_id);
                let Some(selected_path) = selected_path else {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: format!("native dogfood: unknown session {session_id}"),
                    }));
                    continue;
                };
                if !native_session_path_is_selectable(&selected_path, &session_path) {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: format!("native dogfood: unknown session {session_id}"),
                    }));
                    continue;
                }
                switch_native_session(
                    &tx,
                    selected_path,
                    NativeSessionSwitchState {
                        current_session_path: &mut session_path,
                        current_session_id: &mut current_session_id,
                        store: &mut store,
                        session_log: &mut session_log,
                        turn_index: &mut turn_index,
                        local_edit_index: &mut local_edit_index,
                    },
                    native_context_budget(provider.as_ref(), project_root.as_deref()),
                )
                .await;
            }
            ClientEvent::SessionPathSelected {
                session_path: selected_session_path,
            } => {
                if active_provider_turn.is_some() {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from(
                            "native dogfood: cannot switch sessions during an active prompt",
                        ),
                    }));
                    continue;
                }
                let selected_path = PathBuf::from(&selected_session_path);
                if !native_session_path_is_selectable(&selected_path, &session_path) {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: format!(
                            "native dogfood: unknown session path {selected_session_path}"
                        ),
                    }));
                    continue;
                }
                switch_native_session(
                    &tx,
                    selected_path,
                    NativeSessionSwitchState {
                        current_session_path: &mut session_path,
                        current_session_id: &mut current_session_id,
                        store: &mut store,
                        session_log: &mut session_log,
                        turn_index: &mut turn_index,
                        local_edit_index: &mut local_edit_index,
                    },
                    native_context_budget(provider.as_ref(), project_root.as_deref()),
                )
                .await;
            }
            ClientEvent::ForkMessagesRequested | ClientEvent::SessionForkRequested { .. } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: String::from(
                        "native dogfood: fork/session tree UI is not available yet",
                    ),
                }));
            }
            ClientEvent::ThinkingLevelSelected { level } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!(
                        "native dogfood: thinking level {level} noted but not used yet"
                    ),
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
                    NativeLocalEditPrepareInput {
                        session_id: NativeSessionId(current_session_id.clone()),
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

fn native_session_path_for_id_in_dir(
    session_dir: Option<&Path>,
    session_id: &str,
) -> Option<PathBuf> {
    if session_id.is_empty() || session_id == "." || session_id == ".." {
        return None;
    }
    if session_id.contains('/') || session_id.contains('\\') {
        return None;
    }
    Some(session_dir?.join(format!("{session_id}.jsonl")))
}

fn native_session_path_is_selectable(
    selected_session_path: &Path,
    current_session_path: &Path,
) -> bool {
    native_session_id_from_log_path(selected_session_path).is_some()
        && selected_session_path.exists()
        && selected_session_path.parent() == current_session_path.parent()
}

async fn switch_native_session(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    selected_path: PathBuf,
    state: NativeSessionSwitchState<'_>,
    context_budget: Option<crate::NativeContextBudget>,
) {
    let NativeSessionSwitchState {
        current_session_path,
        current_session_id,
        store,
        session_log,
        turn_index,
        local_edit_index,
    } = state;
    let Some(selected_session_id) = native_session_id_from_log_path(&selected_path) else {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!(
                "native dogfood: unknown session path {}",
                selected_path.display()
            ),
        }));
        return;
    };
    let selected_store = NativeJsonlSessionStore::new(selected_path.clone());
    let load_store = selected_store.clone();
    let loaded = match tokio::task::spawn_blocking(move || load_store.load_with_warnings()).await {
        Ok(load_result) => native_session_state_from_load_result(tx, load_result),
        Err(error) => {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("native dogfood: failed to load session log: {error}"),
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
    provider: Option<&NativeProviderDogfoodConfig>,
    provider_setup_error: Option<&str>,
) {
    let session_file = Some(session_path.to_string_lossy().into_owned());
    let _ = tx.send(BackendEvent::Server(ServerEvent::Ready {
        handshake: Handshake::new(
            "yach-native-dogfood",
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
            model_id: Some(native_active_model(provider, provider_setup_error).id),
            model_name: Some(native_active_model(provider, provider_setup_error).name),
            model_provider: Some(native_active_model(provider, provider_setup_error).provider),
            session_id: Some(session_id.to_owned()),
            session_file,
            thinking_level: Some(String::from("low")),
            is_streaming: false,
            is_compacting: false,
            message_count: native_session_message_count(session_path),
            pending_message_count: Some(0),
        },
    )));
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: native_status_message(provider, provider_setup_error),
    }));
    send_native_models(tx, provider, provider_setup_error);
}

/// Switch the runner's provider model between turns. Refused while a
/// prompt is in progress (the active turn cloned the old config) and when
/// the selection names a different provider than the configured one.
fn apply_native_model_selection(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    provider: &mut Option<NativeProviderDogfoodConfig>,
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
const NATIVE_ANTHROPIC_MODEL_CHOICES: &[(&str, &str)] = &[
    ("claude-sonnet-5", "Claude Sonnet 5"),
    ("claude-opus-4-8", "Claude Opus 4.8"),
    ("claude-haiku-4-5", "Claude Haiku 4.5"),
];

fn send_native_models(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    provider: Option<&NativeProviderDogfoodConfig>,
    provider_setup_error: Option<&str>,
) {
    let active = native_active_model(provider, provider_setup_error);
    let mut models = vec![active.clone()];
    if provider.is_some_and(|provider| provider.provider_label() == "anthropic") {
        models.extend(
            NATIVE_ANTHROPIC_MODEL_CHOICES
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

fn native_active_model(
    provider: Option<&NativeProviderDogfoodConfig>,
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

fn native_status_message(
    provider: Option<&NativeProviderDogfoodConfig>,
    provider_setup_error: Option<&str>,
) -> String {
    if let Some(provider) = provider {
        let model = native_active_model(Some(provider), None);
        format!(
            "backend: native provider dogfood via {}/{}; read/search/list and exact/create edit tools available",
            model.provider, model.id
        )
    } else if let Some(setup_error) = provider_setup_error {
        format!("{setup_error}; set the provider environment and relaunch yach tui")
    } else {
        String::from(
            "backend: native dogfood; local read-only project inspection available; provider tools require native-provider",
        )
    }
}

fn handle_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    log: &mut NativeSessionLog,
    session: NativePromptSessionInput<'_>,
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
            message: format!("native dogfood: unknown session {session_id}"),
        }));
        return;
    }
    let native_session_id = NativeSessionId(session_id.clone());

    let turn_id = NativeTurnId(format!("turn-{turn_index}"));
    let user_entry_id = NativeEntryId(format!("entry-{turn_index}-user"));
    let assistant_entry_id = NativeEntryId(format!("entry-{turn_index}-assistant"));
    let response = format!("native dogfood fixture response: {prompt}");
    let fixture_outcome = native_fixture_outcome(prompt);
    let mut pending_events = Vec::new();
    push_native_session_event(
        log,
        &mut pending_events,
        NativeSessionEvent::EntryAppended {
            session_id: native_session_id.clone(),
            entry_id: user_entry_id.clone(),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
            text: prompt.to_owned(),
            provider: None,
        },
    );

    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("turn_start native dogfood"),
    }));

    if let Err(error) = append_pending_native_session_events(store, &mut pending_events) {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native dogfood: failed to persist session log: {error}"),
        }));
    }

    match fixture_outcome {
        NativeFixtureOutcome::Completed => {
            for delta in native_response_chunks(&response) {
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
                        &native_session_id,
                        &turn_id,
                        prompt_started,
                    );
                    push_native_session_event(
                        log,
                        &mut pending_events,
                        NativeSessionEvent::TurnFinished {
                            session_id: native_session_id.clone(),
                            turn_id,
                            outcome: NativeTurnOutcome::Cancelled,
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
                &native_session_id,
                &turn_id,
                prompt_started,
            );
            push_native_session_event(
                log,
                &mut pending_events,
                NativeSessionEvent::EntryAppended {
                    session_id: native_session_id.clone(),
                    entry_id: assistant_entry_id,
                    parent_entry_id: Some(user_entry_id),
                    turn_id: turn_id.clone(),
                    role: NativeRole::Assistant,
                    text: response,
                    provider: None,
                },
            );
            push_native_session_event(
                log,
                &mut pending_events,
                NativeSessionEvent::TurnFinished {
                    session_id: native_session_id.clone(),
                    turn_id,
                    outcome: NativeTurnOutcome::Completed,
                    reason: None,
                },
            );
        }
        NativeFixtureOutcome::Failed => {
            push_native_prompt_total_metric(
                log,
                &mut pending_events,
                &native_session_id,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                log,
                &mut pending_events,
                &native_session_id,
                turn_id,
                NativeTurnOutcome::Failed,
                &ProviderError::fixture_failure(),
            );
        }
        NativeFixtureOutcome::Malformed => {
            push_native_prompt_total_metric(
                log,
                &mut pending_events,
                &native_session_id,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                log,
                &mut pending_events,
                &native_session_id,
                turn_id,
                NativeTurnOutcome::Failed,
                &ProviderError::malformed_stream("native dogfood fixture malformed stream"),
            );
        }
        NativeFixtureOutcome::Cancelled => {
            push_native_prompt_total_metric(
                log,
                &mut pending_events,
                &native_session_id,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                log,
                &mut pending_events,
                &native_session_id,
                turn_id,
                NativeTurnOutcome::Cancelled,
                &ProviderError::cancelled("native dogfood fixture cancellation"),
            );
        }
    }

    let status = match append_pending_native_session_events(store, &mut pending_events) {
        Ok(()) => fixture_outcome.status_message().to_owned(),
        Err(error) => format!("native dogfood: failed to persist session log: {error}"),
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
struct NativeUnconfiguredProviderPrompt<'a> {
    prompt: &'a str,
    turn_index: u64,
    prompt_started: Instant,
    setup_error: &'a str,
}

/// Fail a submitted prompt with the provider setup error instead of producing
/// fixture output, so an unconfigured launch stays honest and recoverable.
fn handle_native_prompt_unconfigured_provider(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    log: &mut NativeSessionLog,
    session: NativePromptSessionInput<'_>,
    prompt: &NativeUnconfiguredProviderPrompt<'_>,
) {
    let session_id =
        if session.requested_session_id.is_empty() || session.requested_session_id == "default" {
            session.current_session_id.to_owned()
        } else {
            session.requested_session_id
        };
    if session_id != session.current_session_id {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native dogfood: unknown session {session_id}"),
        }));
        return;
    }
    let native_session_id = NativeSessionId(session_id);

    let turn_id = NativeTurnId(format!("turn-{}", prompt.turn_index));
    let user_entry_id = NativeEntryId(format!("entry-{}-user", prompt.turn_index));
    let mut pending_events = Vec::new();
    push_native_session_event(
        log,
        &mut pending_events,
        NativeSessionEvent::EntryAppended {
            session_id: native_session_id.clone(),
            entry_id: user_entry_id,
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
            text: prompt.prompt.to_owned(),
            provider: None,
        },
    );
    push_native_prompt_total_metric(
        log,
        &mut pending_events,
        &native_session_id,
        &turn_id,
        prompt.prompt_started,
    );
    push_native_session_event(
        log,
        &mut pending_events,
        NativeSessionEvent::TurnFinished {
            session_id: native_session_id.clone(),
            turn_id,
            outcome: NativeTurnOutcome::Failed,
            reason: Some(format!("provider_unconfigured {}", prompt.setup_error)),
        },
    );
    finish_native_prompt(
        tx,
        store,
        log,
        &mut pending_events,
        NativePromptCompletion {
            session_id: &native_session_id.0,
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
struct StartedNativePrompt {
    session_id: NativeSessionId,
    prompt: String,
    log: NativeSessionLog,
    pending_events: Vec<NativeSessionEvent>,
    turn: NativeTurnId,
    user_entry: NativeEntryId,
    assistant_entry: NativeEntryId,
    prompt_started: Instant,
}

fn start_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    log: &mut NativeSessionLog,
    session: NativePromptSessionInput<'_>,
    prompt: String,
    turn_index: u64,
    prompt_started: Instant,
) -> Option<StartedNativePrompt> {
    let session_id =
        if session.requested_session_id.is_empty() || session.requested_session_id == "default" {
            session.current_session_id.to_owned()
        } else {
            session.requested_session_id
        };
    if session_id != session.current_session_id {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native dogfood: unknown session {session_id}"),
        }));
        return None;
    }
    let native_session_id = NativeSessionId(session_id);

    let turn = NativeTurnId(format!("turn-{turn_index}"));
    let user_entry = NativeEntryId(format!("entry-{turn_index}-user"));
    let assistant_entry = NativeEntryId(format!("entry-{turn_index}-assistant"));
    let mut pending_events = Vec::new();
    push_native_session_event(
        log,
        &mut pending_events,
        NativeSessionEvent::EntryAppended {
            session_id: native_session_id.clone(),
            entry_id: user_entry.clone(),
            parent_entry_id: None,
            turn_id: turn.clone(),
            role: NativeRole::User,
            text: prompt.clone(),
            provider: None,
        },
    );

    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("turn_start native dogfood"),
    }));

    if let Err(error) = append_pending_native_session_events(store, &mut pending_events) {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native dogfood: failed to persist session log: {error}"),
        }));
        return None;
    }

    Some(StartedNativePrompt {
        session_id: native_session_id,
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
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    event: NativeSessionEvent,
) {
    log.push(event.clone());
    pending_events.push(event);
}

fn push_native_prompt_total_metric(
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    session_id: &NativeSessionId,
    turn_id: &NativeTurnId,
    prompt_started: Instant,
) {
    push_native_session_event(
        log,
        pending_events,
        native_duration_metric_event(
            session_id.clone(),
            Some(turn_id.clone()),
            "native_prompt_total",
            prompt_started.elapsed(),
        ),
    );
}

fn native_duration_metric_event(
    session_id: NativeSessionId,
    turn_id: Option<NativeTurnId>,
    name: impl Into<String>,
    duration: Duration,
) -> NativeSessionEvent {
    let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    NativeSessionEvent::MetricRecorded {
        session_id,
        turn_id,
        metric: NativeDurationMetric {
            name: name.into(),
            duration_ms,
            attributes: Vec::new(),
        },
    }
}

/// Compact one-line-per-message rendering of provider context for shape
/// assertions and diagnostics (the Codex snapshot pattern): `role:prefix`.
#[must_use]
pub fn native_provider_message_shapes(messages: &[ProviderMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|message| {
            let role = match message.role {
                NativeRole::User => "user",
                NativeRole::Assistant => "assistant",
                NativeRole::Tool => "tool",
                NativeRole::System => "system",
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
fn native_compaction_summary_message(summary: &str) -> ProviderMessage {
    ProviderMessage {
        role: NativeRole::System,
        content: format!(
            "Earlier work in this session was compacted. The summary below is \
authoritative for everything before the messages that follow it.\n\n{summary}"
        ),
    }
}

fn native_provider_messages_from_log(
    log: &NativeSessionLog,
    current_turn_id: &NativeTurnId,
) -> Vec<ProviderMessage> {
    let checkpoint = crate::compaction::newest_compaction_checkpoint(log);
    let kept_events = checkpoint.as_ref().map_or(&log.events[..], |view| {
        &log.events[view.kept_start_index.min(log.events.len())..]
    });

    let completed_turns = log
        .events
        .iter()
        .filter_map(|event| match event {
            NativeSessionEvent::TurnFinished {
                turn_id,
                outcome: NativeTurnOutcome::Completed,
                ..
            } => Some(turn_id),
            NativeSessionEvent::EntryAppended { .. }
            | NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. }
            | NativeSessionEvent::PermissionDecisionRecorded { .. }
            | NativeSessionEvent::EditTraceRecorded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. }
            | NativeSessionEvent::CompactionCheckpoint { .. } => None,
        })
        .collect::<std::collections::HashSet<_>>();

    let mut tool_context_by_request_id: std::collections::HashMap<
        String,
        (String, Option<String>),
    > = std::collections::HashMap::new();
    let mut messages = checkpoint
        .map(|view| vec![native_compaction_summary_message(view.summary)])
        .unwrap_or_default();
    messages.extend(kept_events.iter().filter_map(|event| match event {
        NativeSessionEvent::EntryAppended {
            turn_id,
            role,
            text,
            ..
        } if turn_id == current_turn_id || completed_turns.contains(turn_id) => {
            Some(ProviderMessage {
                role: *role,
                content: text.clone(),
            })
        }
        NativeSessionEvent::ToolRequestRecorded {
            turn_id,
            tool_request_id,
            tool_name,
            argument_content,
            ..
        } if turn_id == current_turn_id || completed_turns.contains(turn_id) => {
            tool_context_by_request_id.insert(
                tool_request_id.0.clone(),
                (tool_name.clone(), argument_content.clone()),
            );
            None
        }
        NativeSessionEvent::ToolExecutionFinished {
            turn_id,
            tool_request_id,
            outcome,
            reason,
            result_summary,
            result_content,
            ..
        } if turn_id == current_turn_id || completed_turns.contains(turn_id) => {
            let (tool_name, arguments) = tool_context_by_request_id
                .get(&tool_request_id.0)
                .cloned()
                .unwrap_or_else(|| (String::from("tool"), None));
            Some(native_provider_tool_activity_message(
                &tool_name,
                arguments.as_deref(),
                *outcome,
                reason.as_deref(),
                result_summary.as_ref(),
                result_content.as_deref(),
            ))
        }
        NativeSessionEvent::EntryAppended { .. }
        | NativeSessionEvent::ToolRequestRecorded { .. }
        | NativeSessionEvent::ToolExecutionFinished { .. }
        | NativeSessionEvent::TurnFinished { .. }
        | NativeSessionEvent::MetricRecorded { .. }
        | NativeSessionEvent::StaticContextIncluded { .. }
        | NativeSessionEvent::PermissionDecisionRecorded { .. }
        | NativeSessionEvent::EditTraceRecorded { .. }
        | NativeSessionEvent::EditTransactionPrepared { .. }
        | NativeSessionEvent::EditTransactionFinished { .. }
        | NativeSessionEvent::CompactionCheckpoint { .. } => None,
    }));
    messages
}

/// Tool-role transcript message describing prior tool activity so provider
/// requests keep tool evidence across turns and resume. Uses persisted
/// payloads when present; logs written before payload persistence fall back
/// to the redacted summary marked as not retained.
fn native_provider_tool_activity_message(
    tool_name: &str,
    arguments: Option<&str>,
    outcome: NativeToolOutcome,
    reason: Option<&str>,
    result_summary: Option<&NativeToolPayloadSummary>,
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
    ProviderMessage {
        role: NativeRole::Tool,
        content: serde_json::json!({
            "tool_name": tool_name,
            "arguments": arguments,
            "status": crate::rig_adapter::native_tool_outcome_label(outcome),
            "reason": reason,
            "content": content,
        })
        .to_string(),
    }
}

/// Baseline guardrails for every native-provider request, kept deliberately
/// small. Each sentence earned its place in dogfooding: without the first
/// two, cheap models assert filesystem state from stale in-conversation
/// memory and retry failed calls verbatim; without the last two, models
/// over-apply project instructions to conversational prompts (reading
/// orientation docs before answering "hello"). See
/// docs/project/records/2026-07-20-baseline-prompt-cohort-check.md.
const NATIVE_PROVIDER_BASELINE_GUIDANCE: &str = "You are a coding agent running in the yach harness. \
Files can change outside this conversation at any time: verify current state with \
a tool call before asserting or acting on remembered file contents. If a tool call \
fails because the target changed, already exists, or is missing, re-check the \
current state and adapt instead of repeating the call. Match effort to the \
request: answer greetings, small talk, and questions you can already answer \
directly, without tool calls. Project instructions in context describe how to \
carry out real work, not a checklist to run before every response.";

fn native_provider_baseline_guidance_message() -> ProviderMessage {
    ProviderMessage {
        role: NativeRole::System,
        content: String::from(NATIVE_PROVIDER_BASELINE_GUIDANCE),
    }
}

fn native_provider_messages_from_log_with_static_context(
    log: &NativeSessionLog,
    current_turn_id: &NativeTurnId,
    context: &NativeStaticContextBundle,
) -> Vec<ProviderMessage> {
    let mut messages = vec![native_provider_baseline_guidance_message()];
    messages.extend(provider_messages_from_static_context(context));
    messages.extend(native_provider_messages_from_log(log, current_turn_id));
    messages
}

fn provider_messages_from_static_context(
    context: &NativeStaticContextBundle,
) -> Vec<ProviderMessage> {
    if context.items.is_empty() {
        return Vec::new();
    }

    let system_content = render_static_context_items(context.items.iter().filter(|item| {
        matches!(
            item.placement,
            NativeStaticContextPlacement::ProjectInstructions
                | NativeStaticContextPlacement::AppendSystem
        )
    }));
    let background_content = render_static_context_items(
        context
            .items
            .iter()
            .filter(|item| item.placement == NativeStaticContextPlacement::BackgroundContext),
    );

    let mut messages = Vec::new();
    if let Some(content) = system_content {
        messages.push(ProviderMessage {
            role: NativeRole::System,
            content,
        });
    }
    if let Some(content) = background_content {
        messages.push(ProviderMessage {
            role: NativeRole::User,
            content,
        });
    }
    messages
}

fn render_static_context_items<'a>(
    items: impl Iterator<Item = &'a NativeStaticContextItem>,
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
    store: &NativeJsonlSessionStore,
    pending_events: &mut Vec<NativeSessionEvent>,
) -> std::io::Result<()> {
    store.append_events(pending_events)?;
    pending_events.clear();
    Ok(())
}

fn native_log_has_finished_turn(log: &NativeSessionLog, turn_id: &NativeTurnId) -> bool {
    log.events.iter().any(|event| {
        matches!(
            event,
            NativeSessionEvent::TurnFinished {
                turn_id: finished_turn_id,
                ..
            } if finished_turn_id == turn_id
        )
    })
}

#[derive(Debug, Clone)]
struct NativeProviderTurnRefs {
    session_id: NativeSessionId,
    turn: NativeTurnId,
    user_entry: NativeEntryId,
    assistant_entry: NativeEntryId,
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

fn native_provider_approved_tools() -> Vec<String> {
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
struct NativeProviderToolLoopPolicy {
    max_tool_rounds: Option<usize>,
    max_tool_calls_per_round: usize,
    max_total_tool_calls: usize,
    max_result_bytes_per_tool: usize,
    max_total_result_bytes: usize,
}

impl NativeProviderToolLoopPolicy {
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
    const fn as_continuation_policy(self) -> NativeToolContinuationPolicy {
        NativeToolContinuationPolicy {
            max_tool_calls: self.max_tool_calls_per_round,
            max_result_bytes: self.max_result_bytes_per_tool,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeProviderToolLoopBudget {
    policy: NativeProviderToolLoopPolicy,
    tool_rounds: usize,
    total_tool_calls: usize,
    total_result_bytes: usize,
}

impl NativeProviderToolLoopBudget {
    const fn new(policy: NativeProviderToolLoopPolicy) -> Self {
        Self {
            policy,
            tool_rounds: 0,
            total_tool_calls: 0,
            total_result_bytes: 0,
        }
    }

    fn begin_tool_round(&mut self, tool_call_count: usize) -> Result<(), NativeProviderRoundError> {
        if self
            .policy
            .max_tool_rounds
            .is_some_and(|max_tool_rounds| self.tool_rounds >= max_tool_rounds)
        {
            return Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_rounds",
            )));
        }
        if tool_call_count > self.policy.max_tool_calls_per_round {
            return Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_round_too_many_calls",
            )));
        }
        let next_total_tool_calls = self.total_tool_calls.saturating_add(tool_call_count);
        if next_total_tool_calls > self.policy.max_total_tool_calls {
            return Err(NativeProviderRoundError::ToolContinuation(String::from(
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
    ) -> Result<(), NativeProviderRoundError> {
        if byte_count > self.policy.max_result_bytes_per_tool {
            return Err(NativeProviderRoundError::ToolContinuation(format!(
                "tool_result_too_large:{tool_request_id}"
            )));
        }
        let next_total_result_bytes = self.total_result_bytes.saturating_add(byte_count);
        if next_total_result_bytes > self.policy.max_total_result_bytes {
            return Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_loop_total_result_too_large",
            )));
        }

        self.total_result_bytes = next_total_result_bytes;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeProviderRoundResult {
    text: String,
    provider_response_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeProviderRoundError {
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
struct NativeProviderFirstRound {
    text: String,
    provider_response_id: Option<String>,
    tool_calls: Vec<ProviderToolCall>,
}

fn collect_native_provider_first_round(
    events: Vec<ProviderStreamEvent>,
) -> Result<NativeProviderFirstRound, NativeProviderRoundError> {
    let mut text = String::new();
    let mut completed = false;
    let mut finish_reason = None;
    let mut provider_response_id = None;
    let mut tool_calls = Vec::new();
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { delta, .. } => text.push_str(&delta),
            ProviderStreamEvent::ToolCallCompleted { tool_call, .. } => tool_calls.push(tool_call),
            ProviderStreamEvent::Completed {
                provider_response_id: response_id,
                finish_reason: reason,
                ..
            } => {
                completed = true;
                finish_reason = reason;
                provider_response_id = response_id;
            }
            ProviderStreamEvent::Failed { error, .. } => {
                return Err(NativeProviderRoundError::Provider(error));
            }
            ProviderStreamEvent::Cancelled { reason, .. } => {
                return Err(NativeProviderRoundError::Cancelled(
                    reason.unwrap_or_else(|| String::from("native provider cancelled")),
                ));
            }
            ProviderStreamEvent::ToolCallStarted { .. }
            | ProviderStreamEvent::ToolCallDelta { .. }
            | ProviderStreamEvent::Started { .. } => {}
        }
    }
    if !completed {
        return Err(NativeProviderRoundError::StreamEndedWithoutCompletion);
    }
    if tool_calls.is_empty() && matches!(finish_reason, Some(ProviderFinishReason::ToolCalls)) {
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "provider_tool_call_incomplete",
        )));
    }
    Ok(NativeProviderFirstRound {
        text,
        provider_response_id,
        tool_calls,
    })
}

#[cfg(test)]
fn collect_native_provider_final_round(
    events: Vec<ProviderStreamEvent>,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError> {
    let first_round = collect_native_provider_first_round(events)?;
    if !first_round.tool_calls.is_empty() {
        return Err(NativeProviderRoundError::SecondRoundToolCall);
    }
    Ok(NativeProviderRoundResult {
        text: first_round.text,
        provider_response_id: first_round.provider_response_id,
    })
}

#[cfg(test)]
struct NativeProviderToolRoundContext<'a, Executor>
where
    Executor: NativeToolExecutor,
{
    model: ProviderModel,
    log: &'a mut NativeSessionLog,
    pending_events: &'a mut Vec<NativeSessionEvent>,
    turn_id: &'a NativeTurnId,
    project_root: Option<NativeResourceRoot>,
    static_context_cwd: Option<PathBuf>,
    extension_static_context_files: Vec<NativeExtensionStaticContextFile>,
    tool_event_store: Option<&'a NativeJsonlSessionStore>,
    registry: &'a NativeToolRegistry,
    permission_policy: &'a NativeToolPermissionPolicy,
    executor: &'a Executor,
    routable_tool_names: &'a [&'a str],
    require_project_root_for_tools: bool,
}

#[cfg(test)]
async fn run_native_provider_one_tool_round_with_registry<Provider, Executor>(
    requester: &mut Provider,
    context: NativeProviderToolRoundContext<'_, Executor>,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError>
where
    Provider: ProviderRequester,
    Executor: NativeToolExecutor,
{
    let NativeProviderToolRoundContext {
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
                NativeProviderRoundError::ToolContinuation(
                    native_provider_tool_advertising_error_label(&error),
                )
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
                NativeStaticContextPolicy::conservative(),
                extension_static_context_files,
            )
        })
        .unwrap_or_default();
    if !static_context_assembly.bundle.items.is_empty()
        || !static_context_assembly.omissions.is_empty()
    {
        log.record_static_context_included(
            NativeSessionId(String::from("default")),
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
            return Err(NativeProviderRoundError::ToolContinuation(String::from(
                "static_context_persist_failed",
            )));
        }
    }
    let initial_request = ProviderRequest {
        turn_id: turn_id.clone(),
        model,
        messages: native_provider_messages_from_log_with_static_context(
            log,
            turn_id,
            &static_context_assembly.bundle,
        ),
        extensions,
    };
    let first_events = requester
        .request(initial_request.clone())
        .await
        .map_err(NativeProviderRoundError::Provider)?;
    let first_round = collect_native_provider_first_round(first_events)?;
    if first_round.tool_calls.is_empty() {
        return Ok(NativeProviderRoundResult {
            text: first_round.text,
            provider_response_id: first_round.provider_response_id,
        });
    }
    if require_project_root_for_tools && project_root.is_none() {
        return Err(NativeProviderRoundError::ProjectRootUnavailable);
    }
    let tool_event_start = log.events.len();
    let tool_results = match (NativeToolContinuationWorkflow {
        registry,
        permission_policy,
        executor,
        continuation_policy: NativeToolContinuationPolicy::fixture_default(),
    })
    .build_provider_tool_results(
        log,
        &NativeToolContinuationContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: turn_id.clone(),
        },
        first_round.tool_calls,
    ) {
        Ok(results) => results,
        Err(error) => {
            pending_events.extend(log.events[tool_event_start..].iter().cloned());
            return Err(NativeProviderRoundError::ToolContinuation(
                native_tool_round_error_label(&error),
            ));
        }
    };
    pending_events.extend(log.events[tool_event_start..].iter().cloned());
    if let Some(store) = tool_event_store
        && append_pending_native_session_events(store, pending_events).is_err()
    {
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
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
            NativeToolContinuationPolicy::fixture_default().max_result_bytes,
        ),
    )
    .map_err(|error| {
        NativeProviderRoundError::ToolContinuation(native_provider_mapping_error_label(&error))
    })?;
    let continuation_request =
        crate::rig_adapter::project_provider_continuation_request(submission);
    let continuation_events = requester
        .request(continuation_request)
        .await
        .map_err(NativeProviderRoundError::Provider)?;
    collect_native_provider_final_round(continuation_events)
}

#[cfg(test)]
async fn run_native_provider_one_readonly_tool_round(
    requester: &mut impl ProviderRequester,
    model: ProviderModel,
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    turn_id: &NativeTurnId,
    project_context: Option<NativeLaunchProjectContext>,
    tool_event_store: Option<&NativeJsonlSessionStore>,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError> {
    let registry = NativeToolRegistry::with_project_read_only_tools();
    let permission_policy =
        NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
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
        NativeProviderToolRoundContext {
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

struct NativeProviderBufferedEventSink<'a> {
    store: Option<&'a NativeJsonlSessionStore>,
    events: Mutex<Vec<NativeSessionEvent>>,
}

impl<'a> NativeProviderBufferedEventSink<'a> {
    fn new(store: Option<&'a NativeJsonlSessionStore>) -> Self {
        Self {
            store,
            events: Mutex::new(Vec::new()),
        }
    }

    fn drain_into(
        &self,
        log: &mut NativeSessionLog,
        pending_events: &mut Vec<NativeSessionEvent>,
    ) -> Result<(), NativeProviderRoundError> {
        let mut events = self.events.lock().map_err(|_| {
            NativeProviderRoundError::ToolContinuation(String::from("tool_event_buffer_poisoned"))
        })?;
        log.events.extend(events.iter().cloned());
        if self.store.is_none() {
            pending_events.extend(events.iter().cloned());
        }
        events.clear();
        Ok(())
    }
}

impl NativeSessionEventSink for NativeProviderBufferedEventSink<'_> {
    fn append_event(&self, event: &NativeSessionEvent) -> std::io::Result<()> {
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

    fn append_events(&self, events: &[NativeSessionEvent]) -> std::io::Result<()> {
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

struct NativeProviderAgentToolRound<'a> {
    session_id: &'a NativeSessionId,
    model: ProviderModel,
    log: &'a mut NativeSessionLog,
    pending_events: &'a mut Vec<NativeSessionEvent>,
    turn_id: &'a NativeTurnId,
    project_context: Option<NativeLaunchProjectContext>,
    extension_static_context_files: Vec<NativeExtensionStaticContextFile>,
    extension_activation_snapshot: crate::ExtensionActivationSnapshot,
    tool_event_store: Option<&'a NativeJsonlSessionStore>,
    review_tx: mpsc::UnboundedSender<BackendEvent>,
    review_decisions: AgentEditDecisionReceiver,
    /// Compaction accounting inputs (`usable = context_window −
    /// max_output_tokens − reserve`).
    context_window: u64,
    max_output_tokens: u64,
}

struct NativeProviderAgentToolBatch<'a> {
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    project_root: NativeResourceRoot,
    shell_policy: crate::NativeShellPolicy,
    registry: &'a NativeToolRegistry,
    resolved_catalog: &'a ResolvedNativeToolCatalog,
    permission_policy: &'a NativeToolPermissionPolicy,
    read_only_executor: &'a ProjectReadOnlyToolExecutor,
    extension_executor: Option<&'a crate::ExtensionToolExecutorRouter>,
    edit_access: &'a mut NativeEditAccess,
    edit_sink: &'a NativeProviderBufferedEventSink<'a>,
    review_tx: mpsc::UnboundedSender<BackendEvent>,
    review_decisions: &'a mut AgentEditDecisionReceiver,
    tool_event_store: Option<&'a NativeJsonlSessionStore>,
    budget: &'a mut NativeProviderToolLoopBudget,
    tool_round_index: usize,
    edit_traces: &'a mut Vec<ProviderContinuationEditTrace>,
    log: &'a mut NativeSessionLog,
    pending_events: &'a mut Vec<NativeSessionEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderContinuationEditTrace {
    trace_id: NativeEditTraceId,
    tool_name: String,
    tool_request_id: NativeToolRequestId,
    provider_call_id: Option<String>,
    preview_id: Option<NativeEditPreviewId>,
    permission_decision_id: Option<NativePermissionDecisionId>,
}

#[derive(Clone, Copy)]
struct ProviderContinuationTraceInput<'a> {
    session_id: &'a NativeSessionId,
    turn_id: &'a NativeTurnId,
    edit_traces: &'a [ProviderContinuationEditTrace],
    started: Instant,
    outcome: NativeEditTraceOutcome,
    reason_label: Option<&'a str>,
}

async fn run_native_provider_one_agent_tool_round(
    requester: &mut impl ProviderRequester,
    round: NativeProviderAgentToolRound<'_>,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError> {
    let NativeProviderAgentToolRound {
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
        NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
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
                NativeProviderRoundError::ToolContinuation(
                    native_provider_tool_advertising_error_label(&error),
                )
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
                NativeStaticContextPolicy::conservative(),
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
            return Err(NativeProviderRoundError::ToolContinuation(String::from(
                "static_context_persist_failed",
            )));
        }
    }

    // Auto-compaction trigger, checked before the turn's first request.
    // Design: docs/superpowers/specs/2026-07-20-context-compaction-design.md.
    let compaction_config = crate::NativeCompactionConfig::load_for_project(
        project_root
            .as_ref()
            .map(NativeResourceRoot::canonical_path),
    );
    let compaction_budget = NativeCompactionBudget {
        context_window,
        max_output_tokens,
        config: &compaction_config,
    };
    if compaction_config.enabled {
        let estimate = native_estimate_provider_messages_tokens(
            &native_provider_messages_from_log_with_static_context(
                log,
                turn_id,
                &static_context_assembly.bundle,
            ),
        );
        if compaction_budget.over_threshold(estimate) {
            let compacted = native_run_compaction(
                requester,
                NativeCompactionRun {
                    session_id,
                    turn_id,
                    model: &model,
                    config: &compaction_config,
                    reason: crate::NativeCompactionReason::Threshold,
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
                let refilled = native_estimate_provider_messages_tokens(
                    &native_provider_messages_from_log_with_static_context(
                        log,
                        turn_id,
                        &static_context_assembly.bundle,
                    ),
                );
                // Thrash guard: compaction succeeded but the kept tail alone
                // still exceeds the threshold; stop instead of looping
                // summary calls.
                if compaction_budget.over_threshold(refilled) {
                    let _ = review_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from(
                            "context refilled immediately after compaction; narrow the \
request or start a fresh session",
                        ),
                    }));
                    return Err(NativeProviderRoundError::ToolContinuation(String::from(
                        "context_refilled_after_compaction",
                    )));
                }
            }
        }
    }

    let initial_request = ProviderRequest {
        turn_id: turn_id.clone(),
        model,
        messages: native_provider_messages_from_log_with_static_context(
            log,
            turn_id,
            &static_context_assembly.bundle,
        ),
        extensions,
    };
    let read_only_executor = project_root
        .as_ref()
        .map(|project_root| ProjectReadOnlyToolExecutor::new(project_root.clone()));
    let shell_policy = crate::NativeShellPolicy::load_for_project(
        project_root
            .as_ref()
            .map(NativeResourceRoot::canonical_path),
    );
    let mut edit_access = NativeEditAccess::default();
    let edit_sink = NativeProviderBufferedEventSink::new(tool_event_store);
    let mut provider_continuation_edit_traces = Vec::new();
    let loop_policy = NativeProviderToolLoopPolicy::agent_default();
    let mut loop_budget = NativeProviderToolLoopBudget::new(loop_policy);
    let mut next_request = initial_request.clone();
    let mut prior_messages = initial_request.messages.clone();
    let mut pending_continuation_trace: Option<(Instant, Vec<ProviderContinuationEditTrace>)> =
        None;
    let mut is_initial_request = true;
    let mut overflow_compaction_used = false;
    loop {
        let provider_events = match requester.request(next_request.clone()).await {
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
                    let compacted = native_run_compaction(
                        requester,
                        NativeCompactionRun {
                            session_id,
                            turn_id,
                            model: &initial_request.model,
                            config: &compaction_config,
                            reason: crate::NativeCompactionReason::Overflow,
                            tokens_before: native_estimate_provider_messages_tokens(
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
                        let messages = native_provider_messages_from_log_with_static_context(
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
                            outcome: NativeEditTraceOutcome::Failed,
                            reason_label: Some("provider_request_failed"),
                        },
                    );
                }
                return Err(NativeProviderRoundError::Provider(error));
            }
        };
        let round = match collect_native_provider_first_round(provider_events) {
            Ok(round) => round,
            Err(error) => {
                if let Some((started, edit_traces)) = pending_continuation_trace.take() {
                    let reason = native_provider_round_error_label(&error);
                    record_provider_continuation_trace_records(
                        log,
                        pending_events,
                        tool_event_store,
                        ProviderContinuationTraceInput {
                            session_id,
                            turn_id,
                            edit_traces: &edit_traces,
                            started,
                            outcome: NativeEditTraceOutcome::Failed,
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
                    outcome: NativeEditTraceOutcome::Completed,
                    reason_label: None,
                },
            );
        }
        if round.tool_calls.is_empty() {
            return Ok(NativeProviderRoundResult {
                text: round.text,
                provider_response_id: round.provider_response_id,
            });
        }

        let Some(project_root) = project_root.clone() else {
            return Err(NativeProviderRoundError::ProjectRootUnavailable);
        };
        let Some(read_only_executor) = read_only_executor.as_ref() else {
            return Err(NativeProviderRoundError::ProjectRootUnavailable);
        };
        let tool_round_index = loop_budget.tool_rounds + 1;
        let edit_trace_start = provider_continuation_edit_traces.len();
        let tool_results = execute_native_provider_agent_tool_batch(
            NativeProviderAgentToolBatch {
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
        next_request = match build_native_provider_tool_continuation_request(
            &initial_request,
            &prior_messages,
            tool_results,
        ) {
            Ok(request) => request,
            Err(NativeProviderRoundError::ToolContinuation(reason)) => {
                record_provider_continuation_trace_records(
                    log,
                    pending_events,
                    tool_event_store,
                    ProviderContinuationTraceInput {
                        session_id,
                        turn_id,
                        edit_traces: &continuation_edit_traces,
                        started: provider_continuation_started,
                        outcome: NativeEditTraceOutcome::Failed,
                        reason_label: Some(reason.as_str()),
                    },
                );
                return Err(NativeProviderRoundError::ToolContinuation(reason));
            }
            Err(error) => return Err(error),
        };
        prior_messages.clone_from(&next_request.messages);
        pending_continuation_trace =
            Some((provider_continuation_started, continuation_edit_traces));
        is_initial_request = false;
    }
}

fn build_native_provider_tool_continuation_request(
    initial_request: &ProviderRequest,
    prior_messages: &[ProviderMessage],
    tool_results: Vec<NativeProviderToolResult>,
) -> Result<ProviderRequest, NativeProviderRoundError> {
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
            NativeProviderToolLoopPolicy::agent_default().max_result_bytes_per_tool,
        ),
    )
    .map_err(|error| {
        NativeProviderRoundError::ToolContinuation(native_provider_mapping_error_label(&error))
    })?;
    Ok(crate::rig_adapter::project_provider_continuation_request(
        submission,
    ))
}

/// Compaction accounting: `usable = context_window − max_output_tokens −
/// reserve`; the trigger fires above the configured percent of usable.
struct NativeCompactionBudget<'a> {
    context_window: u64,
    max_output_tokens: u64,
    config: &'a crate::NativeCompactionConfig,
}

impl NativeCompactionBudget<'_> {
    fn threshold_tokens(&self) -> u64 {
        let usable = crate::NativeContextBudget {
            context_window: self.context_window,
            max_output_tokens: self.max_output_tokens,
            reserve_tokens: self.config.reserve_tokens,
        }
        .usable_tokens();
        usable.saturating_mul(u64::from(self.config.auto_threshold_percent_clamped())) / 100
    }

    fn over_threshold(&self, estimated_tokens: u64) -> bool {
        estimated_tokens > self.threshold_tokens()
    }
}

/// Context-meter budget from the active provider config plus the
/// compaction reserve; `None` without a configured provider.
fn native_context_budget(
    provider: Option<&NativeProviderDogfoodConfig>,
    project_root: Option<&Path>,
) -> Option<crate::NativeContextBudget> {
    let provider = provider?;
    let config = crate::NativeCompactionConfig::load_for_project(project_root);
    Some(crate::NativeContextBudget {
        context_window: provider.adapter.context_window,
        max_output_tokens: provider.adapter.max_tokens,
        reserve_tokens: config.reserve_tokens,
    })
}

fn native_estimate_provider_messages_tokens(messages: &[ProviderMessage]) -> u64 {
    messages
        .iter()
        .map(|message| crate::estimate_text_tokens(&message.content))
        .sum()
}

struct NativeCompactionRun<'a> {
    session_id: &'a NativeSessionId,
    turn_id: &'a NativeTurnId,
    model: &'a ProviderModel,
    config: &'a crate::NativeCompactionConfig,
    reason: crate::NativeCompactionReason,
    tokens_before: u64,
    focus_instructions: Option<String>,
    log: &'a mut NativeSessionLog,
    pending_events: &'a mut Vec<NativeSessionEvent>,
    tool_event_store: Option<&'a NativeJsonlSessionStore>,
    review_tx: &'a mpsc::UnboundedSender<BackendEvent>,
}

/// Run one compaction: select the cut, produce the summary via the
/// provider, and append the checkpoint. Returns false (leaving the session
/// uncompacted, with a visible status) when there is nothing to fold or
/// the summary call fails; the caller decides what that means for the
/// turn. Only checkpoint persistence failures are hard errors.
async fn native_run_compaction<Requester>(
    requester: &mut Requester,
    run: NativeCompactionRun<'_>,
) -> Result<bool, NativeProviderRoundError>
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
        messages: vec![ProviderMessage {
            role: NativeRole::User,
            content: crate::build_summary_prompt(&preparation),
        }],
        extensions: Vec::new(),
    };
    let summary = match requester.request(summary_request).await {
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
        .filter(|event| matches!(event, NativeSessionEvent::CompactionCheckpoint { .. }))
        .count()
        + 1;
    push_native_session_event(
        run.log,
        run.pending_events,
        NativeSessionEvent::CompactionCheckpoint {
            session_id: run.session_id.clone(),
            turn_id: run.turn_id.clone(),
            checkpoint_id: crate::NativeCompactionCheckpointId(format!(
                "compaction-{checkpoint_index}"
            )),
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
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
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

fn native_provider_tool_batch_result_budget_failure(
    error: NativeProviderRoundError,
) -> (NativeProviderRoundError, String) {
    match error {
        NativeProviderRoundError::ToolContinuation(label)
            if label.starts_with("tool_result_too_large:") =>
        {
            let reason = String::from("tool_round_result_too_large");
            (
                NativeProviderRoundError::ToolContinuation(reason.clone()),
                reason,
            )
        }
        NativeProviderRoundError::ToolContinuation(label) => (
            NativeProviderRoundError::ToolContinuation(label.clone()),
            label,
        ),
        other => (other, String::from("tool_round_result_too_large")),
    }
}

/// Failed-but-continuable tool result for sensitive-file denies, following
/// the recoverable edit-failure shape: categorical error plus explicit
/// next-step guidance.
fn native_sensitive_denied_tool_result(
    request: &PendingNativeToolRequest,
) -> NativeProviderToolResult {
    let content = serde_json::json!({
        "outcome": "failed",
        "tool_request_id": request.request_id,
        "error": "sensitive_path_denied",
        "guidance": "This path matches the sensitive-file deny list, so its contents are \
    not available to tools. If access is intended, ask the user to allow the path under \
    files.allow in .yach/config.json and retry.",
    })
    .to_string();
    NativeProviderToolResult {
        tool_request_id: request.request_id.clone(),
        provider_call_id: request.provider_call_id.clone(),
        status: NativeToolOutcome::Failed,
        byte_count: content.len(),
        content,
        redacted: true,
        truncated: false,
        reason: Some(String::from("sensitive_path_denied")),
    }
}

/// Categorical reason + guidance for read-only tool failures the model can
/// recover from. `None` means harness-integrity failure: abort the turn.
fn native_recoverable_readonly_failure(
    error: &crate::NativeToolExecutionError,
) -> Option<(&'static str, &'static str)> {
    match error {
        crate::NativeToolExecutionError::ResourceReadTooLarge => Some((
            "resource_read_too_large",
            "The file exceeds the read_text_file size limit (32KB). Use the bash tool to \
sample it instead (for example `head -c 20000 <path>`, `wc -l <path>`, or `sed -n '1,50p' \
<path>`), or read a smaller file.",
        )),
        crate::NativeToolExecutionError::ResourceReadNotUtf8 => Some((
            "resource_read_not_utf8",
            "The file is not valid UTF-8 text. Use the bash tool to inspect it (for example \
`file <path>` or `head -c 200 <path> | xxd`), or skip it.",
        )),
        crate::NativeToolExecutionError::ResourcePath { error } => match error {
            crate::NativeResourcePathError::Missing => Some((
                "path_missing",
                "The path does not exist. Use list_project_paths to inspect the project layout.",
            )),
            crate::NativeResourcePathError::EscapesRoot => Some((
                "path_outside_project",
                "Paths must stay inside the project root. Use project-relative paths.",
            )),
            crate::NativeResourcePathError::ExpectedFile => Some((
                "expected_file",
                "The path is a directory. Use list_project_paths to browse it, or name a file.",
            )),
            crate::NativeResourcePathError::ExpectedDirectory => Some((
                "expected_directory",
                "The path is a file, not a directory. Use read_text_file for file contents.",
            )),
            crate::NativeResourcePathError::RootUnavailable
            | crate::NativeResourcePathError::SensitiveDenied => None,
        },
        crate::NativeToolExecutionError::UnknownTool
        | crate::NativeToolExecutionError::PermissionDenied
        | crate::NativeToolExecutionError::UnsupportedTool
        | crate::NativeToolExecutionError::MalformedResult
        | crate::NativeToolExecutionError::ExtensionHost { .. } => None,
    }
}

fn execute_native_provider_readonly_tool_request(
    batch: &mut NativeProviderAgentToolBatch<'_>,
    request: PendingNativeToolRequest,
) -> Result<NativeProviderToolResult, NativeProviderRoundError> {
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
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_round_validation_failed",
        )));
    };
    let execution = match batch
        .read_only_executor
        .execute(batch.registry, &request, &validation)
    {
        Ok(execution) => execution,
        Err(crate::NativeToolExecutionError::ResourcePath {
            error: crate::NativeResourcePathError::SensitiveDenied,
        }) => {
            // Recoverable: the model asked for a path on the sensitive-file
            // deny list. Fail the tool call with guidance and continue the
            // loop instead of aborting the turn.
            let result = native_sensitive_denied_tool_result(&request);
            batch.log.push(NativeSessionEvent::ToolExecutionFinished {
                session_id: batch.session_id.clone(),
                turn_id: batch.turn_id.clone(),
                tool_request_id: NativeToolRequestId(request.request_id.clone()),
                outcome: NativeToolOutcome::Failed,
                reason: Some(String::from("sensitive_path_denied")),
                result_summary: Some(NativeToolPayloadSummary {
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
            if let Some((reason, guidance)) = native_recoverable_readonly_failure(&error) {
                let result = native_failed_tool_result(&request, reason, guidance);
                batch.log.push(NativeSessionEvent::ToolExecutionFinished {
                    session_id: batch.session_id.clone(),
                    turn_id: batch.turn_id.clone(),
                    tool_request_id: NativeToolRequestId(request.request_id.clone()),
                    outcome: NativeToolOutcome::Failed,
                    reason: Some(String::from(reason)),
                    result_summary: Some(NativeToolPayloadSummary {
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
            batch.log.push(NativeSessionEvent::ToolExecutionFinished {
                session_id: batch.session_id.clone(),
                turn_id: batch.turn_id.clone(),
                tool_request_id: NativeToolRequestId(request.request_id.clone()),
                outcome: NativeToolOutcome::Failed,
                reason: Some(String::from("tool_round_execution_failed")),
                result_summary: None,
                result_content: None,
            });
            batch
                .pending_events
                .extend(batch.log.events[tool_event_start..].iter().cloned());
            return Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_round_execution_failed",
            )));
        }
    };
    if let Err(error) = batch
        .budget
        .record_tool_result(&request.request_id, execution.byte_count)
    {
        let (error, reason) = native_provider_tool_batch_result_budget_failure(error);
        batch.log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: NativeToolRequestId(request.request_id.clone()),
            outcome: NativeToolOutcome::Failed,
            reason: Some(reason),
            result_summary: None,
            result_content: None,
        });
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(error);
    }
    let result_summary =
        native_provider_readonly_tool_result_summary(&request.tool_name, &execution);
    batch.log.push(NativeSessionEvent::ToolExecutionFinished {
        session_id: batch.session_id.clone(),
        turn_id: batch.turn_id.clone(),
        tool_request_id: NativeToolRequestId(request.request_id.clone()),
        outcome: NativeToolOutcome::Completed,
        reason: None,
        result_summary: Some(result_summary),
        result_content: Some(execution.summary.clone()),
    });
    batch
        .pending_events
        .extend(batch.log.events[tool_event_start..].iter().cloned());
    Ok(NativeProviderToolResult {
        tool_request_id: request.request_id,
        provider_call_id: request.provider_call_id,
        status: NativeToolOutcome::Completed,
        content: execution.summary,
        byte_count: execution.byte_count,
        redacted: execution.redacted,
        truncated: execution.truncated,
        reason: None,
    })
}

fn execute_native_provider_extension_tool_request(
    batch: &mut NativeProviderAgentToolBatch<'_>,
    request: PendingNativeToolRequest,
    implementation_name: &str,
) -> Result<NativeProviderToolResult, NativeProviderRoundError> {
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
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_round_validation_failed",
        )));
    };
    let Some(extension_executor) = batch.extension_executor else {
        batch.log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: NativeToolRequestId(request.request_id.clone()),
            outcome: NativeToolOutcome::Failed,
            reason: Some(String::from("tool_round_execution_failed")),
            result_summary: None,
            result_content: None,
        });
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_round_execution_failed",
        )));
    };
    let mut implementation_request = request.clone();
    implementation_request.tool_name = String::from(implementation_name);
    let Ok(execution) =
        extension_executor.execute(batch.registry, &implementation_request, &validation)
    else {
        batch.log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: NativeToolRequestId(request.request_id.clone()),
            outcome: NativeToolOutcome::Failed,
            reason: Some(String::from("tool_round_execution_failed")),
            result_summary: None,
            result_content: None,
        });
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_round_execution_failed",
        )));
    };
    if let Err(error) = batch
        .budget
        .record_tool_result(&request.request_id, execution.byte_count)
    {
        let (error, reason) = native_provider_tool_batch_result_budget_failure(error);
        batch.log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: NativeToolRequestId(request.request_id.clone()),
            outcome: NativeToolOutcome::Failed,
            reason: Some(reason),
            result_summary: None,
            result_content: None,
        });
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(error);
    }
    let result_summary =
        native_provider_readonly_tool_result_summary(&request.tool_name, &execution);
    batch.log.push(NativeSessionEvent::ToolExecutionFinished {
        session_id: batch.session_id.clone(),
        turn_id: batch.turn_id.clone(),
        tool_request_id: NativeToolRequestId(request.request_id.clone()),
        outcome: NativeToolOutcome::Completed,
        reason: None,
        result_summary: Some(result_summary),
        result_content: Some(execution.summary.clone()),
    });
    batch
        .pending_events
        .extend(batch.log.events[tool_event_start..].iter().cloned());
    Ok(NativeProviderToolResult {
        tool_request_id: request.request_id,
        provider_call_id: request.provider_call_id,
        status: NativeToolOutcome::Completed,
        content: execution.summary,
        byte_count: execution.byte_count,
        redacted: execution.redacted,
        truncated: execution.truncated,
        reason: None,
    })
}

async fn execute_native_provider_edit_tool_request(
    batch: &mut NativeProviderAgentToolBatch<'_>,
    request: PendingNativeToolRequest,
) -> Result<NativeProviderToolResult, NativeProviderRoundError> {
    if let Some(store) = batch.tool_event_store
        && append_pending_native_session_events(store, batch.pending_events).is_err()
    {
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_event_persist_failed",
        )));
    }
    let tool_name = request.tool_name.clone();
    let prepared = prepare_agent_edit_tool_request(
        batch.registry,
        &batch.project_root,
        batch.edit_access,
        batch.edit_sink,
        NativeAgentEditToolContext {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            permission_policy: NativePermissionPolicy::default_local_edit(),
            edit_policy: NativeEditPolicy::conservative(),
        },
        request,
    );
    batch
        .edit_sink
        .drain_into(batch.log, batch.pending_events)?;
    let prepared = prepared.map_err(|error| {
        NativeProviderRoundError::ToolContinuation(native_tool_round_error_label(&error))
    })?;
    let result = match prepared {
        NativeAgentEditToolPrepared::Completed { trace_id, result }
        | NativeAgentEditToolPrepared::Failed { trace_id, result } => {
            batch.edit_traces.push(ProviderContinuationEditTrace {
                trace_id,
                tool_name,
                tool_request_id: NativeToolRequestId(result.tool_request_id.clone()),
                provider_call_id: result.provider_call_id.clone(),
                preview_id: None,
                permission_decision_id: None,
            });
            result
        }
        NativeAgentEditToolPrepared::Denied { result, .. } => {
            return Err(NativeProviderRoundError::ToolExecutionDenied {
                tool_request_id: result.tool_request_id,
                tool_name,
                reason: result.reason.unwrap_or_else(|| String::from("denied")),
            });
        }
        NativeAgentEditToolPrepared::NeedsUserReview {
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
                tool_request_id: NativeToolRequestId(pending.request_id.clone()),
                provider_call_id: Some(pending.provider_call_id.clone()),
                preview_id: Some(pending.preview_id.clone()),
                permission_decision_id: Some(pending.permission_decision_id.clone()),
            };
            let preview_summary = native_local_edit_preview_summary(preview, path, operation);
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
                return Err(NativeProviderRoundError::Cancelled(String::from(
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
                    NativeEditTraceOutcome::Completed,
                    None,
                ),
                Ok(LocalEditDecision::Reject) => record_review_wait_trace(
                    batch.edit_sink,
                    &pending,
                    review_wait_started,
                    NativeEditTraceOutcome::Rejected,
                    None,
                ),
                Err(error) => record_review_wait_trace(
                    batch.edit_sink,
                    &pending,
                    review_wait_started,
                    native_review_wait_error_outcome(error),
                    Some(native_provider_round_error_label(error)),
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
                NativeProviderRoundError::ToolContinuation(native_tool_round_error_label(&error))
            })?;
            batch.edit_traces.push(continuation_trace);
            result
        }
    };
    batch
        .budget
        .record_tool_result(&result.tool_request_id, result.byte_count)
        .map_err(|error| native_provider_tool_batch_result_budget_failure(error).0)?;
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
fn native_failed_tool_result(
    request: &PendingNativeToolRequest,
    reason: &str,
    guidance: &str,
) -> NativeProviderToolResult {
    let content = serde_json::json!({
        "outcome": "failed",
        "tool_request_id": request.request_id,
        "error": reason,
        "guidance": guidance,
    })
    .to_string();
    NativeProviderToolResult {
        tool_request_id: request.request_id.clone(),
        provider_call_id: request.provider_call_id.clone(),
        status: NativeToolOutcome::Failed,
        byte_count: content.len(),
        content,
        redacted: true,
        truncated: false,
        reason: Some(reason.to_owned()),
    }
}

fn record_native_bash_finished_event(
    batch: &mut NativeProviderAgentToolBatch<'_>,
    request_id: &str,
    outcome: NativeToolOutcome,
    reason: Option<String>,
    result: &NativeProviderToolResult,
) {
    batch.log.push(NativeSessionEvent::ToolExecutionFinished {
        session_id: batch.session_id.clone(),
        turn_id: batch.turn_id.clone(),
        tool_request_id: NativeToolRequestId(request_id.to_owned()),
        outcome,
        reason,
        result_summary: Some(NativeToolPayloadSummary {
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
) -> Result<LocalEditDecision, NativeProviderRoundError> {
    let Some(decision) = review_decisions.recv().await else {
        return Err(NativeProviderRoundError::Cancelled(String::from(
            "tool review decision channel closed",
        )));
    };
    if decision.request_id == request_id
        && decision.preview_id == review_id
        && decision.permission_decision_id == permission_decision_id
    {
        return Ok(decision.decision);
    }
    Err(NativeProviderRoundError::ToolContinuation(String::from(
        "stale_tool_review_decision",
    )))
}

async fn execute_native_provider_bash_tool_request(
    batch: &mut NativeProviderAgentToolBatch<'_>,
    request: PendingNativeToolRequest,
) -> Result<NativeProviderToolResult, NativeProviderRoundError> {
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
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
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

    let finish_failed = |batch: &mut NativeProviderAgentToolBatch<'_>,
                         reason: &str,
                         guidance: &str|
     -> Result<NativeProviderToolResult, NativeProviderRoundError> {
        let result = native_failed_tool_result(&request, reason, guidance);
        record_native_bash_finished_event(
            batch,
            &request.request_id,
            NativeToolOutcome::Failed,
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
    let approved_by = if shell_policy.auto_run_eligible(&command) {
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
            return Err(NativeProviderRoundError::Cancelled(String::from(
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
    let run =
        crate::NativeCommandExecutor::run(&crate::HostCommandExecutor, prepared, Some(chunk_tx));
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

    let content = serde_json::json!({
        "outcome": "completed",
        "tool_request_id": request.request_id,
        "approved_by": approved_by,
        "exit_code": outcome.exit_code,
        "duration_ms": outcome.duration_ms,
        "output": outcome.output,
        "output_bytes_total": outcome.output_bytes_total,
        "truncated": outcome.truncated,
    })
    .to_string();
    let result = NativeProviderToolResult {
        tool_request_id: request.request_id.clone(),
        provider_call_id: request.provider_call_id.clone(),
        status: NativeToolOutcome::Completed,
        byte_count: content.len(),
        content,
        redacted: true,
        truncated: outcome.truncated,
        reason: None,
    };
    record_native_bash_finished_event(
        batch,
        &request.request_id,
        NativeToolOutcome::Completed,
        None,
        &result,
    );
    batch
        .pending_events
        .extend(batch.log.events[tool_event_start..].iter().cloned());
    if let Some(store) = batch.tool_event_store
        && append_pending_native_session_events(store, batch.pending_events).is_err()
    {
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_event_persist_failed",
        )));
    }
    batch
        .budget
        .record_tool_result(&request.request_id, result.byte_count)
        .map_err(|error| native_provider_tool_batch_result_budget_failure(error).0)?;
    Ok(result)
}

async fn execute_native_provider_agent_tool_batch(
    mut batch: NativeProviderAgentToolBatch<'_>,
    tool_calls: Vec<ProviderToolCall>,
) -> Result<Vec<NativeProviderToolResult>, NativeProviderRoundError> {
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
                        matches!(definition.owner, crate::NativeToolOwner::Extension { .. })
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
                return Err(NativeProviderRoundError::ToolContinuation(String::from(
                    "tool_round_validation_failed",
                )));
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let reason = native_provider_round_error_label(&error);
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
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_event_persist_failed",
        )));
    }
    Ok(tool_results)
}

fn emit_native_provider_tool_call_started(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    request: &PendingNativeToolRequest,
) -> Result<(), NativeProviderRoundError> {
    tx.send(BackendEvent::Server(ServerEvent::ToolCallStarted {
        tool_call_id: Some(request.request_id.clone()),
        tool_name: request.tool_name.clone(),
        preview: native_provider_tool_call_preview(&request.tool_name, &request.arguments),
    }))
    .map_err(|_| {
        NativeProviderRoundError::Cancelled(String::from(
            "ui receiver dropped during tool progress",
        ))
    })
}

fn emit_native_provider_tool_call_finished(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    tool_name: &str,
    result: &NativeProviderToolResult,
) -> Result<(), NativeProviderRoundError> {
    let is_error = result.status != NativeToolOutcome::Completed;
    emit_native_provider_tool_call_result(
        tx,
        Some(result.tool_request_id.clone()),
        tool_name.to_owned(),
        native_provider_tool_progress_output(tool_name, result),
        is_error,
    )
}

fn emit_native_provider_tool_call_error(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    tool_call_id: Option<String>,
    tool_name: String,
    reason: &str,
) -> Result<(), NativeProviderRoundError> {
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
) -> Result<(), NativeProviderRoundError> {
    tx.send(BackendEvent::Server(ServerEvent::ToolCallFinished(
        ToolResult {
            tool_call_id,
            tool_name,
            output,
            is_error,
        },
    )))
    .map_err(|_| {
        NativeProviderRoundError::Cancelled(String::from(
            "ui receiver dropped during tool progress",
        ))
    })
}

fn native_provider_tool_progress_output(
    tool_name: &str,
    result: &NativeProviderToolResult,
) -> String {
    native_tool_result_display(
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
pub(super) fn native_tool_result_display(
    tool_name: &str,
    status: NativeToolOutcome,
    content: Option<&str>,
    byte_count: usize,
    truncated: bool,
    reason: Option<&str>,
) -> String {
    let status_label = match status {
        NativeToolOutcome::Completed => "completed",
        NativeToolOutcome::Failed => "failed",
        NativeToolOutcome::Denied => "denied",
        NativeToolOutcome::Cancelled => "cancelled",
        NativeToolOutcome::ValidationFailed => "validation_failed",
    };
    if status == NativeToolOutcome::Completed
        && let Some(content) = content
        && let Some(display) = native_provider_visible_tool_progress_output(tool_name, content)
    {
        return display;
    }
    if status == NativeToolOutcome::Failed
        && let Some(content) = content
        && let Some(display) = native_provider_visible_failed_progress(content)
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

fn native_provider_visible_tool_progress_output(tool_name: &str, content: &str) -> Option<String> {
    match tool_name {
        "read_text_file" => native_provider_visible_read_progress(content),
        "search_project" => native_provider_visible_search_progress(content),
        "list_project_paths" => native_provider_visible_list_progress(content),
        "bash" => native_provider_visible_bash_progress(content),
        "project_path_info" => native_provider_visible_path_info_progress(content),
        _ => None,
    }
}

/// Failed tool results carry `{error, guidance}` JSON; show the
/// categorical error and its guidance instead of the redacted meta-line.
fn native_provider_visible_failed_progress(content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let error = value.get("error")?.as_str()?;
    match value.get("guidance").and_then(serde_json::Value::as_str) {
        Some(guidance) => Some(format!("failed: {error}\n{guidance}")),
        None => Some(format!("failed: {error}")),
    }
}

fn native_provider_visible_path_info_progress(content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let relative_path = value.get("relative_path")?.as_str()?;
    let kind = value.get("kind")?.as_str()?;
    // Directories report no byte size (null); shape without it.
    match value.get("byte_size").and_then(serde_json::Value::as_u64) {
        Some(byte_size) => Some(format!(
            "completed: {relative_path}; {kind}, {byte_size} bytes"
        )),
        None => Some(format!("completed: {relative_path}; {kind}")),
    }
}

/// Finished bash rows keep this many trailing output lines visible, so the
/// command's evidence survives the live stream (which the finished summary
/// replaces) and reappears on resume through the shared shaping path.
const BASH_PROGRESS_TAIL_LINES: usize = 8;

fn native_provider_visible_bash_progress(content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let output = value.get("output")?.as_str()?;
    let output_bytes_total = value.get("output_bytes_total")?.as_u64()?;
    let duration_ms = value.get("duration_ms")?.as_u64()?;
    let exit_code = value
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
        .map_or_else(|| String::from("unknown"), |code| code.to_string());
    let duration = if duration_ms >= 1_000 {
        format!("{}.{}s", duration_ms / 1_000, (duration_ms % 1_000) / 100)
    } else {
        format!("{duration_ms}ms")
    };
    let mut lines = vec![format!(
        "completed: exit {exit_code}; {duration}; {output_bytes_total} bytes"
    )];
    let output_lines = output.lines().collect::<Vec<_>>();
    if output_lines.len() > BASH_PROGRESS_TAIL_LINES {
        lines.push(format!(
            "... {} earlier lines",
            output_lines.len() - BASH_PROGRESS_TAIL_LINES
        ));
    }
    lines.extend(
        output_lines
            .iter()
            .rev()
            .take(BASH_PROGRESS_TAIL_LINES)
            .rev()
            .map(|line| (*line).to_owned()),
    );
    Some(lines.join("\n"))
}

const MAX_TOOL_CALL_PREVIEW_CHARS: usize = 80;

/// Short argument-derived preview shown next to the tool name in the TUI
/// (the reviewed argument for review-gated tools; the primary target for
/// read-only tools), so users can see what a tool call touched.
fn native_provider_tool_call_preview(
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Option<String> {
    let argument_name = match tool_name {
        "read_text_file" | "project_path_info" | "list_project_paths" => "path",
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

fn native_provider_visible_read_progress(content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let path = value.get("path")?.as_str()?;
    let byte_count = value.get("byte_count")?.as_u64()?;
    let text = value.get("text")?.as_str()?;
    let line_count = text.lines().count().max(1);
    let line_label = if line_count == 1 { "line" } else { "lines" };
    Some(format!(
        "completed: {path}; {line_count} {line_label}, {byte_count} bytes"
    ))
}

fn native_provider_visible_search_progress(content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let matches = value.get("matches")?.as_array()?;
    let truncated = value.get("truncated")?.as_bool()?;
    let mut lines = vec![format!(
        "completed: {} matches; truncated={truncated}",
        matches.len()
    )];
    for matched in matches.iter().take(8) {
        let path = matched.get("path")?.as_str()?;
        let line_number = matched.get("line_number")?.as_u64()?;
        let line = matched.get("line")?.as_str()?;
        lines.push(format!("{path}:{line_number}: {line}"));
    }
    if matches.len() > 8 {
        lines.push(format!("... {} more matches", matches.len() - 8));
    }
    Some(lines.join("\n"))
}

fn native_provider_visible_list_progress(content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let entries = value.get("entries")?.as_array()?;
    let truncated = value.get("truncated")?.as_bool()?;
    let mut lines = vec![format!(
        "completed: {} entries; truncated={truncated}",
        entries.len()
    )];
    for entry in entries.iter().take(12) {
        let path = entry.get("path")?.as_str()?;
        let kind = entry.get("kind")?.as_str()?;
        lines.push(format!("{kind} {path}"));
    }
    if entries.len() > 12 {
        lines.push(format!("... {} more entries", entries.len() - 12));
    }
    Some(lines.join("\n"))
}

async fn wait_for_agent_edit_review_decision(
    review_decisions: &mut AgentEditDecisionReceiver,
    pending: &PendingAgentEditToolReview,
) -> Result<LocalEditDecision, NativeProviderRoundError> {
    let Some(decision) = review_decisions.recv().await else {
        return Err(NativeProviderRoundError::Cancelled(String::from(
            "tool review decision channel closed",
        )));
    };
    if decision.request_id == pending.request_id
        && decision.preview_id == pending.preview_id.0
        && decision.permission_decision_id == pending.permission_decision_id.0
    {
        return Ok(decision.decision);
    }
    Err(NativeProviderRoundError::ToolContinuation(String::from(
        "stale_tool_review_decision",
    )))
}

fn native_provider_readonly_tool_result_summary(
    tool_name: &str,
    execution: &NativeToolExecutionResult,
) -> NativeToolPayloadSummary {
    let summary = match tool_name {
        "read_text_file" => String::from("read_text_file result redacted"),
        "search_project" => {
            native_provider_content_result_count_summary("search_project", &execution.summary)
                .unwrap_or_else(|| String::from("search_project result redacted"))
        }
        "list_project_paths" => {
            native_provider_content_result_count_summary("list_project_paths", &execution.summary)
                .unwrap_or_else(|| String::from("list_project_paths result redacted"))
        }
        _ => execution.summary.clone(),
    };
    NativeToolPayloadSummary {
        summary,
        byte_count: execution.byte_count,
        redacted: matches!(
            tool_name,
            "read_text_file" | "search_project" | "list_project_paths"
        ),
        truncated: execution.truncated,
    }
}

fn native_provider_content_result_count_summary(tool_name: &str, content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    match tool_name {
        "search_project" => Some(format!(
            "search_project matches={} truncated={}",
            value.get("matches")?.as_array()?.len(),
            value.get("truncated")?.as_bool()?
        )),
        "list_project_paths" => Some(format!(
            "list_project_paths entries={} truncated={}",
            value.get("entries")?.as_array()?.len(),
            value.get("truncated")?.as_bool()?
        )),
        _ => None,
    }
}

fn record_review_wait_trace(
    sink: &impl NativeSessionEventSink,
    pending: &PendingAgentEditToolReview,
    started: Instant,
    outcome: NativeEditTraceOutcome,
    reason_label: Option<String>,
) {
    let mut log = NativeSessionLog::default();
    log.record_edit_trace(
        pending.session_id.clone(),
        pending.turn_id.clone(),
        NativeEditTraceRecord {
            trace_id: pending.trace_id.clone(),
            phase: NativeEditTracePhase::ReviewWait,
            source: NativeEditTraceSource::ProviderTool,
            tool_name: Some(pending.operation.clone()),
            tool_request_id: Some(NativeToolRequestId(pending.request_id.clone())),
            provider_call_id: Some(pending.provider_call_id.clone()),
            preview_id: Some(pending.preview_id.clone()),
            permission_decision_id: Some(pending.permission_decision_id.clone()),
            transaction_id: None,
            outcome,
            duration_ms: native_elapsed_ms(started),
            reason_label,
            attributes: vec![native_trace_attribute(
                "operation",
                pending.operation.clone(),
            )],
        },
    );
    if let Some(event) = log.events.last() {
        let _ = sink.append_event(event);
    }
}

fn record_provider_continuation_trace_records(
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    store: Option<&NativeJsonlSessionStore>,
    input: ProviderContinuationTraceInput<'_>,
) {
    for edit_trace in input.edit_traces {
        log.record_edit_trace(
            input.session_id.clone(),
            input.turn_id.clone(),
            NativeEditTraceRecord {
                trace_id: edit_trace.trace_id.clone(),
                phase: NativeEditTracePhase::ProviderContinuation,
                source: NativeEditTraceSource::ProviderTool,
                tool_name: Some(edit_trace.tool_name.clone()),
                tool_request_id: Some(edit_trace.tool_request_id.clone()),
                provider_call_id: edit_trace.provider_call_id.clone(),
                preview_id: edit_trace.preview_id.clone(),
                permission_decision_id: edit_trace.permission_decision_id.clone(),
                transaction_id: None,
                outcome: input.outcome,
                duration_ms: native_elapsed_ms(input.started),
                reason_label: input.reason_label.map(str::to_owned),
                attributes: vec![native_trace_attribute(
                    "operation",
                    edit_trace.tool_name.clone(),
                )],
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

fn native_trace_attribute(key: &str, value: impl Into<String>) -> NativeMetricAttribute {
    NativeMetricAttribute {
        key: key.to_owned(),
        value: value.into(),
    }
}

fn native_elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn native_provider_round_error_label(error: &NativeProviderRoundError) -> String {
    match error {
        NativeProviderRoundError::Provider(_) => String::from("provider_failed"),
        NativeProviderRoundError::Cancelled(_) => String::from("provider_cancelled"),
        NativeProviderRoundError::StreamEndedWithoutCompletion => {
            String::from("stream_ended_without_completion")
        }
        NativeProviderRoundError::ProjectRootUnavailable => {
            String::from("project_root_unavailable")
        }
        NativeProviderRoundError::ToolContinuation(reason) => reason.clone(),
        NativeProviderRoundError::ToolExecutionDenied { .. } => {
            String::from("tool_execution_denied")
        }
        #[cfg(test)]
        NativeProviderRoundError::SecondRoundToolCall => String::from("unexpected_tool_call"),
    }
}

fn native_review_wait_error_outcome(error: &NativeProviderRoundError) -> NativeEditTraceOutcome {
    match error {
        NativeProviderRoundError::Cancelled(_) => NativeEditTraceOutcome::Cancelled,
        NativeProviderRoundError::Provider(_)
        | NativeProviderRoundError::StreamEndedWithoutCompletion
        | NativeProviderRoundError::ProjectRootUnavailable
        | NativeProviderRoundError::ToolContinuation(_)
        | NativeProviderRoundError::ToolExecutionDenied { .. } => NativeEditTraceOutcome::Failed,
        #[cfg(test)]
        NativeProviderRoundError::SecondRoundToolCall => NativeEditTraceOutcome::Failed,
    }
}

fn native_tool_round_error_label(error: &NativeToolContinuationError) -> String {
    match error {
        NativeToolContinuationError::TooManyToolCalls { .. } => {
            String::from("tool_round_too_many_calls")
        }
        NativeToolContinuationError::Validation(_) => String::from("tool_round_validation_failed"),
        NativeToolContinuationError::Execution(_) => String::from("tool_round_execution_failed"),
        NativeToolContinuationError::ResultTooLarge { .. } => {
            String::from("tool_round_result_too_large")
        }
    }
}

fn native_provider_mapping_error_label(error: &ProviderContinuationMappingError) -> String {
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

fn native_provider_tool_loop_stop_message(reason: &str) -> &'static str {
    match reason {
        "tool_loop_too_many_rounds"
        | "tool_loop_too_many_total_calls"
        | "tool_loop_total_result_too_large" => {
            "Native provider tool loop stopped before completion"
        }
        "context_refilled_after_compaction" => {
            "Context refilled immediately after compaction; narrow the request \
or start a fresh session"
        }
        _ => "Native provider tool continuation failed",
    }
}

fn native_provider_tool_advertising_error_label(error: &ProviderToolAdvertisingError) -> String {
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

fn native_provider_round_error_to_provider_error(
    error: &NativeProviderRoundError,
) -> ProviderError {
    match error {
        NativeProviderRoundError::Provider(error) => error.clone(),
        NativeProviderRoundError::Cancelled(reason) => ProviderError::cancelled(reason.clone()),
        NativeProviderRoundError::StreamEndedWithoutCompletion => ProviderError {
            kind: ProviderErrorKind::MalformedStream,
            message: String::from("Native provider stream ended without completion"),
            redacted_debug: None,
        },
        NativeProviderRoundError::ProjectRootUnavailable => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider project root unavailable"),
            redacted_debug: None,
        },
        NativeProviderRoundError::ToolContinuation(reason) => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from(native_provider_tool_loop_stop_message(reason)),
            redacted_debug: Some(reason.clone()),
        },
        NativeProviderRoundError::ToolExecutionDenied {
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
        NativeProviderRoundError::SecondRoundToolCall => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider tool continuation failed"),
            redacted_debug: Some(String::from("unexpected_tool_call")),
        },
    }
}

#[derive(Debug, Clone)]
struct NativeLaunchProjectContext {
    project_root: NativeResourceRoot,
    cwd: PathBuf,
}

#[cfg(test)]
impl NativeLaunchProjectContext {
    fn from_project_root(project_root: NativeResourceRoot) -> Self {
        let cwd = project_root.canonical_path().to_path_buf();
        Self { project_root, cwd }
    }
}

fn native_launch_project_context(
    launch_cwd: impl AsRef<Path>,
) -> Option<NativeLaunchProjectContext> {
    let cwd = launch_cwd.as_ref().canonicalize().ok()?;
    let project_root_path = nearest_project_marker_root(&cwd).unwrap_or_else(|| cwd.clone());
    let project_root = configured_project_root(project_root_path)?;
    Some(NativeLaunchProjectContext { project_root, cwd })
}

fn native_launch_project_context_from_root(
    project_root: impl AsRef<Path>,
) -> Option<NativeLaunchProjectContext> {
    let project_root = configured_project_root(project_root)?;
    let cwd = project_root.canonical_path().to_path_buf();
    Some(NativeLaunchProjectContext { project_root, cwd })
}

/// Project resource root with the config-resolved sensitive-file policy
/// applied. Config load failures fail closed to the built-in defaults;
/// warnings surface separately at runner startup.
fn configured_project_root(project_root: impl AsRef<Path>) -> Option<NativeResourceRoot> {
    let root = NativeResourceRoot::project(project_root).ok()?;
    let (policy, _warnings) =
        crate::NativeSensitivePathPolicy::load_for_project(Some(root.canonical_path()));
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
    store: NativeJsonlSessionStore,
    provider: NativeProviderDogfoodConfig,
    started_prompt: StartedNativePrompt,
    mut requester: Requester,
    project_runtime: NativeProviderPromptProjectRuntime,
    review_decisions: AgentEditDecisionReceiver,
) -> NativeSessionLog
where
    Requester: ProviderRequester,
{
    let StartedNativePrompt {
        session_id,
        prompt,
        mut log,
        mut pending_events,
        turn,
        user_entry,
        assistant_entry,
        prompt_started,
    } = started_prompt;
    let NativeProviderPromptProjectRuntime {
        project_context,
        extension_manifest_scan_state,
        extension_activation_state,
    } = project_runtime;

    handle_native_provider_prompt(NativeProviderPromptRequest {
        tx: &tx,
        store: &store,
        _prompt: &prompt,
        provider,
        requester: &mut requester,
        log: &mut log,
        pending_events: &mut pending_events,
        ids: NativeProviderTurnRefs {
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

struct NativeProviderPromptRequest<'a, Requester> {
    tx: &'a mpsc::UnboundedSender<BackendEvent>,
    store: &'a NativeJsonlSessionStore,
    _prompt: &'a str,
    provider: NativeProviderDogfoodConfig,
    requester: &'a mut Requester,
    log: &'a mut NativeSessionLog,
    pending_events: &'a mut Vec<NativeSessionEvent>,
    ids: NativeProviderTurnRefs,
    project_context: Option<NativeLaunchProjectContext>,
    extension_static_context_files: Vec<NativeExtensionStaticContextFile>,
    extension_activation_snapshot: crate::ExtensionActivationSnapshot,
    review_decisions: AgentEditDecisionReceiver,
}

async fn handle_native_provider_prompt<Requester>(
    request: NativeProviderPromptRequest<'_, Requester>,
) where
    Requester: ProviderRequester,
{
    let NativeProviderPromptRequest {
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
        native_launch_project_context(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    });
    let context_budget = native_context_budget(
        Some(&provider),
        project_context
            .as_ref()
            .map(|context| context.project_root.canonical_path()),
    );
    let result = run_native_provider_one_agent_tool_round(
        requester,
        NativeProviderAgentToolRound {
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
            let response_chunks = if round.text.trim().is_empty() {
                vec![String::from(EMPTY_ASSISTANT_RESPONSE_MESSAGE)]
            } else {
                native_response_chunks(&round.text)
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
                        NativeSessionEvent::TurnFinished {
                            session_id: ids.session_id.clone(),
                            turn_id: ids.turn,
                            outcome: NativeTurnOutcome::Cancelled,
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
                NativeSessionEvent::EntryAppended {
                    session_id: ids.session_id.clone(),
                    entry_id: ids.assistant_entry,
                    parent_entry_id: Some(ids.user_entry),
                    turn_id: ids.turn.clone(),
                    role: NativeRole::Assistant,
                    text: round.text,
                    provider: Some(ProviderMetadata {
                        provider: provider_name.to_owned(),
                        model: model_id,
                        response_id: round.provider_response_id,
                    }),
                },
            );
            push_native_session_event(
                log,
                pending_events,
                NativeSessionEvent::TurnFinished {
                    session_id: ids.session_id.clone(),
                    turn_id: ids.turn,
                    outcome: NativeTurnOutcome::Completed,
                    reason: None,
                },
            );
            finish_native_prompt(
                tx,
                store,
                log,
                pending_events,
                NativePromptCompletion {
                    session_id: &ids.session_id.0,
                    status: "turn_end native provider",
                    outcome: PromptOutcome::Completed,
                    context_budget,
                },
            );
        }
        Err(error) => {
            let provider_error = native_provider_round_error_to_provider_error(&error);
            let (turn_outcome, prompt_outcome, status) =
                if matches!(error, NativeProviderRoundError::Cancelled(_)) {
                    (
                        NativeTurnOutcome::Cancelled,
                        PromptOutcome::Cancelled,
                        "turn_end native provider cancelled",
                    )
                } else {
                    (
                        NativeTurnOutcome::Failed,
                        PromptOutcome::Failed,
                        "turn_end native provider failed",
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
                NativePromptCompletion {
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
struct NativePromptCompletion<'a> {
    session_id: &'a str,
    status: &'a str,
    outcome: PromptOutcome,
    context_budget: Option<crate::NativeContextBudget>,
}

fn finish_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    log: &NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    completion: NativePromptCompletion<'_>,
) {
    let status = match append_pending_native_session_events(store, pending_events) {
        Ok(()) => completion.status.to_owned(),
        Err(error) => format!("native dogfood: failed to persist session log: {error}"),
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
    store: &NativeJsonlSessionStore,
    log: &mut NativeSessionLog,
    session_id: &NativeSessionId,
    turn_id: NativeTurnId,
    prompt_started: Instant,
    reason: &str,
) {
    if native_log_has_finished_turn(log, &turn_id) {
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
        NativeSessionEvent::TurnFinished {
            session_id: session_id.clone(),
            turn_id,
            outcome: NativeTurnOutcome::Cancelled,
            reason: Some(reason.to_owned()),
        },
    );
    finish_native_prompt(
        tx,
        store,
        log,
        &mut pending_events,
        NativePromptCompletion {
            session_id: &session_id.0,
            status: "turn_end native provider cancelled",
            outcome: PromptOutcome::Cancelled,
            context_budget: None,
        },
    );
}

fn persist_native_fixture_error(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    session_id: &NativeSessionId,
    turn_id: NativeTurnId,
    outcome: NativeTurnOutcome,
    error: &ProviderError,
) {
    let reason = native_provider_error_reason(error);
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: native_provider_failure_status(error),
    }));
    push_native_session_event(
        log,
        pending_events,
        NativeSessionEvent::TurnFinished {
            session_id: session_id.clone(),
            turn_id,
            outcome,
            reason: Some(reason),
        },
    );
}

fn native_provider_error_reason(error: &ProviderError) -> String {
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

fn native_provider_failure_status(error: &ProviderError) -> String {
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
        "native provider failed ({}): {hint}",
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
enum NativeFixtureOutcome {
    Completed,
    Failed,
    Malformed,
    Cancelled,
}

impl NativeFixtureOutcome {
    const fn status_message(self) -> &'static str {
        match self {
            Self::Completed => "turn_end native dogfood",
            Self::Failed => "turn_end native dogfood failed",
            Self::Malformed => "turn_end native dogfood malformed",
            Self::Cancelled => "turn_end native dogfood cancelled",
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

fn native_fixture_outcome(prompt: &str) -> NativeFixtureOutcome {
    if prompt.contains("/native-fixture-fail") {
        NativeFixtureOutcome::Failed
    } else if prompt.contains("/native-fixture-malformed") {
        NativeFixtureOutcome::Malformed
    } else if prompt.contains("/native-fixture-cancel") {
        NativeFixtureOutcome::Cancelled
    } else {
        NativeFixtureOutcome::Completed
    }
}

fn native_response_chunks(response: &str) -> Vec<String> {
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
        ExtensionActivationSnapshotState, ExtensionManifestScanState, MAX_TOOL_CALL_PREVIEW_CHARS,
        NativeFixtureOutcome, NativeLaunchProjectContext, NativeProviderAgentToolBatch,
        NativeProviderAgentToolRound, NativeProviderBufferedEventSink, NativeProviderDogfoodConfig,
        NativeProviderRoundError, NativeProviderRoundResult, NativeProviderToolLoopBudget,
        NativeProviderToolLoopPolicy, NativeProviderToolRoundContext, ProviderRequester,
        collect_native_provider_first_round, execute_native_provider_agent_tool_batch,
        handle_native_extension_diagnostic_snapshot_request,
        handle_native_extension_lifecycle_request, load_native_session_log_for_runner,
        load_native_session_log_for_runner_with_loader, native_fixture_outcome,
        native_launch_project_context, native_local_edit_error_message,
        native_log_has_finished_turn, native_provider_messages_from_log,
        native_provider_messages_from_log_with_static_context, native_provider_round_error_label,
        native_provider_round_error_to_provider_error, native_provider_tool_call_preview,
        native_provider_tool_progress_output, native_response_chunks, native_status_message,
        native_tool_result_display, record_provider_continuation_trace_records,
        run_native_provider_one_agent_tool_round, run_native_provider_one_readonly_tool_round,
        run_native_provider_one_tool_round_with_registry, send_native_initial_state,
        send_native_session_messages_from_log,
    };
    use crate::rig_adapter::{RigProviderAdapterConfig, RigProviderConfig};
    use crate::{
        ExtensionActivationDiagnostic, ExtensionActivationSnapshot, ExtensionActivationState,
        ExtensionInstallScope, ExtensionManifestIndex, ExtensionPackageRoot,
        ExtensionToolExecutorRouter, ExtensionToolHandler, NativeEditAccess, NativeEditAccessError,
        NativeEditError, NativeEditEvidenceOutcome, NativeEditEvidenceSummary,
        NativeEditOperationEvidence, NativeEditPreviewId, NativeEditTraceId,
        NativeEditTraceOutcome, NativeEditTracePhase, NativeEditTraceRecord,
        NativeEditTransactionId, NativeEntryId, NativeJsonlSessionStore,
        NativePermissionDecisionId, NativePermissionDecisionOutcome, NativeProviderToolResult,
        NativeResourceRoot, NativeRole, NativeSessionEvent, NativeSessionEventSink,
        NativeSessionId, NativeSessionLoadResult, NativeSessionLog, NativeStaticContextBundle,
        NativeStaticContextItem, NativeStaticContextPlacement, NativeStaticContextPriority,
        NativeStaticContextSource, NativeToolContinuationPolicy, NativeToolDefinition,
        NativeToolInputSchema, NativeToolOutcome, NativeToolPayloadSummary,
        NativeToolPermissionPolicy, NativeToolPermissionState, NativeToolRegistry,
        NativeToolReplacementPolicy, NativeToolReplacementRule, NativeToolReplacementSource,
        NativeToolRequestId, NativeToolResolutionMode, NativeTurnId, NativeTurnOutcome,
        PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY, ProjectReadOnlyToolExecutor, ProviderError,
        ProviderErrorKind, ProviderFinishReason, ProviderMessage, ProviderModel, ProviderRequest,
        ProviderStreamEvent, ProviderToolCall, ProviderToolVisibility, completed_text_exchange,
        parse_provider_tool_advertising_extensions, sha256_hex_for_test,
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
    fn native_provider_tool_loop_policy_matches_design_limits() {
        let policy = NativeProviderToolLoopPolicy::agent_default();

        assert_eq!(policy.max_tool_rounds, None);
        assert_eq!(policy.max_tool_calls_per_round, 16);
        assert_eq!(policy.max_total_tool_calls, 200);
        assert_eq!(policy.max_result_bytes_per_tool, 64 * 1024);
        assert_eq!(policy.max_total_result_bytes, 512 * 1024);

        let continuation_policy = policy.as_continuation_policy();
        assert_eq!(
            continuation_policy,
            NativeToolContinuationPolicy {
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
            let mut registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
            assert!(
                registry
                    .register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                        "example.toy-tools",
                        "toy_tool",
                        "toy metadata",
                        NativeToolInputSchema::string_object(
                            ["label"],
                            std::iter::empty::<&str>(),
                            512,
                        ),
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
                registry: NativeToolRegistry::with_project_read_only_and_agent_edit_tools(),
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
    fn native_provider_tool_loop_budget_rejects_round_call_and_byte_overages() {
        let policy = NativeProviderToolLoopPolicy {
            max_tool_rounds: Some(1),
            max_tool_calls_per_round: 2,
            max_total_tool_calls: 3,
            max_result_bytes_per_tool: 8,
            max_total_result_bytes: 12,
        };

        assert_eq!(
            NativeProviderToolLoopBudget::new(policy).begin_tool_round(3),
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_round_too_many_calls"
            )))
        );

        let mut budget = NativeProviderToolLoopBudget::new(policy);
        assert_eq!(budget.begin_tool_round(1), Ok(()));
        assert_eq!(
            budget.begin_tool_round(1),
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_rounds"
            )))
        );

        let total_call_policy = NativeProviderToolLoopPolicy {
            max_tool_rounds: None,
            ..policy
        };
        let mut budget = NativeProviderToolLoopBudget::new(total_call_policy);
        assert_eq!(budget.begin_tool_round(2), Ok(()));
        assert_eq!(
            budget.begin_tool_round(2),
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_total_calls"
            )))
        );

        assert_eq!(
            NativeProviderToolLoopBudget::new(policy).record_tool_result("call-too-large", 9),
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_result_too_large:call-too-large"
            )))
        );

        let mut budget = NativeProviderToolLoopBudget::new(policy);
        assert_eq!(budget.record_tool_result("call-a", 8), Ok(()));
        assert_eq!(
            budget.record_tool_result("call-b", 5),
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_loop_total_result_too_large"
            )))
        );
    }

    #[test]
    fn native_provider_agent_loop_limit_maps_to_redacted_provider_error() {
        let error =
            NativeProviderRoundError::ToolContinuation(String::from("tool_loop_too_many_rounds"));

        let provider_error = native_provider_round_error_to_provider_error(&error);

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
    fn native_provider_round_error_label_maps_second_round_helper_to_unexpected_tool_call() {
        assert_eq!(
            native_provider_round_error_label(&NativeProviderRoundError::SecondRoundToolCall),
            "unexpected_tool_call"
        );
    }

    #[test]
    fn native_provider_agent_tool_batch_executes_read_tool_results() {
        let root = TempProject::new("native-provider-agent-tool-batch-read");
        root.write("src/lib.rs", "alpha\n");
        let project_root = NativeResourceRoot::project(root.root());
        assert!(project_root.is_ok());
        let Ok(project_root) = project_root else {
            return;
        };
        let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        let permission_policy =
            NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
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
        let mut edit_access = NativeEditAccess::default();
        let edit_sink = NativeProviderBufferedEventSink::new(None);
        let (review_tx, _review_rx) = mpsc::unbounded_channel();
        let (_decision_tx, mut review_decisions) = mpsc::unbounded_channel();
        let mut budget =
            NativeProviderToolLoopBudget::new(NativeProviderToolLoopPolicy::agent_default());
        let mut edit_traces = Vec::new();
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = NativeTurnId(String::from("turn-1"));

        let results = futures::executor::block_on(execute_native_provider_agent_tool_batch(
            NativeProviderAgentToolBatch {
                session_id: NativeSessionId(String::from("default")),
                shell_policy: crate::NativeShellPolicy::default(),
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
        let content = serde_json::from_str::<serde_json::Value>(&results[0].content);
        assert!(content.is_ok());
        let Ok(content) = content else {
            return;
        };
        assert_eq!(
            content.get("text").and_then(serde_json::Value::as_str),
            Some("alpha\n")
        );
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolExecutionFinished {
                tool_request_id,
                outcome: NativeToolOutcome::Completed,
                ..
            } if tool_request_id == &NativeToolRequestId(String::from("tool-request-1-1"))
        )));
    }

    #[test]
    fn native_provider_agent_tool_batch_executes_extension_metadata_tool_results() {
        let root = TempProject::new("native-provider-agent-tool-batch-extension");
        let project_root = NativeResourceRoot::project(root.root());
        assert!(project_root.is_ok());
        let Ok(project_root) = project_root else {
            return;
        };
        let mut registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                NativeToolInputSchema::string_object(
                    std::iter::empty::<&str>(),
                    std::iter::empty::<&str>(),
                    512,
                ),
                ProviderToolVisibility::Visible,
            )),
            Ok(())
        );
        let permission_policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_tool",
        ]);
        let resolved_catalog =
            registry.resolve_provider_turn_catalog(&permission_policy, ["toy_tool"]);
        let read_only_executor = ProjectReadOnlyToolExecutor::new(project_root.clone());
        let extension_executor = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata("example.toy-tools", "{\"ok\":true}"),
        )]);
        let mut edit_access = NativeEditAccess::default();
        let edit_sink = NativeProviderBufferedEventSink::new(None);
        let (review_tx, _review_rx) = mpsc::unbounded_channel();
        let (_decision_tx, mut review_decisions) = mpsc::unbounded_channel();
        let mut budget =
            NativeProviderToolLoopBudget::new(NativeProviderToolLoopPolicy::agent_default());
        let mut edit_traces = Vec::new();
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = NativeTurnId(String::from("turn-1"));

        let results = futures::executor::block_on(execute_native_provider_agent_tool_batch(
            NativeProviderAgentToolBatch {
                session_id: NativeSessionId(String::from("default")),
                shell_policy: crate::NativeShellPolicy::default(),
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
            NativeSessionEvent::ToolExecutionFinished {
                tool_request_id,
                outcome: NativeToolOutcome::Completed,
                ..
            } if tool_request_id == &NativeToolRequestId(String::from("tool-request-1-1"))
        )));
    }

    #[test]
    fn native_provider_agent_tool_batch_records_replacement_provenance_evidence() {
        let root = TempProject::new("native-provider-agent-tool-batch-replacement");
        let project_root = NativeResourceRoot::project(root.root());
        assert!(project_root.is_ok());
        let Ok(project_root) = project_root else {
            return;
        };
        let mut registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        assert_eq!(
            registry.register_extension_tool(
                NativeToolDefinition::extension_metadata_tool_with_version(
                    "example.toy-tools",
                    Some("1.2.3"),
                    "toy_path_info",
                    "Replacement path metadata implementation.",
                    NativeToolDefinition::project_path_info().input_schema,
                    ProviderToolVisibility::Visible,
                )
            ),
            Ok(())
        );
        let permission_policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_path_info",
        ]);
        let replacement_policy =
            NativeToolReplacementPolicy::from_rules([NativeToolReplacementRule {
                builtin_name: String::from("project_path_info"),
                extension_id: String::from("example.toy-tools"),
                extension_tool: String::from("toy_path_info"),
                mode: NativeToolResolutionMode::ReplaceBuiltin,
                source: NativeToolReplacementSource::Profile,
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
        let mut edit_access = NativeEditAccess::default();
        let edit_sink = NativeProviderBufferedEventSink::new(None);
        let (review_tx, _review_rx) = mpsc::unbounded_channel();
        let (_decision_tx, mut review_decisions) = mpsc::unbounded_channel();
        let mut budget =
            NativeProviderToolLoopBudget::new(NativeProviderToolLoopPolicy::agent_default());
        let mut edit_traces = Vec::new();
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = NativeTurnId(String::from("turn-1"));

        let results = futures::executor::block_on(execute_native_provider_agent_tool_batch(
            NativeProviderAgentToolBatch {
                session_id: NativeSessionId(String::from("default")),
                shell_policy: crate::NativeShellPolicy::default(),
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
            NativeSessionEvent::ToolRequestRecorded {
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
            let marker = super::NativeStartupTraceMarker::new(move |label| {
                if let Ok(mut labels) = marker_labels.lock() {
                    labels.push(label.to_owned());
                }
            });
            let handle = tokio::spawn(super::run_native_dogfood_loop(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
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
            let handle = tokio::spawn(super::run_native_dogfood_loop(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
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

    fn edit_trace_records(log: &NativeSessionLog) -> Vec<NativeEditTraceRecord> {
        log.events
            .iter()
            .filter_map(|event| match event {
                NativeSessionEvent::EditTraceRecorded { trace, .. } => Some(trace.clone()),
                NativeSessionEvent::EntryAppended { .. }
                | NativeSessionEvent::ToolRequestRecorded { .. }
                | NativeSessionEvent::ToolExecutionFinished { .. }
                | NativeSessionEvent::TurnFinished { .. }
                | NativeSessionEvent::MetricRecorded { .. }
                | NativeSessionEvent::StaticContextIncluded { .. }
                | NativeSessionEvent::PermissionDecisionRecorded { .. }
                | NativeSessionEvent::EditTransactionPrepared { .. }
                | NativeSessionEvent::EditTransactionFinished { .. }
                | NativeSessionEvent::CompactionCheckpoint { .. } => None,
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
    fn native_response_chunks_preserve_unicode() {
        let chunks = native_response_chunks("hello 🙂 native dogfood");

        assert_eq!(chunks.concat(), "hello 🙂 native dogfood");
        assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
    }

    #[test]
    fn native_fixture_outcome_uses_explicit_markers() {
        assert_eq!(
            native_fixture_outcome("hello"),
            NativeFixtureOutcome::Completed
        );
        assert_eq!(
            native_fixture_outcome("/native-fixture-fail"),
            NativeFixtureOutcome::Failed
        );
        assert_eq!(
            native_fixture_outcome("/native-fixture-malformed"),
            NativeFixtureOutcome::Malformed
        );
        assert_eq!(
            native_fixture_outcome("/native-fixture-cancel"),
            NativeFixtureOutcome::Cancelled
        );
    }

    #[test]
    fn native_status_reports_local_read_only_resources_available() {
        let status = native_status_message(None, None);

        assert_eq!(
            status,
            "backend: native dogfood; local read-only project inspection available; provider tools require native-provider"
        );
    }

    #[test]
    fn native_unconfigured_provider_status_reports_setup_error_and_recovery() {
        let status = native_status_message(
            None,
            Some(
                "native provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY",
            ),
        );

        assert_eq!(
            status,
            "native provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY; set the provider environment and relaunch yach tui"
        );
    }

    #[test]
    fn native_provider_status_reports_agent_tools_available() {
        let config = native_provider_test_config();
        let status = native_status_message(Some(&config), None);

        assert_eq!(
            status,
            "backend: native provider dogfood via anthropic/fixture-model; read/search/list and exact/create edit tools available"
        );
    }

    #[test]
    fn native_launch_project_context_discovers_marker_root_from_nested_cwd() {
        let root = TempProject::new("launch-marker-root");
        assert!(std::fs::create_dir_all(root.root().join(".git")).is_ok());
        let nested_cwd = root.root().join("crates/yach-backend/src");
        assert!(std::fs::create_dir_all(&nested_cwd).is_ok());

        let context = native_launch_project_context(&nested_cwd);

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
    fn native_launch_project_context_prefers_parent_git_over_nested_session_yach() {
        let root = TempProject::new("launch-git-over-session-yach");
        assert!(std::fs::create_dir_all(root.root().join(".git")).is_ok());
        let nested_cwd = root.root().join("crates/yach-backend/src");
        assert!(std::fs::create_dir_all(nested_cwd.join(".yach/native-sessions")).is_ok());

        let context = native_launch_project_context(&nested_cwd);

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
    fn native_launch_project_context_discovers_yach_append_system_marker_without_git() {
        let root = TempProject::new("launch-yach-append-system-marker");
        root.write(".yach/APPEND_SYSTEM.md", "project system rules");
        let nested_cwd = root.root().join("nested/cwd");
        assert!(std::fs::create_dir_all(&nested_cwd).is_ok());

        let context = native_launch_project_context(&nested_cwd);

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
    fn native_launch_project_context_falls_back_to_cwd_without_project_marker() {
        let root = TempProject::new("launch-no-marker");
        root.write("AGENTS.md", "parent rules should not be discovered");
        let nested_cwd = root.root().join("nested/cwd");
        assert!(std::fs::create_dir_all(&nested_cwd).is_ok());

        let context = native_launch_project_context(&nested_cwd);

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
    fn native_session_log_has_finished_turn_detects_terminal_event() {
        let turn_id = NativeTurnId(String::from("turn-7"));
        let mut log = NativeSessionLog::default();

        assert!(!native_log_has_finished_turn(&log, &turn_id));

        log.push(NativeSessionEvent::TurnFinished {
            session_id: NativeSessionId(String::from("default")),
            turn_id: turn_id.clone(),
            outcome: NativeTurnOutcome::Completed,
            reason: None,
        });

        assert!(native_log_has_finished_turn(&log, &turn_id));
        assert!(!native_log_has_finished_turn(
            &log,
            &NativeTurnId(String::from("turn-8"))
        ));
    }

    #[test]
    fn native_provider_messages_include_resumed_transcript() {
        let session_id = NativeSessionId(String::from("default"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "first prompt",
        );
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-0",
            "entry-0-assistant",
            NativeRole::Assistant,
            "first answer",
        );
        finish_native_provider_test_turn(
            &mut log,
            &session_id,
            "turn-0",
            NativeTurnOutcome::Completed,
        );
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "second prompt",
        );

        assert_eq!(
            native_provider_messages_from_log(&log, &NativeTurnId(String::from("turn-1"))),
            vec![
                ProviderMessage {
                    role: NativeRole::User,
                    content: String::from("first prompt"),
                },
                ProviderMessage {
                    role: NativeRole::Assistant,
                    content: String::from("first answer"),
                },
                ProviderMessage {
                    role: NativeRole::User,
                    content: String::from("second prompt"),
                },
            ]
        );
    }

    #[test]
    fn native_provider_messages_ignore_local_edit_evidence() {
        let session_id = NativeSessionId(String::from("default"));
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "current prompt",
        );

        let summary = NativeEditEvidenceSummary {
            operation_count: 1,
            operations: vec![NativeEditOperationEvidence::CreateTextFile {
                relative_path: String::from("notes.txt"),
                after_sha256: String::from("after"),
                after_bytes: 4,
                bytes_written: Some(4),
            }],
            diff_summary: NativeToolPayloadSummary {
                summary: String::from("+new\n"),
                byte_count: 5,
                redacted: false,
                truncated: false,
            },
        };
        log.push(NativeSessionEvent::EditTransactionPrepared {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
            transaction_id: NativeEditTransactionId(String::from("edit-1")),
            summary: summary.clone(),
        });
        log.push(NativeSessionEvent::EditTransactionFinished {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
            transaction_id: Some(NativeEditTransactionId(String::from("edit-1"))),
            outcome: NativeEditEvidenceOutcome::Completed,
            reason: None,
            summary: Some(summary),
        });

        assert_eq!(
            native_provider_messages_from_log(&log, &turn_id),
            vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("current prompt"),
            }]
        );
    }

    #[test]
    fn native_provider_messages_ignore_agent_edit_evidence() {
        let session_id = NativeSessionId(String::from("default"));
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "current prompt",
        );
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 42,
                redacted: true,
                truncated: false,
            },
            argument_content: None,
        });
        log.push(NativeSessionEvent::EditTransactionPrepared {
            session_id,
            turn_id: turn_id.clone(),
            tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
            transaction_id: NativeEditTransactionId(String::from("edit-1")),
            summary: NativeEditEvidenceSummary {
                operation_count: 1,
                operations: vec![NativeEditOperationEvidence::ModifyTextFile {
                    relative_path: String::from("src/lib.rs"),
                    before_sha256: String::from("before"),
                    after_sha256: String::from("after"),
                    before_bytes: 12,
                    after_bytes: 14,
                    hunk_count: 1,
                    bytes_written: None,
                }],
                diff_summary: NativeToolPayloadSummary {
                    summary: String::from("tool payload redacted"),
                    byte_count: 42,
                    redacted: true,
                    truncated: false,
                },
            },
        });

        let messages = native_provider_messages_from_log(&log, &turn_id);
        assert_eq!(
            messages,
            vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("current prompt"),
            }]
        );
        let rendered = format!("{messages:?}");
        assert!(!rendered.contains("edit_text_file"));
        assert!(!rendered.contains("call-edit-1"));
        assert!(!rendered.contains("tool-request-1"));
    }

    #[test]
    fn native_provider_messages_include_tool_activity_with_persisted_payloads() {
        let session_id = NativeSessionId(String::from("default"));
        let prior_turn = NativeTurnId(String::from("turn-1"));
        let current_turn = NativeTurnId(String::from("turn-2"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "list src",
        );
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: prior_turn.clone(),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("list_project_paths"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 15,
                redacted: true,
                truncated: false,
            },
            argument_content: Some(String::from("{\"path\":\"src\"}")),
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id: prior_turn.clone(),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(NativeToolPayloadSummary {
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
            NativeRole::Assistant,
            "listed",
        );
        finish_native_provider_test_turn(
            &mut log,
            &session_id,
            "turn-1",
            NativeTurnOutcome::Completed,
        );
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-2",
            "entry-2-user",
            NativeRole::User,
            "current prompt",
        );

        let messages = native_provider_messages_from_log(&log, &current_turn);

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1].role, NativeRole::Tool);
        let tool_message = serde_json::from_str::<serde_json::Value>(&messages[1].content);
        assert!(tool_message.is_ok(), "tool message should be json");
        let Ok(tool_message) = tool_message else {
            return;
        };
        assert_eq!(tool_message["tool_name"], "list_project_paths");
        assert_eq!(tool_message["status"], "completed");
        assert_eq!(tool_message["arguments"]["path"], "src");
        assert_eq!(tool_message["content"]["entries"][0]["path"], "src/lib.rs");
    }

    #[test]
    fn native_provider_messages_mark_pre_persistence_tool_activity_as_not_retained() {
        let session_id = NativeSessionId(String::from("default"));
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "search",
        );
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("search_project"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 20,
                redacted: true,
                truncated: false,
            },
            argument_content: None,
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(NativeToolPayloadSummary {
                summary: String::from("search_project matches=2 truncated=false"),
                byte_count: 64,
                redacted: true,
                truncated: false,
            }),
            result_content: None,
        });

        let messages = native_provider_messages_from_log(&log, &turn_id);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, NativeRole::Tool);
        assert!(messages[1].content.contains("output not retained"));
        assert!(
            messages[1]
                .content
                .contains("search_project matches=2 truncated=false")
        );
    }

    #[test]
    fn native_provider_messages_exclude_tool_activity_from_unfinished_prior_turns() {
        let session_id = NativeSessionId(String::from("default"));
        let prior_turn = NativeTurnId(String::from("turn-1"));
        let current_turn = NativeTurnId(String::from("turn-2"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "failed prompt",
        );
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id: prior_turn,
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: None,
            result_content: Some(String::from("{\"outcome\":\"list\"}")),
        });
        finish_native_provider_test_turn(
            &mut log,
            &session_id,
            "turn-1",
            NativeTurnOutcome::Failed,
        );
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-2",
            "entry-2-user",
            NativeRole::User,
            "current prompt",
        );

        let messages = native_provider_messages_from_log(&log, &current_turn);

        assert!(
            messages
                .iter()
                .all(|message| message.role != NativeRole::Tool)
        );
    }

    #[test]
    fn native_provider_messages_rebuild_from_newest_compaction_checkpoint() {
        let session_id = NativeSessionId(String::from("default"));
        let current_turn = NativeTurnId(String::from("turn-3"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "old work that was folded",
        );
        finish_native_provider_test_turn(
            &mut log,
            &session_id,
            "turn-1",
            NativeTurnOutcome::Completed,
        );
        log.push(NativeSessionEvent::CompactionCheckpoint {
            session_id: session_id.clone(),
            turn_id: NativeTurnId(String::from("turn-2")),
            checkpoint_id: crate::NativeCompactionCheckpointId(String::from("checkpoint-1")),
            summary: String::from("anchored summary of the folded work"),
            first_kept_entry_id: NativeEntryId(String::from("entry-2-user")),
            tokens_before: 90_000,
            tokens_after_estimate: 12_000,
            reason: crate::NativeCompactionReason::Threshold,
            compactor: String::from("summary"),
            details: serde_json::json!({}),
        });
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-2",
            "entry-2-user",
            NativeRole::User,
            "kept turn prompt",
        );
        finish_native_provider_test_turn(
            &mut log,
            &session_id,
            "turn-2",
            NativeTurnOutcome::Completed,
        );
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-3",
            "entry-3-user",
            NativeRole::User,
            "current prompt",
        );

        let messages = native_provider_messages_from_log(&log, &current_turn);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, NativeRole::System);
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
            super::native_provider_message_shapes(&messages),
            vec![
                String::from("system:Earlier work in this session was compacted. The"),
                String::from("user:kept turn prompt"),
                String::from("user:current prompt"),
            ]
        );
    }

    #[test]
    fn native_provider_messages_prepend_static_context_before_transcript() {
        let mut log = NativeSessionLog::default();
        let session_id = NativeSessionId(String::from("session-static-context"));
        let turn_id = NativeTurnId(String::from("turn-static-context"));
        log.push(NativeSessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: NativeEntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
            text: String::from("hello"),
            provider: None,
        });
        let context = NativeStaticContextBundle {
            items: vec![NativeStaticContextItem {
                source: NativeStaticContextSource::AgentsMd,
                relative_path: String::from("AGENTS.md"),
                placement: NativeStaticContextPlacement::ProjectInstructions,
                title: String::from("AGENTS.md instructions for ."),
                content: String::from("root rules"),
                byte_count: "root rules".len(),
                priority: NativeStaticContextPriority::ProjectInstructions,
            }],
            total_bytes: "root rules".len(),
        };

        let messages =
            native_provider_messages_from_log_with_static_context(&log, &turn_id, &context);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, NativeRole::System);
        assert!(
            messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert!(messages[0].content.contains("Match effort to the request"));
        assert_eq!(messages[1].role, NativeRole::System);
        assert!(
            messages[1]
                .content
                .contains("# AGENTS.md instructions for .")
        );
        assert!(messages[1].content.contains("root rules"));
        assert_eq!(messages[2].role, NativeRole::User);
        assert_eq!(messages[2].content, "hello");
    }

    #[test]
    fn native_provider_messages_render_extension_background_as_non_system_context() {
        let mut log = NativeSessionLog::default();
        let session_id = NativeSessionId(String::from("session-extension-background-context"));
        let turn_id = NativeTurnId(String::from("turn-extension-background-context"));
        log.push(NativeSessionEvent::EntryAppended {
            session_id,
            entry_id: NativeEntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
            text: String::from("hello"),
            provider: None,
        });
        let context = NativeStaticContextBundle {
            items: vec![
                NativeStaticContextItem {
                    source: NativeStaticContextSource::AgentsMd,
                    relative_path: String::from("AGENTS.md"),
                    placement: NativeStaticContextPlacement::ProjectInstructions,
                    title: String::from("AGENTS.md instructions for ."),
                    content: String::from("root rules"),
                    byte_count: "root rules".len(),
                    priority: NativeStaticContextPriority::ProjectInstructions,
                },
                NativeStaticContextItem {
                    source: NativeStaticContextSource::ExtensionFile {
                        extension_id: String::from("example.context-pack"),
                        item_id: String::from("rust-style-guide"),
                    },
                    relative_path: String::from("context/rust.md"),
                    placement: NativeStaticContextPlacement::BackgroundContext,
                    title: String::from("Extension background context: Rust style guide"),
                    content: String::from("extension guidance"),
                    byte_count: "extension guidance".len(),
                    priority: NativeStaticContextPriority::ExtensionBackground,
                },
            ],
            total_bytes: "root rulesextension guidance".len(),
        };

        let messages =
            native_provider_messages_from_log_with_static_context(&log, &turn_id, &context);

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, NativeRole::System);
        assert!(
            messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(messages[1].role, NativeRole::System);
        assert!(messages[1].content.contains("root rules"));
        assert!(!messages[1].content.contains("extension guidance"));
        assert_eq!(messages[2].role, NativeRole::User);
        assert!(
            messages[2]
                .content
                .contains("# Extension background context: Rust style guide")
        );
        assert!(messages[2].content.contains("extension guidance"));
        assert_eq!(messages[3].role, NativeRole::User);
        assert_eq!(messages[3].content, "hello");
    }

    #[test]
    fn native_provider_request_includes_project_static_context_and_records_evidence() {
        let root = TempProject::new("provider-static-context");
        root.write("AGENTS.md", "root rules");
        root.write(".yach/APPEND_SYSTEM.md", "system rules");
        let project_root = NativeResourceRoot::project(root.root()).ok();
        let executor_root = NativeResourceRoot::project(root.root());
        assert!(executor_root.is_ok());
        let Some(executor_root) = executor_root.ok() else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = NativeTurnId(String::from("turn-static-context-provider"));
        log.push(NativeSessionEvent::EntryAppended {
            session_id: NativeSessionId(String::from("default")),
            entry_id: NativeEntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
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
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let permission_policy =
            NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let executor = ProjectReadOnlyToolExecutor::new(executor_root);
        let routable_tool_names = ["project_path_info"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            NativeProviderToolRoundContext {
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
            Ok(NativeProviderRoundResult {
                text: String::from("ok"),
                provider_response_id: None,
            })
        );
        assert_eq!(requester.requests.len(), 1);
        let Some(request) = requester.requests.first() else {
            return;
        };
        assert_eq!(request.messages[0].role, NativeRole::System);
        assert!(
            request.messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(request.messages[1].role, NativeRole::System);
        assert!(request.messages[1].content.contains("root rules"));
        assert!(request.messages[1].content.contains("system rules"));
        assert!(pending_events.iter().any(|event| {
            matches!(event, NativeSessionEvent::StaticContextIncluded { summary, .. }
                if summary.items.len() == 2)
        }));
    }

    #[test]
    fn native_provider_messages_do_not_include_extension_static_context_before_manifest_scan() {
        let root = TempProject::new("provider-extension-static-context-before-scan");
        let package = TempProject::new("provider-extension-static-context-package-before");
        package.write(
            "yach.extension.json",
            extension_static_context_manifest_json(),
        );
        package.write("context/rust.md", "extension context should wait for scan");
        let project_root = NativeResourceRoot::project(root.root()).ok();
        let executor_root = NativeResourceRoot::project(root.root());
        assert!(executor_root.is_ok());
        let Some(executor_root) = executor_root.ok() else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = NativeTurnId(String::from("turn-extension-static-context-before"));
        log.push(NativeSessionEvent::EntryAppended {
            session_id: NativeSessionId(String::from("default")),
            entry_id: NativeEntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
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
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let permission_policy =
            NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let executor = ProjectReadOnlyToolExecutor::new(executor_root);
        let routable_tool_names = ["project_path_info"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            NativeProviderToolRoundContext {
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

        assert!(matches!(result, Ok(NativeProviderRoundResult { .. })));
        let Some(request) = requester.requests.first() else {
            return;
        };
        assert_eq!(request.messages[0].role, NativeRole::System);
        assert!(
            request.messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(request.messages[1].role, NativeRole::User);
        assert!(request.messages.iter().all(|message| {
            !message
                .content
                .contains("extension context should wait for scan")
        }));
        assert!(
            !pending_events
                .iter()
                .any(|event| { matches!(event, NativeSessionEvent::StaticContextIncluded { .. }) })
        );
    }

    #[test]
    fn native_provider_messages_include_extension_static_context_after_manifest_scan() {
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
        let project_root = NativeResourceRoot::project(root.root()).ok();
        let executor_root = NativeResourceRoot::project(root.root());
        assert!(executor_root.is_ok());
        let Some(executor_root) = executor_root.ok() else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = NativeTurnId(String::from("turn-extension-static-context-after"));
        log.push(NativeSessionEvent::EntryAppended {
            session_id: NativeSessionId(String::from("default")),
            entry_id: NativeEntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
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
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let permission_policy =
            NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let executor = ProjectReadOnlyToolExecutor::new(executor_root);
        let routable_tool_names = ["project_path_info"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            NativeProviderToolRoundContext {
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

        assert!(matches!(result, Ok(NativeProviderRoundResult { .. })));
        let Some(request) = requester.requests.first() else {
            return;
        };
        assert_eq!(request.messages[0].role, NativeRole::System);
        assert!(
            request.messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(request.messages[1].role, NativeRole::User);
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
        assert_eq!(request.messages[2].role, NativeRole::User);
        assert_eq!(request.messages[2].content, "hello");
        assert!(request.messages.iter().all(|message| {
            message.role != NativeRole::System
                || !message.content.contains("extension context after scan")
        }));
        assert!(pending_events.iter().any(|event| {
            matches!(event, NativeSessionEvent::StaticContextIncluded { summary, omissions, .. }
            if omissions.is_empty()
                && summary.items == vec![crate::NativeStaticContextItemSummary {
                    source: NativeStaticContextSource::ExtensionFile {
                        extension_id: String::from("example.context-pack"),
                        item_id: String::from("rust-style-guide"),
                    },
                    relative_path: String::from("context/rust.md"),
                    placement: NativeStaticContextPlacement::BackgroundContext,
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
        let store = NativeJsonlSessionStore::new(blocked_parent.join("session.jsonl"));
        let project_root = NativeResourceRoot::project(root.root()).ok();
        assert!(project_root.is_some());
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let turn_id = NativeTurnId(String::from("turn-static-context-persist-failure"));
        log.push(NativeSessionEvent::EntryAppended {
            session_id: NativeSessionId(String::from("default")),
            entry_id: NativeEntryId(String::from("entry-user")),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
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
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let permission_policy =
            NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let Some(project_root_for_executor) = project_root.clone() else {
            return;
        };
        let executor = ProjectReadOnlyToolExecutor::new(project_root_for_executor);
        let routable_tool_names = ["project_path_info"];

        let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
            &mut requester,
            NativeProviderToolRoundContext {
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
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "static_context_persist_failed"
            )))
        );
        assert!(requester.requests.is_empty());
        assert!(pending_events.iter().any(|event| {
            matches!(event, NativeSessionEvent::StaticContextIncluded { summary, .. }
                if summary.items.len() == 1)
        }));
    }

    #[test]
    fn native_provider_request_from_nested_cwd_includes_root_and_nested_agents_md() {
        let root = TempProject::new("provider-nested-static-context");
        assert!(std::fs::create_dir_all(root.root().join(".git")).is_ok());
        root.write("AGENTS.md", "root rules");
        root.write("crates/yach-backend/AGENTS.md", "backend rules");
        let nested_cwd = root.root().join("crates/yach-backend/src");
        assert!(std::fs::create_dir_all(&nested_cwd).is_ok());
        let context = native_launch_project_context(&nested_cwd);
        assert!(context.is_some());
        let Some(context) = context else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "hello",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            Ok(NativeProviderRoundResult {
                text: String::from("ok"),
                provider_response_id: None,
            })
        );
        assert_eq!(requester.requests.len(), 1);
        let guidance_message = &requester.requests[0].messages[0];
        assert_eq!(guidance_message.role, NativeRole::System);
        assert!(
            guidance_message
                .content
                .contains("coding agent running in the yach harness")
        );
        let system_message = &requester.requests[0].messages[1];
        assert_eq!(system_message.role, NativeRole::System);
        assert!(system_message.content.contains("root rules"));
        assert!(system_message.content.contains("backend rules"));
        assert!(pending_events.iter().any(|event| {
            matches!(event, NativeSessionEvent::StaticContextIncluded { summary, .. }
                if summary.items.iter().any(|item| item.relative_path == "AGENTS.md")
                    && summary
                        .items
                        .iter()
                        .any(|item| item.relative_path == "crates/yach-backend/AGENTS.md"))
        }));
    }

    #[test]
    fn native_provider_one_round_without_tools_preserves_one_shot_response() {
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            Ok(NativeProviderRoundResult {
                text: String::from("plain answer"),
                provider_response_id: Some(String::from("response-1")),
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
    fn native_provider_initial_request_advertises_registered_extension_tool_for_future_turn() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let Ok(()) =
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                NativeToolInputSchema::string_object(
                    std::iter::empty::<&str>(),
                    std::iter::empty::<&str>(),
                    1024,
                ),
                ProviderToolVisibility::Visible,
            ))
        else {
            return;
        };
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_tool",
        ]);
        let executor = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata("example.toy-tools", "{\"ok\":true}"),
        )]);
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect toy metadata",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            NativeProviderToolRoundContext {
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
            Ok(NativeProviderRoundResult {
                text: String::from("done"),
                provider_response_id: Some(String::from("response-1")),
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
    fn native_provider_extension_tool_continuation_does_not_require_project_root() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let Ok(()) =
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                NativeToolInputSchema::string_object(
                    std::iter::empty::<&str>(),
                    std::iter::empty::<&str>(),
                    1024,
                ),
                ProviderToolVisibility::Visible,
            ))
        else {
            return;
        };
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools(["toy_tool"]);
        let executor = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata("example.toy-tools", "{\"ok\":true}"),
        )]);
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect toy metadata",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            NativeProviderToolRoundContext {
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
            Ok(NativeProviderRoundResult {
                text: String::from("done"),
                provider_response_id: Some(String::from("response-2")),
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
    fn native_provider_one_round_rejects_incomplete_tool_call_stream() {
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "provider_tool_call_incomplete"
            )))
        );
        assert_eq!(requester.requests.len(), 1);
        assert!(pending_events.is_empty());
    }

    #[test]
    fn native_initial_state_handshake_advertises_local_edit() {
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
    fn native_provider_initial_request_advertises_content_tools_for_agent_edit_context() {
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: NativeTurnId(String::from("turn-1")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
            },
            ProviderStreamEvent::TextDelta {
                turn_id: NativeTurnId(String::from("turn-1")),
                delta: String::from("done"),
            },
            ProviderStreamEvent::Completed {
                turn_id: NativeTurnId(String::from("turn-1")),
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: Some(String::from("response-1")),
            },
        ])]);
        let root_guard = temp_native_provider_root("agent-content-advertising");
        let resource_root = NativeResourceRoot::project(root_guard.path());
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "inspect project",
        );
        let turn_id = NativeTurnId(String::from("turn-1"));
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            NativeProviderAgentToolRound {
                session_id: &NativeSessionId(String::from("default")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
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
            Ok(NativeProviderRoundResult {
                text: String::from("done"),
                provider_response_id: Some(String::from("response-1")),
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
    fn native_provider_agent_round_advertises_and_executes_active_extension_tool() {
        let mut registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512,),
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
        let turn_id = NativeTurnId(String::from("turn-active-extension"));
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
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-active-extension",
            "entry-active-extension-user",
            NativeRole::User,
            "inspect toy metadata",
        );
        let root_guard = temp_native_provider_root("active-extension-agent-round");
        let resource_root = NativeResourceRoot::project(root_guard.path());
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            NativeProviderAgentToolRound {
                session_id: &NativeSessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
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
            Ok(NativeProviderRoundResult {
                text: String::from("done"),
                provider_response_id: Some(String::from("response-2")),
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
            NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn native_provider_agent_continuation_preserves_tool_advertising() {
        let root_guard = temp_native_provider_root("agent-continuation-tool-advertising");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("README.md"), "tool advertising\n").is_ok());
        let resource_root = NativeResourceRoot::project(root_path);
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "read README",
        );
        let turn_id = NativeTurnId(String::from("turn-1"));
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
            NativeProviderAgentToolRound {
                session_id: &NativeSessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
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
            Ok(NativeProviderRoundResult {
                text: String::from("read complete"),
                provider_response_id: Some(String::from("response-2")),
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
    fn native_provider_agent_loop_reads_then_edits_in_later_round() {
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
            assert!(
                std::fs::write(
                    root_path.join("note.txt"),
                    "native provider edit dogfood ok"
                )
                .is_ok()
            );
            let resource_root = NativeResourceRoot::project(root_path);
            assert!(resource_root.is_ok());
            let Ok(resource_root) = resource_root else {
                return;
            };
            let mut log = NativeSessionLog::default();
            let mut pending_events = Vec::new();
            append_native_provider_test_entry(
                &mut log,
                &NativeSessionId(String::from("default")),
                "turn-1",
                "entry-1-user",
                NativeRole::User,
                "read and update note",
            );
            let turn_id = NativeTurnId(String::from("turn-1"));
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
            let session_id = NativeSessionId(String::from("default"));
            let run = run_native_provider_one_agent_tool_round(
                &mut requester,
                NativeProviderAgentToolRound {
                    session_id: &session_id,
                    model,
                    log: &mut log,
                    pending_events: &mut pending_events,
                    turn_id: &turn_id,
                    project_context: Some(NativeLaunchProjectContext::from_project_root(
                        resource_root,
                    )),
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
            assert_eq!(edited, "native provider edit dogfood passed");
            assert!(result.is_ok());
            let Ok(result) = result else {
                return;
            };
            assert_eq!(result.text, "Updated note.txt.");
        });
    }

    #[test]
    fn native_provider_agent_loop_emits_tool_progress_before_final_answer() {
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
            let resource_root = NativeResourceRoot::project(root_path);
            assert!(resource_root.is_ok());
            let Ok(resource_root) = resource_root else {
                return;
            };
            let mut log = NativeSessionLog::default();
            let mut pending_events = Vec::new();
            append_native_provider_test_entry(
                &mut log,
                &NativeSessionId(String::from("default")),
                "turn-1",
                "entry-1-user",
                NativeRole::User,
                "read note",
            );
            let turn_id = NativeTurnId(String::from("turn-1"));
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
                NativeProviderAgentToolRound {
                    session_id: &NativeSessionId(String::from("default")),
                    model,
                    log: &mut log,
                    pending_events: &mut pending_events,
                    turn_id: &turn_id,
                    project_context: Some(NativeLaunchProjectContext::from_project_root(
                        resource_root,
                    )),
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
                    && result.output == "completed: note.txt; 1 line, 16 bytes"
                )),
                "{progress_events:#?}"
            );
        });
    }

    #[test]
    fn native_provider_tool_call_preview_targets_primary_argument() {
        assert_eq!(
            native_provider_tool_call_preview(
                "read_text_file",
                &serde_json::json!({"path": "docs/project/state.md"})
            ),
            Some(String::from("docs/project/state.md"))
        );
        assert_eq!(
            native_provider_tool_call_preview(
                "search_project",
                &serde_json::json!({"query": "needle"})
            ),
            Some(String::from("needle"))
        );
        assert_eq!(
            native_provider_tool_call_preview(
                "bash",
                &serde_json::json!({"command": "cargo test\n--workspace"})
            ),
            Some(String::from("cargo test..."))
        );
        let long_command = "x".repeat(MAX_TOOL_CALL_PREVIEW_CHARS + 1);
        assert_eq!(
            native_provider_tool_call_preview(
                "bash",
                &serde_json::json!({"command": long_command})
            ),
            Some(format!("{}...", "x".repeat(MAX_TOOL_CALL_PREVIEW_CHARS)))
        );
        assert_eq!(
            native_provider_tool_call_preview(
                "edit_text_file",
                &serde_json::json!({"path": "a.rs"})
            ),
            None
        );
        assert_eq!(
            native_provider_tool_call_preview("read_text_file", &serde_json::json!({})),
            None
        );
    }

    #[test]
    fn native_tool_result_display_shapes_read_text_file_with_path_and_counts() {
        let content = serde_json::json!({
            "byte_count": 31,
            "outcome": "read",
            "path": "src/lib.rs",
            "text": "alpha line\nneedle evidence line\n",
        })
        .to_string();
        assert_eq!(
            native_tool_result_display(
                "read_text_file",
                NativeToolOutcome::Completed,
                Some(&content),
                31,
                false,
                None,
            ),
            "completed: src/lib.rs; 2 lines, 31 bytes"
        );
        // Non-JSON content falls back to the redacted summary line.
        assert_eq!(
            native_tool_result_display(
                "read_text_file",
                NativeToolOutcome::Completed,
                Some("not json"),
                8,
                false,
                None,
            ),
            "completed; bytes=8; content=redacted; truncated=false"
        );
    }

    #[test]
    fn native_tool_result_display_shapes_project_path_info() {
        let content = serde_json::json!({
            "relative_path": "testdata/sample-session.jsonl",
            "kind": "file",
            "byte_size": 31_744,
            "provider_visibility": "never",
        })
        .to_string();
        assert_eq!(
            native_tool_result_display(
                "project_path_info",
                NativeToolOutcome::Completed,
                Some(&content),
                content.len(),
                false,
                None,
            ),
            "completed: testdata/sample-session.jsonl; file, 31744 bytes"
        );
        // Directories report byte_size: null and shape without a size.
        let directory_content = serde_json::json!({
            "relative_path": ".",
            "kind": "directory",
            "byte_size": serde_json::Value::Null,
            "provider_visibility": "never",
        })
        .to_string();
        assert_eq!(
            native_tool_result_display(
                "project_path_info",
                NativeToolOutcome::Completed,
                Some(&directory_content),
                directory_content.len(),
                false,
                None,
            ),
            "completed: .; directory"
        );
    }

    #[test]
    fn native_tool_result_display_shapes_bash_with_exit_and_output_tail() {
        let content = serde_json::json!({
            "outcome": "completed",
            "tool_request_id": "tool-request-1-1",
            "approved_by": "user",
            "exit_code": 0,
            "duration_ms": 2_340,
            "output": "line-1\nline-2\n",
            "output_bytes_total": 14,
            "truncated": false,
        })
        .to_string();
        assert_eq!(
            native_tool_result_display(
                "bash",
                NativeToolOutcome::Completed,
                Some(&content),
                content.len(),
                false,
                None,
            ),
            "completed: exit 0; 2.3s; 14 bytes\nline-1\nline-2"
        );
    }

    #[test]
    fn native_tool_result_display_bounds_bash_output_tail_and_reports_nonzero_exit() {
        let output = (0..20)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let content = serde_json::json!({
            "outcome": "completed",
            "tool_request_id": "tool-request-1-1",
            "approved_by": "allowlist",
            "exit_code": 101,
            "duration_ms": 250,
            "output": output,
            "output_bytes_total": 147,
            "truncated": false,
        })
        .to_string();
        let display = native_tool_result_display(
            "bash",
            NativeToolOutcome::Completed,
            Some(&content),
            content.len(),
            false,
            None,
        );
        assert!(display.starts_with("completed: exit 101; 250ms; 147 bytes"));
        assert!(display.contains("... 12 earlier lines"));
        assert!(!display.contains("line-11\n"));
        assert!(display.contains("line-12"));
        assert!(display.ends_with("line-19"));
    }

    #[test]
    fn native_provider_agent_loop_records_read_and_edit_evidence_before_final_answer() {
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
            assert!(
                std::fs::write(
                    root_path.join("note.txt"),
                    "native provider edit dogfood ok"
                )
                .is_ok()
            );
            let resource_root = NativeResourceRoot::project(root_path);
            assert!(resource_root.is_ok());
            let Ok(resource_root) = resource_root else {
                return;
            };
            let mut log = NativeSessionLog::default();
            let mut pending_events = Vec::new();
            append_native_provider_test_entry(
                &mut log,
                &NativeSessionId(String::from("default")),
                "turn-1",
                "entry-1-user",
                NativeRole::User,
                "read and update note",
            );
            let turn_id = NativeTurnId(String::from("turn-1"));
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
            let session_id = NativeSessionId(String::from("default"));
            let run = run_native_provider_one_agent_tool_round(
                &mut requester,
                NativeProviderAgentToolRound {
                    session_id: &session_id,
                    model,
                    log: &mut log,
                    pending_events: &mut pending_events,
                    turn_id: &turn_id,
                    project_context: Some(NativeLaunchProjectContext::from_project_root(
                        resource_root,
                    )),
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
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Completed,
                    ..
                }
            )));
            let traces = edit_trace_records(&log);
            assert!(traces.iter().any(|trace| {
                trace.phase == NativeEditTracePhase::ProviderContinuation
                    && trace.outcome == NativeEditTraceOutcome::Completed
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
    fn native_provider_one_round_executes_read_search_list_and_continues_with_persisted_evidence() {
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
        let resource_root = NativeResourceRoot::project(root_path);
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "inspect content",
        );
        let turn_id = NativeTurnId(String::from("turn-1"));
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
            NativeProviderAgentToolRound {
                session_id: &NativeSessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
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
            Ok(NativeProviderRoundResult {
                text: String::from("content inspected"),
                provider_response_id: Some(String::from("response-2")),
            })
        );
        let mut progress_outputs = Vec::new();
        while let Ok(event) = backend_rx.try_recv() {
            if let BackendEvent::Server(ServerEvent::ToolCallFinished(result)) = event {
                progress_outputs.push((result.tool_name, result.output));
            }
        }
        assert!(progress_outputs.iter().any(|(tool_name, output)| {
            tool_name == "read_text_file" && output == "completed: src/lib.rs; 2 lines, 32 bytes"
        }));
        assert!(progress_outputs.iter().any(|(tool_name, output)| {
            tool_name == "search_project"
                && output.contains("completed: 1 matches")
                && output.contains("src/lib.rs:2: needle evidence line")
        }));
        assert!(progress_outputs.iter().any(|(tool_name, output)| {
            tool_name == "list_project_paths"
                && output.contains("completed: 2 entries")
                && output.contains("file src/lib.rs")
                && output.contains("file src/main.rs")
        }));
        assert_eq!(requester.requests.len(), 2);
        let tool_messages = requester.requests[1]
            .messages
            .iter()
            .filter(|message| message.role == NativeRole::Tool)
            .collect::<Vec<_>>();
        assert_eq!(tool_messages.len(), 3);
        let rendered_tool_messages = tool_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered_tool_messages.contains("call-read-1"));
        assert!(rendered_tool_messages.contains("call-search-1"));
        assert!(rendered_tool_messages.contains("call-list-1"));
        let tool_contents = tool_messages
            .iter()
            .filter_map(|message| {
                serde_json::from_str::<serde_json::Value>(&message.content)
                    .ok()
                    .and_then(|outer| {
                        outer
                            .get("content")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|content| {
                                serde_json::from_str::<serde_json::Value>(content).ok()
                            })
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_contents.len(), 3);
        assert!(tool_contents.iter().any(|content| {
            content.get("outcome").and_then(serde_json::Value::as_str) == Some("read")
                && content.get("text").and_then(serde_json::Value::as_str)
                    == Some("alpha line\nneedle evidence line\n")
        }));
        assert!(tool_contents.iter().any(|content| {
            content
                .get("matches")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|matches| {
                    matches.iter().any(|matched| {
                        matched.get("line").and_then(serde_json::Value::as_str)
                            == Some("needle evidence line")
                    })
                })
        }));
        assert!(tool_contents.iter().any(|content| {
            content
                .get("entries")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|entries| {
                    let paths = entries
                        .iter()
                        .filter_map(|entry| entry.get("path").and_then(serde_json::Value::as_str))
                        .collect::<Vec<_>>();
                    paths.contains(&"src/lib.rs") && paths.contains(&"src/main.rs")
                })
        }));

        let finished_summaries = pending_events
            .iter()
            .filter_map(|event| match event {
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Completed,
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
            NativeSessionEvent::ToolRequestRecorded {
                tool_name,
                argument_content: Some(content),
                ..
            } if tool_name == "search_project" && content.contains("needle")
        )));
    }

    #[test]
    fn native_provider_one_round_allows_read_text_results_above_metadata_fixture_limit() {
        let root_guard = temp_native_provider_root("agent-content-large-read");
        let root_path = root_guard.path();
        let large_readme = "native provider content\n".repeat(32);
        assert!(
            large_readme.len() > NativeToolContinuationPolicy::fixture_default().max_result_bytes
        );
        assert!(std::fs::write(root_path.join("README.md"), &large_readme).is_ok());
        let resource_root = NativeResourceRoot::project(root_path);
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-large-read",
            "entry-large-read-user",
            NativeRole::User,
            "read README",
        );
        let turn_id = NativeTurnId(String::from("turn-large-read"));
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
            NativeProviderAgentToolRound {
                session_id: &NativeSessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
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
            Ok(NativeProviderRoundResult {
                text: String::from("read complete"),
                provider_response_id: Some(String::from("response-2")),
            })
        );
        assert_eq!(requester.requests.len(), 2);
        assert!(requester.requests[1].messages.iter().any(|message| {
            message.role == NativeRole::Tool && message.content.contains("native provider content")
        }));
    }

    #[test]
    fn native_runner_prepares_and_applies_local_edit() {
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
            let handle = tokio::spawn(super::run_native_dogfood_loop(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
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
            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            let permission_summaries = log.events.iter().filter_map(|event| match event {
                NativeSessionEvent::PermissionDecisionRecorded { summary, .. } => Some(summary),
                _ => None,
            });
            assert!(permission_summaries.clone().any(|summary| {
                summary.outcome == NativePermissionDecisionOutcome::NeedsUserReview
                    && !summary.user_override
            }));
            assert!(permission_summaries.clone().any(|summary| {
                summary.outcome == NativePermissionDecisionOutcome::Allowed
                    && summary.reason == "user_approved"
                    && summary.user_override
            }));
            assert!(
                log.events.iter().any(|event| matches!(
                    event,
                    NativeSessionEvent::EditTransactionPrepared { .. }
                ))
            );
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::EditTransactionFinished {
                    outcome: NativeEditEvidenceOutcome::ApplyStarted,
                    reason: Some(reason),
                    ..
                } if reason == "apply_started"
            )));
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::EditTransactionFinished {
                    outcome: NativeEditEvidenceOutcome::Completed,
                    summary: Some(summary),
                    ..
                } if summary.operations.iter().all(|operation| matches!(
                    operation,
                    NativeEditOperationEvidence::ModifyTextFile {
                        bytes_written: Some(_),
                        ..
                    } | NativeEditOperationEvidence::CreateTextFile {
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
    fn native_local_edit_error_messages_are_categorical() {
        let message = native_local_edit_error_message(&NativeEditAccessError::Preview(
            NativeEditError::HashMismatch {
                path: String::from("/private/project/secrets.txt"),
                expected_sha256: String::from("expected-secret-hash"),
                actual_sha256: String::from("actual-secret-hash"),
            },
        ));

        assert!(message.contains("hash_mismatch"));
        assert!(!message.contains("/private/project"));
        assert!(!message.contains("expected-secret-hash"));
        assert!(!message.contains("actual-secret-hash"));
    }

    #[test]
    fn native_runner_rejects_stale_local_edit_decision() {
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
            let handle = tokio::spawn(super::run_native_dogfood_loop(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
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
            let log = NativeJsonlSessionStore::new(session_path)
                .load()
                .unwrap_or_default();
            assert!(log.events.is_empty());

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_runner_unconfigured_provider_prompt_fails_with_setup_error() {
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
                "native provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY";
            let handle = tokio::spawn(super::run_native_dogfood_loop(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
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

            let log = NativeJsonlSessionStore::new(session_path)
                .load()
                .unwrap_or_default();
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::EntryAppended {
                    role: NativeRole::User,
                    text,
                    ..
                } if text == "hello without provider config"
            )));
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::TurnFinished {
                    outcome: NativeTurnOutcome::Failed,
                    reason: Some(reason),
                    ..
                } if reason.starts_with("provider_unconfigured ") && reason.contains(setup_error)
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_runner_does_not_apply_when_local_edit_evidence_preflight_fails() {
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
            let handle = tokio::spawn(super::run_native_dogfood_loop(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
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
    fn native_provider_agent_edit_tool_pauses_for_user_review_and_continues() {
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
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: NativeTurnId(String::from("turn-1")),
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
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        delta: String::from("edit applied"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::ToolRequestRecorded {
                    provider_call_id: Some(id),
                    tool_name,
                    ..
                } if id == "call-edit-1" && tool_name == "edit_text_file"
            )));
            let traces = edit_trace_records(&log);
            let preview_trace = traces.iter().find(|trace| {
                trace.phase == NativeEditTracePhase::Preview
                    && trace.provider_call_id.as_deref() == Some("call-edit-1")
            });
            assert!(preview_trace.is_some());
            let Some(preview_trace) = preview_trace else {
                return;
            };
            let trace_id = preview_trace.trace_id.clone();
            assert!(traces.iter().any(|trace| {
                trace.trace_id == trace_id
                    && trace.phase == NativeEditTracePhase::ReviewWait
                    && trace.outcome == NativeEditTraceOutcome::Completed
                    && trace.provider_call_id.as_deref() == Some("call-edit-1")
                    && trace.tool_request_id
                        == Some(NativeToolRequestId(String::from("tool-request-1-1")))
                    && trace.preview_id.is_some()
                    && trace.permission_decision_id.is_some()
            }));
            assert!(traces.iter().any(|trace| {
                trace.trace_id == trace_id
                    && trace.phase == NativeEditTracePhase::ProviderContinuation
                    && trace.outcome == NativeEditTraceOutcome::Completed
                    && trace.provider_call_id.as_deref() == Some("call-edit-1")
                    && trace.tool_request_id
                        == Some(NativeToolRequestId(String::from("tool-request-1-1")))
            }));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_provider_agent_edit_continuation_records_each_edit_trace() {
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let traces = vec![
            super::ProviderContinuationEditTrace {
                trace_id: NativeEditTraceId(String::from("edit-trace-1")),
                tool_name: String::from("edit_text_file"),
                tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
                provider_call_id: Some(String::from("call-edit-1")),
                preview_id: Some(NativeEditPreviewId(String::from("edit-preview-1"))),
                permission_decision_id: Some(NativePermissionDecisionId(String::from(
                    "permission-decision-1",
                ))),
            },
            super::ProviderContinuationEditTrace {
                trace_id: NativeEditTraceId(String::from("edit-trace-2")),
                tool_name: String::from("create_text_file"),
                tool_request_id: NativeToolRequestId(String::from("tool-request-2")),
                provider_call_id: Some(String::from("call-edit-2")),
                preview_id: Some(NativeEditPreviewId(String::from("edit-preview-2"))),
                permission_decision_id: Some(NativePermissionDecisionId(String::from(
                    "permission-decision-2",
                ))),
            },
        ];

        record_provider_continuation_trace_records(
            &mut log,
            &mut pending_events,
            None,
            super::ProviderContinuationTraceInput {
                session_id: &NativeSessionId(String::from("default")),
                turn_id: &NativeTurnId(String::from("turn-1")),
                edit_traces: &traces,
                started: std::time::Instant::now(),
                outcome: NativeEditTraceOutcome::Completed,
                reason_label: None,
            },
        );

        let continuation_traces = edit_trace_records(&log)
            .into_iter()
            .filter(|trace| trace.phase == NativeEditTracePhase::ProviderContinuation)
            .collect::<Vec<_>>();
        assert_eq!(continuation_traces.len(), 2);
        assert!(continuation_traces.iter().any(|trace| {
            trace.trace_id == NativeEditTraceId(String::from("edit-trace-1"))
                && trace.tool_name.as_deref() == Some("edit_text_file")
                && trace.tool_request_id
                    == Some(NativeToolRequestId(String::from("tool-request-1")))
                && trace.provider_call_id.as_deref() == Some("call-edit-1")
                && trace.outcome == NativeEditTraceOutcome::Completed
        }));
        assert!(continuation_traces.iter().any(|trace| {
            trace.trace_id == NativeEditTraceId(String::from("edit-trace-2"))
                && trace.tool_name.as_deref() == Some("create_text_file")
                && trace.tool_request_id
                    == Some(NativeToolRequestId(String::from("tool-request-2")))
                && trace.provider_call_id.as_deref() == Some("call-edit-2")
                && trace.outcome == NativeEditTraceOutcome::Completed
        }));
        assert_eq!(pending_events.len(), 2);
    }

    #[test]
    fn native_provider_agent_edit_tool_denial_does_not_continue_provider_round() {
        let mut requester = FakeProviderRequester::with_responses([Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: NativeTurnId(String::from("turn-1")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
            },
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: NativeTurnId(String::from("turn-1")),
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
                turn_id: NativeTurnId(String::from("turn-1")),
                finish_reason: Some(ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ])]);
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        let root_guard = temp_native_provider_root("agent-edit-denied");
        let resource_root = NativeResourceRoot::project(root_guard.path());
        assert!(resource_root.is_ok());
        let Ok(resource_root) = resource_root else {
            return;
        };
        let (_review_tx, review_rx) = mpsc::unbounded_channel();
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let turn_id = NativeTurnId(String::from("turn-1"));

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            NativeProviderAgentToolRound {
                session_id: &NativeSessionId(String::from("default")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
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
            Err(NativeProviderRoundError::ToolExecutionDenied { .. })
        ));
        assert_eq!(requester.requests.len(), 1);
    }

    #[test]
    fn native_provider_agent_duplicate_create_fails_tool_and_continues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-agent-duplicate-create");
            root.write("dogfood.txt", "existing content\n");
            let session_path = root.root().join("session.jsonl");
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let provider = FakeProviderRequester::with_responses([
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-create-1"),
                            name: String::from("create_text_file"),
                            arguments_json: serde_json::json!({
                                "path": "dogfood.txt",
                                "content": "hello"
                            }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        delta: String::from("the file already exists"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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
                        prompt: String::from("create dogfood.txt"),
                    })
                    .is_ok()
            );

            let (deltas, finished) = recv_prompt_deltas_until_finished(&mut backend_rx).await;
            assert_eq!(finished, Some(PromptOutcome::Completed));
            assert!(deltas.join("").contains("the file already exists"));
            assert_eq!(
                std::fs::read_to_string(root.root().join("dogfood.txt")).ok(),
                Some(String::from("existing content\n"))
            );

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Failed,
                    reason: Some(reason),
                    ..
                } if reason == "target_exists"
            )));
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::TurnFinished {
                    outcome: NativeTurnOutcome::Completed,
                    ..
                }
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_provider_agent_sensitive_read_fails_tool_and_continues() {
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
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({"path": ".env"}),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        delta: String::from("that file is protected"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Failed,
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
                    turn_id: NativeTurnId(String::from("turn-1")),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-bash-1"),
                        name: String::from("bash"),
                        arguments_json: serde_json::json!({ "command": command }),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    delta: String::from(final_text),
                },
                ProviderStreamEvent::Completed {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
        ]
    }

    #[test]
    fn native_provider_agent_bash_review_approval_runs_command() {
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

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Completed,
                    result_content: Some(content),
                    ..
                } if content.contains("\"exit_code\":4")
                    && content.contains("run-evidence")
                    && content.contains("\"approved_by\":\"user\"")
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_provider_agent_bash_review_rejection_fails_tool_and_continues() {
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

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Failed,
                    reason: Some(reason),
                    ..
                } if reason == "user_rejected"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_provider_agent_bash_allowlist_auto_runs_without_review() {
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

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Completed,
                    result_content: Some(content),
                    ..
                } if content.contains("allowlist-evidence")
                    && content.contains("\"approved_by\":\"allowlist\"")
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    fn seed_completed_turn(session_path: &Path, turn: &str, user_text: &str) {
        let store = NativeJsonlSessionStore::new(session_path.to_path_buf());
        let session_id = NativeSessionId(String::from("default"));
        let mut events = vec![
            NativeSessionEvent::EntryAppended {
                session_id: session_id.clone(),
                entry_id: NativeEntryId(format!("{turn}-user")),
                parent_entry_id: None,
                turn_id: NativeTurnId(String::from(turn)),
                role: NativeRole::User,
                text: String::from(user_text),
                provider: None,
            },
            NativeSessionEvent::EntryAppended {
                session_id: session_id.clone(),
                entry_id: NativeEntryId(format!("{turn}-assistant")),
                parent_entry_id: None,
                turn_id: NativeTurnId(String::from(turn)),
                role: NativeRole::Assistant,
                text: String::from("acknowledged"),
                provider: None,
            },
            NativeSessionEvent::TurnFinished {
                session_id,
                turn_id: NativeTurnId(String::from(turn)),
                outcome: NativeTurnOutcome::Completed,
                reason: None,
            },
        ];
        assert!(super::append_pending_native_session_events(&store, &mut events).is_ok());
    }

    fn provider_text_response(text: &str) -> Vec<ProviderStreamEvent> {
        vec![
            ProviderStreamEvent::Started {
                turn_id: NativeTurnId(String::from("turn-1")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
            },
            ProviderStreamEvent::TextDelta {
                turn_id: NativeTurnId(String::from("turn-1")),
                delta: String::from(text),
            },
            ProviderStreamEvent::Completed {
                turn_id: NativeTurnId(String::from("turn-1")),
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
    fn native_provider_agent_threshold_compaction_checkpoints_and_continues() {
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

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::CompactionCheckpoint {
                    reason: crate::NativeCompactionReason::Threshold,
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
    fn native_manual_compaction_checkpoints_and_refreshes_session_views() {
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

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::CompactionCheckpoint {
                    reason: crate::NativeCompactionReason::Manual,
                    summary,
                    ..
                } if summary == "manual anchored summary"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_model_selection_switches_next_prompt_and_rejects_other_providers() {
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

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            // The assistant entry's provider metadata records the switched
            // model, proving the request went out with it.
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::EntryAppended {
                    role: NativeRole::Assistant,
                    provider: Some(metadata),
                    ..
                } if metadata.model == "claude-opus-4-8"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_oversized_read_fails_recoverably_and_turn_continues() {
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
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({ "path": "big.jsonl" }),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(provider_text_response(
                    "the file is too large; sampling with bash instead",
                )),
            ]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Failed,
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
    fn native_manual_compaction_reports_nothing_to_fold() {
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

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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
    fn native_provider_agent_overflow_error_compacts_and_retries() {
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

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

            let log = NativeJsonlSessionStore::new(session_path).load();
            assert!(log.is_ok());
            let Ok(log) = log else {
                return;
            };
            assert!(log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::CompactionCheckpoint {
                    reason: crate::NativeCompactionReason::Overflow,
                    summary,
                    ..
                } if summary == "overflow recovery summary"
            )));

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_provider_agent_compaction_thrash_guard_fails_turn_visibly() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let root = TempProject::new("native-provider-compaction-thrash");
            root.write(
                ".yach/config.json",
                r#"{"compaction":{"auto_threshold_percent":10,"keep_recent_tokens":200}}"#,
            );
            let session_path = root.root().join("session.jsonl");
            seed_completed_turn(&session_path, "turn-0", &"legacy work ".repeat(10_000));
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            // Only the summary response: the current prompt alone refills
            // the threshold, so the turn must fail before any further
            // provider request.
            let provider = FakeProviderRequester::with_responses([Ok(provider_text_response(
                "summary that cannot save this turn",
            ))]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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
                    .any(|status| status.contains("refilled immediately after compaction"))
            );

            drop(client_tx);
            assert!(handle.await.is_ok());
        });
    }

    #[test]
    fn native_provider_agent_edit_tool_long_path_result_stays_bounded_after_apply() {
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
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: NativeTurnId(String::from("turn-1")),
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
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        delta: String::from("edit applied"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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
    fn native_provider_prompt_uses_in_memory_log_after_startup() {
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
                        turn_id: NativeTurnId(String::from("turn-0")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: NativeTurnId(String::from("turn-0")),
                        delta: String::from("first answer"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-0")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::TextDelta {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        delta: String::from("second answer"),
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_requester_factory(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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
                NativeSessionId(String::from("default")),
                NativeEntryId(String::from("entry-injected-user")),
                NativeEntryId(String::from("entry-injected-assistant")),
                NativeTurnId(String::from("turn-injected")),
                String::from("injected disk prompt"),
                String::from("injected disk answer"),
            );
            let store = NativeJsonlSessionStore::new(session_path);
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
    fn native_dogfood_loop_switches_to_selected_session_path() {
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
                NativeSessionId(String::from("session-a")),
                NativeEntryId(String::from("entry-a-user")),
                NativeEntryId(String::from("entry-a-assistant")),
                NativeTurnId(String::from("turn-a")),
                String::from("prompt from session a"),
                String::from("answer from session a"),
            );
            let session_b_log = completed_text_exchange(
                NativeSessionId(String::from("session-b")),
                NativeEntryId(String::from("entry-b-user")),
                NativeEntryId(String::from("entry-b-assistant")),
                NativeTurnId(String::from("turn-b")),
                String::from("prompt from session b"),
                String::from("answer from session b"),
            );
            assert!(
                NativeJsonlSessionStore::new(session_a_path.clone())
                    .append_events(&session_a_log.events)
                    .is_ok()
            );
            assert!(
                NativeJsonlSessionStore::new(session_b_path.clone())
                    .append_events(&session_b_log.events)
                    .is_ok()
            );

            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(super::run_native_dogfood_loop(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
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
    fn native_provider_agent_edit_tool_mismatched_review_decision_finishes_failed() {
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
                    turn_id: NativeTurnId(String::from("turn-1")),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: NativeTurnId(String::from("turn-1")),
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
                    turn_id: NativeTurnId(String::from("turn-1")),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ])]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path,
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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
    fn native_provider_empty_tool_continuation_emits_no_text_marker() {
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
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::ToolCallCompleted {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        tool_call: ProviderToolCall {
                            call_id: String::from("call-read-1"),
                            name: String::from("read_text_file"),
                            arguments_json: serde_json::json!({"path": "notes.txt"}),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::ToolCalls),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
                Ok(vec![
                    ProviderStreamEvent::Started {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        model: ProviderModel {
                            provider: String::from("fixture"),
                            model: String::from("fixture-model"),
                        },
                    },
                    ProviderStreamEvent::Completed {
                        turn_id: NativeTurnId(String::from("turn-1")),
                        finish_reason: Some(ProviderFinishReason::Stop),
                        usage: None,
                        provider_response_id: None,
                    },
                ]),
            ]);

            let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::NativeDogfoodRunnerConfig {
                    session_path,
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(native_provider_test_config()),
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

    fn native_provider_test_config() -> NativeProviderDogfoodConfig {
        NativeProviderDogfoodConfig {
            adapter: RigProviderAdapterConfig {
                provider: RigProviderConfig::Anthropic {
                    api_key: String::from("test-key"),
                },
                timeout: std::time::Duration::from_secs(30),
                max_tokens: 1000,
                context_window: 200_000,
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
    fn native_provider_one_round_executes_project_path_info_and_continues() {
        let root_guard = temp_native_provider_root("native-provider-one-round-success");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = NativeResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            Some(NativeLaunchProjectContext::from_project_root(root)),
            None,
        ));

        assert_eq!(
            result,
            Ok(NativeProviderRoundResult {
                text: String::from("Cargo.toml is a file."),
                provider_response_id: Some(String::from("response-2")),
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
            .find(|message| message.role == NativeRole::System);
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
        assert_eq!(requester.requests[1].messages[3].role, NativeRole::Tool);
        let tool_message_content = &requester.requests[1].messages[3].content;
        assert!(!tool_message_content.contains(root_path.to_string_lossy().as_ref()));
        assert!(!tool_message_content.contains("\"path\":\"Cargo.toml\""));
        let tool_message = serde_json::from_str::<serde_json::Value>(tool_message_content);
        assert!(
            tool_message.is_ok(),
            "tool message should be json: {tool_message:?}"
        );
        let Ok(tool_message) = tool_message else {
            return;
        };
        assert_eq!(tool_message["provider_call_id"], "provider-call-1");
        let tool_content = tool_message["content"].as_str();
        assert!(
            tool_content.is_some(),
            "tool content should be a json string"
        );
        let Some(tool_content) = tool_content else {
            return;
        };
        let metadata = serde_json::from_str::<serde_json::Value>(tool_content);
        assert!(
            metadata.is_ok(),
            "tool content should be metadata json: {metadata:?}"
        );
        let Ok(metadata) = metadata else {
            return;
        };
        assert_eq!(metadata["relative_path"], "Cargo.toml");
        assert_eq!(metadata["kind"], "file");
        assert_eq!(metadata["provider_visibility"], "never");
        assert!(!tool_content.contains("[package]"));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
        )));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn native_provider_one_round_persists_tool_events_before_continuation_request() {
        let root_guard = temp_native_provider_root("native-provider-tool-event-flush");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let store = NativeJsonlSessionStore::new(root_path.join("session.jsonl"));
        let root = NativeResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            Some(NativeLaunchProjectContext::from_project_root(root)),
            Some(&store),
        ));

        assert_eq!(
            result,
            Ok(NativeProviderRoundResult {
                text: String::from("Cargo.toml is a file."),
                provider_response_id: Some(String::from("response-2")),
            })
        );
        assert_eq!(requester.requests.len(), 2);
        assert!(pending_events.is_empty());
    }

    #[test]
    fn native_provider_one_round_keeps_pending_tool_events_when_flush_fails() {
        let root_guard = temp_native_provider_root("native-provider-tool-event-flush-failure");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let blocked_parent = root_path.join("session-parent");
        assert!(std::fs::write(&blocked_parent, "not a directory").is_ok());
        let store = NativeJsonlSessionStore::new(blocked_parent.join("session.jsonl"));
        let root = NativeResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            Some(NativeLaunchProjectContext::from_project_root(root)),
            Some(&store),
        ));

        assert_eq!(
            result,
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_event_persist_failed"
            )))
        );
        assert_eq!(requester.requests.len(), 1);
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
        )));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn native_provider_tool_batch_configured_round_limit_stops_before_next_provider_request() {
        let root_guard = temp_native_provider_root("native-provider-tool-loop-limit");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = NativeResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
        let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        let permission_policy =
            NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
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
        let mut edit_access = NativeEditAccess::default();
        let edit_sink = NativeProviderBufferedEventSink::new(None);
        let (review_tx, _review_rx) = mpsc::unbounded_channel();
        let (_decision_tx, mut review_decisions) = mpsc::unbounded_channel();
        let mut budget = NativeProviderToolLoopBudget::new(
            NativeProviderToolLoopPolicy::agent_default().with_max_tool_rounds(2),
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
                        .map_err(|error| native_provider_round_error_to_provider_error(&error))
                });
            assert!(round.is_ok());
            let Ok(round) = round else {
                return Ok(());
            };
            execute_native_provider_agent_tool_batch(
                NativeProviderAgentToolBatch {
                    session_id: NativeSessionId(String::from("default")),
                    shell_policy: crate::NativeShellPolicy::default(),
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
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_loop_too_many_rounds"
            )))
        );
        assert_eq!(requester.requests.len(), 1);
        assert!(pending_events.is_empty());
    }

    #[test]
    fn native_provider_agent_default_loop_has_no_round_limit() {
        let root_guard = temp_native_provider_root("native-provider-default-tool-loop");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = NativeResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo repeatedly",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            NativeProviderAgentToolRound {
                session_id: &NativeSessionId(String::from("default")),
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn,
                project_context: Some(NativeLaunchProjectContext::from_project_root(root)),
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
            Ok(NativeProviderRoundResult {
                text: String::from("done after five rounds"),
                provider_response_id: None,
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
            NativeSessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
        )));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn native_provider_one_round_rejects_unknown_tool_before_second_request() {
        let root_guard = temp_native_provider_root("native-provider-unknown-tool");
        let root = NativeResourceRoot::project(root_guard.path()).ok();
        assert!(root.is_some());
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            Some(NativeLaunchProjectContext::from_project_root(root)),
            None,
        ));

        assert_eq!(
            result,
            Err(NativeProviderRoundError::ToolContinuation(String::from(
                "tool_round_validation_failed"
            )))
        );
        assert_eq!(requester.requests.len(), 1);
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::ValidationFailed,
                ..
            }
        )));
    }

    #[test]
    fn native_provider_one_round_maps_second_provider_failure() {
        let root_guard = temp_native_provider_root("native-provider-second-failure");
        let root_path = root_guard.path();
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = NativeResourceRoot::project(root_path).ok();
        assert!(root.is_some());
        let mut log = NativeSessionLog::default();
        let mut pending_events = Vec::new();
        append_native_provider_test_entry(
            &mut log,
            &NativeSessionId(String::from("default")),
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "inspect cargo",
        );
        let turn = NativeTurnId(String::from("turn-0"));
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
            Some(NativeLaunchProjectContext::from_project_root(root)),
            None,
        ));

        assert_eq!(
            result,
            Err(NativeProviderRoundError::Provider(
                ProviderError::malformed_stream("second provider request failed")
            ))
        );
        assert_eq!(requester.requests.len(), 2);
        assert_eq!(requester.requests[1].messages.len(), 4);
        assert_eq!(requester.requests[1].messages[0].role, NativeRole::System);
        assert!(
            requester.requests[1].messages[0]
                .content
                .contains("coding agent running in the yach harness")
        );
        assert_eq!(requester.requests[1].messages[2].role, NativeRole::System);
        assert!(
            requester.requests[1].messages[2]
                .content
                .contains("You may call more advertised tools")
        );
        assert_eq!(requester.requests[1].messages[3].role, NativeRole::Tool);
        assert!(
            requester.requests[1].messages[3]
                .content
                .contains("provider-call-1")
        );
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
        )));
        assert!(pending_events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                ..
            }
        )));
    }

    struct StoreCheckingProviderRequester {
        requests: Vec<ProviderRequest>,
        responses: std::collections::VecDeque<Result<Vec<ProviderStreamEvent>, ProviderError>>,
        store: NativeJsonlSessionStore,
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
                    NativeSessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
                )));
                assert!(stored_log.events.iter().any(|event| matches!(
                    event,
                    NativeSessionEvent::ToolExecutionFinished {
                        outcome: NativeToolOutcome::Completed,
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
    fn native_provider_messages_exclude_failed_prior_turns() {
        let session_id = NativeSessionId(String::from("default"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "failed prompt",
        );
        finish_native_provider_test_turn(
            &mut log,
            &session_id,
            "turn-0",
            NativeTurnOutcome::Failed,
        );
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "current prompt",
        );

        assert_eq!(
            native_provider_messages_from_log(&log, &NativeTurnId(String::from("turn-1"))),
            vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("current prompt"),
            }]
        );
    }

    #[test]
    fn native_provider_messages_exclude_cancelled_prior_turns() {
        let session_id = NativeSessionId(String::from("default"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-0",
            "entry-0-user",
            NativeRole::User,
            "cancelled prompt",
        );
        finish_native_provider_test_turn(
            &mut log,
            &session_id,
            "turn-0",
            NativeTurnOutcome::Cancelled,
        );
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "current prompt",
        );

        assert_eq!(
            native_provider_messages_from_log(&log, &NativeTurnId(String::from("turn-1"))),
            vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("current prompt"),
            }]
        );
    }

    #[test]
    fn native_session_load_warning_status_preserves_valid_events() {
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
            NativeSessionId(String::from("default")),
            NativeEntryId(String::from("entry-user-0")),
            NativeEntryId(String::from("entry-assistant-0")),
            NativeTurnId(String::from("turn-0")),
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
        let store = NativeJsonlSessionStore::new(path);
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
    fn native_session_startup_load_runs_on_blocking_thread() {
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
                Ok(NativeSessionLoadResult {
                    log: NativeSessionLog::default(),
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
    fn native_session_messages_include_tool_execution_results() {
        let session_id = NativeSessionId(String::from("default"));
        let turn_id = NativeTurnId(String::from("turn-1"));
        let tool_request_id = NativeToolRequestId(String::from("tool-request-1"));
        let mut log = NativeSessionLog::default();
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-user",
            NativeRole::User,
            "read file",
        );
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("read_text_file"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("path=README.md"),
                byte_count: 16,
                redacted: false,
                truncated: false,
            },
            argument_content: None,
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id,
            tool_request_id,
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(NativeToolPayloadSummary {
                summary: String::from("read_text_file result redacted"),
                byte_count: 56,
                redacted: true,
                truncated: false,
            }),
            result_content: Some(String::from(
                "{\"path\":\"README.md\",\"content\":\"hello\",\"truncated\":false}",
            )),
        });
        append_native_provider_test_entry(
            &mut log,
            &session_id,
            "turn-1",
            "entry-1-assistant",
            NativeRole::Assistant,
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
        assert!(messages[1].text.contains("bytes=56"));
    }

    #[test]
    fn native_session_messages_render_persisted_tool_content_like_live_progress() {
        let session_id = NativeSessionId(String::from("default"));
        let turn_id = NativeTurnId(String::from("turn-1"));
        let tool_request_id = NativeToolRequestId(String::from("tool-request-1"));
        let list_content = serde_json::json!({
            "outcome": "list",
            "entries": [
                {"path": "src/lib.rs", "kind": "file"},
                {"path": "src/main.rs", "kind": "file"},
            ],
            "truncated": false,
        })
        .to_string();
        let live_result = NativeProviderToolResult {
            tool_request_id: tool_request_id.0.clone(),
            provider_call_id: Some(String::from("call-1")),
            status: NativeToolOutcome::Completed,
            byte_count: list_content.len(),
            content: list_content.clone(),
            redacted: true,
            truncated: false,
            reason: None,
        };
        let live_display = native_provider_tool_progress_output("list_project_paths", &live_result);

        let mut log = NativeSessionLog::default();
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("list_project_paths"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 15,
                redacted: true,
                truncated: false,
            },
            argument_content: Some(String::from("{\"path\":\"src\"}")),
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id,
            turn_id,
            tool_request_id,
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(NativeToolPayloadSummary {
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
        assert!(messages[0].text.contains("completed: 2 entries"));
        assert!(messages[0].text.contains("file src/lib.rs"));
        assert!(messages[0].text.contains("file src/main.rs"));
    }

    #[test]
    fn native_session_messages_note_missing_content_for_pre_persistence_logs() {
        let session_id = NativeSessionId(String::from("default"));
        let turn_id = NativeTurnId(String::from("turn-1"));
        let tool_request_id = NativeToolRequestId(String::from("tool-request-1"));
        let mut log = NativeSessionLog::default();
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("search_project"),
            provider_call_id: Some(String::from("call-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 20,
                redacted: true,
                truncated: false,
            },
            argument_content: None,
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id,
            turn_id,
            tool_request_id,
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(NativeToolPayloadSummary {
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
        log: &mut NativeSessionLog,
        session_id: &NativeSessionId,
        turn_id: &str,
        entry_id: &str,
        role: NativeRole,
        text: &str,
    ) {
        log.push(NativeSessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: NativeEntryId(entry_id.to_owned()),
            parent_entry_id: None,
            turn_id: NativeTurnId(turn_id.to_owned()),
            role,
            text: text.to_owned(),
            provider: None,
        });
    }

    fn finish_native_provider_test_turn(
        log: &mut NativeSessionLog,
        session_id: &NativeSessionId,
        turn_id: &str,
        outcome: NativeTurnOutcome,
    ) {
        log.push(NativeSessionEvent::TurnFinished {
            session_id: session_id.clone(),
            turn_id: NativeTurnId(turn_id.to_owned()),
            outcome,
            reason: None,
        });
    }

    struct NativeProviderTempRoot {
        path: std::path::PathBuf,
    }

    impl NativeProviderTempRoot {
        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for NativeProviderTempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn temp_native_provider_root(label: &str) -> NativeProviderTempRoot {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "yach-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        assert!(std::fs::create_dir_all(&path).is_ok());
        NativeProviderTempRoot { path }
    }
}
