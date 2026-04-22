use yach_proto::{DialogRequest, DialogResponse, ServerEvent};

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
}

#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub content: String,
    pub is_user: bool,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append_delta(&mut self, delta: &str) {
        if let Some(last) = self.entries.last_mut()
            && !last.is_user
        {
            last.content.push_str(delta);
            return;
        }
        self.entries.push(TranscriptEntry {
            content: delta.to_owned(),
            is_user: false,
        });
    }

    pub fn append_user_message(&mut self, message: &str) {
        self.entries.push(TranscriptEntry {
            content: message.to_owned(),
            is_user: true,
        });
    }

    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn content(&self) -> String {
        self.entries
            .iter()
            .map(|entry| {
                let prefix = if entry.is_user {
                    "User: "
                } else {
                    "Assistant: "
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
    use super::{DispatchAction, Transcript, dispatch_event, resolve_dialog};
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
        assert!(!transcript.entries()[0].is_user);
        assert!(transcript.entries()[1].is_user);
        assert!(!transcript.entries()[2].is_user);
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
}
