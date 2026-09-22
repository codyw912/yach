use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use yach_proto::ApprovalMode;

use crate::review::{
    ActionClass, PolicyRevision, RestrictionMatcher, ReviewPolicy, ReviewRestriction,
};

static PERMISSION_DECISION_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const RESTRICTION_ASK_FIRST_REASON: &str = "restriction_ask_first";
pub const RESTRICTION_HUMAN_PERFORMS_REASON: &str = "restriction_human_performs";
pub const REVIEWER_HOLD_RISK_REASON: &str = "reviewer_hold_risk";
pub const REVIEWER_HOLD_EVIDENCE_REASON: &str = "reviewer_hold_evidence";
pub const REVIEWER_ERROR_REASON: &str = "reviewer_error";
pub const REVIEWER_UNAVAILABLE_REASON: &str = "reviewer_unavailable";

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
        .filter(|mode| *mode != ApprovalMode::FullAccess && *mode != ApprovalMode::AutoReview)
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
    (settings.mode == ApprovalMode::FullAccess || settings.mode == ApprovalMode::AutoReview)
        .then(|| String::from("approval_mode_config: stored dangerous mode ignored; using review"))
}

pub fn persist_project_approval_mode(project_root: &Path, mode: ApprovalMode) -> io::Result<()> {
    if mode == ApprovalMode::FullAccess || mode == ApprovalMode::AutoReview {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "full-access and auto-review approval modes are session-only",
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
    /// Argv-normalized command when the request has a shell surface.
    /// Restriction matchers read this; absent means match `target.operation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
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
        #[serde(default)]
        policy_revision: PolicyRevision,
    },
    Denied {
        decision_id: PermissionDecisionId,
        reviewer: PermissionReviewer,
        mode: PermissionMode,
        reason: String,
        rationale: Option<String>,
        #[serde(default)]
        policy_revision: PolicyRevision,
    },
    NeedsUserReview {
        decision_id: PermissionDecisionId,
        reviewer: PermissionReviewer,
        mode: PermissionMode,
        reason: String,
        prompt: PermissionPrompt,
        #[serde(default)]
        policy_revision: PolicyRevision,
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
    /// Restriction-set revision copied into evidence. Absent in logs written
    /// before intent-aware review, which replay as revision 0.
    #[serde(default)]
    pub policy_revision: PolicyRevision,
    /// Authorization generation copied into evidence. Absent old logs replay as 0.
    #[serde(default)]
    pub authorization_revision: u64,
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
                policy_revision,
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
                policy_revision: *policy_revision,
                authorization_revision: 0,
            },
            Self::Denied {
                decision_id,
                reviewer,
                mode,
                reason,
                rationale,
                policy_revision,
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
                policy_revision: *policy_revision,
                authorization_revision: 0,
            },
            Self::NeedsUserReview {
                decision_id,
                reviewer,
                mode,
                reason,
                policy_revision,
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
                policy_revision: *policy_revision,
                authorization_revision: 0,
            },
        }
    }
}

pub struct PermissionDecisionEngine;

impl PermissionDecisionEngine {
    #[must_use]
    pub fn decide(
        request: &PermissionRequest,
        policy: &PermissionPolicy,
        review_policy: &ReviewPolicy,
    ) -> PermissionDecision {
        let restriction_decision = Self::check_restrictions(request, review_policy);
        if let Some(decision) = restriction_decision {
            return decision;
        }
        if extension_self_approval_requested(request) {
            return PermissionDecision::Denied {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode: policy.edit_mode,
                reason: String::from("extension_self_approval_denied"),
                rationale: None,
                policy_revision: review_policy.revision,
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
                    policy_revision: review_policy.revision,
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
                policy_revision: review_policy.revision,
            },
            PermissionMode::Ask => PermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::User,
                mode,
                reason: String::from("permission_mode_ask"),
                prompt: permission_prompt(request),
                policy_revision: review_policy.revision,
            },
            PermissionMode::Deny => PermissionDecision::Denied {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode,
                reason: String::from("permission_mode_denied"),
                rationale: None,
                policy_revision: review_policy.revision,
            },
            PermissionMode::AutoReview => PermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::AutoReview,
                mode,
                reason: String::from("route_to_reviewer"),
                prompt: permission_prompt(request),
                policy_revision: review_policy.revision,
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
        session_granted: bool,
        review_policy: &ReviewPolicy,
    ) -> PermissionDecision {
        let restriction_decision = Self::check_restrictions(request, review_policy);
        if let Some(decision) = restriction_decision {
            return decision;
        }
        if request.capability != PermissionCapability::ShellCommand
            || request.risk != PermissionRisk::ProcessExecution
        {
            return Self::deny_shell_at(request, "permission_risk_denied", review_policy.revision);
        }
        if user_allowlisted {
            return PermissionDecision::Allowed {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode: PermissionMode::Allow,
                reason: String::from("shell_user_allowlist"),
                rationale: None,
                policy_revision: review_policy.revision,
            };
        }
        // A session grant is the user's own prior review of this exact
        // command, so it carries its own reason rather than masquerading as
        // config authority or as a mode change.
        if session_granted {
            return PermissionDecision::Allowed {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::User,
                mode: PermissionMode::Allow,
                reason: String::from("shell_session_grant"),
                rationale: None,
                policy_revision: review_policy.revision,
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
                    policy_revision: review_policy.revision,
                }
            }
            ApprovalMode::FullAccess => PermissionDecision::Allowed {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::None,
                mode: PermissionMode::Allow,
                reason: String::from("approval_mode_full_access"),
                rationale: None,
                policy_revision: review_policy.revision,
            },
            // The caller intercepts this route and invokes the review
            // coordinator instead of the user widget. Reaching the widget
            // means no reviewer session was live.
            ApprovalMode::AutoReview => PermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: PermissionReviewer::AutoReview,
                mode: PermissionMode::AutoReview,
                reason: String::from("route_to_reviewer"),
                prompt: permission_prompt(request),
                policy_revision: review_policy.revision,
            },
        }
    }

    #[must_use]
    pub fn deny_shell(request: &PermissionRequest, reason: &str) -> PermissionDecision {
        Self::deny_shell_at(request, reason, PolicyRevision(0))
    }

    fn deny_shell_at(
        request: &PermissionRequest,
        reason: &str,
        policy_revision: PolicyRevision,
    ) -> PermissionDecision {
        debug_assert_eq!(request.capability, PermissionCapability::ShellCommand);
        PermissionDecision::Denied {
            decision_id: next_permission_decision_id(),
            reviewer: PermissionReviewer::None,
            mode: PermissionMode::Deny,
            reason: reason.to_owned(),
            rationale: None,
            policy_revision,
        }
    }

    /// Standing user restrictions. `None` means no restriction matches.
    /// Global matches are considered before project matches; a project entry
    /// can add a hold but cannot relax a global `HumanPerforms`.
    #[must_use]
    pub fn check_restrictions(
        request: &PermissionRequest,
        policy: &ReviewPolicy,
    ) -> Option<PermissionDecision> {
        let restriction = selected_restriction(policy, request)?;
        let reason = match restriction {
            ReviewRestriction::AskFirst { .. } => String::from(RESTRICTION_ASK_FIRST_REASON),
            ReviewRestriction::HumanPerforms { .. } => {
                String::from(RESTRICTION_HUMAN_PERFORMS_REASON)
            }
        };
        Some(PermissionDecision::NeedsUserReview {
            decision_id: next_permission_decision_id(),
            reviewer: PermissionReviewer::User,
            mode: PermissionMode::Ask,
            reason,
            prompt: restriction_prompt(request, restriction),
            policy_revision: policy.revision,
        })
    }
}

fn selected_restriction<'a>(
    policy: &'a ReviewPolicy,
    request: &PermissionRequest,
) -> Option<&'a ReviewRestriction> {
    let global = strongest_match(&policy.global, request);
    let project = strongest_match(&policy.project, request);
    match (global, project) {
        (Some(global @ ReviewRestriction::HumanPerforms { .. }), _) => Some(global),
        (_, Some(project @ ReviewRestriction::HumanPerforms { .. })) => Some(project),
        (Some(global), _) => Some(global),
        (None, project) => project,
    }
}

fn strongest_match<'a>(
    restrictions: &'a [ReviewRestriction],
    request: &PermissionRequest,
) -> Option<&'a ReviewRestriction> {
    let mut selected: Option<&ReviewRestriction> = None;
    for restriction in restrictions {
        if !restriction_matches(restriction, request) {
            continue;
        }
        let replace = match selected {
            None => true,
            Some(ReviewRestriction::AskFirst { .. })
                if matches!(restriction, ReviewRestriction::HumanPerforms { .. }) =>
            {
                true
            }
            Some(_) => false,
        };
        if replace {
            selected = Some(restriction);
        }
    }
    selected
}

fn restriction_matches(restriction: &ReviewRestriction, request: &PermissionRequest) -> bool {
    let matcher = match restriction {
        ReviewRestriction::AskFirst { matcher, .. }
        | ReviewRestriction::HumanPerforms { matcher, .. } => matcher,
    };
    match matcher {
        RestrictionMatcher::CommandPrefix { prefix } => {
            command_candidates(request).any(|command| command_prefix_matches(command, prefix))
        }
        RestrictionMatcher::PathPrefix { prefix } => {
            path_prefix_matches(&request.target.resource, prefix)
        }
        RestrictionMatcher::ActionClass { class } => command_candidates(request)
            .any(|command| inferred_action_class(command) == Some(*class)),
    }
}

fn command_candidates(request: &PermissionRequest) -> impl Iterator<Item = &str> {
    let explicit = request
        .command
        .as_deref()
        .filter(|command| !command.is_empty());
    let operation = (request.capability == PermissionCapability::ShellCommand)
        .then_some(request.target.operation.as_str())
        .filter(|operation| !operation.is_empty());
    explicit.into_iter().chain(operation)
}

fn command_prefix_matches(command: &str, prefix: &str) -> bool {
    let command = normalized_argv(command);
    let prefix = normalized_argv(prefix);
    if prefix.is_empty() || command.len() < prefix.len() {
        return false;
    }
    command
        .iter()
        .zip(prefix.iter())
        .all(|(left, right)| left == right)
}

fn normalized_argv(command: &str) -> Vec<String> {
    match shell_words::split(command) {
        Ok(argv) if !argv.is_empty() => argv,
        _ => command.split_whitespace().map(str::to_owned).collect(),
    }
}

fn path_prefix_matches(resource: &str, prefix: &str) -> bool {
    let resource = resource.trim();
    let prefix = prefix.trim();
    if prefix.is_empty() || resource.is_empty() {
        return false;
    }
    Path::new(resource).starts_with(Path::new(prefix))
}

fn inferred_action_class(command: &str) -> Option<ActionClass> {
    let argv = normalized_argv(command);
    let first = argv.first().map(String::as_str)?;
    let second = argv.get(1).map(String::as_str);
    match (first, second) {
        ("nixos-rebuild" | "darwin-rebuild", _) => Some(ActionClass::HostActivation),
        ("cargo" | "npm" | "pnpm" | "yarn", Some("publish")) => Some(ActionClass::ExternalPublish),
        ("cargo" | "brew", Some("install")) | ("nix-env", _) => {
            Some(ActionClass::PersistentInstall)
        }
        ("nix", Some("profile")) => Some(ActionClass::PersistentInstall),
        ("rm" | "shred", _) | ("git", Some("clean")) => Some(ActionClass::DestructiveDelete),
        _ => None,
    }
}

fn restriction_prompt(
    request: &PermissionRequest,
    restriction: &ReviewRestriction,
) -> PermissionPrompt {
    let note = match restriction {
        ReviewRestriction::AskFirst { note, .. }
        | ReviewRestriction::HumanPerforms { note, .. } => note.as_str(),
    };
    let title = match restriction {
        ReviewRestriction::HumanPerforms { .. } => String::from("Human performs this action"),
        ReviewRestriction::AskFirst { .. } => format!("Approve {}", request.target.operation),
    };
    PermissionPrompt {
        title: truncate_chars(&title, 128),
        body: truncate_chars(note, 512),
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
        PermissionDecisionId, PermissionDecisionSummary, PermissionMode, PermissionPolicy,
        PermissionRequest, PermissionReviewer, PermissionRisk, PermissionTargetSummary,
        persist_project_approval_mode,
    };
    use crate::review::{PolicyRevision, ReviewPolicy};
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
            command: None,
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
            command: None,
        }
    }

    #[test]
    fn ask_mode_routes_edit_to_user_review() {
        let decision = PermissionDecisionEngine::decide(
            &edit_request(),
            &PermissionPolicy::for_edit_mode(PermissionMode::Ask),
            &ReviewPolicy::empty(),
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
            &ReviewPolicy::empty(),
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
            &ReviewPolicy::empty(),
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
    fn auto_review_routes_to_reviewer() {
        let decision = PermissionDecisionEngine::decide(
            &edit_request(),
            &PermissionPolicy::for_edit_mode(PermissionMode::AutoReview),
            &ReviewPolicy::empty(),
        );

        assert!(matches!(
            decision,
            PermissionDecision::NeedsUserReview {
                reviewer: PermissionReviewer::AutoReview,
                mode: PermissionMode::AutoReview,
                reason,
                ..
            } if reason == "route_to_reviewer"
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
            &ReviewPolicy::empty(),
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
            &ReviewPolicy::empty(),
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
            policy_revision: PolicyRevision(0),
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
        let review = PermissionDecisionEngine::decide_shell(
            &shell_request(),
            ApprovalMode::Review,
            false,
            false,
            &ReviewPolicy::empty(),
        );
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
            false,
            &ReviewPolicy::empty(),
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
            false,
            &ReviewPolicy::empty(),
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
    fn a_session_grant_allows_with_its_own_provenance() {
        // A grant must be distinguishable from config authority and from a
        // mode change, so an audit can tell why a command ran unprompted.
        let granted = PermissionDecisionEngine::decide_shell(
            &shell_request(),
            ApprovalMode::Review,
            false,
            true,
            &ReviewPolicy::empty(),
        );
        assert!(
            matches!(
                granted,
                PermissionDecision::Allowed {
                    mode: PermissionMode::Allow,
                    reviewer: PermissionReviewer::User,
                    ref reason,
                    ..
                } if reason == "shell_session_grant"
            ),
            "expected a session-grant allow, got {granted:?}"
        );
    }

    #[test]
    fn config_allowlist_outranks_a_session_grant() {
        // Both allow, but the durable reason must win so evidence reports
        // the standing authority rather than a transient grant.
        let both = PermissionDecisionEngine::decide_shell(
            &shell_request(),
            ApprovalMode::Review,
            true,
            true,
            &ReviewPolicy::empty(),
        );
        assert!(
            matches!(
                both,
                PermissionDecision::Allowed { ref reason, .. }
                    if reason == "shell_user_allowlist"
            ),
            "expected allowlist provenance, got {both:?}"
        );
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

    fn human_performs(prefix: &str) -> crate::ReviewRestriction {
        crate::ReviewRestriction::HumanPerforms {
            matcher: crate::RestrictionMatcher::CommandPrefix {
                prefix: prefix.to_owned(),
            },
            note: String::from("I run rebuilds"),
        }
    }

    #[test]
    fn project_ask_first_cannot_relax_global_human_performs() {
        let request = PermissionRequest {
            target: PermissionTargetSummary {
                operation: String::from("nixos-rebuild switch"),
                resource: String::from("."),
            },
            ..shell_request()
        };
        let review_policy = crate::ReviewPolicy {
            revision: crate::PolicyRevision(4),
            global: vec![human_performs("nixos-rebuild")],
            project: vec![crate::ReviewRestriction::AskFirst {
                matcher: crate::RestrictionMatcher::CommandPrefix {
                    prefix: String::from("nixos-rebuild"),
                },
                note: String::from("project wants to ask"),
            }],
        };

        let decision = PermissionDecisionEngine::decide_shell(
            &request,
            ApprovalMode::FullAccess,
            true,
            true,
            &review_policy,
        );

        assert!(
            matches!(
                decision,
                PermissionDecision::NeedsUserReview { ref reason, .. }
                    if reason == "restriction_human_performs"
            ),
            "global human-performs must outrank project ask-first, allowlist, session grant, and full-access, got {decision:?}"
        );
    }

    #[test]
    fn project_restriction_holds_when_global_does_not_match() {
        let request = PermissionRequest {
            target: PermissionTargetSummary {
                operation: String::from("cargo publish -p yach"),
                resource: String::from("."),
            },
            ..shell_request()
        };
        let review_policy = crate::ReviewPolicy {
            revision: crate::PolicyRevision(1),
            global: vec![human_performs("nixos-rebuild")],
            project: vec![crate::ReviewRestriction::AskFirst {
                matcher: crate::RestrictionMatcher::CommandPrefix {
                    prefix: String::from("cargo publish"),
                },
                note: String::from("confirm publish"),
            }],
        };

        let decision = PermissionDecisionEngine::decide_shell(
            &request,
            ApprovalMode::FullAccess,
            true,
            true,
            &review_policy,
        );

        assert!(
            matches!(
                decision,
                PermissionDecision::NeedsUserReview { ref reason, .. }
                    if reason == "restriction_ask_first"
            ),
            "a project restriction must add a hold the global set does not cover, got {decision:?}"
        );
    }

    #[test]
    fn allow_mode_edit_still_holds_for_global_path_restriction() {
        let request = PermissionRequest {
            target: PermissionTargetSummary {
                operation: String::from("modify_text_file"),
                resource: String::from("secrets/keys.txt"),
            },
            ..edit_request()
        };
        let review_policy = crate::ReviewPolicy {
            revision: crate::PolicyRevision(2),
            global: vec![crate::ReviewRestriction::HumanPerforms {
                matcher: crate::RestrictionMatcher::PathPrefix {
                    prefix: String::from("secrets"),
                },
                note: String::from("I handle secrets"),
            }],
            project: vec![crate::ReviewRestriction::AskFirst {
                matcher: crate::RestrictionMatcher::PathPrefix {
                    prefix: String::from("secrets"),
                },
                note: String::from("project would only ask"),
            }],
        };

        let decision = PermissionDecisionEngine::decide(
            &request,
            &PermissionPolicy::for_edit_mode(PermissionMode::Allow),
            &review_policy,
        );

        assert!(
            matches!(
                decision,
                PermissionDecision::NeedsUserReview { ref reason, .. }
                    if reason == "restriction_human_performs"
            ),
            "edit allow mode must not bypass a global human-performs path restriction, got {decision:?}"
        );
    }

    #[test]
    fn host_activation_class_holds_under_full_access_and_allowlist() {
        let request = PermissionRequest {
            target: PermissionTargetSummary {
                operation: String::from("nixos-rebuild switch"),
                resource: String::from("."),
            },
            ..shell_request()
        };
        let review_policy = ReviewPolicy {
            revision: crate::PolicyRevision(1),
            global: vec![crate::ReviewRestriction::HumanPerforms {
                matcher: crate::RestrictionMatcher::ActionClass {
                    class: crate::ActionClass::HostActivation,
                },
                note: String::from("I run rebuilds"),
            }],
            project: Vec::new(),
        };
        let decision = PermissionDecisionEngine::decide_shell(
            &request,
            ApprovalMode::FullAccess,
            true,
            true,
            &review_policy,
        );
        assert!(
            matches!(
                decision,
                PermissionDecision::NeedsUserReview { ref reason, .. }
                    if reason == "restriction_human_performs"
            ),
            "host activation must hold even when allowlisted under full-access, got {decision:?}"
        );
    }

    #[test]
    fn empty_command_prefix_does_not_hold_every_command() {
        let review_policy = ReviewPolicy {
            revision: crate::PolicyRevision(1),
            global: vec![crate::ReviewRestriction::HumanPerforms {
                matcher: crate::RestrictionMatcher::CommandPrefix {
                    prefix: String::new(),
                },
                note: String::from("blank"),
            }],
            project: Vec::new(),
        };
        let decision = PermissionDecisionEngine::decide_shell(
            &shell_request(),
            ApprovalMode::FullAccess,
            true,
            false,
            &review_policy,
        );
        assert!(
            matches!(
                decision,
                PermissionDecision::Allowed { ref reason, .. } if reason == "shell_user_allowlist"
            ),
            "an empty prefix must not match every command, got {decision:?}"
        );
    }

    #[test]
    fn old_permission_summary_without_revisions_replays() {
        let decision = PermissionDecisionEngine::decide(
            &edit_request(),
            &PermissionPolicy::for_edit_mode(PermissionMode::Ask),
            &ReviewPolicy::empty(),
        );
        let summary = decision.summary(&edit_request(), false);
        let encoded = serde_json::to_value(&summary);
        assert!(encoded.is_ok());
        let Ok(mut encoded) = encoded else {
            return;
        };
        let Some(object) = encoded.as_object_mut() else {
            return;
        };
        object.remove("policy_revision");
        object.remove("authorization_revision");
        let replayed = serde_json::from_value::<PermissionDecisionSummary>(encoded);
        assert!(replayed.is_ok());
        let Ok(replayed) = replayed else {
            return;
        };
        assert_eq!(replayed.policy_revision, crate::PolicyRevision(0));
        assert_eq!(replayed.authorization_revision, 0);
        assert_eq!(replayed.reason, summary.reason);
    }

    #[test]
    fn restriction_decision_records_evaluated_policy_revision() {
        let request = PermissionRequest {
            target: PermissionTargetSummary {
                operation: String::from("nixos-rebuild switch"),
                resource: String::from("."),
            },
            ..shell_request()
        };
        let review_policy = ReviewPolicy {
            revision: crate::PolicyRevision(7),
            global: vec![human_performs("nixos-rebuild")],
            project: Vec::new(),
        };
        let decision = PermissionDecisionEngine::decide_shell(
            &request,
            ApprovalMode::FullAccess,
            true,
            true,
            &review_policy,
        );
        let summary = decision.summary(&request, false);
        assert_eq!(summary.policy_revision, crate::PolicyRevision(7));
        assert_eq!(summary.reason, "restriction_human_performs");
    }

    #[test]
    fn path_prefix_matches_root_and_rejects_sibling_stem() {
        let root_hold = PermissionDecisionEngine::decide(
            &PermissionRequest {
                target: PermissionTargetSummary {
                    operation: String::from("modify_text_file"),
                    resource: String::from("/etc/nixos/configuration.nix"),
                },
                ..edit_request()
            },
            &PermissionPolicy::for_edit_mode(PermissionMode::Allow),
            &ReviewPolicy {
                revision: crate::PolicyRevision(1),
                global: vec![crate::ReviewRestriction::AskFirst {
                    matcher: crate::RestrictionMatcher::PathPrefix {
                        prefix: String::from("/"),
                    },
                    note: String::from("anywhere absolute"),
                }],
                project: Vec::new(),
            },
        );
        assert!(
            matches!(
                root_hold,
                PermissionDecision::NeedsUserReview { ref reason, .. }
                    if reason == "restriction_ask_first"
            ),
            "root path prefix must match an absolute resource, got {root_hold:?}"
        );

        let sibling = PermissionDecisionEngine::decide(
            &PermissionRequest {
                target: PermissionTargetSummary {
                    operation: String::from("modify_text_file"),
                    resource: String::from("/foo/bar"),
                },
                ..edit_request()
            },
            &PermissionPolicy::for_edit_mode(PermissionMode::Allow),
            &ReviewPolicy {
                revision: crate::PolicyRevision(1),
                global: vec![crate::ReviewRestriction::HumanPerforms {
                    matcher: crate::RestrictionMatcher::PathPrefix {
                        prefix: String::from("/foo/ba"),
                    },
                    note: String::from("not a sibling"),
                }],
                project: Vec::new(),
            },
        );
        assert!(
            matches!(
                sibling,
                PermissionDecision::Allowed { ref reason, .. }
                    if reason == "permission_mode_allowed"
            ),
            "path prefix must not match a sibling stem, got {sibling:?}"
        );
    }
}
