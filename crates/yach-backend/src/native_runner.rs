use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use yach_proto::{
    BackendEvent, BackendState, Capability, ClientEvent, Handshake, ModelInfo, PromptOutcome,
    RecentSession, ServerEvent, SessionMessage, SessionStats,
};

use crate::rig_adapter::{RigProviderAdapterConfig, RigProviderConfig, run_provider_request};
use crate::{
    NativeDurationMetric, NativeEntryId, NativeJsonlSessionStore, NativeResourceRoot, NativeRole,
    NativeSessionEvent, NativeSessionEventSink, NativeSessionId, NativeSessionLog,
    NativeToolContinuationContext, NativeToolContinuationError, NativeToolContinuationPolicy,
    NativeToolPermissionPolicy, NativeToolRegistry, NativeTurnId, NativeTurnOutcome,
    ProviderContinuationMappingError, ProviderContinuationRequest,
    ProviderContinuationValidationPolicy, ProviderError, ProviderErrorKind, ProviderFinishReason,
    ProviderMessage, ProviderMetadata, ProviderModel, ProviderRequest, ProviderStreamEvent,
    ProviderToolAdvertisingError, ProviderToolCall, build_project_readonly_provider_tool_results,
    build_provider_continuation_submission,
};

/// Native dogfood runner configuration owned by the backend Module.
#[derive(Debug, Clone)]
pub struct NativeDogfoodRunnerConfig {
    pub session_path: PathBuf,
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
    mut rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    config: NativeDogfoodRunnerConfig,
) {
    let NativeDogfoodRunnerConfig {
        session_path,
        provider,
    } = config;
    let store = NativeJsonlSessionStore::new(session_path.clone());
    send_native_initial_state(&tx, &session_path, provider.as_ref());
    let mut turn_index = store.load().unwrap_or_default().next_turn_index();
    let mut active_provider_turn: Option<(tokio::task::JoinHandle<()>, NativeTurnId, Instant)> =
        None;

    while let Some(event) = rx.recv().await {
        match event {
            ClientEvent::Initialize(_) => {
                send_native_initial_state(&tx, &session_path, provider.as_ref());
            }
            ClientEvent::AvailableModelsRequested => {
                send_native_models(&tx, provider.as_ref());
            }
            ClientEvent::PromptCancelled { .. } => {
                if let Some((handle, turn_id, prompt_started)) = active_provider_turn.take()
                    && !handle.is_finished()
                {
                    handle.abort();
                    let _ = handle.await;
                    persist_native_cancelled_turn(
                        &tx,
                        &store,
                        turn_id,
                        prompt_started,
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
                if let Some(provider) = provider.clone() {
                    if active_provider_turn
                        .as_ref()
                        .is_some_and(|(handle, _, _)| handle.is_finished())
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
                    let handle = tokio::spawn(handle_started_native_provider_prompt(
                        tx.clone(),
                        store.clone(),
                        provider,
                        started_prompt,
                    ));
                    active_provider_turn = Some((handle, turn_id, prompt_started));
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
            vec![Capability::PromptStreaming, Capability::PromptCancellation],
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
            | NativeSessionEvent::MetricRecorded { .. } => None,
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
            | NativeSessionEvent::MetricRecorded { .. } => None,
        })
        .collect()
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
}

impl ProviderRequester for RigProviderRequester {
    fn request(
        &mut self,
        request: ProviderRequest,
    ) -> BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>> {
        let adapter = self.adapter.clone();
        Box::pin(async move { run_provider_request(adapter, request).await })
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
            | ProviderStreamEvent::ToolCallDelta { .. } => {}
            ProviderStreamEvent::Started { .. } => {}
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

async fn run_native_provider_one_readonly_tool_round(
    requester: &mut impl ProviderRequester,
    model: ProviderModel,
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    turn_id: &NativeTurnId,
    project_root: Option<NativeResourceRoot>,
    tool_event_store: Option<&NativeJsonlSessionStore>,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError> {
    let initial_request = ProviderRequest {
        turn_id: turn_id.clone(),
        model,
        messages: native_provider_messages_from_log(log, turn_id),
        extensions: vec![
            crate::build_project_path_info_provider_tool_advertising_extension().map_err(
                |error| {
                    NativeProviderRoundError::ToolContinuation(
                        native_provider_tool_advertising_error_label(&error),
                    )
                },
            )?,
        ],
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
    let root = project_root.ok_or(NativeProviderRoundError::ProjectRootUnavailable)?;
    let tool_event_start = log.events.len();
    let tool_results = match build_project_readonly_provider_tool_results(
        log,
        &NativeToolContinuationContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: turn_id.clone(),
        },
        first_round.tool_calls,
        root,
        &NativeToolRegistry::with_project_read_only_tools(),
        &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
        NativeToolContinuationPolicy::fixture_default(),
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
        NativeProviderRoundError::SecondRoundToolCall => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider requested another tool round"),
            redacted_debug: Some(String::from("second_round_tool_call")),
        },
    }
}

async fn handle_started_native_provider_prompt(
    tx: mpsc::UnboundedSender<BackendEvent>,
    store: NativeJsonlSessionStore,
    provider: NativeProviderDogfoodConfig,
    started_prompt: StartedNativePrompt,
) {
    let StartedNativePrompt {
        prompt,
        mut log,
        mut pending_events,
        turn,
        user_entry,
        assistant_entry,
        prompt_started,
    } = started_prompt;

    handle_native_provider_prompt(
        &tx,
        &store,
        &prompt,
        provider,
        &mut log,
        &mut pending_events,
        NativeProviderTurnRefs {
            turn,
            user_entry,
            assistant_entry,
            prompt_started,
        },
    )
    .await;
}

async fn handle_native_provider_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    _prompt: &str,
    provider: NativeProviderDogfoodConfig,
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    ids: NativeProviderTurnRefs,
) {
    let provider_name = provider.provider_label();
    let model_id = provider.model.clone();
    if let Some(delay_ms) = provider.test_delay_ms {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native provider test delay: {delay_ms}ms"),
        }));
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    let project_root =
        NativeResourceRoot::project(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .ok();
    let mut requester = RigProviderRequester {
        adapter: provider.adapter,
    };
    let result = run_native_provider_one_readonly_tool_round(
        &mut requester,
        ProviderModel {
            provider: provider_name.to_owned(),
            model: model_id.clone(),
        },
        log,
        pending_events,
        &ids.turn,
        project_root,
        Some(store),
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
            | NativeSessionEvent::MetricRecorded { .. } => None,
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
            | NativeSessionEvent::MetricRecorded { .. } => None,
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
            | NativeSessionEvent::MetricRecorded { .. } => None,
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
        NativeFixtureOutcome, NativeProviderRoundError, NativeProviderRoundResult,
        ProviderRequester, native_fixture_outcome, native_log_has_finished_turn,
        native_provider_messages_from_log, native_response_chunks, native_status_message,
        run_native_provider_one_readonly_tool_round, send_native_initial_state,
    };
    use crate::{
        NativeEntryId, NativeJsonlSessionStore, NativeResourceRoot, NativeRole, NativeSessionEvent,
        NativeSessionId, NativeSessionLog, NativeToolOutcome, NativeTurnId, NativeTurnOutcome,
        PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY, ProviderError, ProviderErrorKind, ProviderMessage,
        ProviderModel, ProviderRequest, ProviderStreamEvent, ProviderToolCall,
        parse_provider_tool_advertising_extensions,
    };
    use tokio::sync::mpsc;
    use yach_proto::{BackendEvent, Capability, ServerEvent};

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
        let advertising =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
                .expect("initial provider request advertising should parse")
                .expect("initial provider request should advertise native project tools");
        assert_eq!(advertising.tools.len(), 1);
        assert_eq!(advertising.tools[0].name, "project_path_info");
        assert!(pending_events.is_empty());
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
    fn native_initial_state_handshake_remains_streaming_and_cancellation_only() {
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
                ]
        ));
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
            Some(root),
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
        let advertising =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
                .expect("initial provider request advertising should parse")
                .expect("initial provider request should advertise native project tools");
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
        assert_eq!(requester.requests[1].messages.len(), 2);
        assert_eq!(requester.requests[1].messages[1].role, NativeRole::Tool);
        let tool_message_content = &requester.requests[1].messages[1].content;
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
            Some(root),
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
            Some(root),
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
            Some(root),
            None,
        ));

        assert_eq!(result, Err(NativeProviderRoundError::SecondRoundToolCall));
        assert_eq!(requester.requests.len(), 2);
        let advertising =
            parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
                .expect("initial provider request advertising should parse")
                .expect("initial provider request should advertise native project tools");
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
        assert_eq!(requester.requests[1].messages.len(), 2);
        assert_eq!(requester.requests[1].messages[1].role, NativeRole::Tool);
        assert!(
            requester.requests[1].messages[1]
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
            Some(root),
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
            Some(root),
            None,
        ));

        assert_eq!(
            result,
            Err(NativeProviderRoundError::Provider(
                ProviderError::malformed_stream("second provider request failed")
            ))
        );
        assert_eq!(requester.requests.len(), 2);
        assert_eq!(requester.requests[1].messages.len(), 2);
        assert_eq!(requester.requests[1].messages[1].role, NativeRole::Tool);
        assert!(
            requester.requests[1].messages[1]
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
