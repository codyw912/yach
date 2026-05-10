use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use tokio::sync::mpsc;
use yach_proto::{
    BackendEvent, BackendState, Capability, ClientEvent, Handshake, ModelInfo, PromptOutcome,
    RecentSession, ServerEvent, SessionMessage, SessionStats,
};

use crate::rig_adapter::{RigProviderAdapterConfig, RigProviderConfig, run_provider_request};
use crate::{
    NativeDurationMetric, NativeEntryId, NativeJsonlSessionStore, NativeRole, NativeSessionEvent,
    NativeSessionEventSink, NativeSessionId, NativeSessionLog, NativeTurnId, NativeTurnOutcome,
    ProviderError, ProviderErrorKind, ProviderMessage, ProviderMetadata, ProviderModel,
    ProviderRequest, ProviderStreamEvent,
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
        String::from("backend: native dogfood; tools/resources/provider APIs are unavailable")
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
    prompt: &str,
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
    let request = ProviderRequest {
        turn_id: ids.turn.clone(),
        model: ProviderModel {
            provider: provider_name.to_owned(),
            model: model_id.clone(),
        },
        messages: vec![ProviderMessage {
            role: NativeRole::User,
            content: prompt.to_owned(),
        }],
        extensions: vec![],
    };
    let events = run_provider_request(provider.adapter, request).await;
    let mut assistant_text = String::new();
    match events {
        Ok(events) => {
            let mut completed = false;
            for event in events {
                match event {
                    ProviderStreamEvent::TextDelta { delta, .. } => {
                        assistant_text.push_str(&delta);
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
                    ProviderStreamEvent::Completed { .. } => completed = true,
                    ProviderStreamEvent::Failed { error, .. } => {
                        push_native_prompt_total_metric(
                            log,
                            pending_events,
                            &ids.turn,
                            ids.prompt_started,
                        );
                        persist_native_fixture_error(
                            tx,
                            log,
                            pending_events,
                            ids.turn,
                            NativeTurnOutcome::Failed,
                            &error,
                        );
                        finish_native_prompt(
                            tx,
                            store,
                            pending_events,
                            "turn_end native provider failed",
                            PromptOutcome::Failed,
                        );
                        return;
                    }
                    ProviderStreamEvent::Cancelled { reason, .. } => {
                        push_native_prompt_total_metric(
                            log,
                            pending_events,
                            &ids.turn,
                            ids.prompt_started,
                        );
                        persist_native_fixture_error(
                            tx,
                            log,
                            pending_events,
                            ids.turn,
                            NativeTurnOutcome::Cancelled,
                            &ProviderError::cancelled(
                                reason.unwrap_or_else(|| String::from("native provider cancelled")),
                            ),
                        );
                        finish_native_prompt(
                            tx,
                            store,
                            pending_events,
                            "turn_end native provider cancelled",
                            PromptOutcome::Cancelled,
                        );
                        return;
                    }
                    _ => {}
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
                    text: assistant_text,
                    provider: Some(ProviderMetadata {
                        provider: provider_name.to_owned(),
                        model: model_id,
                        response_id: None,
                    }),
                },
            );
            push_native_session_event(
                log,
                pending_events,
                NativeSessionEvent::TurnFinished {
                    session_id: NativeSessionId(String::from("default")),
                    turn_id: ids.turn,
                    outcome: if completed {
                        NativeTurnOutcome::Completed
                    } else {
                        NativeTurnOutcome::Failed
                    },
                    reason: if completed {
                        None
                    } else {
                        Some(String::from("provider stream ended without completion"))
                    },
                },
            );
            let outcome = if completed {
                PromptOutcome::Completed
            } else {
                PromptOutcome::Failed
            };
            finish_native_prompt(
                tx,
                store,
                pending_events,
                "turn_end native provider",
                outcome,
            );
        }
        Err(error) => {
            push_native_prompt_total_metric(log, pending_events, &ids.turn, ids.prompt_started);
            persist_native_fixture_error(
                tx,
                log,
                pending_events,
                ids.turn,
                NativeTurnOutcome::Failed,
                &error,
            );
            finish_native_prompt(
                tx,
                store,
                pending_events,
                "turn_end native provider failed",
                PromptOutcome::Failed,
            );
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
        NativeFixtureOutcome, native_fixture_outcome, native_log_has_finished_turn,
        native_response_chunks,
    };
    use crate::{
        NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeTurnId, NativeTurnOutcome,
    };

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
}
