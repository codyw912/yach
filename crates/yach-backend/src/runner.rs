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

/// Started backend session state shared by CLI launchers.
#[derive(Debug)]
pub struct BackendSession {
    /// User-visible backend metadata.
    pub metadata: BackendMetadata,
    /// UI-facing channels consumed by the TUI.
    pub channels: BackendChannels,
    /// Runner-facing endpoints consumed by backend implementations.
    pub endpoints: BackendEndpoints,
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

/// Start a backend session by creating channels and announcing connection.
#[must_use]
pub fn start_backend_session(
    metadata: BackendMetadata,
    negotiated: NegotiatedCapabilities,
) -> BackendSession {
    let (channels, endpoints) = backend_channels();
    let _connected = announce_connected(&endpoints.backend_tx, negotiated);

    BackendSession {
        metadata,
        channels,
        endpoints,
    }
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
