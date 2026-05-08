#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLifecycle {
    Started,
    Ended,
    Internal,
}

#[must_use]
pub fn status_lifecycle(message: &str) -> Option<StatusLifecycle> {
    if message.starts_with("agent_start") || message.starts_with("turn_start") {
        Some(StatusLifecycle::Started)
    } else if message.starts_with("agent_end") || message.starts_with("turn_end") {
        Some(StatusLifecycle::Ended)
    } else if matches!(message, "agent_started" | "message_start" | "message_end") {
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
            status_lifecycle("turn_start native dogfood"),
            Some(StatusLifecycle::Started)
        );
        assert_eq!(status_lifecycle("agent_end"), Some(StatusLifecycle::Ended));
        assert_eq!(
            status_lifecycle("message_start"),
            Some(StatusLifecycle::Internal)
        );
        assert!(!is_lifecycle_status("backend: native dogfood"));
    }
}
