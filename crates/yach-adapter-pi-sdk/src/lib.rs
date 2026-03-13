#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RichUiCapabilities {
    pub custom_overlays: bool,
    pub header_footer_replacement: bool,
    pub editor_replacement: bool,
}

impl RichUiCapabilities {
    #[must_use]
    pub const fn sidecar() -> Self {
        Self {
            custom_overlays: true,
            header_footer_replacement: true,
            editor_replacement: true,
        }
    }
}

#[must_use]
pub const fn supports_rich_ui() -> bool {
    let capabilities = RichUiCapabilities::sidecar();

    capabilities.custom_overlays
        && capabilities.header_footer_replacement
        && capabilities.editor_replacement
}

#[cfg(test)]
mod tests {
    use super::{RichUiCapabilities, supports_rich_ui};

    #[test]
    fn sidecar_targets_rich_parity() {
        let capabilities = RichUiCapabilities::sidecar();

        assert!(capabilities.custom_overlays);
        assert!(capabilities.header_footer_replacement);
        assert!(capabilities.editor_replacement);
    }

    #[test]
    fn sidecar_reports_rich_ui_support() {
        assert!(supports_rich_ui());
    }
}
