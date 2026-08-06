#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLifecycle {
    Started,
    Ended,
    Internal,
}
fn is_internal_extension_lifecycle_status(message: &str) -> bool {
    matches!(
        message,
        "extension_manifest_scan_scheduled"
            | "extension_manifest_scan_started"
            | "extension_background_activation_scheduled"
            | "extension_background_activation_started"
    ) || message.starts_with("extension_manifest_scan_finished")
        || message.starts_with("extension_background_activation_finished")
}

#[must_use]
pub fn status_lifecycle(message: &str) -> Option<StatusLifecycle> {
    if message.starts_with("agent_start") || message.starts_with("turn_start") {
        Some(StatusLifecycle::Started)
    } else if message.starts_with("agent_end") || message.starts_with("turn_end") {
        Some(StatusLifecycle::Ended)
    } else if matches!(message, "agent_started" | "message_start" | "message_end")
        || is_internal_extension_lifecycle_status(message)
    {
        Some(StatusLifecycle::Internal)
    } else {
        None
    }
}

#[must_use]
pub fn is_lifecycle_status(message: &str) -> bool {
    status_lifecycle(message).is_some()
}

#[cfg(test)]
mod tests {
    use super::{StatusLifecycle, is_lifecycle_status, status_lifecycle};

    #[test]
    fn classifies_exact_and_prefixed_lifecycle_status() {
        assert_eq!(
            status_lifecycle("turn_start"),
            Some(StatusLifecycle::Started)
        );
        // Suffixed variants must classify too (`turn_end failed`).
        assert_eq!(
            status_lifecycle("turn_end failed"),
            Some(StatusLifecycle::Ended)
        );
        assert_eq!(status_lifecycle("agent_end"), Some(StatusLifecycle::Ended));
        assert_eq!(
            status_lifecycle("message_start"),
            Some(StatusLifecycle::Internal)
        );
        assert!(!is_lifecycle_status("backend: native"));
    }
    #[test]
    fn extension_background_activation_is_internal_lifecycle_status() {
        assert_eq!(
            status_lifecycle("extension_background_activation_started"),
            Some(StatusLifecycle::Internal)
        );
        assert_eq!(
            status_lifecycle(
                "extension_background_activation_finished active_extension_count=0 registered_tool_count=0 host_start_count=0"
            ),
            Some(StatusLifecycle::Internal)
        );
        assert_eq!(
            status_lifecycle("extension_background_activation_failed reason=join_failed"),
            None
        );
    }

    #[test]
    fn extension_manifest_scan_is_internal_lifecycle_status() {
        for message in [
            "extension_manifest_scan_scheduled",
            "extension_manifest_scan_started",
            "extension_manifest_scan_finished extension_count=0 host_start_count=0",
        ] {
            assert_eq!(
                status_lifecycle(message),
                Some(StatusLifecycle::Internal),
                "{message}"
            );
        }
        assert_eq!(
            status_lifecycle("extension_manifest_scan_failed reason=join_failed"),
            None
        );
    }
}
