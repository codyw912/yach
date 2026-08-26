mod app;
mod approval_selector;
mod fork_picker;
mod help_overlay;
mod input;
mod layout;
mod lifecycle;
mod model_selector;
mod perf_metrics;
mod perf_overlay;
mod session_picker;
mod session_tree;
mod slash_commands;
mod slash_popup;
mod status_bar;
mod theme;
mod thinking_level;
mod thinking_selector;
mod transcript;

pub use app::{
    BenchmarkApp, RunTuiOptions, StartupTrace, run_tui, run_tui_with_startup_trace,
    run_tui_with_startup_trace_and_options,
};
pub use theme::{Theme, ThemeColors, ThemeLoadError, ThemeSpacing};
pub use transcript::Transcript;

use yach_proto::{Capability, Handshake, NegotiatedCapabilities, default_ui_handshake};

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
            | Capability::PromptCancellation
            | Capability::SessionForking
            | Capability::LocalEdit
            | Capability::ExtensionLifecycle
            | Capability::FirstRenderEvents
            | Capability::ProviderConnections
            | Capability::ToolOutputStreaming
            | Capability::StructuredReviewRows
            | Capability::ApprovalModes
            | Capability::ModelState => true,
            Capability::RichUi => false,
        }
    }
}

#[must_use]
pub fn alpha_handshake() -> Handshake {
    default_ui_handshake()
}

#[must_use]
pub fn negotiate_with(adapter: &Handshake) -> NegotiatedCapabilities {
    NegotiatedCapabilities::from_handshakes(&alpha_handshake(), adapter)
}

#[cfg(test)]
mod tests {
    use super::{UiCapabilities, alpha_handshake, negotiate_with};
    use yach_proto::{Capability, default_backend_handshake};

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
        assert!(capabilities.supports(Capability::LocalEdit));
        assert!(handshake.supports(Capability::LocalEdit));
        assert!(capabilities.supports(Capability::ExtensionLifecycle));
        assert!(handshake.supports(Capability::ExtensionLifecycle));
        assert!(capabilities.supports(Capability::ProviderConnections));
        assert!(handshake.supports(Capability::ProviderConnections));
        assert_eq!(
            capabilities.supports(Capability::RichUi),
            handshake.supports(Capability::RichUi)
        );
    }

    #[test]
    fn negotiation_filters_unsupported_capabilities() {
        let negotiation = negotiate_with(&default_backend_handshake());

        assert!(negotiation.supports(Capability::PromptStreaming));
        assert!(!negotiation.supports(Capability::ThemeLoading));
    }
}
