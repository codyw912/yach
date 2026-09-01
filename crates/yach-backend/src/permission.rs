use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use yach_proto::ApprovalMode;

static PERMISSION_DECISION_COUNTER: AtomicU64 = AtomicU64::new(0);

const APPROVAL_SETTINGS_SCHEMA: &str = "yach.approval-settings.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredApprovalSettings {
    schema: String,
    mode: ApprovalMode,
}

#[must_use]
pub fn load_project_approval_mode(project_root: &Path) -> ApprovalMode {
    stored_project_approval_mode(project_root).unwrap_or(ApprovalMode::Review)
}

#[must_use]
pub fn stored_project_approval_mode(project_root: &Path) -> Option<ApprovalMode> {
    approval_settings_path(project_root)
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<StoredApprovalSettings>(&raw).ok())
        .filter(|settings| settings.schema == APPROVAL_SETTINGS_SCHEMA)
        .map(|settings| settings.mode)
        .filter(|mode| *mode != ApprovalMode::FullAccess)
}

#[must_use]
pub fn project_approval_mode_warning(project_root: &Path) -> Option<String> {
    let path = approval_settings_path(project_root)?;
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(_) => {
            return Some(String::from(
                "approval_mode_config: could not read user state; using review",
            ));
        }
    };
    let Ok(settings) = serde_json::from_str::<StoredApprovalSettings>(&raw) else {
        return Some(String::from(
            "approval_mode_config: invalid user state; using review",
        ));
    };
    if settings.schema != APPROVAL_SETTINGS_SCHEMA {
        return Some(String::from(
            "approval_mode_config: unsupported user-state schema; using review",
        ));
    }
    (settings.mode == ApprovalMode::FullAccess)
        .then(|| String::from("approval_mode_config: stored full-access ignored; using review"))
}

pub fn persist_project_approval_mode(project_root: &Path, mode: ApprovalMode) -> io::Result<()> {
    if mode == ApprovalMode::FullAccess {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "full-access approval mode is session-only",
        ));
    }
    let path = approval_settings_path(project_root).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME and USERPROFILE are unset; cannot persist approval mode",
        )
    })?;
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("approval settings path has no parent"));
    };
    create_private_dir(parent)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    serde_json::to_writer(
        &mut file,
        &StoredApprovalSettings {
            schema: String::from(APPROVAL_SETTINGS_SCHEMA),
            mode,
        },
    )
    .map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temp, &path)
}

fn approval_settings_path(project_root: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let canonical = project_root.canonicalize().ok()?;
    Some(
        PathBuf::from(home)
            .join(".yach")
            .join("permissions")
            .join(format!(
                "{}.json",
                crate::runner::project_state_key(&canonical)
            )),
    )
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDecisionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionCapability {
    EditTransaction,
    ShellCommand,
    NetworkAccess,
    VerificationAction,
    ExtensionTool,
    ProviderVisibleTool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionActor {
    UserLocalUi,
    Core,
    Provider,
    Extension { extension_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Allow,
    Ask,
    Deny,
    AutoReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionReviewer {
    None,
    User,
    AutoReview,
    Extension { extension_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRisk {
    ReadOnly,
    WorkspaceWrite,
    ExternalWrite,
    Network,
    ProcessExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionTargetSummary {
    pub operation: String,
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub actor: PermissionActor,
    pub capability: PermissionCapability,
    pub target: PermissionTargetSummary,
    pub risk: PermissionRisk,
    pub requested_reviewer: Option<PermissionReviewer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPolicy {
    pub edit_mode: PermissionMode,
}

impl PermissionPolicy {
    #[must_use]
    pub const fn for_edit_mode(edit_mode: PermissionMode) -> Self {
        Self { edit_mode }
    }

    #[must_use]
    pub const fn default_local_edit() -> Self {
        Self {
            edit_mode: PermissionMode::Ask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PermissionDecision {
    Allowed {
        decision_id: PermissionDecisionId,
        reviewer: PermissionReviewer,
        mode: PermissionMode,
        reason: String,
        rationale: Option<String>,
    },
    Denied {
        decision_id: PermissionDecisionId,
        reviewer: PermissionReviewer,
        mode: PermissionMode,
        reason: String,
        rationale: Option<String>,
    },
    NeedsUserReview {
        decision_id: PermissionDecisionId,
        reviewer: PermissionReviewer,
        mode: PermissionMode,
        reason: String,
        prompt: PermissionPrompt,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPrompt {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDecisionSummary {
    pub request_id: String,
    pub decision_id: PermissionDecisionId,
    pub actor: PermissionActor,
    pub capability: PermissionCapability,
    pub target: PermissionTargetSummary,
    pub risk: PermissionRisk,
    pub configured_mode: PermissionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
    pub reviewer: PermissionReviewer,
    pub outcome: PermissionDecisionOutcome,
    pub reason: String,
    pub rationale: Option<String>,
    pub user_override: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionOutcome {
    Allowed,
    Denied,
    NeedsUserReview,
}

impl PermissionDecision {
    #[must_use]
    pub fn decision_id(&self) -> PermissionDecisionId {
        match self {
            Self::Allowed { decision_id, .. }
            | Self::Denied { decision_id, .. }
            | Self::NeedsUserReview { decision_id, .. } => decision_id.clone(),
        }
    }

    #[must_use]
    pub fn summary(
        &self,
        request: &PermissionRequest,
        user_override: bool,
    ) -> PermissionDecisionSummary {
        match self {
            Self::Allowed {
                decision_id,
                reviewer,
                mode,
                reason,
                rationale,
            } => PermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: sanitized_target_summary(&request.target),
                risk: request.risk,
                configured_mode: *mode,
                approval_mode: None,
                reviewer: reviewer.clone(),
                outcome: PermissionDecisionOutcome::Allowed,
                reason: reason.clone(),
                rationale: sanitized_rationale(rationale.as_deref()),
                user_override,
            },
            Self::Denied {
                decision_id,
                reviewer,
                mode,
                reason,
                rationale,
            } => PermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: sanitized_target_summary(&request.target),
                risk: request.risk,
                configured_mode: *mode,
                approval_mode: None,
                reviewer: reviewer.clone(),
                outcome: PermissionDecisionOutcome::Denied,
                reason: reason.clone(),
                rationale: sanitized_rationale(rationale.as_deref()),
                user_override,
            },
            Self::NeedsUserReview {
                decision_id,
                reviewer,
                mode,
                reason,
                ..
            } => PermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: sanitized_target_summary(&request.target),
                risk: request.risk,
                configured_mode: *mode,
                approval_mode: None,
                reviewer: reviewer.clone(),
                outcome: PermissionDecisionOutcome::NeedsUserReview,
                reason: reason.clone(),
                rationale: None,
                user_override,
            },
        }
    }
}

pub struct PermissionDecisionEngine;

impl PermissionDecisionEngine {
    #[must_use]
    pub fn decide(request: &PermissionRequest, policy: &PermissionPolicy) -> PermissionDecision {
        if extension_self_approval_requested(request) {
            return PermissionDecision::Denied {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode: policy.edit_mode,
                reason: String::from("extension_self_approval_denied"),
                rationale: None,
            };
        }

        let mode = match request.capability {
            PermissionCapability::EditTransaction
                if request.risk == PermissionRisk::WorkspaceWrite =>
            {
                policy.edit_mode
            }
            PermissionCapability::EditTransaction => {
                return PermissionDecision::Denied {
                    decision_id: next_permission_decision_id(),
                    reviewer: PermissionReviewer::None,
                    mode: policy.edit_mode,
                    reason: String::from("permission_risk_denied"),
                    rationale: None,
                };
            }
            PermissionCapability::ShellCommand
            | PermissionCapability::NetworkAccess
            | PermissionCapability::VerificationAction
            | PermissionCapability::ExtensionTool
            | PermissionCapability::ProviderVisibleTool => PermissionMode::Deny,
        };

        match mode {
            PermissionMode::Allow => PermissionDecision::Allowed {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode,
                reason: String::from("permission_mode_allowed"),
                rationale: None,
            },
            PermissionMode::Ask => PermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::User,
                mode,
                reason: String::from("permission_mode_ask"),
                prompt: permission_prompt(request),
            },
            PermissionMode::Deny => PermissionDecision::Denied {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode,
                reason: String::from("permission_mode_denied"),
                rationale: None,
            },
            PermissionMode::AutoReview => PermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::User,
                mode,
                reason: String::from("auto_review_unavailable_fallback_ask"),
                prompt: permission_prompt(request),
            },
        }
    }

    /// Resolve one provider-originated host command through the approval mode
    /// and the authoritative user allowlist.
    #[must_use]
    pub fn decide_shell(
        request: &PermissionRequest,
        approval_mode: ApprovalMode,
        user_allowlisted: bool,
    ) -> PermissionDecision {
        if request.capability != PermissionCapability::ShellCommand
            || request.risk != PermissionRisk::ProcessExecution
        {
            return Self::deny_shell(request, "permission_risk_denied");
        }
        if user_allowlisted {
            return PermissionDecision::Allowed {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode: PermissionMode::Allow,
                reason: String::from("shell_user_allowlist"),
                rationale: None,
            };
        }
        match approval_mode {
            ApprovalMode::Review | ApprovalMode::AcceptEdits => {
                PermissionDecision::NeedsUserReview {
                    decision_id: next_permission_decision_id(),
                    reviewer: PermissionReviewer::User,
                    mode: PermissionMode::Ask,
                    reason: String::from("approval_mode_requires_review"),
                    prompt: permission_prompt(request),
                }
            }
            ApprovalMode::FullAccess => PermissionDecision::Allowed {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode: PermissionMode::Allow,
                reason: String::from("approval_mode_full_access"),
                rationale: None,
            },
        }
    }

    #[must_use]
    pub fn deny_shell(request: &PermissionRequest, reason: &str) -> PermissionDecision {
        debug_assert_eq!(request.capability, PermissionCapability::ShellCommand);
        PermissionDecision::Denied {
            decision_id: next_permission_decision_id(),
            reviewer: PermissionReviewer::None,
            mode: PermissionMode::Deny,
            reason: reason.to_owned(),
            rationale: None,
        }
    }
}

fn sanitized_target_summary(target: &PermissionTargetSummary) -> PermissionTargetSummary {
    PermissionTargetSummary {
        operation: sanitized_label(&target.operation),
        resource: sanitized_resource(&target.resource),
    }
}

fn sanitized_label(label: &str) -> String {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return String::from("<empty>");
    }
    if trimmed.chars().any(char::is_control) {
        return String::from("<redacted>");
    }
    truncate_chars(trimmed, 128)
}

fn sanitized_resource(resource: &str) -> String {
    let trimmed = resource.trim();
    if trimmed.is_empty() {
        return String::from("<empty_path>");
    }
    if trimmed.starts_with('{') || trimmed.chars().any(char::is_control) {
        return String::from("<redacted_resource>");
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return String::from("<absolute_path>");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return String::from("<path_traversal>");
    }
    if trimmed == ".yach" || trimmed.starts_with(".yach/") {
        return String::from("<metadata_path>");
    }
    truncate_chars(trimmed, 256)
}

fn sanitized_rationale(rationale: Option<&str>) -> Option<String> {
    rationale.map(|_| String::from("<redacted_rationale>"))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn permission_prompt(request: &PermissionRequest) -> PermissionPrompt {
    PermissionPrompt {
        title: format!("Approve {}", request.target.operation),
        body: format!(
            "{} on {}",
            request.target.operation, request.target.resource
        ),
    }
}

fn extension_self_approval_requested(request: &PermissionRequest) -> bool {
    match (&request.actor, &request.requested_reviewer) {
        (
            PermissionActor::Extension {
                extension_id: actor,
            },
            Some(PermissionReviewer::Extension {
                extension_id: reviewer,
            }),
        ) => actor == reviewer,
        _ => false,
    }
}

fn next_permission_decision_id() -> PermissionDecisionId {
    let next = PERMISSION_DECISION_COUNTER.fetch_add(1, Ordering::Relaxed);
    PermissionDecisionId(format!("permission-decision-{next}"))
}

#[cfg(test)]
mod tests {
    use super::{
        PermissionActor, PermissionCapability, PermissionDecision, PermissionDecisionEngine,
        PermissionDecisionId, PermissionMode, PermissionPolicy, PermissionRequest,
        PermissionReviewer, PermissionRisk, PermissionTargetSummary, persist_project_approval_mode,
    };
    use std::path::Path;
    use yach_proto::ApprovalMode;

    fn edit_request() -> PermissionRequest {
        PermissionRequest {
            request_id: String::from("perm-1"),
            actor: PermissionActor::UserLocalUi,
            capability: PermissionCapability::EditTransaction,
            target: PermissionTargetSummary {
                operation: String::from("modify_text_file"),
                resource: String::from("src/lib.rs"),
            },
            risk: PermissionRisk::WorkspaceWrite,
            requested_reviewer: None,
        }
    }

    fn shell_request() -> PermissionRequest {
        PermissionRequest {
            request_id: String::from("shell-1"),
            actor: PermissionActor::Provider,
            capability: PermissionCapability::ShellCommand,
            target: PermissionTargetSummary {
                operation: String::from("bash"),
                resource: String::from("."),
            },
            risk: PermissionRisk::ProcessExecution,
            requested_reviewer: None,
        }
    }

    #[test]
    fn ask_mode_routes_edit_to_user_review() {
        let decision = PermissionDecisionEngine::decide(
            &edit_request(),
            &PermissionPolicy::for_edit_mode(PermissionMode::Ask),
        );

        assert!(matches!(
            decision,
            PermissionDecision::NeedsUserReview {
                reviewer: PermissionReviewer::User,
                ..
            }
        ));
    }

    #[test]
    fn allow_mode_allows_without_reviewer() {
        let decision = PermissionDecisionEngine::decide(
            &edit_request(),
            &PermissionPolicy::for_edit_mode(PermissionMode::Allow),
        );

        assert!(matches!(
            decision,
            PermissionDecision::Allowed {
                reviewer: PermissionReviewer::None,
                ..
            }
        ));
    }

    #[test]
    fn deny_mode_denies_before_edit_preview() {
        let decision = PermissionDecisionEngine::decide(
            &edit_request(),
            &PermissionPolicy::for_edit_mode(PermissionMode::Deny),
        );

        assert!(matches!(
            decision,
            PermissionDecision::Denied {
                reason,
                reviewer: PermissionReviewer::None,
                ..
            } if reason == "permission_mode_denied"
        ));
    }

    #[test]
    fn auto_review_is_represented_and_falls_back_to_user_review() {
        let decision = PermissionDecisionEngine::decide(
            &edit_request(),
            &PermissionPolicy::for_edit_mode(PermissionMode::AutoReview),
        );

        assert!(matches!(
            decision,
            PermissionDecision::NeedsUserReview {
                reviewer: PermissionReviewer::User,
                mode: PermissionMode::AutoReview,
                reason,
                ..
            } if reason == "auto_review_unavailable_fallback_ask"
        ));
    }

    #[test]
    fn extension_cannot_self_approve() {
        let request = PermissionRequest {
            actor: PermissionActor::Extension {
                extension_id: String::from("ext-a"),
            },
            requested_reviewer: Some(PermissionReviewer::Extension {
                extension_id: String::from("ext-a"),
            }),
            ..edit_request()
        };

        let decision = PermissionDecisionEngine::decide(
            &request,
            &PermissionPolicy::for_edit_mode(PermissionMode::Allow),
        );

        assert!(matches!(
            decision,
            PermissionDecision::Denied {
                reason,
                ..
            } if reason == "extension_self_approval_denied"
        ));
    }

    #[test]
    fn edit_transaction_denies_inconsistent_risk() {
        let request = PermissionRequest {
            risk: PermissionRisk::ExternalWrite,
            ..edit_request()
        };

        let decision = PermissionDecisionEngine::decide(
            &request,
            &PermissionPolicy::for_edit_mode(PermissionMode::Allow),
        );

        assert!(matches!(
            decision,
            PermissionDecision::Denied {
                reason,
                ..
            } if reason == "permission_risk_denied"
        ));
    }

    #[test]
    fn summaries_redact_unsafe_resource_and_rationale() {
        let request = PermissionRequest {
            target: PermissionTargetSummary {
                operation: String::from("modify_text_file"),
                resource: String::from("/tmp/secret-file"),
            },
            ..edit_request()
        };
        let decision = PermissionDecision::Allowed {
            decision_id: PermissionDecisionId(String::from("permission-decision-test")),
            reviewer: PermissionReviewer::None,
            mode: PermissionMode::Allow,
            reason: String::from("permission_mode_allowed"),
            rationale: Some(String::from("raw hidden reviewer rationale")),
        };

        let summary = decision.summary(&request, false);

        assert_eq!(summary.target.resource, "<absolute_path>");
        assert_eq!(
            summary.rationale,
            Some(String::from("<redacted_rationale>"))
        );
    }

    #[test]
    fn shell_modes_preserve_allowlist_and_full_access_reasons() {
        let review =
            PermissionDecisionEngine::decide_shell(&shell_request(), ApprovalMode::Review, false);
        assert!(matches!(
            review,
            PermissionDecision::NeedsUserReview {
                mode: PermissionMode::Ask,
                reason,
                ..
            } if reason == "approval_mode_requires_review"
        ));

        let full_access = PermissionDecisionEngine::decide_shell(
            &shell_request(),
            ApprovalMode::FullAccess,
            false,
        );
        assert!(matches!(
            full_access,
            PermissionDecision::Allowed {
                mode: PermissionMode::Allow,
                reason,
                ..
            } if reason == "approval_mode_full_access"
        ));

        let allowlisted = PermissionDecisionEngine::decide_shell(
            &shell_request(),
            ApprovalMode::FullAccess,
            true,
        );
        assert!(matches!(
            allowlisted,
            PermissionDecision::Allowed {
                reason,
                ..
            } if reason == "shell_user_allowlist"
        ));
    }

    #[test]
    fn full_access_cannot_be_persisted() {
        let result = persist_project_approval_mode(Path::new("."), ApprovalMode::FullAccess);
        assert!(result.is_err());
        let Err(error) = result else {
            return;
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
