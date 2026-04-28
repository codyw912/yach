//! Backend runner groundwork for yach.
//!
//! This crate owns backend-facing concepts that are not specific to the
//! temporary Pi RPC adapter or to the eventual native provider implementation.
//! The first slice intentionally stays small: runner extraction, session
//! persistence, and provider adapters will exercise these boundaries before
//! they split into larger APIs.

use tokio::sync::mpsc;
use yach_proto::{BackendEvent, ClientEvent, NegotiatedCapabilities};

/// UI-facing channels exposed by any backend runner.
#[derive(Debug)]
pub struct BackendChannels {
    /// Sender cloned into the TUI so user actions can reach the backend.
    pub client_tx: mpsc::UnboundedSender<ClientEvent>,
    /// Receiver consumed by the TUI for backend/server events.
    pub backend_rx: mpsc::UnboundedReceiver<BackendEvent>,
}

/// Backend-side channel endpoints used by runner implementations.
#[derive(Debug)]
pub struct BackendEndpoints {
    /// Receives client events submitted by the TUI.
    pub client_rx: mpsc::UnboundedReceiver<ClientEvent>,
    /// Sends backend events consumed by the TUI.
    pub backend_tx: mpsc::UnboundedSender<BackendEvent>,
}

/// Create the standard channel pair used between the TUI and a backend runner.
#[must_use]
pub fn backend_channels() -> (BackendChannels, BackendEndpoints) {
    let (client_tx, client_rx) = mpsc::unbounded_channel();
    let (backend_tx, backend_rx) = mpsc::unbounded_channel();

    (
        BackendChannels {
            client_tx,
            backend_rx,
        },
        BackendEndpoints {
            client_rx,
            backend_tx,
        },
    )
}

/// Send the initial connected event for a started runner.
#[must_use]
pub fn announce_connected(
    backend_tx: &mpsc::UnboundedSender<BackendEvent>,
    negotiated: NegotiatedCapabilities,
) -> bool {
    backend_tx
        .send(BackendEvent::Connected { negotiated })
        .is_ok()
}

/// Stable backend families that a future runner selector can expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// The current stock Pi RPC compatibility adapter.
    PiRpc,
    /// The planned yach-owned native backend runtime.
    Native,
}

/// Coarse capability flags for a backend runner.
///
/// These describe behavior that the CLI/TUI may need to surface before a full
/// runner handle exists. They are deliberately backend-owned and provider-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Backend can accept prompt submissions and stream assistant text.
    pub prompt_streaming: bool,
    /// Backend owns an inspectable local session persistence path.
    pub file_first_sessions: bool,
    /// Backend can expose native tool execution through yach-owned policy.
    pub tool_execution: bool,
}

impl BackendCapabilities {
    /// Capabilities expected from the current Pi RPC compatibility path.
    #[must_use]
    pub const fn pi_rpc_compatibility() -> Self {
        Self {
            prompt_streaming: true,
            file_first_sessions: false,
            tool_execution: false,
        }
    }

    /// Capabilities for the first native dogfood runner before tools/resources land.
    #[must_use]
    pub const fn native_dogfood() -> Self {
        Self {
            prompt_streaming: true,
            file_first_sessions: true,
            tool_execution: false,
        }
    }
}

/// Human-readable runner metadata for status and selection surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendMetadata {
    /// Stable backend family.
    pub kind: BackendKind,
    /// Short display label suitable for status/help text.
    pub label: &'static str,
    /// Current coarse capabilities for this backend.
    pub capabilities: BackendCapabilities,
}

impl BackendMetadata {
    /// Metadata for the default Pi-backed runner.
    #[must_use]
    pub const fn pi_rpc() -> Self {
        Self {
            kind: BackendKind::PiRpc,
            label: "pi rpc",
            capabilities: BackendCapabilities::pi_rpc_compatibility(),
        }
    }

    /// Metadata for the constrained native dogfood runner.
    #[must_use]
    pub const fn native_dogfood() -> Self {
        Self {
            kind: BackendKind::Native,
            label: "native dogfood",
            capabilities: BackendCapabilities::native_dogfood(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackendCapabilities, BackendKind, BackendMetadata, announce_connected, backend_channels,
    };
    use yach_proto::{BackendEvent, Capability, ClientEvent, Handshake, NegotiatedCapabilities};

    #[test]
    fn pi_rpc_metadata_identifies_compatibility_runner() {
        let metadata = BackendMetadata::pi_rpc();

        assert_eq!(metadata.kind, BackendKind::PiRpc);
        assert_eq!(metadata.label, "pi rpc");
        assert_eq!(
            metadata.capabilities,
            BackendCapabilities::pi_rpc_compatibility()
        );
        assert!(metadata.capabilities.prompt_streaming);
        assert!(!metadata.capabilities.file_first_sessions);
        assert!(!metadata.capabilities.tool_execution);
    }

    #[test]
    fn native_dogfood_metadata_identifies_file_first_runner() {
        let metadata = BackendMetadata::native_dogfood();

        assert_eq!(metadata.kind, BackendKind::Native);
        assert_eq!(metadata.label, "native dogfood");
        assert_eq!(metadata.capabilities, BackendCapabilities::native_dogfood());
        assert!(metadata.capabilities.prompt_streaming);
        assert!(metadata.capabilities.file_first_sessions);
        assert!(!metadata.capabilities.tool_execution);
    }

    #[test]
    fn metadata_has_debug_and_equality_behavior() {
        let left = BackendMetadata::native_dogfood();
        let right = BackendMetadata::native_dogfood();

        assert_eq!(left, right);
        assert_eq!(format!("{left:?}"), format!("{right:?}"));
    }

    #[test]
    fn backend_channels_connect_ui_sender_to_runner_receiver() {
        let (channels, mut endpoints) = backend_channels();

        assert!(
            channels
                .client_tx
                .send(ClientEvent::RecentSessionsRequested)
                .is_ok()
        );

        assert_eq!(
            endpoints.client_rx.blocking_recv(),
            Some(ClientEvent::RecentSessionsRequested)
        );
    }

    #[test]
    fn connected_announcement_reaches_ui_receiver() {
        let (mut channels, endpoints) = backend_channels();
        let ui = Handshake::new("ui", vec![Capability::PromptStreaming]);
        let backend = Handshake::new("backend", vec![Capability::PromptStreaming]);
        let negotiated = NegotiatedCapabilities::from_handshakes(&ui, &backend);

        assert!(announce_connected(
            &endpoints.backend_tx,
            negotiated.clone()
        ));

        assert_eq!(
            channels.backend_rx.blocking_recv(),
            Some(BackendEvent::Connected { negotiated })
        );
    }
}
