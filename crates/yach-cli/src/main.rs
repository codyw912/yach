use yach_adapter_pi_rpc::{
    AdapterCapabilities, negotiate_with as negotiate_with_rpc, parse_server_line,
    serialize_client_message, stock_rpc_handshake,
};
use yach_proto::{Capability, ClientEvent, MessageMeta, TransportMessage};
use yach_ui::{UiCapabilities, alpha_handshake, negotiate_with as negotiate_with_ui};

fn main() {
    let ui_capabilities = UiCapabilities::alpha();
    let adapter_capabilities = AdapterCapabilities::stock_rpc();
    let ui_handshake = alpha_handshake();
    let adapter_handshake = stock_rpc_handshake();
    let ui_negotiation = negotiate_with_ui(&adapter_handshake);
    let adapter_negotiation = negotiate_with_rpc(&ui_handshake);
    let bootstrap_message = TransportMessage::client(
        MessageMeta::new("bootstrap-1").with_correlation_id("session-bootstrap"),
        ClientEvent::Initialize(ui_handshake.clone()),
    );
    let bootstrap_line = serialize_client_message(&bootstrap_message);
    let parsed_ready = parse_server_line(r#"{"method":"ready","params":{}}"#, "server-1");

    let _bootstrap_ready = ui_capabilities.supports(Capability::PromptStreaming)
        && adapter_capabilities.supports(Capability::PromptStreaming)
        && ui_handshake.supports(Capability::Dialogs)
        && adapter_handshake.supports(Capability::Dialogs)
        && ui_negotiation.supports(Capability::PromptStreaming)
        && adapter_negotiation.supports(Capability::Dialogs)
        && bootstrap_line.is_ok()
        && parsed_ready.is_ok();
}
