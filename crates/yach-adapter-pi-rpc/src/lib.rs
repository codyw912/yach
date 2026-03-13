use yach_proto::{Capability, Handshake, default_rpc_handshake};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCapabilities {
    pub prompt_streaming: bool,
    pub dialogs: bool,
    pub widgets: bool,
}

impl AdapterCapabilities {
    #[must_use]
    pub const fn stock_rpc() -> Self {
        Self {
            prompt_streaming: true,
            dialogs: true,
            widgets: true,
        }
    }

    #[must_use]
    pub const fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::PromptStreaming => self.prompt_streaming,
            Capability::Dialogs => self.dialogs,
            Capability::Widgets => self.widgets,
            Capability::Notifications | Capability::StatusEntries | Capability::SessionForking => true,
            Capability::ThemeLoading | Capability::RichUi => false,
        }
    }
}

#[must_use]
pub fn stock_rpc_handshake() -> Handshake {
    default_rpc_handshake()
}

#[cfg(test)]
mod tests {
    use super::{AdapterCapabilities, stock_rpc_handshake};
    use yach_proto::Capability;

    #[test]
    fn stock_rpc_supports_phase_one_basics() {
        let capabilities = AdapterCapabilities::stock_rpc();

        assert!(capabilities.prompt_streaming);
        assert!(capabilities.dialogs);
        assert!(capabilities.widgets);
    }

    #[test]
    fn stock_rpc_matches_proto_handshake() {
        let capabilities = AdapterCapabilities::stock_rpc();
        let handshake = stock_rpc_handshake();

        assert!(capabilities.supports(Capability::PromptStreaming));
        assert!(handshake.supports(Capability::PromptStreaming));
        assert_eq!(capabilities.supports(Capability::ThemeLoading), handshake.supports(Capability::ThemeLoading));
    }
}
