use serde_json::json;
use yach_proto::{ClientEvent, DialogResponse, MessageBody, MessageMeta, TransportMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializeError {
    WrongDirection,
    WrongBodyType,
}

pub fn serialize_client_message(message: &TransportMessage) -> Result<String, SerializeError> {
    if message.direction != yach_proto::MessageDirection::ClientToAdapter {
        return Err(SerializeError::WrongDirection);
    }

    let MessageBody::ClientEvent(event) = &message.body else {
        return Err(SerializeError::WrongBodyType);
    };

    Ok(serialize_client_event(event, &message.meta))
}

fn serialize_client_event(event: &ClientEvent, _meta: &MessageMeta) -> String {
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
        ClientEvent::ModelSelected { model } => json!({
            "type": "set_model",
            "model": model,
        }),
        ClientEvent::SessionForkRequested { session_id } => json!({
            "type": "fork_session",
            "sessionId": session_id,
        }),
        ClientEvent::DialogResolved { dialog_id, response } => json!({
            "type": "extension_ui_response",
            "id": dialog_id,
            "response": dialog_response_payload(response),
        }),
        ClientEvent::WidgetCleared { widget_id } => json!({
            "type": "clear_widget",
            "widgetKey": widget_id,
        }),
    };

    let mut line = envelope.to_string();
    line.push('\n');
    line
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
        ClientEvent, DialogResponse, MessageMeta, TransportMessage, default_ui_handshake,
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
            ClientEvent::ModelSelected {
                model: String::from("gpt-5"),
            },
        );
        let model_line = serialize_client_message(&model);
        assert!(model_line.is_ok());
        let Ok(model_line) = model_line else {
            return;
        };
        assert!(model_line.contains("\"type\":\"set_model\""));

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
