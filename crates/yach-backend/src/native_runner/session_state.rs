use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use tokio::sync::mpsc;
use yach_proto::{BackendEvent, RecentSession, ServerEvent, SessionMessage, SessionStats};

use crate::{
    NativeJsonlSessionStore, NativeRole, NativeSessionEvent, NativeSessionLoadResult,
    NativeSessionLoadWarning, NativeSessionLog, NativeToolOutcome, NativeToolPayloadSummary,
};

use super::native_session_id_from_log_path;

pub(super) fn send_native_session_messages_from_log(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &NativeSessionLog,
) {
    let mut tool_names_by_request_id = BTreeMap::new();
    let messages = log
        .events
        .iter()
        .filter_map(|event| match event {
            NativeSessionEvent::EntryAppended {
                entry_id,
                role,
                text,
                ..
            } => Some(SessionMessage {
                role: native_role_label(*role),
                text: text.clone(),
                entry_id: Some(entry_id.0.clone()),
                tool_name: None,
                is_error: None,
            }),
            NativeSessionEvent::ToolRequestRecorded {
                tool_request_id,
                tool_name,
                ..
            } => {
                tool_names_by_request_id.insert(tool_request_id.0.clone(), tool_name.clone());
                None
            }
            NativeSessionEvent::ToolExecutionFinished {
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
                let text = if let Some(content) = result_content.as_deref() {
                    super::native_tool_result_display(
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
                    let mut text = native_session_tool_result_text(
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
                    is_error: Some(*outcome != NativeToolOutcome::Completed),
                })
            }
            NativeSessionEvent::CompactionCheckpoint {
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
            NativeSessionEvent::TurnFinished { .. }
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

fn native_session_tool_result_text(
    outcome: NativeToolOutcome,
    reason: Option<&str>,
    result_summary: Option<&NativeToolPayloadSummary>,
) -> String {
    let status = match outcome {
        NativeToolOutcome::Completed => "completed",
        NativeToolOutcome::Failed => "failed",
        NativeToolOutcome::Denied => "denied",
        NativeToolOutcome::Cancelled => "cancelled",
        NativeToolOutcome::ValidationFailed => "validation_failed",
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
    log: &NativeSessionLog,
    context_budget: Option<crate::NativeContextBudget>,
) {
    let messages = log
        .events
        .iter()
        .filter_map(|event| match event {
            NativeSessionEvent::EntryAppended { role, .. } => Some(*role),
            NativeSessionEvent::ToolRequestRecorded { .. }
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
        .collect::<Vec<_>>();
    let message_count = u64::try_from(messages.len()).ok();
    let user_message_count = count_native_role(&messages, NativeRole::User);
    let assistant_message_count = count_native_role(&messages, NativeRole::Assistant);
    let tool_message_count = count_native_role(&messages, NativeRole::Tool);
    let context_used_percent = context_budget
        .map(|budget| budget.used_percent(crate::estimate_current_context_tokens(log)));
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
        .filter_map(|entry| native_recent_session_from_path(&entry.path()))
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

fn native_recent_session_from_path(path: &Path) -> Option<RecentSession> {
    let session_id = native_session_id_from_log_path(path)?;
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
        message_count: native_session_message_count(path),
        first_message: native_session_first_message(path),
    })
}

pub(super) async fn load_native_session_log_for_runner(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
) -> NativeSessionLog {
    let store = store.clone();
    load_native_session_log_for_runner_with_loader_inner(tx, move || store.load_with_warnings())
        .await
}

#[cfg(test)]
pub(super) async fn load_native_session_log_for_runner_with_loader<Load>(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    load: Load,
) -> NativeSessionLog
where
    Load: FnOnce() -> std::io::Result<NativeSessionLoadResult> + Send + 'static,
{
    load_native_session_log_for_runner_with_loader_inner(tx, load).await
}

async fn load_native_session_log_for_runner_with_loader_inner<Load>(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    load: Load,
) -> NativeSessionLog
where
    Load: FnOnce() -> std::io::Result<NativeSessionLoadResult> + Send + 'static,
{
    match tokio::task::spawn_blocking(load).await {
        Ok(load_result) => native_session_state_from_load_result(tx, load_result),
        Err(error) => {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("native dogfood: failed to load session log: {error}"),
            }));
            NativeSessionLog::default()
        }
    }
}

pub(super) fn native_session_state_from_load_result(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    load_result: std::io::Result<NativeSessionLoadResult>,
) -> NativeSessionLog {
    match load_result {
        Ok(load) => {
            for warning in load.warnings {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: native_session_load_warning_message(&warning),
                }));
            }
            load.log
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => NativeSessionLog::default(),
        Err(error) => {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("native dogfood: failed to load session log: {error}"),
            }));
            NativeSessionLog::default()
        }
    }
}

fn native_session_load_warning_message(warning: &NativeSessionLoadWarning) -> String {
    match warning {
        NativeSessionLoadWarning::InvalidJson {
            line_number,
            reason,
        } => format!(
            "native dogfood: skipped corrupt session log line {line_number}: {}",
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

fn load_native_log_or_default(path: &Path) -> NativeSessionLog {
    NativeSessionLog::load_from_file(path).unwrap_or_default()
}

pub(super) fn native_session_message_count(path: &Path) -> Option<u64> {
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
            | NativeSessionEvent::EditTransactionFinished { .. }
            | NativeSessionEvent::CompactionCheckpoint { .. } => None,
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
