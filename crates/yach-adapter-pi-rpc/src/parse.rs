use serde::Deserialize;
use serde_json::Value;
use yach_proto::{
    DialogKind, DialogOption, DialogRequest, MessageMeta, Notification, ServerEvent,
    TransportMessage, WidgetState,
};

use crate::capabilities::stock_rpc_handshake;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    EmptyLine,
    InvalidJson(String),
    UnsupportedMethod(String),
    MissingField(&'static str),
}

#[derive(Debug, Deserialize)]
struct PiRpcEnvelope {
    #[serde(default)]
    id: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

pub fn parse_server_line(
    line: &str,
    message_id: impl Into<String>,
) -> Result<TransportMessage, ParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyLine);
    }

    let envelope: PiRpcEnvelope =
        serde_json::from_str(trimmed).map_err(|error| ParseError::InvalidJson(error.to_string()))?;

    let meta = build_message_meta(message_id.into(), &envelope);
    let event = map_server_event(&envelope)?;

    Ok(TransportMessage::server(meta, event))
}

fn build_message_meta(message_id: String, envelope: &PiRpcEnvelope) -> MessageMeta {
    let mut meta = MessageMeta::new(message_id);

    if let Some(correlation_id) = &envelope.id {
        meta = meta.with_correlation_id(correlation_id.clone());
    }

    if let Some(stream_id) = envelope
        .params
        .get("stream_id")
        .and_then(Value::as_str)
        .or_else(|| envelope.params.get("session_id").and_then(Value::as_str))
    {
        meta = meta.with_stream_id(stream_id.to_owned());
    }

    meta
}

fn map_server_event(envelope: &PiRpcEnvelope) -> Result<ServerEvent, ParseError> {
    match envelope.method.as_str() {
        "ready" => Ok(ServerEvent::Ready {
            handshake: stock_rpc_handshake(),
        }),
        "prompt_delta" | "promptDelta" => Ok(ServerEvent::PromptDelta {
            session_id: required_string(&envelope.params, "session_id")?,
            delta: required_string(&envelope.params, "delta")?,
        }),
        "tool_call_started" | "toolCallStarted" => Ok(ServerEvent::ToolCallStarted {
            tool_name: required_string(&envelope.params, "tool_name")?,
        }),
        "status_updated" | "setStatus" => Ok(ServerEvent::StatusUpdated {
            message: required_string(&envelope.params, "message")?,
        }),
        "session_changed" | "switchSession" => Ok(ServerEvent::SessionChanged {
            session_id: required_string(&envelope.params, "session_id")?,
        }),
        "model_changed" | "setModel" => Ok(ServerEvent::ModelChanged {
            model: required_string(&envelope.params, "model")?,
        }),
        "notify" => Ok(ServerEvent::NotificationRaised(Notification {
            level: optional_string(&envelope.params, "level").unwrap_or_else(|| String::from("info")),
            message: required_string(&envelope.params, "message")?,
        })),
        "setWidget" | "widget_updated" => Ok(ServerEvent::WidgetUpdated(WidgetState {
            id: required_string(&envelope.params, "id")?,
            title: optional_string(&envelope.params, "title").unwrap_or_else(|| String::from("Widget")),
            body: optional_string(&envelope.params, "body")
                .or_else(|| optional_string(&envelope.params, "text"))
                .unwrap_or_default(),
        })),
        "setTitle" | "title_changed" => Ok(ServerEvent::TitleChanged {
            title: required_string(&envelope.params, "title")?,
        }),
        "select" | "confirm" | "input" | "editor" => {
            Ok(ServerEvent::DialogRequested(parse_dialog_request(envelope)?))
        }
        _ => Err(ParseError::UnsupportedMethod(envelope.method.clone())),
    }
}

fn parse_dialog_request(envelope: &PiRpcEnvelope) -> Result<DialogRequest, ParseError> {
    let title = optional_string(&envelope.params, "title");
    let prompt = optional_string(&envelope.params, "prompt").or_else(|| optional_string(&envelope.params, "message"));

    let kind = match envelope.method.as_str() {
        "select" => DialogKind::Select {
            options: dialog_options(&envelope.params),
        },
        "confirm" => DialogKind::Confirm,
        "input" => DialogKind::Input {
            default: optional_string(&envelope.params, "default"),
        },
        "editor" => DialogKind::Editor {
            initial_text: optional_string(&envelope.params, "text")
                .or_else(|| optional_string(&envelope.params, "initial_text")),
        },
        other => return Err(ParseError::UnsupportedMethod(String::from(other))),
    };

    Ok(DialogRequest {
        id: envelope.id.clone(),
        title,
        prompt,
        kind,
    })
}

fn dialog_options(params: &Value) -> Vec<DialogOption> {
    params
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    let label = option.get("label").and_then(Value::as_str)?;
                    let value = option
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or(label);

                    Some(DialogOption {
                        label: String::from(label),
                        value: String::from(value),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn required_string(params: &Value, field_name: &'static str) -> Result<String, ParseError> {
    params
        .get(field_name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(ParseError::MissingField(field_name))
}

fn optional_string(params: &Value, field_name: &'static str) -> Option<String> {
    params
        .get(field_name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{ParseError, parse_server_line};
    use yach_proto::{DialogKind, MessageBody, Notification, ServerEvent, WidgetState};

    #[test]
    fn parser_maps_prompt_delta_lines_into_transport_messages() {
        let line =
            r#"{"id":"req-1","method":"prompt_delta","params":{"session_id":"sess-1","delta":"hello","stream_id":"stream-9"}}"#;

        let message = parse_server_line(line, "msg-1");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(message.meta.message_id, "msg-1");
        assert_eq!(message.meta.correlation_id.as_deref(), Some("req-1"));
        assert_eq!(message.meta.stream_id.as_deref(), Some("stream-9"));
        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::PromptDelta {
                session_id: String::from("sess-1"),
                delta: String::from("hello"),
            })
        );
    }

    #[test]
    fn parser_maps_status_aliases() {
        let line = r#"{"method":"setStatus","params":{"message":"syncing"}}"#;

        let message = parse_server_line(line, "msg-2");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("syncing"),
            })
        );
    }

    #[test]
    fn parser_maps_dialog_requests() {
        let line = r#"{"id":"dlg-1","method":"select","params":{"title":"Pick one","options":[{"label":"Alpha","value":"a"}]}}"#;

        let message = parse_server_line(line, "msg-3");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        let MessageBody::ServerEvent(ServerEvent::DialogRequested(dialog)) = message.body else {
            unreachable!();
        };
        assert_eq!(dialog.id.as_deref(), Some("dlg-1"));
        assert_eq!(dialog.title.as_deref(), Some("Pick one"));
        let DialogKind::Select { options } = dialog.kind else {
            unreachable!();
        };
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].value, "a");
    }

    #[test]
    fn parser_maps_session_and_model_events() {
        let session = parse_server_line(
            r#"{"method":"switchSession","params":{"session_id":"sess-2"}}"#,
            "msg-4",
        );
        assert!(session.is_ok());
        let Ok(session) = session else {
            return;
        };
        assert_eq!(
            session.body,
            MessageBody::ServerEvent(ServerEvent::SessionChanged {
                session_id: String::from("sess-2"),
            })
        );

        let model = parse_server_line(
            r#"{"method":"setModel","params":{"model":"gpt-5"}}"#,
            "msg-5",
        );
        assert!(model.is_ok());
        let Ok(model) = model else {
            return;
        };
        assert_eq!(
            model.body,
            MessageBody::ServerEvent(ServerEvent::ModelChanged {
                model: String::from("gpt-5"),
            })
        );
    }

    #[test]
    fn parser_maps_notification_widget_and_title_events() {
        let notification = parse_server_line(
            r#"{"method":"notify","params":{"level":"warn","message":"heads up"}}"#,
            "msg-8",
        );
        assert!(notification.is_ok());
        let Ok(notification) = notification else {
            return;
        };
        assert_eq!(
            notification.body,
            MessageBody::ServerEvent(ServerEvent::NotificationRaised(Notification {
                level: String::from("warn"),
                message: String::from("heads up"),
            }))
        );

        let widget = parse_server_line(
            r#"{"method":"setWidget","params":{"id":"tool-1","title":"Bash","body":"running"}}"#,
            "msg-9",
        );
        assert!(widget.is_ok());
        let Ok(widget) = widget else {
            return;
        };
        assert_eq!(
            widget.body,
            MessageBody::ServerEvent(ServerEvent::WidgetUpdated(WidgetState {
                id: String::from("tool-1"),
                title: String::from("Bash"),
                body: String::from("running"),
            }))
        );

        let title = parse_server_line(
            r#"{"method":"setTitle","params":{"title":"yach"}}"#,
            "msg-10",
        );
        assert!(title.is_ok());
        let Ok(title) = title else {
            return;
        };
        assert_eq!(
            title.body,
            MessageBody::ServerEvent(ServerEvent::TitleChanged {
                title: String::from("yach"),
            })
        );
    }

    #[test]
    fn parser_rejects_unknown_methods() {
        let error = parse_server_line(r#"{"method":"unknown_call","params":{}}"#, "msg-6");
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        assert_eq!(error, ParseError::UnsupportedMethod(String::from("unknown_call")));
    }

    #[test]
    fn parser_rejects_missing_required_fields() {
        let error = parse_server_line(r#"{"method":"prompt_delta","params":{"delta":"hello"}}"#, "msg-7");
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        assert_eq!(error, ParseError::MissingField("session_id"));
    }
}
