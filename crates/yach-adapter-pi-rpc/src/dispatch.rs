use yach_proto::{DialogRequest, DialogResponse, ServerEvent};

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    UserMessage,
    AssistantText,
    ToolCall { name: String },
    ToolResult { name: String },
    Compaction,
}

#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub content: String,
    pub kind: EntryKind,
}

impl TranscriptEntry {
    pub fn is_user(&self) -> bool {
        matches!(self.kind, EntryKind::UserMessage)
    }
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append_delta(&mut self, delta: &str) {
        if let Some(last) = self.entries.last_mut()
            && matches!(last.kind, EntryKind::AssistantText)
        {
            last.content.push_str(delta);
            return;
        }
        self.entries.push(TranscriptEntry {
            content: delta.to_owned(),
            kind: EntryKind::AssistantText,
        });
    }

    pub fn append_user_message(&mut self, message: &str) {
        self.entries.push(TranscriptEntry {
            content: message.to_owned(),
            kind: EntryKind::UserMessage,
        });
    }

    pub fn append_tool_call(&mut self, name: &str) {
        self.entries.push(TranscriptEntry {
            content: String::new(),
            kind: EntryKind::ToolCall {
                name: name.to_owned(),
            },
        });
    }

    pub fn append_tool_result(&mut self, name: &str, result: &str) {
        self.entries.push(TranscriptEntry {
            content: result.to_owned(),
            kind: EntryKind::ToolResult {
                name: name.to_owned(),
            },
        });
    }

    pub fn append_compaction(&mut self) {
        self.entries.push(TranscriptEntry {
            content: String::from("context compacted"),
            kind: EntryKind::Compaction,
        });
    }

    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn turn_boundaries(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.kind, EntryKind::UserMessage))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn tool_call_boundaries(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.kind, EntryKind::ToolCall { .. }))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn content(&self) -> String {
        self.entries
            .iter()
            .map(|entry| {
                let prefix = match &entry.kind {
                    EntryKind::UserMessage => "User: ",
                    EntryKind::AssistantText => "Assistant: ",
                    EntryKind::ToolCall { name } => &format!("[tool call: {name}] "),
                    EntryKind::ToolResult { name } => &format!("[tool result: {name}] "),
                    EntryKind::Compaction => "[compaction] ",
                };
                format!("{prefix}{}", entry.content)
            })
            .collect::<Vec<_>>()
            .join("\n---\n")
    }
}

pub enum DispatchAction {
    AppendDelta(String),
    DialogRequested(DialogRequest),
    StatusMessage(String),
    ToolCallStarted { tool_name: String },
    SessionChanged { session_id: String },
    ModelChanged { model: String },
    TitleChanged { title: String },
    Notification { level: String, message: String },
    StreamComplete,
}

pub fn dispatch_event(event: ServerEvent) -> Option<DispatchAction> {
    match event {
        ServerEvent::PromptDelta { delta, .. } => Some(DispatchAction::AppendDelta(delta)),
        ServerEvent::DialogRequested(request) => Some(DispatchAction::DialogRequested(request)),
        ServerEvent::StatusUpdated { message } => Some(DispatchAction::StatusMessage(message)),
        ServerEvent::ToolCallStarted { tool_name } => {
            Some(DispatchAction::ToolCallStarted { tool_name })
        }
        ServerEvent::SessionChanged { session_id } => {
            Some(DispatchAction::SessionChanged { session_id })
        }
        ServerEvent::ModelChanged { model } => Some(DispatchAction::ModelChanged { model }),
        ServerEvent::TitleChanged { title } => Some(DispatchAction::TitleChanged { title }),
        ServerEvent::NotificationRaised(notification) => Some(DispatchAction::Notification {
            level: notification.level,
            message: notification.message,
        }),
        ServerEvent::WidgetUpdated(widget) => Some(DispatchAction::StatusMessage(format!(
            "[widget: {}] {}",
            widget.title, widget.body
        ))),
        ServerEvent::Ready { .. } => None,
    }
}

pub fn resolve_dialog(request: &DialogRequest, input: &str) -> DialogResponse {
    match &request.kind {
        yach_proto::DialogKind::Confirm => DialogResponse::Confirmed {
            accepted: matches!(input.to_lowercase().as_str(), "y" | "yes" | "true"),
        },
        yach_proto::DialogKind::Input { .. } | yach_proto::DialogKind::Editor { .. } => {
            DialogResponse::Text {
                value: input.to_owned(),
            }
        }
        yach_proto::DialogKind::Select { options } => {
            if options.is_empty() {
                return DialogResponse::Cancelled;
            }
            let trimmed = input.trim();
            if let Ok(index) = trimmed.parse::<usize>()
                && index < options.len()
            {
                return DialogResponse::Selection {
                    value: options[index].value.clone(),
                };
            }
            if let Some(option) = options
                .iter()
                .find(|opt| opt.label.eq_ignore_ascii_case(trimmed))
            {
                DialogResponse::Selection {
                    value: option.value.clone(),
                }
            } else {
                DialogResponse::Selection {
                    value: options[0].value.clone(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DispatchAction, EntryKind, Transcript, dispatch_event, resolve_dialog};
    use yach_proto::{DialogKind, DialogOption, DialogRequest, ServerEvent};

    #[test]
    fn transcript_accumulates_deltas_into_single_entry() {
        let mut transcript = Transcript::new();
        transcript.append_delta("hello");
        transcript.append_delta(" world");

        assert_eq!(transcript.entries().len(), 1);
        assert_eq!(transcript.entries()[0].content, "hello world");
    }

    #[test]
    fn transcript_separates_user_and_assistant_entries() {
        let mut transcript = Transcript::new();
        transcript.append_delta("assistant reply");
        transcript.append_user_message("user question");
        transcript.append_delta("another reply");

        assert_eq!(transcript.entries().len(), 3);
        assert!(matches!(
            transcript.entries()[0].kind,
            EntryKind::AssistantText
        ));
        assert!(matches!(
            transcript.entries()[1].kind,
            EntryKind::UserMessage
        ));
        assert!(matches!(
            transcript.entries()[2].kind,
            EntryKind::AssistantText
        ));
    }

    #[test]
    fn dispatch_maps_prompt_delta_to_append_action() {
        let event = ServerEvent::PromptDelta {
            session_id: String::from("sess-1"),
            delta: String::from("text"),
        };
        let action = dispatch_event(event);
        assert!(action.is_some());
        let Some(DispatchAction::AppendDelta(delta)) = action else {
            unreachable!();
        };
        assert_eq!(delta, "text");
    }

    #[test]
    fn dispatch_maps_dialog_request() {
        let event = ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-1")),
            title: None,
            prompt: None,
            kind: DialogKind::Confirm,
        });
        let action = dispatch_event(event);
        assert!(action.is_some());
        let Some(DispatchAction::DialogRequested(request)) = action else {
            unreachable!();
        };
        assert_eq!(request.id.as_deref(), Some("dlg-1"));
    }

    #[test]
    fn dispatch_returns_none_for_ready_event() {
        let handshake = yach_proto::Handshake::new("test", vec![]);
        let event = ServerEvent::Ready { handshake };
        assert!(dispatch_event(event).is_none());
    }

    #[test]
    fn resolve_dialog_confirms_on_yes() {
        let request = DialogRequest {
            id: None,
            title: None,
            prompt: None,
            kind: DialogKind::Confirm,
        };
        let response = resolve_dialog(&request, "yes");
        let yach_proto::DialogResponse::Confirmed { accepted } = response else {
            unreachable!();
        };
        assert!(accepted);
    }

    #[test]
    fn resolve_dialog_confirms_n_on_no() {
        let request = DialogRequest {
            id: None,
            title: None,
            prompt: None,
            kind: DialogKind::Confirm,
        };
        let response = resolve_dialog(&request, "no");
        let yach_proto::DialogResponse::Confirmed { accepted } = response else {
            unreachable!();
        };
        assert!(!accepted);
    }

    #[test]
    fn resolve_dialog_selects_by_label() {
        let request = DialogRequest {
            id: None,
            title: None,
            prompt: None,
            kind: DialogKind::Select {
                options: vec![
                    DialogOption {
                        label: String::from("Alpha"),
                        value: String::from("a"),
                    },
                    DialogOption {
                        label: String::from("Beta"),
                        value: String::from("b"),
                    },
                ],
            },
        };
        let response = resolve_dialog(&request, "Beta");
        let yach_proto::DialogResponse::Selection { value } = response else {
            unreachable!();
        };
        assert_eq!(value, "b");
    }

    #[test]
    fn resolve_dialog_selects_by_index() {
        let request = DialogRequest {
            id: None,
            title: None,
            prompt: None,
            kind: DialogKind::Select {
                options: vec![
                    DialogOption {
                        label: String::from("Alpha"),
                        value: String::from("a"),
                    },
                    DialogOption {
                        label: String::from("Beta"),
                        value: String::from("b"),
                    },
                ],
            },
        };
        let response = resolve_dialog(&request, "1");
        let yach_proto::DialogResponse::Selection { value } = response else {
            unreachable!();
        };
        assert_eq!(value, "b");
    }

    #[test]
    fn resolve_dialog_returns_text_for_input() {
        let request = DialogRequest {
            id: None,
            title: None,
            prompt: None,
            kind: DialogKind::Input { default: None },
        };
        let response = resolve_dialog(&request, "my answer");
        let yach_proto::DialogResponse::Text { value } = response else {
            unreachable!();
        };
        assert_eq!(value, "my answer");
    }

    #[test]
    fn transcript_tracks_tool_call_entries() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("run a tool");
        transcript.append_tool_call("Read");
        transcript.append_tool_result("Read", "file contents");
        transcript.append_delta("here is the result");

        assert_eq!(transcript.entries().len(), 4);
        assert!(matches!(
            transcript.entries()[1].kind,
            EntryKind::ToolCall { .. }
        ));
        assert!(matches!(
            transcript.entries()[2].kind,
            EntryKind::ToolResult { .. }
        ));
    }

    #[test]
    fn transcript_tracks_compaction_entries() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("first message");
        transcript.append_delta("reply");
        transcript.append_compaction();

        assert_eq!(transcript.entries().len(), 3);
        assert!(matches!(
            transcript.entries()[2].kind,
            EntryKind::Compaction
        ));
    }

    #[test]
    fn turn_boundaries_returns_user_message_indices() {
        let mut transcript = Transcript::new();
        transcript.append_delta("initial");
        transcript.append_user_message("first turn");
        transcript.append_delta("reply one");
        transcript.append_user_message("second turn");
        transcript.append_delta("reply two");

        let boundaries = transcript.turn_boundaries();
        assert_eq!(boundaries, vec![1, 3]);
    }

    #[test]
    fn tool_call_boundaries_returns_tool_call_indices() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("do something");
        transcript.append_tool_call("Read");
        transcript.append_delta("reading...");
        transcript.append_tool_call("Write");

        let boundaries = transcript.tool_call_boundaries();
        assert_eq!(boundaries, vec![1, 3]);
    }
}
