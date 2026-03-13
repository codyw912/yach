use yach_adapter_pi_rpc::{AdapterCapabilities, stock_rpc_handshake};
use yach_proto::Capability;
use yach_ui::{UiCapabilities, alpha_handshake};

fn main() {
    let ui_capabilities = UiCapabilities::alpha();
    let adapter_capabilities = AdapterCapabilities::stock_rpc();
    let ui_handshake = alpha_handshake();
    let adapter_handshake = stock_rpc_handshake();

    let _bootstrap_ready = ui_capabilities.supports(Capability::PromptStreaming)
        && adapter_capabilities.supports(Capability::PromptStreaming)
        && ui_handshake.supports(Capability::Dialogs)
        && adapter_handshake.supports(Capability::Dialogs);
}
