use serde_json::json;
use yach_proto::{ClientEvent, DialogResponse, MessageBody, MessageMeta, TransportMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializeError {
    WrongDirection,
    WrongBodyType,
    UnsupportedEvent,
}

pub fn serialize_client_message(message: &TransportMessage) -> Result<String, SerializeError> {
    if message.direction != yach_proto::MessageDirection::ClientToAdapter {
        return Err(SerializeError::WrongDirection);
    }

    let MessageBody::ClientEvent(event) = &message.body else {
        return Err(SerializeError::WrongBodyType);
    };

    serialize_client_event(event, &message.meta)
}

fn serialize_client_event(
    event: &ClientEvent,
    _meta: &MessageMeta,
) -> Result<String, SerializeError> {
    let envelope = match event {
        ClientEvent::Initialize(handshake) => json!({
            "type": "get_state",
            "handshake": {
                "protocol_version": handshake.protocol_version,
                "agent_name": handshake.agent_name,
                "capabilities": handshake.capabilities,
            }
        }),
        ClientEvent::PromptSubmitted { prompt, .. } => json!({
            "type": "prompt",
            "message": prompt,
        }),
        ClientEvent::SessionSelected { session_id } => json!({
            "type": "switch_session",
            "sessionId": session_id,
        }),
        ClientEvent::SessionPathSelected { session_path } => json!({
            "type": "switch_session",
            "sessionPath": session_path,
        }),
        ClientEvent::AvailableModelsRequested => json!({
            "type": "get_available_models",
        }),
        ClientEvent::ForkMessagesRequested => json!({
            "type": "get_fork_messages",
        }),
        ClientEvent::SessionMessagesRequested => json!({
            "type": "get_messages",
        }),
        ClientEvent::SessionStatsRequested => json!({
            "type": "get_session_stats",
        }),
        ClientEvent::PromptCancelled { .. } | ClientEvent::RecentSessionsRequested => {
            return Err(SerializeError::UnsupportedEvent);
        }
        ClientEvent::ModelSelected { model } => legacy_model_selection(model),
        ClientEvent::ModelSelectedDetailed { provider, model_id } => json!({
            "type": "set_model",
            "provider": provider,
            "modelId": model_id,
        }),
        ClientEvent::SessionForkRequested {
            entry_id: Some(entry_id),
            position,
            ..
        } => json!({
            "type": "fork",
            "entryId": entry_id,
            "position": position.as_rpc_value(),
        }),
        ClientEvent::SessionForkRequested { .. } => json!({
            "type": "clone",
        }),
        ClientEvent::DialogResolved {
            dialog_id,
            response,
        } => json!({
            "type": "extension_ui_response",
            "id": dialog_id,
            "response": dialog_response_payload(response),
        }),
        ClientEvent::WidgetCleared { widget_id } => json!({
            "type": "clear_widget",
            "widgetKey": widget_id,
        }),
        ClientEvent::ThinkingLevelSelected { level } => json!({
            "type": "set_thinking_level",
            "level": level,
        }),
    };

    let mut line = envelope.to_string();
    line.push('\n');
    Ok(line)
}

fn legacy_model_selection(model: &str) -> serde_json::Value {
    if let Some((provider, model_id)) = model.split_once('/') {
        json!({
            "type": "set_model",
            "provider": provider,
            "modelId": model_id,
        })
    } else {
        json!({
            "type": "set_model",
            "model": model,
        })
    }
}

fn dialog_response_payload(response: &DialogResponse) -> serde_json::Value {
    match response {
        DialogResponse::Confirmed { accepted } => json!({ "confirmed": accepted }),
        DialogResponse::Text { value } | DialogResponse::Selection { value } => {
            json!({ "value": value })
        }
        DialogResponse::Cancelled => json!({ "cancelled": true }),
    }
}

#[cfg(test)]
mod tests {
    use super::{SerializeError, serialize_client_message};
    use yach_proto::{
        ClientEvent, DialogResponse, ForkPosition, MessageMeta, TransportMessage,
        default_ui_handshake,
    };

    #[test]
    fn serializer_maps_prompt_messages_into_rpc_lines() {
        let message = TransportMessage::client(
            MessageMeta::new("msg-5")
                .with_correlation_id("req-5")
                .with_stream_id("stream-5"),
            ClientEvent::PromptSubmitted {
                session_id: String::from("sess-5"),
                prompt: String::from("hello from yach"),
            },
        );

        let line = serialize_client_message(&message);
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };

        assert!(line.ends_with('\n'));
        assert!(line.contains("\"type\":\"prompt\""));
        assert!(line.contains("\"message\":\"hello from yach\""));
    }

    #[test]
    fn serializer_maps_initialize_messages_into_rpc_lines() {
        let message = TransportMessage::client(
            MessageMeta::new("msg-6"),
            ClientEvent::Initialize(default_ui_handshake()),
        );

        let line = serialize_client_message(&message);
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };

        assert!(line.contains("\"type\":\"get_state\""));
        assert!(line.contains("\"agent_name\":\"yach-ui\""));
        assert!(line.contains("\"capabilities\""));
    }

    #[test]
    fn serializer_maps_dialog_responses() {
        let message = TransportMessage::client(
            MessageMeta::new("msg-7"),
            ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-1"),
                response: DialogResponse::Selection {
                    value: String::from("a"),
                },
            },
        );

        let line = serialize_client_message(&message);
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };

        assert!(line.contains("\"type\":\"extension_ui_response\""));
        assert!(line.contains("\"id\":\"dlg-1\""));
        assert!(line.contains("\"value\":\"a\""));
    }

    #[test]
    fn serializer_maps_model_and_widget_commands() {
        let model = TransportMessage::client(
            MessageMeta::new("msg-9"),
            ClientEvent::ModelSelectedDetailed {
                provider: String::from("openai"),
                model_id: String::from("gpt-5"),
            },
        );
        let model_line = serialize_client_message(&model);
        assert!(model_line.is_ok());
        let Ok(model_line) = model_line else {
            return;
        };
        assert!(model_line.contains("\"type\":\"set_model\""));
        assert!(model_line.contains("\"provider\":\"openai\""));
        assert!(model_line.contains("\"modelId\":\"gpt-5\""));

        let legacy_model = TransportMessage::client(
            MessageMeta::new("msg-9-legacy"),
            ClientEvent::ModelSelected {
                model: String::from("anthropic/claude-sonnet-4-20250514"),
            },
        );
        let legacy_model_line = serialize_client_message(&legacy_model);
        assert!(legacy_model_line.is_ok());
        let Ok(legacy_model_line) = legacy_model_line else {
            return;
        };
        assert!(legacy_model_line.contains("\"provider\":\"anthropic\""));
        assert!(legacy_model_line.contains("\"modelId\":\"claude-sonnet-4-20250514\""));

        let models = TransportMessage::client(
            MessageMeta::new("msg-9a"),
            ClientEvent::AvailableModelsRequested,
        );
        let models_line = serialize_client_message(&models);
        assert!(models_line.is_ok());
        let Ok(models_line) = models_line else {
            return;
        };
        assert!(models_line.contains("\"type\":\"get_available_models\""));

        let fork_messages = TransportMessage::client(
            MessageMeta::new("msg-9a-fork-messages"),
            ClientEvent::ForkMessagesRequested,
        );
        let fork_messages_line = serialize_client_message(&fork_messages);
        assert!(fork_messages_line.is_ok());
        let Ok(fork_messages_line) = fork_messages_line else {
            return;
        };
        assert!(fork_messages_line.contains("\"type\":\"get_fork_messages\""));

        let fork = TransportMessage::client(
            MessageMeta::new("msg-9b"),
            ClientEvent::SessionForkRequested {
                session_id: String::from("current"),
                entry_id: None,
                position: ForkPosition::Before,
            },
        );
        let fork_line = serialize_client_message(&fork);
        assert!(fork_line.is_ok());
        let Ok(fork_line) = fork_line else {
            return;
        };
        assert!(fork_line.contains("\"type\":\"clone\""));
        assert!(!fork_line.contains("sessionId"));

        let entry_fork = TransportMessage::client(
            MessageMeta::new("msg-9c"),
            ClientEvent::SessionForkRequested {
                session_id: String::from("current"),
                entry_id: Some(String::from("entry-7")),
                position: ForkPosition::Before,
            },
        );
        let entry_fork_line = serialize_client_message(&entry_fork);
        assert!(entry_fork_line.is_ok());
        let Ok(entry_fork_line) = entry_fork_line else {
            return;
        };
        assert!(entry_fork_line.contains("\"type\":\"fork\""));
        assert!(entry_fork_line.contains("\"entryId\":\"entry-7\""));
        assert!(entry_fork_line.contains("\"position\":\"before\""));

        let widget = TransportMessage::client(
            MessageMeta::new("msg-10"),
            ClientEvent::WidgetCleared {
                widget_id: String::from("tool-1"),
            },
        );
        let widget_line = serialize_client_message(&widget);
        assert!(widget_line.is_ok());
        let Ok(widget_line) = widget_line else {
            return;
        };
        assert!(widget_line.contains("\"type\":\"clear_widget\""));
        assert!(widget_line.contains("\"widgetKey\":\"tool-1\""));
    }

    #[test]
    fn serializer_rejects_prompt_cancel_for_pi_rpc() {
        let message = TransportMessage::client(
            MessageMeta::new("msg-cancel"),
            ClientEvent::PromptCancelled {
                session_id: String::from("default"),
            },
        );

        let error = serialize_client_message(&message);

        assert_eq!(error, Err(SerializeError::UnsupportedEvent));
    }

    #[test]
    fn serializer_rejects_server_messages() {
        let message = TransportMessage::server(
            MessageMeta::new("msg-8"),
            yach_proto::ServerEvent::StatusUpdated {
                message: String::from("ready"),
            },
        );

        let error = serialize_client_message(&message);
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        assert_eq!(error, SerializeError::WrongDirection);
    }
}
