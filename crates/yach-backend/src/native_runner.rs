use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use tokio::sync::mpsc;
use yach_proto::{
    BackendEvent, BackendState, Capability, ClientEvent, Handshake, ModelInfo, PromptOutcome,
    RecentSession, ServerEvent, SessionMessage, SessionStats,
};

use crate::rig_adapter::{RigProviderAdapterConfig, RigProviderConfig, run_provider_request};
use crate::{
    NativeEntryId, NativeRole, NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeTurnId,
    NativeTurnOutcome, ProviderError, ProviderErrorKind, ProviderMessage, ProviderMetadata,
    ProviderModel, ProviderRequest, ProviderStreamEvent,
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
    send_native_initial_state(&tx, &session_path, provider.as_ref());
    let mut turn_index = 0_u64;
    let mut active_provider_turn: Option<(tokio::task::JoinHandle<()>, NativeTurnId)> = None;

    while let Some(event) = rx.recv().await {
        match event {
            ClientEvent::Initialize(_) => {
                send_native_initial_state(&tx, &session_path, provider.as_ref());
            }
            ClientEvent::AvailableModelsRequested => {
                send_native_models(&tx, provider.as_ref());
            }
            ClientEvent::PromptCancelled { session_id } => {
                if let Some((handle, turn_id)) = active_provider_turn.take() {
                    handle.abort();
                    persist_native_cancelled_turn(
                        &tx,
                        &session_path,
                        turn_id,
                        "native provider prompt cancelled",
                    );
                }
                let _ = tx.send(BackendEvent::Server(ServerEvent::PromptFinished {
                    session_id,
                    outcome: PromptOutcome::Cancelled,
                    message: Some(String::from("native dogfood prompt cancelled")),
                }));
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
                turn_index = turn_index.saturating_add(1);
                if provider.is_some() {
                    if active_provider_turn
                        .as_ref()
                        .is_some_and(|(handle, _)| handle.is_finished())
                    {
                        active_provider_turn = None;
                    }
                    if active_provider_turn.is_some() {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                            message: String::from("native provider: prompt already in progress"),
                        }));
                        continue;
                    }
                    let turn_id = NativeTurnId(format!("turn-{turn_index}"));
                    let handle = tokio::spawn(handle_native_prompt(
                        tx.clone(),
                        session_path.clone(),
                        session_id,
                        prompt,
                        turn_index,
                        provider.clone(),
                    ));
                    active_provider_turn = Some((handle, turn_id));
                } else {
                    handle_native_prompt(
                        tx.clone(),
                        session_path.clone(),
                        session_id,
                        prompt,
                        turn_index,
                        provider.clone(),
                    )
                    .await;
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

async fn handle_native_prompt(
    tx: mpsc::UnboundedSender<BackendEvent>,
    session_path: PathBuf,
    session_id: String,
    prompt: String,
    turn_index: u64,
    provider: Option<NativeProviderDogfoodConfig>,
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
    let fixture_outcome = native_fixture_outcome(&prompt);
    let mut log = load_native_log_or_default(&session_path);
    log.push(NativeSessionEvent::EntryAppended {
        session_id: NativeSessionId(String::from("default")),
        entry_id: user_entry_id.clone(),
        parent_entry_id: None,
        turn_id: turn_id.clone(),
        role: NativeRole::User,
        text: prompt.clone(),
        provider: None,
    });

    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("turn_start native dogfood"),
    }));

    if let Some(provider) = provider {
        if let Err(error) = log.write_to_file(&session_path) {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("native dogfood: failed to persist session log: {error}"),
            }));
        }
        handle_native_provider_prompt(
            &tx,
            &session_path,
            &prompt,
            provider,
            &mut log,
            NativeProviderTurnRefs {
                turn: turn_id,
                user_entry: user_entry_id,
                assistant_entry: assistant_entry_id,
            },
        )
        .await;
        return;
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
                    log.push(NativeSessionEvent::TurnFinished {
                        session_id: NativeSessionId(String::from("default")),
                        turn_id,
                        outcome: NativeTurnOutcome::Cancelled,
                        reason: Some(String::from("ui receiver dropped")),
                    });
                    let _ = log.write_to_file(&session_path);
                    return;
                }
            }
            log.push(NativeSessionEvent::EntryAppended {
                session_id: NativeSessionId(String::from("default")),
                entry_id: assistant_entry_id,
                parent_entry_id: Some(user_entry_id),
                turn_id: turn_id.clone(),
                role: NativeRole::Assistant,
                text: response,
                provider: None,
            });
            log.push(NativeSessionEvent::TurnFinished {
                session_id: NativeSessionId(String::from("default")),
                turn_id,
                outcome: NativeTurnOutcome::Completed,
                reason: None,
            });
        }
        NativeFixtureOutcome::Failed => {
            persist_native_fixture_error(
                &tx,
                &mut log,
                turn_id,
                NativeTurnOutcome::Failed,
                &ProviderError::fixture_failure(),
            );
        }
        NativeFixtureOutcome::Malformed => {
            persist_native_fixture_error(
                &tx,
                &mut log,
                turn_id,
                NativeTurnOutcome::Failed,
                &ProviderError::malformed_stream("native dogfood fixture malformed stream"),
            );
        }
        NativeFixtureOutcome::Cancelled => {
            persist_native_fixture_error(
                &tx,
                &mut log,
                turn_id,
                NativeTurnOutcome::Cancelled,
                &ProviderError::cancelled("native dogfood fixture cancellation"),
            );
        }
    }

    let status = match log.write_to_file(&session_path) {
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
    send_native_session_stats(&tx, &session_path);
}

#[derive(Debug, Clone)]
struct NativeProviderTurnRefs {
    turn: NativeTurnId,
    user_entry: NativeEntryId,
    assistant_entry: NativeEntryId,
}

async fn handle_native_provider_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    prompt: &str,
    provider: NativeProviderDogfoodConfig,
    log: &mut NativeSessionLog,
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
                            log.push(NativeSessionEvent::TurnFinished {
                                session_id: NativeSessionId(String::from("default")),
                                turn_id: ids.turn,
                                outcome: NativeTurnOutcome::Cancelled,
                                reason: Some(String::from("ui receiver dropped")),
                            });
                            let _ = log.write_to_file(session_path);
                            return;
                        }
                    }
                    ProviderStreamEvent::Completed { .. } => completed = true,
                    ProviderStreamEvent::Failed { error, .. } => {
                        persist_native_fixture_error(
                            tx,
                            log,
                            ids.turn,
                            NativeTurnOutcome::Failed,
                            &error,
                        );
                        finish_native_prompt(
                            tx,
                            session_path,
                            log,
                            "turn_end native provider failed",
                            PromptOutcome::Failed,
                        );
                        return;
                    }
                    ProviderStreamEvent::Cancelled { reason, .. } => {
                        persist_native_fixture_error(
                            tx,
                            log,
                            ids.turn,
                            NativeTurnOutcome::Cancelled,
                            &ProviderError::cancelled(
                                reason.unwrap_or_else(|| String::from("native provider cancelled")),
                            ),
                        );
                        finish_native_prompt(
                            tx,
                            session_path,
                            log,
                            "turn_end native provider cancelled",
                            PromptOutcome::Cancelled,
                        );
                        return;
                    }
                    _ => {}
                }
            }
            log.push(NativeSessionEvent::EntryAppended {
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
            });
            log.push(NativeSessionEvent::TurnFinished {
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
            });
            let outcome = if completed {
                PromptOutcome::Completed
            } else {
                PromptOutcome::Failed
            };
            finish_native_prompt(tx, session_path, log, "turn_end native provider", outcome);
        }
        Err(error) => {
            persist_native_fixture_error(tx, log, ids.turn, NativeTurnOutcome::Failed, &error);
            finish_native_prompt(
                tx,
                session_path,
                log,
                "turn_end native provider failed",
                PromptOutcome::Failed,
            );
        }
    }
}

fn finish_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    log: &NativeSessionLog,
    status: &str,
    outcome: PromptOutcome,
) {
    let status = match log.write_to_file(session_path) {
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
    send_native_session_stats(tx, session_path);
}

fn persist_native_cancelled_turn(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    turn_id: NativeTurnId,
    reason: &str,
) {
    let mut log = load_native_log_or_default(session_path);
    log.push(NativeSessionEvent::TurnFinished {
        session_id: NativeSessionId(String::from("default")),
        turn_id,
        outcome: NativeTurnOutcome::Cancelled,
        reason: Some(reason.to_owned()),
    });
    finish_native_prompt(
        tx,
        session_path,
        &log,
        "turn_end native provider cancelled",
        PromptOutcome::Cancelled,
    );
}

fn persist_native_fixture_error(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &mut NativeSessionLog,
    turn_id: NativeTurnId,
    outcome: NativeTurnOutcome,
    error: &ProviderError,
) {
    let reason = native_provider_error_reason(error);
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: native_provider_failure_status(error),
    }));
    log.push(NativeSessionEvent::TurnFinished {
        session_id: NativeSessionId(String::from("default")),
        turn_id,
        outcome,
        reason: Some(reason),
    });
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
    use super::{NativeFixtureOutcome, native_fixture_outcome, native_response_chunks};

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
}
