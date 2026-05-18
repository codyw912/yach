use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use yach_proto::{
    BackendEvent, BackendState, Capability, ClientEvent, Handshake, LocalEditDecision,
    LocalEditFinishedOutcome, LocalEditOperationInput, LocalEditPreviewSummary,
    LocalEditReviewState, ModelInfo, PromptOutcome, RecentSession, ServerEvent, SessionMessage,
    SessionStats, ToolReviewPayload,
};

use crate::agent_edit_tools::{
    NativeAgentEditToolContext, NativeAgentEditToolPrepared, PendingAgentEditToolReview,
    apply_agent_edit_tool_review, prepare_agent_edit_tool_request, reject_agent_edit_tool_review,
};
use crate::rig_adapter::{
    RigProviderAdapterConfig, RigProviderConfig, run_provider_request_with_approved_tools,
};
use crate::{
    NativeDurationMetric, NativeEditAccess, NativeEditAccessContext, NativeEditAccessError,
    NativeEditAccessReviewState, NativeEditHunk, NativeEditOperation, NativeEditPolicy,
    NativeEditPreview, NativeEditPreviewId, NativeEditTraceId, NativeEditTraceOutcome,
    NativeEditTracePhase, NativeEditTraceRecord, NativeEditTraceSource,
    NativeEditTransactionRequest, NativeEntryId, NativeJsonlSessionStore, NativeMetricAttribute,
    NativePermissionDecisionId, NativePermissionPolicy, NativeProviderToolResult,
    NativeResourceRoot, NativeRole, NativeSessionEvent, NativeSessionEventSink, NativeSessionId,
    NativeSessionLog, NativeStaticContextBundle, NativeStaticContextPolicy,
    NativeToolContinuationError, NativeToolContinuationPolicy, NativeToolExecutionResult,
    NativeToolExecutor, NativeToolOutcome, NativeToolPayloadSummary, NativeToolPermissionPolicy,
    NativeToolRegistry, NativeToolRequestId, NativeTurnId, NativeTurnOutcome,
    PendingNativeToolRequest, ProjectReadOnlyToolExecutor, ProviderContinuationMappingError,
    ProviderContinuationRequest, ProviderContinuationValidationPolicy, ProviderError,
    ProviderErrorKind, ProviderFinishReason, ProviderMessage, ProviderMetadata, ProviderModel,
    ProviderRequest, ProviderStreamEvent, ProviderToolAdvertisingError, ProviderToolCall,
    assemble_project_static_context, build_provider_continuation_submission,
    build_provider_tool_advertising_extension, native_edit_error_label,
    pending_tool_request_from_provider_call, record_native_tool_validation,
};
#[cfg(test)]
use crate::{NativeToolContinuationContext, NativeToolContinuationWorkflow};

/// Native dogfood runner configuration owned by the backend Module.
#[derive(Debug, Clone)]
pub struct NativeDogfoodRunnerConfig {
    pub session_path: PathBuf,
    pub project_root: Option<PathBuf>,
    pub provider: Option<NativeProviderDogfoodConfig>,
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
    handle: tokio::task::JoinHandle<()>,
    turn_id: NativeTurnId,
    prompt_started: Instant,
    review_decision_tx: mpsc::UnboundedSender<AgentEditReviewDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentEditReviewDecision {
    request_id: String,
    preview_id: String,
    permission_decision_id: String,
    decision: LocalEditDecision,
}

type AgentEditDecisionReceiver = mpsc::UnboundedReceiver<AgentEditReviewDecision>;

#[must_use]
pub fn native_session_log_path(session_id: &str) -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".yach")
        .join("native-sessions")
        .join(format!("{session_id}.jsonl"))
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
        session_path,
        project_root,
        provider,
    } = config;
    let store = NativeJsonlSessionStore::new(session_path.clone());
    let provider_project_context = project_root
        .as_ref()
        .and_then(native_launch_project_context_from_root);
    let edit_root = native_local_edit_root(project_root.clone());
    let mut edit_access = NativeEditAccess::default();
    send_native_initial_state(&tx, &session_path, provider.as_ref());
    let mut turn_index = store.load().unwrap_or_default().next_turn_index();
    let mut local_edit_index = turn_index;
    let mut active_provider_turn: Option<ActiveProviderTurn> = None;

    while let Some(event) = rx.recv().await {
        if active_provider_turn
            .as_ref()
            .is_some_and(|turn| turn.handle.is_finished())
        {
            active_provider_turn = None;
        }
        match event {
            ClientEvent::Initialize(_) => {
                send_native_initial_state(&tx, &session_path, provider.as_ref());
            }
            ClientEvent::AvailableModelsRequested => {
                send_native_models(&tx, provider.as_ref());
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
                        active.turn_id,
                        active.prompt_started,
                        "native provider prompt cancelled",
                    );
                }
            }
            ClientEvent::RecentSessionsRequested => send_native_recent_sessions(&tx, &session_path),
            ClientEvent::SessionMessagesRequested => {
                send_native_session_messages(&tx, &session_path);
            }
            ClientEvent::SessionStatsRequested => send_native_session_stats(&tx, &session_path),
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
                        session_id,
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
                        provider_project_context.clone(),
                        review_decision_rx,
                    ));
                    active_provider_turn = Some(ActiveProviderTurn {
                        handle,
                        turn_id,
                        prompt_started,
                        review_decision_tx,
                    });
                } else {
                    let prompt_started = Instant::now();
                    handle_native_prompt(
                        &tx,
                        &store,
                        session_id,
                        &prompt,
                        prompt_turn_index,
                        prompt_started,
                    );
                }
            }
            ClientEvent::ModelSelected { model } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::ModelChanged { model }));
            }
            ClientEvent::ModelSelectedDetailed { provider, model_id } => {
                let model = format!("{provider}/{model_id}");
                let _ = tx.send(BackendEvent::Server(ServerEvent::ModelChanged { model }));
            }
            ClientEvent::SessionSelected { session_id } if session_id == "default" => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::SessionChanged {
                    session_id,
                }));
            }
            ClientEvent::SessionSelected { session_id } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!("native dogfood: unknown session {session_id}"),
                }));
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
                    request_id,
                    operation,
                    edit_turn_index,
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
            ClientEvent::SessionPathSelected { .. }
            | ClientEvent::DialogResolved { .. }
            | ClientEvent::WidgetCleared { .. } => {}
        }
    }
}

fn send_native_initial_state(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    provider: Option<&NativeProviderDogfoodConfig>,
) {
    let session_file = Some(session_path.to_string_lossy().into_owned());
    let _ = tx.send(BackendEvent::Server(ServerEvent::Ready {
        handshake: Handshake::new(
            "yach-native-dogfood",
            vec![
                Capability::PromptStreaming,
                Capability::PromptCancellation,
                Capability::LocalEdit,
            ],
        ),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::StateUpdated(
        BackendState {
            model_id: Some(native_active_model(provider).id),
            model_name: Some(native_active_model(provider).name),
            model_provider: Some(native_active_model(provider).provider),
            session_id: Some(String::from("default")),
            session_file,
            thinking_level: Some(String::from("low")),
            is_streaming: false,
            is_compacting: false,
            message_count: native_session_message_count(session_path),
            pending_message_count: Some(0),
        },
    )));
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: native_status_message(provider),
    }));
    send_native_models(tx, provider);
}

fn native_local_edit_root(project_root: Option<PathBuf>) -> Result<NativeResourceRoot, String> {
    let root_path = project_root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    NativeResourceRoot::project(&root_path).map_err(|error| {
        format!(
            "native dogfood: local edit root unavailable at {}: {error}",
            root_path.display()
        )
    })
}

fn handle_native_local_edit_prepare(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    edit_access: &mut NativeEditAccess,
    edit_root: Result<&NativeResourceRoot, &String>,
    request_id: String,
    operation: LocalEditOperationInput,
    turn_index: u64,
) {
    let Ok(edit_root) = edit_root else {
        let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
            preview_id: None,
            outcome: LocalEditFinishedOutcome::Failed,
            message: edit_root
                .err()
                .cloned()
                .unwrap_or_else(|| String::from("native dogfood: local edit root unavailable")),
        }));
        return;
    };
    let LocalEditRequestParts {
        request,
        path,
        operation,
    } = native_local_edit_request_from_input(operation);
    let mut log = NativeSessionLog::default();
    let context = NativeEditAccessContext {
        session_id: NativeSessionId(String::from("default")),
        turn_id: NativeTurnId(format!("turn-{turn_index}")),
        permission_policy: NativePermissionPolicy::default_local_edit(),
        edit_policy: NativeEditPolicy::conservative(),
        tool_request_id: None,
    };

    match edit_access.prepare(edit_root, request, context, &mut log) {
        Ok(preview) => {
            if let Err(error) = store.append_events(&log.events) {
                let mut discard_log = NativeSessionLog::default();
                let _ = edit_access.reject(
                    &preview.preview_id,
                    &preview.permission_decision_id,
                    &mut discard_log,
                );
                let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                    preview_id: None,
                    outcome: LocalEditFinishedOutcome::Failed,
                    message: format!(
                        "native dogfood: failed to persist local edit evidence: {error}"
                    ),
                }));
                return;
            }
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditPreviewReady {
                request_id,
                preview: native_local_edit_preview_summary(preview, path, operation),
            }));
        }
        Err(NativeEditAccessError::PermissionDenied { reason }) => {
            let outcome = if store.append_events(&log.events).is_ok() {
                LocalEditFinishedOutcome::Denied
            } else {
                LocalEditFinishedOutcome::Failed
            };
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                preview_id: None,
                outcome,
                message: format!("native dogfood: local edit denied: {reason}"),
            }));
        }
        Err(error) => {
            let _ = store.append_events(&log.events);
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                preview_id: None,
                outcome: LocalEditFinishedOutcome::Failed,
                message: native_local_edit_error_message(&error),
            }));
        }
    }
}

fn handle_native_local_edit_decision(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    edit_access: &mut NativeEditAccess,
    preview_id: String,
    permission_decision_id: String,
    decision: LocalEditDecision,
) {
    let preview_id = NativeEditPreviewId(preview_id);
    let decision_id = NativePermissionDecisionId(permission_decision_id);
    match decision {
        LocalEditDecision::Apply => {
            match edit_access.apply_with_evidence_sink(&preview_id, &decision_id, store) {
                Ok((_, true)) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Applied,
                        message: String::from("native dogfood: local edit applied"),
                    }));
                }
                Ok((_, false)) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Applied,
                        message: String::from(
                            "native dogfood: local edit applied; completed evidence persist failed",
                        ),
                    }));
                }
                Err(error) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Failed,
                        message: native_local_edit_error_message(&error),
                    }));
                }
            }
        }
        LocalEditDecision::Reject => {
            let mut log = NativeSessionLog::default();
            if let Err(error) = store.append_events(&[]) {
                let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                    preview_id: Some(preview_id.0),
                    outcome: LocalEditFinishedOutcome::Failed,
                    message: format!(
                        "native dogfood: failed to persist local edit evidence: {error}"
                    ),
                }));
                return;
            }
            match edit_access.reject(&preview_id, &decision_id, &mut log) {
                Ok(()) => {
                    if let Err(error) = store.append_events(&log.events) {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                            preview_id: Some(preview_id.0),
                            outcome: LocalEditFinishedOutcome::Failed,
                            message: format!(
                                "native dogfood: failed to persist local edit evidence: {error}"
                            ),
                        }));
                        return;
                    }
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Rejected,
                        message: String::from("native dogfood: local edit rejected"),
                    }));
                }
                Err(error) => {
                    let _ = store.append_events(&log.events);
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Failed,
                        message: native_local_edit_error_message(&error),
                    }));
                }
            }
        }
    }
}

struct LocalEditRequestParts {
    request: NativeEditTransactionRequest,
    path: String,
    operation: String,
}

fn native_local_edit_request_from_input(input: LocalEditOperationInput) -> LocalEditRequestParts {
    match input {
        LocalEditOperationInput::ModifyTextFile {
            path,
            expected_sha256,
            find,
            replace,
        } => LocalEditRequestParts {
            request: NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: path.clone(),
                    expected_sha256,
                    hunks: vec![NativeEditHunk { find, replace }],
                }],
            },
            path,
            operation: String::from("modify_text_file"),
        },
        LocalEditOperationInput::CreateTextFile { path, content } => LocalEditRequestParts {
            request: NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: path.clone(),
                    content,
                }],
            },
            path,
            operation: String::from("create_text_file"),
        },
    }
}

fn native_local_edit_preview_summary(
    preview: NativeEditPreview,
    path: String,
    operation: String,
) -> LocalEditPreviewSummary {
    let review_state = native_local_edit_review_state(&preview.review_state);
    LocalEditPreviewSummary {
        preview_id: preview.preview_id.0,
        transaction_id: preview.transaction_id.0,
        permission_decision_id: preview.permission_decision_id.0,
        path,
        operation,
        review_state,
        diff_summary: preview.diff_summary,
        diff_summary_truncated: preview.diff_summary_truncated,
    }
}

const fn native_local_edit_review_state(
    review_state: &NativeEditAccessReviewState,
) -> LocalEditReviewState {
    match review_state {
        NativeEditAccessReviewState::Allowed => LocalEditReviewState::Allowed,
        NativeEditAccessReviewState::NeedsUserApproval => LocalEditReviewState::NeedsUserApproval,
        NativeEditAccessReviewState::AutoReviewUnavailable => {
            LocalEditReviewState::AutoReviewUnavailable
        }
    }
}

fn native_local_edit_error_message(error: &NativeEditAccessError) -> String {
    match error {
        NativeEditAccessError::PermissionDenied { reason } => {
            format!("native dogfood: local edit denied: {reason}")
        }
        NativeEditAccessError::Preview(error) => {
            format!(
                "native dogfood: local edit preview failed: {}",
                native_edit_error_label(error)
            )
        }
        NativeEditAccessError::Apply(error) => {
            format!(
                "native dogfood: local edit apply failed: {}",
                native_edit_error_label(error)
            )
        }
        NativeEditAccessError::PreviewNotFound => {
            String::from("native dogfood: stale local edit preview")
        }
        NativeEditAccessError::DecisionMismatch => {
            String::from("native dogfood: stale local edit permission decision")
        }
        NativeEditAccessError::EvidencePersistFailed => {
            String::from("native dogfood: failed to persist local edit evidence")
        }
    }
}

fn send_native_models(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    provider: Option<&NativeProviderDogfoodConfig>,
) {
    let _ = tx.send(BackendEvent::Server(ServerEvent::AvailableModelsUpdated {
        models: vec![native_active_model(provider)],
    }));
}

fn native_active_model(provider: Option<&NativeProviderDogfoodConfig>) -> ModelInfo {
    let Some(provider) = provider else {
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

fn native_status_message(provider: Option<&NativeProviderDogfoodConfig>) -> String {
    if let Some(provider) = provider {
        let model = native_active_model(Some(provider));
        format!(
            "backend: native provider dogfood via {}/{}; tools/resources unavailable",
            model.provider, model.id
        )
    } else {
        String::from(
            "backend: native dogfood; local read-only project inspection available; provider tools unavailable",
        )
    }
}

fn handle_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    session_id: String,
    prompt: &str,
    turn_index: u64,
    prompt_started: Instant,
) {
    let session_id = if session_id.is_empty() {
        String::from("default")
    } else {
        session_id
    };
    if session_id != "default" {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native dogfood: unknown session {session_id}"),
        }));
        return;
    }

    let turn_id = NativeTurnId(format!("turn-{turn_index}"));
    let user_entry_id = NativeEntryId(format!("entry-{turn_index}-user"));
    let assistant_entry_id = NativeEntryId(format!("entry-{turn_index}-assistant"));
    let response = format!("native dogfood fixture response: {prompt}");
    let fixture_outcome = native_fixture_outcome(prompt);
    let log_load_started = Instant::now();
    let mut log = store.load().unwrap_or_default();
    let mut pending_events = Vec::new();
    push_native_session_event(
        &mut log,
        &mut pending_events,
        native_duration_metric_event(
            Some(turn_id.clone()),
            "session_log_load",
            log_load_started.elapsed(),
        ),
    );
    push_native_session_event(
        &mut log,
        &mut pending_events,
        NativeSessionEvent::EntryAppended {
            session_id: NativeSessionId(String::from("default")),
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
                        session_id: String::from("default"),
                        delta,
                    }))
                    .is_err()
                {
                    push_native_prompt_total_metric(
                        &mut log,
                        &mut pending_events,
                        &turn_id,
                        prompt_started,
                    );
                    push_native_session_event(
                        &mut log,
                        &mut pending_events,
                        NativeSessionEvent::TurnFinished {
                            session_id: NativeSessionId(String::from("default")),
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
                &mut log,
                &mut pending_events,
                &turn_id,
                prompt_started,
            );
            push_native_session_event(
                &mut log,
                &mut pending_events,
                NativeSessionEvent::EntryAppended {
                    session_id: NativeSessionId(String::from("default")),
                    entry_id: assistant_entry_id,
                    parent_entry_id: Some(user_entry_id),
                    turn_id: turn_id.clone(),
                    role: NativeRole::Assistant,
                    text: response,
                    provider: None,
                },
            );
            push_native_session_event(
                &mut log,
                &mut pending_events,
                NativeSessionEvent::TurnFinished {
                    session_id: NativeSessionId(String::from("default")),
                    turn_id,
                    outcome: NativeTurnOutcome::Completed,
                    reason: None,
                },
            );
        }
        NativeFixtureOutcome::Failed => {
            push_native_prompt_total_metric(
                &mut log,
                &mut pending_events,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                &mut log,
                &mut pending_events,
                turn_id,
                NativeTurnOutcome::Failed,
                &ProviderError::fixture_failure(),
            );
        }
        NativeFixtureOutcome::Malformed => {
            push_native_prompt_total_metric(
                &mut log,
                &mut pending_events,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                &mut log,
                &mut pending_events,
                turn_id,
                NativeTurnOutcome::Failed,
                &ProviderError::malformed_stream("native dogfood fixture malformed stream"),
            );
        }
        NativeFixtureOutcome::Cancelled => {
            push_native_prompt_total_metric(
                &mut log,
                &mut pending_events,
                &turn_id,
                prompt_started,
            );
            persist_native_fixture_error(
                tx,
                &mut log,
                &mut pending_events,
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
        session_id: String::from("default"),
        outcome,
        message: Some(status),
    }));
    send_native_session_stats(tx, store.path());
}

#[derive(Debug, Clone)]
struct StartedNativePrompt {
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
    session_id: String,
    prompt: String,
    turn_index: u64,
    prompt_started: Instant,
) -> Option<StartedNativePrompt> {
    let session_id = if session_id.is_empty() {
        String::from("default")
    } else {
        session_id
    };
    if session_id != "default" {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native dogfood: unknown session {session_id}"),
        }));
        return None;
    }

    let turn = NativeTurnId(format!("turn-{turn_index}"));
    let user_entry = NativeEntryId(format!("entry-{turn_index}-user"));
    let assistant_entry = NativeEntryId(format!("entry-{turn_index}-assistant"));
    let log_load_started = Instant::now();
    let mut log = store.load().unwrap_or_default();
    let mut pending_events = Vec::new();
    push_native_session_event(
        &mut log,
        &mut pending_events,
        native_duration_metric_event(
            Some(turn.clone()),
            "session_log_load",
            log_load_started.elapsed(),
        ),
    );
    push_native_session_event(
        &mut log,
        &mut pending_events,
        NativeSessionEvent::EntryAppended {
            session_id: NativeSessionId(String::from("default")),
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
        prompt,
        log,
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
    turn_id: &NativeTurnId,
    prompt_started: Instant,
) {
    push_native_session_event(
        log,
        pending_events,
        native_duration_metric_event(
            Some(turn_id.clone()),
            "native_prompt_total",
            prompt_started.elapsed(),
        ),
    );
}

fn native_duration_metric_event(
    turn_id: Option<NativeTurnId>,
    name: impl Into<String>,
    duration: Duration,
) -> NativeSessionEvent {
    let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    NativeSessionEvent::MetricRecorded {
        session_id: NativeSessionId(String::from("default")),
        turn_id,
        metric: NativeDurationMetric {
            name: name.into(),
            duration_ms,
            attributes: Vec::new(),
        },
    }
}

fn native_provider_messages_from_log(
    log: &NativeSessionLog,
    current_turn_id: &NativeTurnId,
) -> Vec<ProviderMessage> {
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
            | NativeSessionEvent::EditTransactionFinished { .. } => None,
        })
        .collect::<std::collections::HashSet<_>>();

    log.events
        .iter()
        .filter_map(|event| match event {
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
            NativeSessionEvent::EntryAppended { .. }
            | NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. }
            | NativeSessionEvent::PermissionDecisionRecorded { .. }
            | NativeSessionEvent::EditTraceRecorded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. } => None,
        })
        .collect()
}

fn native_provider_messages_from_log_with_static_context(
    log: &NativeSessionLog,
    current_turn_id: &NativeTurnId,
    context: &NativeStaticContextBundle,
) -> Vec<ProviderMessage> {
    let mut messages = Vec::new();
    if let Some(message) = provider_message_from_static_context(context) {
        messages.push(message);
    }
    messages.extend(native_provider_messages_from_log(log, current_turn_id));
    messages
}

fn provider_message_from_static_context(
    context: &NativeStaticContextBundle,
) -> Option<ProviderMessage> {
    if context.items.is_empty() {
        return None;
    }
    let content = context
        .items
        .iter()
        .map(|item| format!("# {}\n\n{}", item.title, item.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(ProviderMessage {
        role: NativeRole::System,
        content,
    })
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
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn native_provider_agent_tool_continuation_policy() -> NativeToolContinuationPolicy {
    NativeProviderToolLoopPolicy::agent_default().as_continuation_policy()
}

#[expect(
    clippy::struct_field_names,
    reason = "limit fields intentionally share the same prefix for policy readability"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeProviderToolLoopPolicy {
    max_tool_rounds: usize,
    max_tool_calls_per_round: usize,
    max_total_tool_calls: usize,
    max_result_bytes_per_tool: usize,
    max_total_result_bytes: usize,
}

impl NativeProviderToolLoopPolicy {
    const fn agent_default() -> Self {
        Self {
            max_tool_rounds: 4,
            max_tool_calls_per_round: 4,
            max_total_tool_calls: 12,
            max_result_bytes_per_tool: 64 * 1024,
            max_total_result_bytes: 256 * 1024,
        }
    }

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

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into provider loop in the next implementation slice"
    )
)]
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
        if self.tool_rounds >= self.policy.max_tool_rounds {
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
        tool_event_store,
        registry,
        permission_policy,
        executor,
        routable_tool_names,
        require_project_root_for_tools,
    } = context;
    let advertising_tools = registry
        .provider_advertising_candidates(permission_policy, routable_tool_names.iter().copied());
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
            assemble_project_static_context(
                root.canonical_path(),
                static_context_cwd
                    .as_deref()
                    .unwrap_or_else(|| root.canonical_path()),
                NativeStaticContextPolicy::conservative(),
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
    events: RefCell<Vec<NativeSessionEvent>>,
}

impl<'a> NativeProviderBufferedEventSink<'a> {
    fn new(store: Option<&'a NativeJsonlSessionStore>) -> Self {
        Self {
            store,
            events: RefCell::new(Vec::new()),
        }
    }

    fn drain_into(&self, log: &mut NativeSessionLog, pending_events: &mut Vec<NativeSessionEvent>) {
        let mut events = self.events.borrow_mut();
        log.events.extend(events.iter().cloned());
        if self.store.is_none() {
            pending_events.extend(events.iter().cloned());
        }
        events.clear();
    }
}

impl NativeSessionEventSink for NativeProviderBufferedEventSink<'_> {
    fn append_event(&self, event: &NativeSessionEvent) -> std::io::Result<()> {
        if let Some(store) = self.store {
            store.append_event(event)?;
        }
        self.events.borrow_mut().push(event.clone());
        Ok(())
    }

    fn append_events(&self, events: &[NativeSessionEvent]) -> std::io::Result<()> {
        if let Some(store) = self.store {
            store.append_events(events)?;
        }
        self.events.borrow_mut().extend(events.iter().cloned());
        Ok(())
    }
}

struct NativeProviderAgentToolRound<'a> {
    model: ProviderModel,
    log: &'a mut NativeSessionLog,
    pending_events: &'a mut Vec<NativeSessionEvent>,
    turn_id: &'a NativeTurnId,
    project_context: Option<NativeLaunchProjectContext>,
    tool_event_store: Option<&'a NativeJsonlSessionStore>,
    review_tx: mpsc::UnboundedSender<BackendEvent>,
    review_decisions: AgentEditDecisionReceiver,
}

struct NativeProviderAgentToolBatch<'a> {
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    project_root: NativeResourceRoot,
    registry: &'a NativeToolRegistry,
    permission_policy: &'a NativeToolPermissionPolicy,
    read_only_executor: &'a ProjectReadOnlyToolExecutor,
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
        model,
        log,
        pending_events,
        turn_id,
        project_context,
        tool_event_store,
        review_tx,
        mut review_decisions,
    } = round;
    let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
    let permission_policy =
        NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
            ["project_path_info"],
            ["read_text_file", "search_project", "list_project_paths"],
            ["edit_text_file", "create_text_file"],
        );
    let routable_tool_names = [
        "project_path_info",
        "read_text_file",
        "search_project",
        "list_project_paths",
        "edit_text_file",
        "create_text_file",
    ];
    let advertising_tools =
        registry.provider_advertising_candidates(&permission_policy, routable_tool_names);
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
            assemble_project_static_context(
                root.canonical_path(),
                static_context_cwd
                    .as_deref()
                    .unwrap_or_else(|| root.canonical_path()),
                NativeStaticContextPolicy::conservative(),
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
    let Some(project_root) = project_root else {
        return Err(NativeProviderRoundError::ProjectRootUnavailable);
    };
    let continuation_policy = native_provider_agent_tool_continuation_policy();
    if first_round.tool_calls.len() > continuation_policy.max_tool_calls {
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_round_too_many_calls",
        )));
    }

    let read_only_executor = ProjectReadOnlyToolExecutor::new(project_root.clone());
    let mut edit_access = NativeEditAccess::default();
    let edit_sink = NativeProviderBufferedEventSink::new(tool_event_store);
    let mut tool_results = Vec::new();
    let mut provider_continuation_edit_traces = Vec::new();
    for (index, tool_call) in first_round.tool_calls.into_iter().enumerate() {
        let request = pending_tool_request_from_provider_call(
            format!("tool-request-{}", index + 1),
            turn_id.clone(),
            tool_call,
        );
        match request.tool_name.as_str() {
            "project_path_info" | "read_text_file" | "search_project" | "list_project_paths" => {
                let tool_event_start = log.events.len();
                let Ok(validation) = record_native_tool_validation(
                    log,
                    NativeSessionId(String::from("default")),
                    &request,
                    &registry,
                    &permission_policy,
                ) else {
                    pending_events.extend(log.events[tool_event_start..].iter().cloned());
                    return Err(NativeProviderRoundError::ToolContinuation(String::from(
                        "tool_round_validation_failed",
                    )));
                };
                let Ok(execution) = read_only_executor.execute(&registry, &request, &validation)
                else {
                    log.push(NativeSessionEvent::ToolExecutionFinished {
                        session_id: NativeSessionId(String::from("default")),
                        turn_id: turn_id.clone(),
                        tool_request_id: NativeToolRequestId(request.request_id.clone()),
                        outcome: NativeToolOutcome::Failed,
                        reason: Some(String::from("tool_round_execution_failed")),
                        result_summary: None,
                    });
                    pending_events.extend(log.events[tool_event_start..].iter().cloned());
                    return Err(NativeProviderRoundError::ToolContinuation(String::from(
                        "tool_round_execution_failed",
                    )));
                };
                if execution.byte_count > continuation_policy.max_result_bytes {
                    log.push(NativeSessionEvent::ToolExecutionFinished {
                        session_id: NativeSessionId(String::from("default")),
                        turn_id: turn_id.clone(),
                        tool_request_id: NativeToolRequestId(request.request_id.clone()),
                        outcome: NativeToolOutcome::Failed,
                        reason: Some(String::from("result_too_large")),
                        result_summary: None,
                    });
                    pending_events.extend(log.events[tool_event_start..].iter().cloned());
                    return Err(NativeProviderRoundError::ToolContinuation(String::from(
                        "tool_round_result_too_large",
                    )));
                }
                let result_summary =
                    native_provider_readonly_tool_result_summary(&request.tool_name, &execution);
                log.push(NativeSessionEvent::ToolExecutionFinished {
                    session_id: NativeSessionId(String::from("default")),
                    turn_id: turn_id.clone(),
                    tool_request_id: NativeToolRequestId(request.request_id.clone()),
                    outcome: NativeToolOutcome::Completed,
                    reason: None,
                    result_summary: Some(result_summary),
                });
                pending_events.extend(log.events[tool_event_start..].iter().cloned());
                tool_results.push(NativeProviderToolResult {
                    tool_request_id: request.request_id,
                    provider_call_id: request.provider_call_id,
                    status: NativeToolOutcome::Completed,
                    content: execution.summary,
                    byte_count: execution.byte_count,
                    redacted: execution.redacted,
                    truncated: execution.truncated,
                    reason: None,
                });
            }
            "edit_text_file" | "create_text_file" => {
                if let Some(store) = tool_event_store
                    && append_pending_native_session_events(store, pending_events).is_err()
                {
                    return Err(NativeProviderRoundError::ToolContinuation(String::from(
                        "tool_event_persist_failed",
                    )));
                }
                let tool_name = request.tool_name.clone();
                let prepared = prepare_agent_edit_tool_request(
                    &registry,
                    &project_root,
                    &mut edit_access,
                    &edit_sink,
                    NativeAgentEditToolContext {
                        session_id: NativeSessionId(String::from("default")),
                        turn_id: turn_id.clone(),
                        permission_policy: NativePermissionPolicy::default_local_edit(),
                        edit_policy: NativeEditPolicy::conservative(),
                    },
                    request,
                );
                edit_sink.drain_into(log, pending_events);
                let prepared = prepared.map_err(|error| {
                    NativeProviderRoundError::ToolContinuation(native_tool_round_error_label(
                        &error,
                    ))
                })?;
                let result = match prepared {
                    NativeAgentEditToolPrepared::Completed { trace_id, result } => {
                        provider_continuation_edit_traces.push(ProviderContinuationEditTrace {
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
                            session_id: NativeSessionId(String::from("default")),
                            turn_id: turn_id.clone(),
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
                        let preview_summary =
                            native_local_edit_preview_summary(preview, path, operation);
                        if review_tx
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
                            wait_for_agent_edit_review_decision(&mut review_decisions, &pending)
                                .await;
                        match &decision_result {
                            Ok(LocalEditDecision::Apply) => record_review_wait_trace(
                                &edit_sink,
                                &pending,
                                review_wait_started,
                                NativeEditTraceOutcome::Completed,
                                None,
                            ),
                            Ok(LocalEditDecision::Reject) => record_review_wait_trace(
                                &edit_sink,
                                &pending,
                                review_wait_started,
                                NativeEditTraceOutcome::Rejected,
                                None,
                            ),
                            Err(error) => record_review_wait_trace(
                                &edit_sink,
                                &pending,
                                review_wait_started,
                                native_review_wait_error_outcome(error),
                                Some(native_provider_round_error_label(error)),
                            ),
                        }
                        let decision = decision_result?;
                        let reviewed = match decision {
                            LocalEditDecision::Apply => {
                                apply_agent_edit_tool_review(&mut edit_access, &edit_sink, pending)
                            }
                            LocalEditDecision::Reject => {
                                reject_agent_edit_tool_review(&mut edit_access, &edit_sink, pending)
                            }
                        };
                        edit_sink.drain_into(log, pending_events);
                        let result = reviewed.map_err(|error| {
                            NativeProviderRoundError::ToolContinuation(
                                native_tool_round_error_label(&error),
                            )
                        })?;
                        provider_continuation_edit_traces.push(continuation_trace);
                        result
                    }
                };
                if result.byte_count > continuation_policy.max_result_bytes {
                    return Err(NativeProviderRoundError::ToolContinuation(String::from(
                        "tool_round_result_too_large",
                    )));
                }
                tool_results.push(result);
            }
            _ => {
                let tool_event_start = log.events.len();
                let _ = record_native_tool_validation(
                    log,
                    NativeSessionId(String::from("default")),
                    &request,
                    &registry,
                    &permission_policy,
                );
                pending_events.extend(log.events[tool_event_start..].iter().cloned());
                return Err(NativeProviderRoundError::ToolContinuation(String::from(
                    "tool_round_validation_failed",
                )));
            }
        }
    }
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
    let provider_continuation_started = Instant::now();
    let submission = match build_provider_continuation_submission(
        &continuation_request,
        ProviderContinuationValidationPolicy::strict_tool_results(
            continuation_policy.max_result_bytes,
        ),
    ) {
        Ok(submission) => submission,
        Err(error) => {
            let reason = native_provider_mapping_error_label(&error);
            record_provider_continuation_trace_records(
                log,
                pending_events,
                tool_event_store,
                ProviderContinuationTraceInput {
                    session_id: &NativeSessionId(String::from("default")),
                    turn_id,
                    edit_traces: &provider_continuation_edit_traces,
                    started: provider_continuation_started,
                    outcome: NativeEditTraceOutcome::Failed,
                    reason_label: Some(reason.as_str()),
                },
            );
            return Err(NativeProviderRoundError::ToolContinuation(reason));
        }
    };
    let continuation_request =
        crate::rig_adapter::project_provider_continuation_request(submission);
    let continuation_events = match requester.request(continuation_request).await {
        Ok(events) => events,
        Err(error) => {
            record_provider_continuation_trace_records(
                log,
                pending_events,
                tool_event_store,
                ProviderContinuationTraceInput {
                    session_id: &NativeSessionId(String::from("default")),
                    turn_id,
                    edit_traces: &provider_continuation_edit_traces,
                    started: provider_continuation_started,
                    outcome: NativeEditTraceOutcome::Failed,
                    reason_label: Some("provider_request_failed"),
                },
            );
            return Err(NativeProviderRoundError::Provider(error));
        }
    };
    let final_round = collect_native_provider_final_round(continuation_events);
    match final_round {
        Ok(result) => {
            record_provider_continuation_trace_records(
                log,
                pending_events,
                tool_event_store,
                ProviderContinuationTraceInput {
                    session_id: &NativeSessionId(String::from("default")),
                    turn_id,
                    edit_traces: &provider_continuation_edit_traces,
                    started: provider_continuation_started,
                    outcome: NativeEditTraceOutcome::Completed,
                    reason_label: None,
                },
            );
            Ok(result)
        }
        Err(error) => {
            let reason = native_provider_round_error_label(&error);
            record_provider_continuation_trace_records(
                log,
                pending_events,
                tool_event_store,
                ProviderContinuationTraceInput {
                    session_id: &NativeSessionId(String::from("default")),
                    turn_id,
                    edit_traces: &provider_continuation_edit_traces,
                    started: provider_continuation_started,
                    outcome: NativeEditTraceOutcome::Failed,
                    reason_label: Some(reason.as_str()),
                },
            );
            Err(error)
        }
    }
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

fn execute_native_provider_readonly_tool_request(
    batch: &mut NativeProviderAgentToolBatch<'_>,
    request: PendingNativeToolRequest,
) -> Result<NativeProviderToolResult, NativeProviderRoundError> {
    let tool_event_start = batch.log.events.len();
    let Ok(validation) = record_native_tool_validation(
        batch.log,
        batch.session_id.clone(),
        &request,
        batch.registry,
        batch.permission_policy,
    ) else {
        batch
            .pending_events
            .extend(batch.log.events[tool_event_start..].iter().cloned());
        return Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_round_validation_failed",
        )));
    };
    let Ok(execution) = batch
        .read_only_executor
        .execute(batch.registry, &request, &validation)
    else {
        batch.log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: batch.session_id.clone(),
            turn_id: batch.turn_id.clone(),
            tool_request_id: NativeToolRequestId(request.request_id.clone()),
            outcome: NativeToolOutcome::Failed,
            reason: Some(String::from("tool_round_execution_failed")),
            result_summary: None,
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
    batch.edit_sink.drain_into(batch.log, batch.pending_events);
    let prepared = prepared.map_err(|error| {
        NativeProviderRoundError::ToolContinuation(native_tool_round_error_label(&error))
    })?;
    let result = match prepared {
        NativeAgentEditToolPrepared::Completed { trace_id, result } => {
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
            batch.edit_sink.drain_into(batch.log, batch.pending_events);
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

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "batch executor is introduced before the multi-round loop wiring slice"
    )
)]
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
        let result = match request.tool_name.as_str() {
            "project_path_info" | "read_text_file" | "search_project" | "list_project_paths" => {
                execute_native_provider_readonly_tool_request(&mut batch, request)?
            }
            "edit_text_file" | "create_text_file" => {
                execute_native_provider_edit_tool_request(&mut batch, request).await?
            }
            _ => {
                let tool_event_start = batch.log.events.len();
                let _ = record_native_tool_validation(
                    batch.log,
                    batch.session_id.clone(),
                    &request,
                    batch.registry,
                    batch.permission_policy,
                );
                batch
                    .pending_events
                    .extend(batch.log.events[tool_event_start..].iter().cloned());
                return Err(NativeProviderRoundError::ToolContinuation(String::from(
                    "tool_round_validation_failed",
                )));
            }
        };
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
        NativeProviderRoundError::SecondRoundToolCall => String::from("second_round_tool_call"),
    }
}

fn native_review_wait_error_outcome(error: &NativeProviderRoundError) -> NativeEditTraceOutcome {
    match error {
        NativeProviderRoundError::Cancelled(_) => NativeEditTraceOutcome::Cancelled,
        NativeProviderRoundError::Provider(_)
        | NativeProviderRoundError::StreamEndedWithoutCompletion
        | NativeProviderRoundError::ProjectRootUnavailable
        | NativeProviderRoundError::ToolContinuation(_)
        | NativeProviderRoundError::ToolExecutionDenied { .. }
        | NativeProviderRoundError::SecondRoundToolCall => NativeEditTraceOutcome::Failed,
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
            message: String::from("Native provider tool continuation failed"),
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
        NativeProviderRoundError::SecondRoundToolCall => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider requested another tool round"),
            redacted_debug: Some(String::from("second_round_tool_call")),
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
    let project_root = NativeResourceRoot::project(project_root_path).ok()?;
    Some(NativeLaunchProjectContext { project_root, cwd })
}

fn native_launch_project_context_from_root(
    project_root: impl AsRef<Path>,
) -> Option<NativeLaunchProjectContext> {
    let project_root = NativeResourceRoot::project(project_root).ok()?;
    let cwd = project_root.canonical_path().to_path_buf();
    Some(NativeLaunchProjectContext { project_root, cwd })
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
    project_context: Option<NativeLaunchProjectContext>,
    review_decisions: AgentEditDecisionReceiver,
) where
    Requester: ProviderRequester,
{
    let StartedNativePrompt {
        prompt,
        mut log,
        mut pending_events,
        turn,
        user_entry,
        assistant_entry,
        prompt_started,
    } = started_prompt;

    handle_native_provider_prompt(NativeProviderPromptRequest {
        tx: &tx,
        store: &store,
        _prompt: &prompt,
        provider,
        requester: &mut requester,
        log: &mut log,
        pending_events: &mut pending_events,
        ids: NativeProviderTurnRefs {
            turn,
            user_entry,
            assistant_entry,
            prompt_started,
        },
        project_context,
        review_decisions,
    })
    .await;
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
    let result = run_native_provider_one_agent_tool_round(
        requester,
        NativeProviderAgentToolRound {
            model: ProviderModel {
                provider: provider_name.to_owned(),
                model: model_id.clone(),
            },
            log,
            pending_events,
            turn_id: &ids.turn,
            project_context,
            tool_event_store: Some(store),
            review_tx: tx.clone(),
            review_decisions,
        },
    )
    .await;
    match result {
        Ok(round) => {
            for delta in native_response_chunks(&round.text) {
                if tx
                    .send(BackendEvent::Server(ServerEvent::PromptDelta {
                        session_id: String::from("default"),
                        delta,
                    }))
                    .is_err()
                {
                    push_native_prompt_total_metric(
                        log,
                        pending_events,
                        &ids.turn,
                        ids.prompt_started,
                    );
                    push_native_session_event(
                        log,
                        pending_events,
                        NativeSessionEvent::TurnFinished {
                            session_id: NativeSessionId(String::from("default")),
                            turn_id: ids.turn,
                            outcome: NativeTurnOutcome::Cancelled,
                            reason: Some(String::from("ui receiver dropped")),
                        },
                    );
                    let _ = append_pending_native_session_events(store, pending_events);
                    return;
                }
            }
            push_native_prompt_total_metric(log, pending_events, &ids.turn, ids.prompt_started);
            push_native_session_event(
                log,
                pending_events,
                NativeSessionEvent::EntryAppended {
                    session_id: NativeSessionId(String::from("default")),
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
                    session_id: NativeSessionId(String::from("default")),
                    turn_id: ids.turn,
                    outcome: NativeTurnOutcome::Completed,
                    reason: None,
                },
            );
            finish_native_prompt(
                tx,
                store,
                pending_events,
                "turn_end native provider",
                PromptOutcome::Completed,
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
            push_native_prompt_total_metric(log, pending_events, &ids.turn, ids.prompt_started);
            persist_native_fixture_error(
                tx,
                log,
                pending_events,
                ids.turn,
                turn_outcome,
                &provider_error,
            );
            finish_native_prompt(tx, store, pending_events, status, prompt_outcome);
        }
    }
}

fn finish_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    pending_events: &mut Vec<NativeSessionEvent>,
    status: &str,
    outcome: PromptOutcome,
) {
    let status = match append_pending_native_session_events(store, pending_events) {
        Ok(()) => status.to_owned(),
        Err(error) => format!("native dogfood: failed to persist session log: {error}"),
    };
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: status.clone(),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::PromptFinished {
        session_id: String::from("default"),
        outcome,
        message: Some(status),
    }));
    send_native_session_stats(tx, store.path());
}

fn persist_native_cancelled_turn(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    turn_id: NativeTurnId,
    prompt_started: Instant,
    reason: &str,
) {
    let mut log = store.load().unwrap_or_default();
    if native_log_has_finished_turn(&log, &turn_id) {
        return;
    }

    let mut pending_events = Vec::new();
    push_native_prompt_total_metric(&mut log, &mut pending_events, &turn_id, prompt_started);
    push_native_session_event(
        &mut log,
        &mut pending_events,
        NativeSessionEvent::TurnFinished {
            session_id: NativeSessionId(String::from("default")),
            turn_id,
            outcome: NativeTurnOutcome::Cancelled,
            reason: Some(reason.to_owned()),
        },
    );
    finish_native_prompt(
        tx,
        store,
        &mut pending_events,
        "turn_end native provider cancelled",
        PromptOutcome::Cancelled,
    );
}

fn persist_native_fixture_error(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
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
            session_id: NativeSessionId(String::from("default")),
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

fn send_native_session_messages(tx: &mpsc::UnboundedSender<BackendEvent>, session_path: &Path) {
    let messages = load_native_log_or_default(session_path)
        .events
        .into_iter()
        .filter_map(|event| match event {
            NativeSessionEvent::EntryAppended {
                entry_id,
                role,
                text,
                ..
            } => Some(SessionMessage {
                role: native_role_label(role),
                text,
                entry_id: Some(entry_id.0),
            }),
            NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. }
            | NativeSessionEvent::PermissionDecisionRecorded { .. }
            | NativeSessionEvent::EditTraceRecorded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. } => None,
        })
        .collect();
    let _ = tx.send(BackendEvent::Server(ServerEvent::SessionMessagesUpdated {
        messages,
    }));
}

fn send_native_session_stats(tx: &mpsc::UnboundedSender<BackendEvent>, session_path: &Path) {
    let messages = load_native_log_or_default(session_path)
        .events
        .into_iter()
        .filter_map(|event| match event {
            NativeSessionEvent::EntryAppended { role, .. } => Some(role),
            NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. }
            | NativeSessionEvent::PermissionDecisionRecorded { .. }
            | NativeSessionEvent::EditTraceRecorded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. } => None,
        })
        .collect::<Vec<_>>();
    let message_count = u64::try_from(messages.len()).ok();
    let user_message_count = count_native_role(&messages, NativeRole::User);
    let assistant_message_count = count_native_role(&messages, NativeRole::Assistant);
    let tool_message_count = count_native_role(&messages, NativeRole::Tool);
    let _ = tx.send(BackendEvent::Server(ServerEvent::SessionStatsUpdated(
        SessionStats {
            message_count,
            user_message_count,
            assistant_message_count,
            tool_message_count,
            total_tokens: None,
        },
    )));
}

fn send_native_recent_sessions(tx: &mpsc::UnboundedSender<BackendEvent>, session_path: &Path) {
    let session = RecentSession {
        path: session_path.to_string_lossy().into_owned(),
        id: Some(String::from("default")),
        name: Some(String::from("native dogfood default")),
        cwd: std::env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        modified_unix_ms: fs::metadata(session_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
        message_count: native_session_message_count(session_path),
        first_message: native_session_first_message(session_path),
    };
    let _ = tx.send(BackendEvent::Server(ServerEvent::RecentSessionsUpdated {
        sessions: vec![session],
    }));
}

fn load_native_log_or_default(path: &Path) -> NativeSessionLog {
    NativeSessionLog::load_from_file(path).unwrap_or_default()
}

fn native_session_message_count(path: &Path) -> Option<u64> {
    u64::try_from(
        load_native_log_or_default(path)
            .events
            .iter()
            .filter(|event| matches!(event, NativeSessionEvent::EntryAppended { .. }))
            .count(),
    )
    .ok()
}

fn native_session_first_message(path: &Path) -> Option<String> {
    load_native_log_or_default(path)
        .events
        .into_iter()
        .find_map(|event| match event {
            NativeSessionEvent::EntryAppended { text, .. } => Some(text),
            NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. }
            | NativeSessionEvent::PermissionDecisionRecorded { .. }
            | NativeSessionEvent::EditTraceRecorded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. } => None,
        })
}

fn native_role_label(role: NativeRole) -> String {
    match role {
        NativeRole::User => String::from("user"),
        NativeRole::Assistant => String::from("assistant"),
        NativeRole::Tool => String::from("tool"),
        NativeRole::System => String::from("system"),
    }
}

fn count_native_role(messages: &[NativeRole], role: NativeRole) -> Option<u64> {
    u64::try_from(
        messages
            .iter()
            .filter(|message_role| **message_role == role)
            .count(),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::{
        NativeFixtureOutcome, NativeLaunchProjectContext, NativeProviderAgentToolBatch,
        NativeProviderAgentToolRound, NativeProviderBufferedEventSink, NativeProviderDogfoodConfig,
        NativeProviderRoundError, NativeProviderRoundResult, NativeProviderToolLoopBudget,
        NativeProviderToolLoopPolicy, NativeProviderToolRoundContext, ProviderRequester,
        execute_native_provider_agent_tool_batch, native_fixture_outcome,
        native_launch_project_context, native_local_edit_error_message,
        native_log_has_finished_turn, native_provider_messages_from_log,
        native_provider_messages_from_log_with_static_context, native_response_chunks,
        native_status_message, record_provider_continuation_trace_records,
        run_native_provider_one_agent_tool_round, run_native_provider_one_readonly_tool_round,
        run_native_provider_one_tool_round_with_registry, send_native_initial_state,
    };
    use crate::rig_adapter::{RigProviderAdapterConfig, RigProviderConfig};
    use crate::{
        ExtensionToolExecutorRouter, ExtensionToolHandler, NativeEditAccess, NativeEditAccessError,
        NativeEditError, NativeEditEvidenceOutcome, NativeEditEvidenceSummary,
        NativeEditOperationEvidence, NativeEditPreviewId, NativeEditTraceId,
        NativeEditTraceOutcome, NativeEditTracePhase, NativeEditTraceRecord,
        NativeEditTransactionId, NativeEntryId, NativeJsonlSessionStore,
        NativePermissionDecisionId, NativePermissionDecisionOutcome, NativeResourceRoot,
        NativeRole, NativeSessionEvent, NativeSessionId, NativeSessionLog,
        NativeStaticContextBundle, NativeStaticContextItem, NativeStaticContextPlacement,
        NativeStaticContextPriority, NativeStaticContextSource, NativeToolContinuationPolicy,
        NativeToolDefinition, NativeToolInputSchema, NativeToolOutcome, NativeToolPayloadSummary,
        NativeToolPermissionPolicy, NativeToolPermissionState, NativeToolRegistry,
        NativeToolRequestId, NativeTurnId, NativeTurnOutcome,
        PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY, ProjectReadOnlyToolExecutor, ProviderError,
        ProviderErrorKind, ProviderFinishReason, ProviderMessage, ProviderModel, ProviderRequest,
        ProviderStreamEvent, ProviderToolCall, ProviderToolVisibility,
        parse_provider_tool_advertising_extensions, sha256_hex_for_test,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::mpsc;
    use yach_proto::{
        BackendEvent, Capability, ClientEvent, LocalEditDecision, LocalEditFinishedOutcome,
        LocalEditOperationInput, LocalEditPreviewSummary, LocalEditReviewState, PromptOutcome,
        ServerEvent, ToolReviewPayload,
    };

    static TEMP_PROJECT_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn native_provider_tool_loop_policy_matches_design_limits() {
        let policy = NativeProviderToolLoopPolicy::agent_default();

        assert_eq!(policy.max_tool_rounds, 4);
        assert_eq!(policy.max_tool_calls_per_round, 4);
        assert_eq!(policy.max_total_tool_calls, 12);
        assert_eq!(policy.max_result_bytes_per_tool, 64 * 1024);
        assert_eq!(policy.max_total_result_bytes, 256 * 1024);

        let continuation_policy = policy.as_continuation_policy();
        assert_eq!(
            continuation_policy,
            NativeToolContinuationPolicy {
                max_tool_calls: 4,
                max_result_bytes: 64 * 1024,
            }
        );
    }

    #[test]
    fn native_provider_tool_loop_budget_rejects_round_call_and_byte_overages() {
        let policy = NativeProviderToolLoopPolicy {
            max_tool_rounds: 1,
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
            max_tool_rounds: 2,
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
                turn_id: turn_id.clone(),
                project_root,
                registry: &registry,
                permission_policy: &permission_policy,
                read_only_executor: &read_only_executor,
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
                | NativeSessionEvent::EditTransactionFinished { .. } => None,
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
        let status = native_status_message(None);

        assert_eq!(
            status,
            "backend: native dogfood; local read-only project inspection available; provider tools unavailable"
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

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, NativeRole::System);
        assert!(
            messages[0]
                .content
                .contains("# AGENTS.md instructions for .")
        );
        assert!(messages[0].content.contains("root rules"));
        assert_eq!(messages[1].role, NativeRole::User);
        assert_eq!(messages[1].content, "hello");
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
        assert!(request.messages[0].content.contains("root rules"));
        assert!(request.messages[0].content.contains("system rules"));
        assert!(pending_events.iter().any(|event| {
            matches!(event, NativeSessionEvent::StaticContextIncluded { summary, .. }
                if summary.items.len() == 2)
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
        let system_message = &requester.requests[0].messages[0];
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

        send_native_initial_state(&tx, root_guard.path(), None);

        let ready = rx.try_recv().ok();
        assert!(matches!(
            ready,
            Some(BackendEvent::Server(ServerEvent::Ready { handshake }))
                if handshake.capabilities == vec![
                    Capability::PromptStreaming,
                    Capability::PromptCancellation,
                    Capability::LocalEdit,
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
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
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
    fn native_provider_one_round_executes_read_search_list_and_continues_with_redacted_evidence() {
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
        let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
        let (_review_tx, review_rx) = mpsc::unbounded_channel();

        let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
            &mut requester,
            NativeProviderAgentToolRound {
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
            },
        ));

        assert_eq!(
            result,
            Ok(NativeProviderRoundResult {
                text: String::from("content inspected"),
                provider_response_id: Some(String::from("response-2")),
            })
        );
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
        assert!(!raw_events.contains("alpha line"));
        assert!(!raw_events.contains("needle evidence line"));
        assert!(!raw_events.contains("src/lib.rs"));
        assert!(!raw_events.contains("src/main.rs"));
        assert!(!raw_events.contains("\"query\":\"needle\""));
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
                model,
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
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
            let ToolReviewPayload::LocalEdit { preview } = review.payload;
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
                        == Some(NativeToolRequestId(String::from("tool-request-1")))
                    && trace.preview_id.is_some()
                    && trace.permission_decision_id.is_some()
            }));
            assert!(traces.iter().any(|trace| {
                trace.trace_id == trace_id
                    && trace.phase == NativeEditTracePhase::ProviderContinuation
                    && trace.outcome == NativeEditTraceOutcome::Completed
                    && trace.provider_call_id.as_deref() == Some("call-edit-1")
                    && trace.tool_request_id
                        == Some(NativeToolRequestId(String::from("tool-request-1")))
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
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
                log: &mut log,
                pending_events: &mut pending_events,
                turn_id: &turn_id,
                project_context: Some(NativeLaunchProjectContext::from_project_root(resource_root)),
                tool_event_store: None,
                review_tx: backend_tx,
                review_decisions: review_rx,
            },
        ));

        assert!(matches!(
            result,
            Err(NativeProviderRoundError::ToolExecutionDenied { .. })
        ));
        assert_eq!(requester.requests.len(), 1);
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
            let ToolReviewPayload::LocalEdit { preview } = review.payload;
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
            let ToolReviewPayload::LocalEdit { preview } = review.payload;
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

    fn native_provider_test_config() -> NativeProviderDogfoodConfig {
        NativeProviderDogfoodConfig {
            adapter: RigProviderAdapterConfig {
                provider: RigProviderConfig::Anthropic {
                    api_key: String::from("test-key"),
                },
                timeout: std::time::Duration::from_secs(30),
                max_tokens: 1000,
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
            .find(|message| message.role == NativeRole::System);
        assert!(guard_message.is_some());
        let Some(guard_message) = guard_message else {
            return;
        };
        assert!(
            guard_message
                .content
                .contains("No additional tools are available")
        );
        assert!(guard_message.content.contains("Do not claim"));
        assert_eq!(requester.requests[1].messages.len(), 3);
        assert_eq!(requester.requests[1].messages[2].role, NativeRole::Tool);
        let tool_message_content = &requester.requests[1].messages[2].content;
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
    fn native_provider_one_round_rejects_second_round_tool_calls() {
        let root_guard = temp_native_provider_root("native-provider-second-tool");
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
        let tool_call = ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("project_path_info"),
            arguments_json: serde_json::json!({"path":"Cargo.toml"}),
        };
        let mut requester = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: turn.clone(),
                    tool_call: tool_call.clone(),
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
                    tool_call,
                },
                ProviderStreamEvent::Completed {
                    turn_id: turn.clone(),
                    finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
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

        assert_eq!(result, Err(NativeProviderRoundError::SecondRoundToolCall));
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
        assert_eq!(requester.requests[1].messages.len(), 3);
        assert_eq!(requester.requests[1].messages[1].role, NativeRole::System);
        assert!(
            requester.requests[1].messages[1]
                .content
                .contains("No additional tools are available")
        );
        assert_eq!(requester.requests[1].messages[2].role, NativeRole::Tool);
        assert!(
            requester.requests[1].messages[2]
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
        assert_eq!(requester.requests[1].messages.len(), 3);
        assert_eq!(requester.requests[1].messages[1].role, NativeRole::System);
        assert!(
            requester.requests[1].messages[1]
                .content
                .contains("No additional tools are available")
        );
        assert_eq!(requester.requests[1].messages[2].role, NativeRole::Tool);
        assert!(
            requester.requests[1].messages[2]
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
