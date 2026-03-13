pub const PROTOCOL_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    PromptStreaming,
    Dialogs,
    Notifications,
    StatusEntries,
    Widgets,
    SessionForking,
    ThemeLoading,
    RichUi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub protocol_version: &'static str,
    pub agent_name: &'static str,
    pub capabilities: Vec<Capability>,
}

impl Handshake {
    #[must_use]
    pub fn new(agent_name: &'static str, capabilities: Vec<Capability>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            agent_name,
            capabilities,
        }
    }

    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    Initialize(Handshake),
    PromptSubmitted { session_id: String, prompt: String },
    SessionSelected { session_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Ready { handshake: Handshake },
    PromptDelta { session_id: String, delta: String },
    ToolCallStarted { tool_name: String },
    StatusUpdated { message: String },
}

#[must_use]
pub fn default_ui_handshake() -> Handshake {
    Handshake::new(
        "yach-ui",
        vec![
            Capability::PromptStreaming,
            Capability::Dialogs,
            Capability::Notifications,
            Capability::StatusEntries,
            Capability::Widgets,
            Capability::SessionForking,
            Capability::ThemeLoading,
        ],
    )
}

#[must_use]
pub fn default_rpc_handshake() -> Handshake {
    Handshake::new(
        "yach-adapter-pi-rpc",
        vec![
            Capability::PromptStreaming,
            Capability::Dialogs,
            Capability::Notifications,
            Capability::StatusEntries,
            Capability::Widgets,
            Capability::SessionForking,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::{
        Capability, ClientEvent, Handshake, PROTOCOL_VERSION, ServerEvent, default_rpc_handshake,
        default_ui_handshake,
    };

    #[test]
    fn protocol_version_tracks_prd_seed() {
        assert_eq!(PROTOCOL_VERSION, "0.1.0");
    }

    #[test]
    fn ui_handshake_exposes_phase_one_capabilities() {
        let handshake = default_ui_handshake();

        assert!(handshake.supports(Capability::PromptStreaming));
        assert!(handshake.supports(Capability::ThemeLoading));
        assert!(!handshake.supports(Capability::RichUi));
    }

    #[test]
    fn rpc_handshake_does_not_claim_theme_loading() {
        let handshake = default_rpc_handshake();

        assert!(!handshake.supports(Capability::ThemeLoading));
        assert!(handshake.supports(Capability::Widgets));
    }

    #[test]
    fn events_are_equatable_for_record_replay_tests() {
        let client_event = ClientEvent::SessionSelected {
            session_id: String::from("session-1"),
        };
        let server_event = ServerEvent::StatusUpdated {
            message: String::from("ready"),
        };

        assert_eq!(
            client_event,
            ClientEvent::SessionSelected {
                session_id: String::from("session-1"),
            }
        );
        assert_eq!(
            server_event,
            ServerEvent::StatusUpdated {
                message: String::from("ready"),
            }
        );
    }

    #[test]
    fn handshakes_capture_agent_identity() {
        let handshake = Handshake::new("test-agent", vec![Capability::Dialogs]);

        assert_eq!(handshake.protocol_version, PROTOCOL_VERSION);
        assert_eq!(handshake.agent_name, "test-agent");
    }
}
