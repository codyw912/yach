use yach_proto::{Capability, Handshake, default_ui_handshake};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiCapabilities {
    pub transcript_streaming: bool,
    pub tool_panes: bool,
    pub theme_loading: bool,
}

impl UiCapabilities {
    #[must_use]
    pub const fn alpha() -> Self {
        Self {
            transcript_streaming: true,
            tool_panes: true,
            theme_loading: true,
        }
    }

    #[must_use]
    pub const fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::PromptStreaming => self.transcript_streaming,
            Capability::ThemeLoading => self.theme_loading,
            Capability::Widgets => self.tool_panes,
            Capability::Dialogs
            | Capability::Notifications
            | Capability::StatusEntries
            | Capability::SessionForking => true,
            Capability::RichUi => false,
        }
    }
}

#[must_use]
pub fn alpha_handshake() -> Handshake {
    default_ui_handshake()
}

#[cfg(test)]
mod tests {
    use super::{UiCapabilities, alpha_handshake};
    use yach_proto::Capability;

    #[test]
    fn alpha_profile_enables_core_features() {
        let capabilities = UiCapabilities::alpha();

        assert!(capabilities.transcript_streaming);
        assert!(capabilities.tool_panes);
        assert!(capabilities.theme_loading);
    }

    #[test]
    fn alpha_profile_matches_proto_handshake() {
        let capabilities = UiCapabilities::alpha();
        let handshake = alpha_handshake();

        assert!(capabilities.supports(Capability::PromptStreaming));
        assert!(handshake.supports(Capability::PromptStreaming));
        assert_eq!(capabilities.supports(Capability::RichUi), handshake.supports(Capability::RichUi));
    }
}
