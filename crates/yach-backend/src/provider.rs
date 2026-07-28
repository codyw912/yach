use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::{Role, TurnId};

/// Provider/model target for a native LLM request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
}

/// Single message sent to a provider adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMessage {
    pub role: Role,
    pub content: String,
}

/// Adapter-owned provider-specific options.
///
/// The common backend seam treats these as validated metadata supplied by the
/// adapter layer, not as core semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderExtension {
    pub key: String,
    pub value: serde_json::Value,
}

/// Provider request owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub turn_id: TurnId,
    pub model: ProviderModel,
    pub messages: Vec<ProviderMessage>,
    pub extensions: Vec<ProviderExtension>,
}

/// Normalized provider error categories surfaced above adapter crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    Authentication,
    RateLimited,
    InvalidRequest,
    ContextLength,
    UnavailableModel,
    Timeout,
    Network,
    ProviderInternal,
    SafetyRefusal,
    MalformedStream,
    Backpressure,
    Cancelled,
    Unknown,
}

/// Redacted provider error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub redacted_debug: Option<String>,
}

impl ProviderError {
    #[must_use]
    pub fn fixture_failure() -> Self {
        Self {
            kind: ProviderErrorKind::ProviderInternal,
            message: String::from("fixture provider failure"),
            redacted_debug: Some(String::from("fixture=failure")),
        }
    }

    #[must_use]
    pub fn malformed_stream(message: impl Into<String>) -> Self {
        Self {
            kind: ProviderErrorKind::MalformedStream,
            message: message.into(),
            redacted_debug: Some(String::from("fixture=malformed_stream")),
        }
    }

    #[must_use]
    pub fn backpressure() -> Self {
        Self {
            kind: ProviderErrorKind::Backpressure,
            message: String::from("Native backend fell behind this stream."),
            redacted_debug: Some(String::from("bounded provider stream buffer full")),
        }
    }

    #[must_use]
    pub fn cancelled(reason: impl Into<String>) -> Self {
        Self {
            kind: ProviderErrorKind::Cancelled,
            message: reason.into(),
            redacted_debug: None,
        }
    }
}

/// Streaming tool-call state emitted by provider adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderToolCall {
    /// Provider call id used to pair tool results with requests.
    pub call_id: String,
    /// Tool/function name requested by the model.
    pub name: String,
    /// Raw JSON argument payload emitted by the provider.
    pub arguments_json: serde_json::Value,
}

/// Token usage reported by a provider stream when available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// Provider finish reason normalized enough for native runner accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFinishReason {
    Stop,
    Length,
    ToolCalls,
    Safety,
    ContentFilter,
    Unknown,
}

/// Streaming events produced by provider adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderStreamEvent {
    Started {
        turn_id: TurnId,
        model: ProviderModel,
    },
    TextDelta {
        turn_id: TurnId,
        delta: String,
    },
    ToolCallStarted {
        turn_id: TurnId,
        call_id: String,
        name: String,
    },
    ToolCallDelta {
        turn_id: TurnId,
        call_id: String,
        arguments_delta: String,
    },
    ToolCallCompleted {
        turn_id: TurnId,
        tool_call: ProviderToolCall,
    },
    Completed {
        turn_id: TurnId,
        finish_reason: Option<ProviderFinishReason>,
        usage: Option<ProviderUsage>,
        provider_response_id: Option<String>,
    },
    Failed {
        turn_id: TurnId,
        error: ProviderError,
    },
    Cancelled {
        turn_id: TurnId,
        reason: Option<String>,
    },
}

impl ProviderStreamEvent {
    #[must_use]
    pub const fn turn_id(&self) -> &TurnId {
        match self {
            Self::Started { turn_id, .. }
            | Self::TextDelta { turn_id, .. }
            | Self::ToolCallStarted { turn_id, .. }
            | Self::ToolCallDelta { turn_id, .. }
            | Self::ToolCallCompleted { turn_id, .. }
            | Self::Completed { turn_id, .. }
            | Self::Failed { turn_id, .. }
            | Self::Cancelled { turn_id, .. } => turn_id,
        }
    }

    #[must_use]
    pub const fn is_lifecycle_boundary(&self) -> bool {
        matches!(
            self,
            Self::Started { .. }
                | Self::ToolCallStarted { .. }
                | Self::ToolCallCompleted { .. }
                | Self::Completed { .. }
                | Self::Failed { .. }
                | Self::Cancelled { .. }
        )
    }
}

/// Bounded fixture buffer used to make native provider-stream backpressure explicit.
#[derive(Debug, Clone)]
pub struct BoundedProviderStreamBuffer {
    capacity: usize,
    events: VecDeque<ProviderStreamEvent>,
}

impl BoundedProviderStreamBuffer {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            events: VecDeque::with_capacity(capacity),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn push(&mut self, event: ProviderStreamEvent) -> Result<(), ProviderStreamEvent> {
        if self.capacity == 0 {
            return Err(Self::backpressure_failure(event.turn_id().clone()));
        }
        if self.events.len() < self.capacity {
            self.events.push_back(event);
            return Ok(());
        }
        if self.coalesce_text_delta(&event) {
            return Ok(());
        }
        if event.is_lifecycle_boundary() && self.drop_oldest_text_delta() {
            self.events.push_back(event);
            return Ok(());
        }
        Err(Self::backpressure_failure(event.turn_id().clone()))
    }

    pub fn pop_front(&mut self) -> Option<ProviderStreamEvent> {
        self.events.pop_front()
    }

    fn coalesce_text_delta(&mut self, event: &ProviderStreamEvent) -> bool {
        let ProviderStreamEvent::TextDelta { turn_id, delta } = event else {
            return false;
        };
        let Some(ProviderStreamEvent::TextDelta {
            turn_id: existing_turn_id,
            delta: existing_delta,
        }) = self.events.back_mut()
        else {
            return false;
        };
        if existing_turn_id != turn_id {
            return false;
        }
        existing_delta.push_str(delta);
        true
    }

    fn drop_oldest_text_delta(&mut self) -> bool {
        let Some(index) = self
            .events
            .iter()
            .position(|event| matches!(event, ProviderStreamEvent::TextDelta { .. }))
        else {
            return false;
        };
        self.events.remove(index).is_some()
    }

    fn backpressure_failure(turn_id: TurnId) -> ProviderStreamEvent {
        ProviderStreamEvent::Failed {
            turn_id,
            error: ProviderError::backpressure(),
        }
    }
}
