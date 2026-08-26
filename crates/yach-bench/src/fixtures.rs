use yach_proto::{
    BackendEvent, BackendState, Capability, Handshake, NegotiatedCapabilities, ServerEvent,
    ThinkingLevel, ToolResult,
};
use yach_ui::Transcript;

pub const SESSION_ID: &str = "bench-session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptScale {
    Small,
    Medium,
    Large,
    Huge,
}

impl TranscriptScale {
    #[must_use]
    pub const fn entries(self) -> usize {
        match self {
            Self::Small => 100,
            Self::Medium => 1_000,
            Self::Large => 10_000,
            Self::Huge => 50_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadScale {
    Small,
    Medium,
    Large,
}

impl PayloadScale {
    #[must_use]
    pub const fn bytes(self) -> usize {
        match self {
            Self::Small => 10 * 1024,
            Self::Medium => 100 * 1024,
            Self::Large => 1024 * 1024,
        }
    }
}

#[must_use]
pub fn negotiated_capabilities() -> NegotiatedCapabilities {
    let ui = Handshake::new(
        "yach-ui-bench",
        vec![
            Capability::PromptStreaming,
            Capability::Dialogs,
            Capability::Notifications,
            Capability::StatusEntries,
            Capability::Widgets,
        ],
    );
    let adapter = Handshake::new(
        "yach-adapter-bench",
        vec![
            Capability::PromptStreaming,
            Capability::Dialogs,
            Capability::Notifications,
            Capability::StatusEntries,
            Capability::Widgets,
        ],
    );
    NegotiatedCapabilities::from_handshakes(&ui, &adapter)
}

#[must_use]
pub fn connected_event() -> BackendEvent {
    BackendEvent::Connected {
        negotiated: negotiated_capabilities(),
    }
}

#[must_use]
pub fn ready_state_event() -> BackendEvent {
    BackendEvent::Server(ServerEvent::StateUpdated(Box::new(BackendState {
        session_model: yach_proto::SessionModelState::Resolving { requested: None },
        default_model: yach_proto::DefaultModelState::Absent,
        session_id: Some(String::from(SESSION_ID)),
        session_file: Some(String::from("/tmp/yach-bench-session.jsonl")),
        thinking_level: Some(ThinkingLevel::Low),
        is_streaming: false,
        is_compacting: false,
        message_count: Some(0),
        pending_message_count: Some(0),
    })))
}

#[must_use]
pub fn transcript_fixture(scale: TranscriptScale) -> Transcript {
    let mut transcript = Transcript::new();
    for i in 0..scale.entries() {
        match i % 3 {
            0 => transcript.append_user_message(&format!("user asks benchmark question {i}")),
            1 => transcript.append_delta(&format!("assistant benchmark response chunk {i}. ")),
            _ => transcript.append_tool_call(
                Some(&format!("tool-call-{i}")),
                "bench_tool",
                Some(&format!("preview {i}")),
            ),
        }
    }
    transcript
}

#[must_use]
pub fn prompt_delta_events(count: usize) -> Vec<BackendEvent> {
    (0..count)
        .map(|i| {
            BackendEvent::Server(ServerEvent::PromptDelta {
                session_id: String::from(SESSION_ID),
                delta: format!("token_{i} "),
            })
        })
        .collect()
}

#[must_use]
pub fn heavy_tool_events(scale: PayloadScale) -> Vec<BackendEvent> {
    let output = repeated_payload(
        scale.bytes(),
        "tool output line with deterministic content\n",
    );
    vec![
        BackendEvent::Server(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("tool-call-heavy")),
            tool_name: String::from("bench_heavy_tool"),
            preview: Some(String::from("generating deterministic heavy output")),
        }),
        BackendEvent::Server(ServerEvent::ToolCallFinished(ToolResult {
            tool_call_id: Some(String::from("tool-call-heavy")),
            tool_name: String::from("bench_heavy_tool"),
            output,
            is_error: false,
            outcome_kind: None,
            metadata: None,
        })),
    ]
}

#[must_use]
pub fn large_paste_payload(scale: PayloadScale) -> String {
    let prefix = "/not-a-command\nfirst line\nUnicode: 🦀 測試 café\n";
    let mut payload = String::from(prefix);
    payload.push_str(&repeated_payload(
        scale.bytes().saturating_sub(prefix.len()),
        "paste line\n",
    ));
    payload
}

fn repeated_payload(target_bytes: usize, unit: &str) -> String {
    let mut payload = String::with_capacity(target_bytes);
    while payload.len() < target_bytes {
        payload.push_str(unit);
    }
    payload.truncate(target_bytes);
    payload
}

#[cfg(test)]
mod tests {
    use yach_proto::{BackendEvent, ServerEvent};

    use super::{
        PayloadScale, TranscriptScale, heavy_tool_events, large_paste_payload, prompt_delta_events,
        transcript_fixture,
    };

    #[test]
    fn huge_transcript_fixture_has_configured_entries() {
        let transcript = transcript_fixture(TranscriptScale::Huge);

        assert_eq!(transcript.entries().len(), TranscriptScale::Huge.entries());
    }

    #[test]
    fn active_stream_fixture_produces_ordered_prompt_deltas() {
        let events = prompt_delta_events(3);

        assert_eq!(events.len(), 3);
        assert_eq!(
            events,
            vec![
                BackendEvent::Server(ServerEvent::PromptDelta {
                    session_id: String::from("bench-session"),
                    delta: String::from("token_0 "),
                }),
                BackendEvent::Server(ServerEvent::PromptDelta {
                    session_id: String::from("bench-session"),
                    delta: String::from("token_1 "),
                }),
                BackendEvent::Server(ServerEvent::PromptDelta {
                    session_id: String::from("bench-session"),
                    delta: String::from("token_2 "),
                }),
            ]
        );
    }

    #[test]
    fn heavy_tool_fixture_includes_start_and_finish() {
        let events = heavy_tool_events(PayloadScale::Small);

        assert_eq!(events.len(), 2);
        assert!(matches!(
            events.first(),
            Some(BackendEvent::Server(ServerEvent::ToolCallStarted { .. }))
        ));
        assert!(matches!(
            events.get(1),
            Some(BackendEvent::Server(ServerEvent::ToolCallFinished(result))) if result.output.len() == PayloadScale::Small.bytes()
        ));
    }

    #[test]
    fn paste_payload_includes_multiline_slash_and_unicode_content() {
        let payload = large_paste_payload(PayloadScale::Small);

        assert!(payload.starts_with("/not-a-command\n"));
        assert!(payload.contains('\n'));
        assert!(payload.contains('🦀'));
        assert!(payload.len() >= PayloadScale::Small.bytes());
    }
}
