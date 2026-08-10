use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use tokio::sync::mpsc;
use yach_proto::{BackendEvent, RecentSession, ServerEvent, SessionMessage, SessionStats};

use crate::{
    JsonlSessionStore, Role, SessionEvent, SessionLoadResult, SessionLoadWarning, SessionLog,
    ToolOutcome, ToolPayloadSummary,
};

use super::session_id_from_log_path;

pub(super) fn send_native_session_messages_from_log(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &SessionLog,
) {
    let masked_result_bytes = crate::masked_result_map(&log.events);
    let mut tool_names_by_request_id = BTreeMap::new();
    let messages = log
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::EntryAppended {
                entry_id,
                role,
                text,
                ..
            } => Some(SessionMessage {
                role: role_label(*role),
                text: text.clone(),
                entry_id: Some(entry_id.0.clone()),
                tool_name: None,
                is_error: None,
            }),
            SessionEvent::ToolRequestRecorded {
                tool_request_id,
                tool_name,
                ..
            } => {
                tool_names_by_request_id.insert(tool_request_id.0.clone(), tool_name.clone());
                None
            }
            SessionEvent::ToolExecutionFinished {
                turn_id,
                tool_request_id,
                outcome,
                reason,
                result_summary,
                result_content,
                ..
            } => {
                let tool_name = tool_names_by_request_id
                    .get(&tool_request_id.0)
                    .cloned()
                    .unwrap_or_else(|| String::from("tool"));
                let text = if let Some(bytes_freed) =
                    masked_result_bytes.get(&(turn_id.clone(), tool_request_id.clone()))
                {
                    crate::mask_marker(*bytes_freed)
                } else if let Some(content) = result_content.as_deref() {
                    super::tool_result_display(
                        &tool_name,
                        *outcome,
                        Some(content),
                        result_summary
                            .as_ref()
                            .map_or(content.len(), |summary| summary.byte_count),
                        result_summary
                            .as_ref()
                            .is_some_and(|summary| summary.truncated),
                        reason.as_deref(),
                    )
                } else {
                    let mut text = session_tool_result_text(
                        *outcome,
                        reason.as_deref(),
                        result_summary.as_ref(),
                    );
                    text.push_str("; output not retained (recorded before payload persistence)");
                    text
                };
                Some(SessionMessage {
                    role: String::from("tool"),
                    text,
                    entry_id: Some(tool_request_id.0.clone()),
                    tool_name: Some(tool_name),
                    is_error: Some(*outcome != ToolOutcome::Completed),
                })
            }
            SessionEvent::CompactionCheckpoint {
                checkpoint_id,
                summary,
                tokens_before,
                tokens_after_estimate,
                ..
            } => Some(SessionMessage {
                role: String::from("system"),
                text: format!(
                    "— compacted: {}K → ~{}K tokens —\n{summary}",
                    tokens_before / 1_000,
                    tokens_after_estimate / 1_000
                ),
                entry_id: Some(checkpoint_id.0.clone()),
                tool_name: None,
                is_error: None,
            }),
            SessionEvent::ToolResultMasked { .. } => None,
            SessionEvent::TurnFinished { .. }
            | SessionEvent::MetricRecorded { .. }
            | SessionEvent::StaticContextIncluded { .. }
            | SessionEvent::PermissionDecisionRecorded { .. }
            | SessionEvent::EditTraceRecorded { .. }
            | SessionEvent::EditTransactionPrepared { .. }
            | SessionEvent::EditTransactionFinished { .. } => None,
        })
        .collect();
    let _ = tx.send(BackendEvent::Server(ServerEvent::SessionMessagesUpdated {
        messages,
    }));
}

fn session_tool_result_text(
    outcome: ToolOutcome,
    reason: Option<&str>,
    result_summary: Option<&ToolPayloadSummary>,
) -> String {
    let status = match outcome {
        ToolOutcome::Completed => "completed",
        ToolOutcome::Failed => "failed",
        ToolOutcome::Denied => "denied",
        ToolOutcome::Cancelled => "cancelled",
        ToolOutcome::ValidationFailed => "validation_failed",
    };
    let mut text = result_summary.map_or_else(
        || status.to_string(),
        |summary| {
            format!(
                "{status}; bytes={}; content=redacted; truncated={}",
                summary.byte_count, summary.truncated
            )
        },
    );
    if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
        text.push_str("; reason=");
        text.push_str(reason);
    }
    text
}

pub(super) fn send_native_session_stats_from_log(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &SessionLog,
    context_budget: Option<crate::ContextBudget>,
) {
    send_native_session_stats_with_estimate(tx, log, context_budget, None);
}

/// Session stats push with an optional context-token estimate override for
/// mid-turn refreshes, where the assembled continuation context is more
/// current than the log (round text and tool results are not yet persisted
/// as entries).
pub(super) fn send_native_session_stats_with_estimate(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &SessionLog,
    context_budget: Option<crate::ContextBudget>,
    estimated_tokens_override: Option<u64>,
) {
    let messages = log
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::EntryAppended { role, .. } => Some(*role),
            SessionEvent::ToolRequestRecorded { .. }
            | SessionEvent::ToolExecutionFinished { .. }
            | SessionEvent::TurnFinished { .. }
            | SessionEvent::MetricRecorded { .. }
            | SessionEvent::StaticContextIncluded { .. }
            | SessionEvent::PermissionDecisionRecorded { .. }
            | SessionEvent::EditTraceRecorded { .. }
            | SessionEvent::EditTransactionPrepared { .. }
            | SessionEvent::EditTransactionFinished { .. }
            | SessionEvent::CompactionCheckpoint { .. }
            | SessionEvent::ToolResultMasked { .. } => None,
        })
        .collect::<Vec<_>>();
    let message_count = u64::try_from(messages.len()).ok();
    let user_message_count = count_native_role(&messages, Role::User);
    let assistant_message_count = count_native_role(&messages, Role::Assistant);
    let tool_message_count = count_native_role(&messages, Role::Tool);
    let context_used_percent = context_budget.map(|budget| {
        budget.used_percent(
            estimated_tokens_override
                .unwrap_or_else(|| crate::estimate_current_context_tokens(log)),
        )
    });
    let _ = tx.send(BackendEvent::Server(ServerEvent::SessionStatsUpdated(
        SessionStats {
            message_count,
            user_message_count,
            assistant_message_count,
            tool_message_count,
            total_tokens: None,
            context_used_percent,
        },
    )));
}

pub(super) fn send_native_recent_sessions(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
) {
    let mut sessions = session_path
        .parent()
        .and_then(|session_dir| fs::read_dir(session_dir).ok())
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| recent_session_from_path(&entry.path()))
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        right
            .modified_unix_ms
            .cmp(&left.modified_unix_ms)
            .then_with(|| left.path.cmp(&right.path))
    });
    let _ = tx.send(BackendEvent::Server(ServerEvent::RecentSessionsUpdated {
        sessions,
    }));
}

fn recent_session_from_path(path: &Path) -> Option<RecentSession> {
    let session_id = session_id_from_log_path(path)?;
    Some(RecentSession {
        path: path.to_string_lossy().into_owned(),
        id: Some(session_id.clone()),
        name: Some(format!("native {session_id}")),
        cwd: std::env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        modified_unix_ms: fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
        message_count: session_message_count(path),
        first_message: session_first_message(path),
    })
}

pub(super) async fn load_native_session_log_for_runner(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &JsonlSessionStore,
) -> SessionLog {
    let store = store.clone();
    load_native_session_log_for_runner_with_loader_inner(tx, move || store.load_with_warnings())
        .await
}

#[cfg(test)]
pub(super) async fn load_native_session_log_for_runner_with_loader<Load>(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    load: Load,
) -> SessionLog
where
    Load: FnOnce() -> std::io::Result<SessionLoadResult> + Send + 'static,
{
    load_native_session_log_for_runner_with_loader_inner(tx, load).await
}

async fn load_native_session_log_for_runner_with_loader_inner<Load>(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    load: Load,
) -> SessionLog
where
    Load: FnOnce() -> std::io::Result<SessionLoadResult> + Send + 'static,
{
    match tokio::task::spawn_blocking(load).await {
        Ok(load_result) => session_state_from_load_result(tx, load_result),
        Err(error) => {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("failed to load session log: {error}"),
            }));
            SessionLog::default()
        }
    }
}

pub(super) fn session_state_from_load_result(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    load_result: std::io::Result<SessionLoadResult>,
) -> SessionLog {
    match load_result {
        Ok(load) => {
            for warning in load.warnings {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: session_load_warning_message(&warning),
                }));
            }
            load.log
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SessionLog::default(),
        Err(error) => {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("failed to load session log: {error}"),
            }));
            SessionLog::default()
        }
    }
}

fn session_load_warning_message(warning: &SessionLoadWarning) -> String {
    match warning {
        SessionLoadWarning::InvalidJson {
            line_number,
            reason,
        } => format!(
            "skipped corrupt session log line {line_number}: {}",
            bounded_session_load_warning_reason(reason)
        ),
    }
}

fn bounded_session_load_warning_reason(reason: &str) -> String {
    const MAX_REASON_BYTES: usize = 160;
    if reason.len() <= MAX_REASON_BYTES {
        return reason.to_owned();
    }

    let mut end = MAX_REASON_BYTES;
    while !reason.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}...", &reason[..end])
}

fn load_native_log_or_default(path: &Path) -> SessionLog {
    SessionLog::load_from_file(path).unwrap_or_default()
}

pub(super) fn session_message_count(path: &Path) -> Option<u64> {
    u64::try_from(
        load_native_log_or_default(path)
            .events
            .iter()
            .filter(|event| matches!(event, SessionEvent::EntryAppended { .. }))
            .count(),
    )
    .ok()
}

fn session_first_message(path: &Path) -> Option<String> {
    load_native_log_or_default(path)
        .events
        .into_iter()
        .find_map(|event| match event {
            SessionEvent::EntryAppended { text, .. } => Some(text),
            SessionEvent::ToolRequestRecorded { .. }
            | SessionEvent::ToolExecutionFinished { .. }
            | SessionEvent::TurnFinished { .. }
            | SessionEvent::MetricRecorded { .. }
            | SessionEvent::StaticContextIncluded { .. }
            | SessionEvent::PermissionDecisionRecorded { .. }
            | SessionEvent::EditTraceRecorded { .. }
            | SessionEvent::EditTransactionPrepared { .. }
            | SessionEvent::EditTransactionFinished { .. }
            | SessionEvent::CompactionCheckpoint { .. }
            | SessionEvent::ToolResultMasked { .. } => None,
        })
}

fn role_label(role: Role) -> String {
    match role {
        Role::User => String::from("user"),
        Role::Assistant => String::from("assistant"),
        Role::Tool => String::from("tool"),
        Role::System => String::from("system"),
    }
}

fn count_native_role(messages: &[Role], role: Role) -> Option<u64> {
    u64::try_from(
        messages
            .iter()
            .filter(|message_role| **message_role == role)
            .count(),
    )
    .ok()
}
