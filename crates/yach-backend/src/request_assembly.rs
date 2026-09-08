use crate::session::{
    completed_text_exchange, EntryId, SessionEvent, SessionId, SessionLog, ToolRequestId, TurnId,
};
use crate::{ProviderMessage, ToolOutcome, ToolPayloadSummary, ToolPermissionState};

#[must_use]
pub fn fixture_log(turns: usize, tool_calls_per_turn: usize) -> SessionLog {
    build_fixture_log(turns, tool_calls_per_turn)
}

#[must_use]
pub fn assemble(
    log: &SessionLog,
    current_turn: &TurnId,
    checkpoint: Option<&str>,
) -> Vec<ProviderMessage> {
    crate::runner::provider_messages_from_event_slice(log, &log.events, current_turn, checkpoint)
}

fn build_fixture_log(turns: usize, tool_calls_per_turn: usize) -> SessionLog {
    let session_id = SessionId(String::from("session-request-assembly"));
    let mut events = Vec::new();
    for turn in 1..=turns {
        let turn_id = TurnId(format!("turn-{turn}"));
        let exchange = completed_text_exchange(
            session_id.clone(),
            EntryId(format!("entry-{turn}-user")),
            EntryId(format!("entry-{turn}-assistant")),
            turn_id.clone(),
            format!("user turn {turn}"),
            format!("assistant turn {turn}"),
        );
        events.extend(exchange.events);
        for call in 1..=tool_calls_per_turn {
            let tool_request_id = ToolRequestId(format!("tool-request-{turn}-{call}"));
            events.push(SessionEvent::ToolRequestRecorded {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_request_id: tool_request_id.clone(),
                tool_name: String::from("read_text_file"),
                provider_call_id: Some(format!("call-{turn}-{call}")),
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
            events.push(SessionEvent::ToolExecutionFinished {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_request_id,
                outcome: ToolOutcome::Completed,
                reason: None,
                result_summary: Some(ToolPayloadSummary {
                    summary: String::from("read_text_file bytes=14 truncated=false"),
                    byte_count: 14,
                    redacted: true,
                    truncated: false,
                }),
                result_content: Some(String::from("fn example() {}")),
            });
        }
    }
    SessionLog { events }
}

#[cfg(test)]
mod tests {
    use super::{assemble, fixture_log};
    use crate::session::TurnId;

    #[test]
    fn fixture_log_has_three_events_per_text_turn() {
        let log = fixture_log(10, 0);
        assert_eq!(log.events.len(), 30);
    }

    #[test]
    fn fixture_log_adds_two_events_per_tool_call() {
        let log = fixture_log(2, 3);
        assert_eq!(log.events.len(), 2 * (3 + 2 * 3));
    }

    #[test]
    fn assemble_yields_one_message_per_entry_plus_tool_pairs() {
        let log = fixture_log(5, 1);
        let messages = assemble(&log, &TurnId(String::from("turn-6")), None);
        // 5 completed turns × (user + assistant + tool-call + tool-result).
        assert_eq!(messages.len(), 20, "got {}", messages.len());
        assert!(messages.iter().any(|m| m.content.contains("turn 4")));
    }
}
