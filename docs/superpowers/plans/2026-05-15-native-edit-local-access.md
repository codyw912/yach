# Native Edit Local Access Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add user-facing native local edit access through a generic permission pipeline, backend-owned prepared edit facade, protocol events, and an initial TUI review/apply flow.

**Architecture:** Core owns permission decisions and edit enforcement. The backend keeps prepared edit transactions in memory behind a session-scoped preview ID, records redacted permission/edit evidence, and sends only summaries through the protocol. The TUI is the first client: `/edit` creates a local request, receives a preview, and submits apply/reject decisions without making mutation provider-visible.

**Tech Stack:** Rust workspace, `serde` JSONL protocol/events, Tokio native backend loop, ratatui TUI, existing native edit engine/session log.

---

## File Structure

- `crates/yach-backend/src/permission.rs`: new generic permission request, mode, route, decision, and redacted evidence model.
- `crates/yach-backend/src/session.rs`: add durable `PermissionDecisionRecorded` events and keep provider transcript/session projections ignoring permission evidence.
- `crates/yach-backend/src/edit_harness.rs`: expose crate-local evidence helpers for the new facade and keep the existing preview-and-apply harness behavior intact.
- `crates/yach-backend/src/edit_access.rs`: new stateful local edit facade that owns pending prepared transactions and is the only runtime path that applies local edit previews.
- `crates/yach-backend/src/native_runner.rs`: wire protocol events to the edit facade, advertise local edit capability to the UI, persist evidence, and keep provider tool advertising unchanged.
- `crates/yach-backend/src/lib.rs`: publish the new backend types needed by tests and the runner.
- `crates/yach-proto/src/lib.rs`: add local edit capability, request/preview/decision/finish DTOs, and JSONL tests.
- `crates/yach-ui/src/slash_commands.rs`: add `/edit`.
- `crates/yach-ui/src/app.rs`: add a minimal multi-step local edit composer/review mode, send local edit events, handle preview/finish events, and test UI states.

Keep `NativeEditEngine::apply` crate-local. Do not add provider-visible edit/write tools. Do not serialize `PreparedNativeEditTransaction`, apply payloads, raw file bodies, or raw diffs beyond existing truncated summaries.

---

### Task 1: Permission Model And Evidence

**Files:**
- Create: `crates/yach-backend/src/permission.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-backend/src/session.rs`

- [ ] **Step 1: Write failing permission decision tests**

Add this test module to the bottom of `crates/yach-backend/src/permission.rs` while creating the file:

```rust
#[cfg(test)]
mod tests {
    use super::{
        NativePermissionActor, NativePermissionCapability, NativePermissionDecision,
        NativePermissionDecisionEngine, NativePermissionMode, NativePermissionPolicy,
        NativePermissionRequest, NativePermissionReviewer, NativePermissionRisk,
        NativePermissionTargetSummary,
    };

    fn edit_request() -> NativePermissionRequest {
        NativePermissionRequest {
            request_id: String::from("perm-1"),
            actor: NativePermissionActor::UserLocalUi,
            capability: NativePermissionCapability::EditTransaction,
            target: NativePermissionTargetSummary {
                operation: String::from("modify_text_file"),
                resource: String::from("src/lib.rs"),
            },
            risk: NativePermissionRisk::WorkspaceWrite,
            requested_reviewer: None,
        }
    }

    #[test]
    fn ask_mode_routes_edit_to_user_review() {
        let decision = NativePermissionDecisionEngine::decide(
            &edit_request(),
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Ask),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::NeedsUserReview {
                reviewer: NativePermissionReviewer::User,
                ..
            }
        ));
    }

    #[test]
    fn allow_mode_allows_without_reviewer() {
        let decision = NativePermissionDecisionEngine::decide(
            &edit_request(),
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::Allowed {
                reviewer: NativePermissionReviewer::None,
                ..
            }
        ));
    }

    #[test]
    fn deny_mode_denies_before_edit_preview() {
        let decision = NativePermissionDecisionEngine::decide(
            &edit_request(),
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Deny),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::Denied {
                reason,
                reviewer: NativePermissionReviewer::None,
                ..
            } if reason == "permission_mode_denied"
        ));
    }

    #[test]
    fn auto_review_is_represented_and_falls_back_to_user_review() {
        let decision = NativePermissionDecisionEngine::decide(
            &edit_request(),
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::AutoReview),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::NeedsUserReview {
                reviewer: NativePermissionReviewer::AutoReview,
                reason,
                ..
            } if reason == "auto_review_unavailable_fallback_ask"
        ));
    }

    #[test]
    fn extension_cannot_self_approve() {
        let request = NativePermissionRequest {
            actor: NativePermissionActor::Extension {
                extension_id: String::from("ext-a"),
            },
            requested_reviewer: Some(NativePermissionReviewer::Extension {
                extension_id: String::from("ext-a"),
            }),
            ..edit_request()
        };

        let decision = NativePermissionDecisionEngine::decide(
            &request,
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::Denied {
                reason,
                ..
            } if reason == "extension_self_approval_denied"
        ));
    }
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
just dev cargo test -p yach-backend permission::tests -- --nocapture
```

Expected: FAIL because `permission.rs` is not wired into the crate or its types are not implemented.

- [ ] **Step 3: Implement the permission module**

Create `crates/yach-backend/src/permission.rs` with this implementation:

```rust
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static PERMISSION_DECISION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionDecisionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePermissionCapability {
    EditTransaction,
    ShellCommand,
    NetworkAccess,
    VerificationAction,
    ExtensionTool,
    ProviderVisibleTool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativePermissionActor {
    UserLocalUi,
    Core,
    Provider,
    Extension { extension_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePermissionMode {
    Allow,
    Ask,
    Deny,
    AutoReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativePermissionReviewer {
    None,
    User,
    AutoReview,
    Extension { extension_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePermissionRisk {
    ReadOnly,
    WorkspaceWrite,
    ExternalWrite,
    Network,
    ProcessExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionTargetSummary {
    pub operation: String,
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionRequest {
    pub request_id: String,
    pub actor: NativePermissionActor,
    pub capability: NativePermissionCapability,
    pub target: NativePermissionTargetSummary,
    pub risk: NativePermissionRisk,
    pub requested_reviewer: Option<NativePermissionReviewer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePermissionPolicy {
    pub edit_mode: NativePermissionMode,
}

impl NativePermissionPolicy {
    #[must_use]
    pub const fn for_edit_mode(edit_mode: NativePermissionMode) -> Self {
        Self { edit_mode }
    }

    #[must_use]
    pub const fn default_local_edit() -> Self {
        Self {
            edit_mode: NativePermissionMode::Ask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum NativePermissionDecision {
    Allowed {
        decision_id: NativePermissionDecisionId,
        reviewer: NativePermissionReviewer,
        mode: NativePermissionMode,
        reason: String,
        rationale: Option<String>,
    },
    Denied {
        decision_id: NativePermissionDecisionId,
        reviewer: NativePermissionReviewer,
        mode: NativePermissionMode,
        reason: String,
        rationale: Option<String>,
    },
    NeedsUserReview {
        decision_id: NativePermissionDecisionId,
        reviewer: NativePermissionReviewer,
        mode: NativePermissionMode,
        reason: String,
        prompt: NativePermissionPrompt,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionPrompt {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionDecisionSummary {
    pub request_id: String,
    pub decision_id: NativePermissionDecisionId,
    pub actor: NativePermissionActor,
    pub capability: NativePermissionCapability,
    pub target: NativePermissionTargetSummary,
    pub risk: NativePermissionRisk,
    pub configured_mode: NativePermissionMode,
    pub reviewer: NativePermissionReviewer,
    pub outcome: String,
    pub reason: String,
    pub rationale: Option<String>,
    pub user_override: bool,
}

impl NativePermissionDecision {
    #[must_use]
    pub fn decision_id(&self) -> NativePermissionDecisionId {
        match self {
            Self::Allowed { decision_id, .. }
            | Self::Denied { decision_id, .. }
            | Self::NeedsUserReview { decision_id, .. } => decision_id.clone(),
        }
    }

    #[must_use]
    pub fn summary(
        &self,
        request: &NativePermissionRequest,
        user_override: bool,
    ) -> NativePermissionDecisionSummary {
        match self {
            Self::Allowed {
                decision_id,
                reviewer,
                mode,
                reason,
                rationale,
            } => NativePermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: request.target.clone(),
                risk: request.risk,
                configured_mode: *mode,
                reviewer: reviewer.clone(),
                outcome: String::from("allowed"),
                reason: reason.clone(),
                rationale: rationale.clone(),
                user_override,
            },
            Self::Denied {
                decision_id,
                reviewer,
                mode,
                reason,
                rationale,
            } => NativePermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: request.target.clone(),
                risk: request.risk,
                configured_mode: *mode,
                reviewer: reviewer.clone(),
                outcome: String::from("denied"),
                reason: reason.clone(),
                rationale: rationale.clone(),
                user_override,
            },
            Self::NeedsUserReview {
                decision_id,
                reviewer,
                mode,
                reason,
                ..
            } => NativePermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: request.target.clone(),
                risk: request.risk,
                configured_mode: *mode,
                reviewer: reviewer.clone(),
                outcome: String::from("needs_user_review"),
                reason: reason.clone(),
                rationale: None,
                user_override,
            },
        }
    }
}

pub struct NativePermissionDecisionEngine;

impl NativePermissionDecisionEngine {
    #[must_use]
    pub fn decide(
        request: &NativePermissionRequest,
        policy: &NativePermissionPolicy,
    ) -> NativePermissionDecision {
        if extension_self_approval_requested(request) {
            return NativePermissionDecision::Denied {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::None,
                mode: policy.edit_mode,
                reason: String::from("extension_self_approval_denied"),
                rationale: None,
            };
        }

        let mode = match request.capability {
            NativePermissionCapability::EditTransaction => policy.edit_mode,
            NativePermissionCapability::ShellCommand
            | NativePermissionCapability::NetworkAccess
            | NativePermissionCapability::VerificationAction
            | NativePermissionCapability::ExtensionTool
            | NativePermissionCapability::ProviderVisibleTool => NativePermissionMode::Deny,
        };

        match mode {
            NativePermissionMode::Allow => NativePermissionDecision::Allowed {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::None,
                mode,
                reason: String::from("permission_mode_allowed"),
                rationale: None,
            },
            NativePermissionMode::Ask => NativePermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::User,
                mode,
                reason: String::from("permission_mode_ask"),
                prompt: permission_prompt(request),
            },
            NativePermissionMode::Deny => NativePermissionDecision::Denied {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::None,
                mode,
                reason: String::from("permission_mode_denied"),
                rationale: None,
            },
            NativePermissionMode::AutoReview => NativePermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::AutoReview,
                mode,
                reason: String::from("auto_review_unavailable_fallback_ask"),
                prompt: permission_prompt(request),
            },
        }
    }
}

fn permission_prompt(request: &NativePermissionRequest) -> NativePermissionPrompt {
    NativePermissionPrompt {
        title: format!("Approve {}", request.target.operation),
        body: format!("{} on {}", request.target.operation, request.target.resource),
    }
}

fn extension_self_approval_requested(request: &NativePermissionRequest) -> bool {
    match (&request.actor, &request.requested_reviewer) {
        (
            NativePermissionActor::Extension { extension_id: actor },
            Some(NativePermissionReviewer::Extension {
                extension_id: reviewer,
            }),
        ) => actor == reviewer,
        _ => false,
    }
}

fn next_permission_decision_id() -> NativePermissionDecisionId {
    let next = PERMISSION_DECISION_COUNTER.fetch_add(1, Ordering::Relaxed);
    NativePermissionDecisionId(format!("permission-decision-{next}"))
}
```

Keep the tests from Step 1 at the bottom of this file.

- [ ] **Step 4: Wire the permission module**

Modify `crates/yach-backend/src/lib.rs` near the module declarations:

```rust
mod permission;
```

Modify the public re-export list in `crates/yach-backend/src/lib.rs`:

```rust
pub use permission::*;
```

- [ ] **Step 5: Add permission evidence to native sessions**

In `crates/yach-backend/src/session.rs`, extend the imports:

```rust
use crate::{
    NativeEditTransactionId, NativePermissionDecisionSummary, NativeToolError,
    NativeToolPermissionState,
};
```

Add this variant to `NativeSessionEvent` after `StaticContextIncluded`:

```rust
    PermissionDecisionRecorded {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        summary: NativePermissionDecisionSummary,
    },
```

Update `last_entry_id`, `transcript_messages`, and every event projection match in `session.rs` so `PermissionDecisionRecorded { .. }` behaves like tool/edit evidence and returns `None`. Update `event_turn_id` so it returns `Some(turn_id)` for permission evidence:

```rust
        | NativeSessionEvent::PermissionDecisionRecorded { turn_id, .. }
        | NativeSessionEvent::EditTransactionPrepared { turn_id, .. }
        | NativeSessionEvent::EditTransactionFinished { turn_id, .. } => Some(turn_id),
```

Also update exhaustive `NativeSessionEvent` matches in `crates/yach-backend/src/native_runner.rs` so permission evidence is ignored by provider replay, session messages, and session stats:

```rust
            | NativeSessionEvent::PermissionDecisionRecorded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. } => None,
```

Add this method to `impl NativeSessionLog`:

```rust
    pub fn record_permission_decision(
        &mut self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        summary: NativePermissionDecisionSummary,
    ) {
        self.push(NativeSessionEvent::PermissionDecisionRecorded {
            session_id,
            turn_id,
            summary,
        });
    }
```

- [ ] **Step 6: Add session evidence tests**

Add these tests to the existing `#[cfg(test)] mod tests` in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_session_permission_evidence_is_not_provider_transcript() {
    let mut log = completed_text_exchange(
        NativeSessionId(String::from("default")),
        NativeEntryId(String::from("entry-1-user")),
        NativeEntryId(String::from("entry-1-assistant")),
        NativeTurnId(String::from("turn-1")),
        String::from("hello"),
        String::from("world"),
    );
    let request = NativePermissionRequest {
        request_id: String::from("perm-1"),
        actor: NativePermissionActor::UserLocalUi,
        capability: NativePermissionCapability::EditTransaction,
        target: NativePermissionTargetSummary {
            operation: String::from("modify_text_file"),
            resource: String::from("src/lib.rs"),
        },
        risk: NativePermissionRisk::WorkspaceWrite,
        requested_reviewer: None,
    };
    let decision = NativePermissionDecisionEngine::decide(
        &request,
        &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
    );

    log.record_permission_decision(
        NativeSessionId(String::from("default")),
        NativeTurnId(String::from("turn-1")),
        decision.summary(&request, false),
    );

    let messages = log.transcript_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "hello");
    assert_eq!(messages[1].text, "world");
}

#[test]
fn native_session_permission_evidence_round_trips_jsonl() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.jsonl");
    let mut log = NativeSessionLog::default();
    let request = NativePermissionRequest {
        request_id: String::from("perm-1"),
        actor: NativePermissionActor::UserLocalUi,
        capability: NativePermissionCapability::EditTransaction,
        target: NativePermissionTargetSummary {
            operation: String::from("create_text_file"),
            resource: String::from("notes.txt"),
        },
        risk: NativePermissionRisk::WorkspaceWrite,
        requested_reviewer: None,
    };
    let decision = NativePermissionDecisionEngine::decide(
        &request,
        &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Ask),
    );
    log.record_permission_decision(
        NativeSessionId(String::from("default")),
        NativeTurnId(String::from("turn-7")),
        decision.summary(&request, false),
    );

    log.write_to_file(&path).expect("write log");
    let loaded = NativeSessionLog::load_from_file(&path).expect("load log");

    assert_eq!(loaded.events, log.events);
    assert_eq!(loaded.next_turn_index(), 8);
}
```

- [ ] **Step 7: Run focused backend tests**

Run:

```bash
just dev cargo test -p yach-backend permission::tests -- --nocapture
just dev cargo test -p yach-backend native_session_permission_evidence -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit Task 1**

Run:

```bash
git add crates/yach-backend/src/lib.rs crates/yach-backend/src/permission.rs crates/yach-backend/src/session.rs
git commit -m "feat: add native permission evidence model"
```

---

### Task 2: Backend-Owned Edit Access Facade

**Files:**
- Create: `crates/yach-backend/src/edit_access.rs`
- Modify: `crates/yach-backend/src/edit_harness.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing edit access tests**

Create `crates/yach-backend/src/edit_access.rs` with the implementation tests first:

```rust
#[cfg(test)]
mod tests {
    use super::{
        NativeEditAccess, NativeEditAccessContext, NativeEditAccessError,
        NativeEditAccessReviewState,
    };
    use crate::{
        NativeEditHunk, NativeEditOperation, NativeEditPolicy, NativeEditTransactionRequest,
        NativePermissionMode, NativePermissionPolicy, NativeResourceRoot, NativeSessionEvent,
        NativeSessionId, NativeSessionLog, NativeTurnId,
    };
    use std::fs;

    fn context(mode: NativePermissionMode) -> NativeEditAccessContext {
        NativeEditAccessContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::for_edit_mode(mode),
            edit_policy: NativeEditPolicy::test(),
        }
    }

    #[test]
    fn prepare_in_ask_mode_keeps_transaction_pending() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("file.txt"), "hello\n").expect("write fixture");
        let root = NativeResourceRoot::project(temp.path()).expect("root");
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();

        let preview = access
            .prepare(
                &root,
                NativeEditTransactionRequest {
                    operations: vec![NativeEditOperation::ModifyTextFile {
                        path: String::from("file.txt"),
                        expected_sha256: crate::sha256_hex_for_test("hello\n"),
                        hunks: vec![NativeEditHunk {
                            find: String::from("hello"),
                            replace: String::from("goodbye"),
                        }],
                    }],
                },
                context(NativePermissionMode::Ask),
                &mut log,
            )
            .expect("preview");

        assert_eq!(preview.review_state, NativeEditAccessReviewState::NeedsUserApproval);
        assert!(access.has_pending_preview(&preview.preview_id));
        assert_eq!(fs::read_to_string(temp.path().join("file.txt")).unwrap(), "hello\n");
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::PermissionDecisionRecorded { .. }
        )));
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::EditTransactionPrepared { .. }
        )));
    }

    #[test]
    fn apply_consumes_pending_preview_and_records_evidence() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("file.txt"), "hello\n").expect("write fixture");
        let root = NativeResourceRoot::project(temp.path()).expect("root");
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let preview = access
            .prepare(
                &root,
                NativeEditTransactionRequest {
                    operations: vec![NativeEditOperation::ModifyTextFile {
                        path: String::from("file.txt"),
                        expected_sha256: crate::sha256_hex_for_test("hello\n"),
                        hunks: vec![NativeEditHunk {
                            find: String::from("hello"),
                            replace: String::from("goodbye"),
                        }],
                    }],
                },
                context(NativePermissionMode::Ask),
                &mut log,
            )
            .expect("preview");

        let result = access
            .apply(&preview.preview_id, preview.permission_decision_id.clone(), &mut log)
            .expect("apply");

        assert_eq!(result.transaction_id, preview.transaction_id);
        assert!(!access.has_pending_preview(&preview.preview_id));
        assert_eq!(
            fs::read_to_string(temp.path().join("file.txt")).unwrap(),
            "goodbye\n"
        );
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::EditTransactionFinished { .. }
        )));
    }

    #[test]
    fn allow_mode_preview_can_apply_through_facade() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("file.txt"), "hello\n").expect("write fixture");
        let root = NativeResourceRoot::project(temp.path()).expect("root");
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let preview = access
            .prepare(
                &root,
                NativeEditTransactionRequest {
                    operations: vec![NativeEditOperation::ModifyTextFile {
                        path: String::from("file.txt"),
                        expected_sha256: crate::sha256_hex_for_test("hello\n"),
                        hunks: vec![NativeEditHunk {
                            find: String::from("hello"),
                            replace: String::from("goodbye"),
                        }],
                    }],
                },
                context(NativePermissionMode::Allow),
                &mut log,
            )
            .expect("preview");

        assert_eq!(preview.review_state, NativeEditAccessReviewState::Allowed);
        access
            .apply(&preview.preview_id, preview.permission_decision_id.clone(), &mut log)
            .expect("apply");

        assert_eq!(
            fs::read_to_string(temp.path().join("file.txt")).unwrap(),
            "goodbye\n"
        );
    }

    #[test]
    fn reject_consumes_pending_preview_without_writing() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("file.txt"), "hello\n").expect("write fixture");
        let root = NativeResourceRoot::project(temp.path()).expect("root");
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let preview = access
            .prepare(
                &root,
                NativeEditTransactionRequest {
                    operations: vec![NativeEditOperation::ModifyTextFile {
                        path: String::from("file.txt"),
                        expected_sha256: crate::sha256_hex_for_test("hello\n"),
                        hunks: vec![NativeEditHunk {
                            find: String::from("hello"),
                            replace: String::from("goodbye"),
                        }],
                    }],
                },
                context(NativePermissionMode::Ask),
                &mut log,
            )
            .expect("preview");

        access
            .reject(&preview.preview_id, preview.permission_decision_id.clone(), &mut log)
            .expect("reject");

        assert!(!access.has_pending_preview(&preview.preview_id));
        assert_eq!(fs::read_to_string(temp.path().join("file.txt")).unwrap(), "hello\n");
    }

    #[test]
    fn deny_mode_rejects_before_preview() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("file.txt"), "hello\n").expect("write fixture");
        let root = NativeResourceRoot::project(temp.path()).expect("root");
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();

        let error = access
            .prepare(
                &root,
                NativeEditTransactionRequest {
                    operations: vec![NativeEditOperation::ModifyTextFile {
                        path: String::from("file.txt"),
                        expected_sha256: crate::sha256_hex_for_test("hello\n"),
                        hunks: vec![NativeEditHunk {
                            find: String::from("hello"),
                            replace: String::from("goodbye"),
                        }],
                    }],
                },
                context(NativePermissionMode::Deny),
                &mut log,
            )
            .expect_err("denied");

        assert!(matches!(error, NativeEditAccessError::PermissionDenied { .. }));
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::PermissionDecisionRecorded { .. }
        )));
        assert!(!log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::EditTransactionPrepared { .. }
        )));
    }

    #[test]
    fn permission_evidence_redacts_absolute_request_paths_before_preview() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("file.txt"), "hello\n").expect("write fixture");
        let root = NativeResourceRoot::project(temp.path()).expect("root");
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let absolute_path = temp.path().join("file.txt").to_string_lossy().into_owned();

        let _ = access
            .prepare(
                &root,
                NativeEditTransactionRequest {
                    operations: vec![NativeEditOperation::ModifyTextFile {
                        path: absolute_path,
                        expected_sha256: crate::sha256_hex_for_test("hello\n"),
                        hunks: vec![NativeEditHunk {
                            find: String::from("hello"),
                            replace: String::from("goodbye"),
                        }],
                    }],
                },
                context(NativePermissionMode::Deny),
                &mut log,
            )
            .expect_err("denied");

        let Some(NativeSessionEvent::PermissionDecisionRecorded { summary, .. }) =
            log.events.iter().find(|event| {
                matches!(event, NativeSessionEvent::PermissionDecisionRecorded { .. })
            })
        else {
            panic!("permission decision evidence should be recorded");
        };
        assert_eq!(summary.target.resource, "<absolute_path>");
    }

    #[test]
    fn stale_preview_id_fails_safely() {
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let error = access
            .apply(
                &crate::NativeEditPreviewId(String::from("missing")),
                crate::NativePermissionDecisionId(String::from("permission-decision-missing")),
                &mut log,
            )
            .expect_err("missing preview");

        assert_eq!(error, NativeEditAccessError::PreviewNotFound);
    }
}
```

- [ ] **Step 2: Run the failing edit access tests**

Run:

```bash
just dev cargo test -p yach-backend edit_access::tests -- --nocapture
```

Expected: FAIL because `edit_access` and `sha256_hex_for_test` are not implemented/exported.

- [ ] **Step 3: Expose crate-local edit evidence helpers**

In `crates/yach-backend/src/edit_harness.rs`, change these helper signatures from private to crate-private:

```rust
pub(crate) fn native_edit_prepared_evidence_summary(
    prepared: &PreparedNativeEditTransaction,
) -> NativeEditEvidenceSummary {
```

```rust
pub(crate) fn native_edit_apply_evidence_summary(
    result: &NativeEditApplyResult,
) -> NativeEditEvidenceSummary {
```

Do not change `NativeEditHarness::preview_and_apply`; existing harness tests should continue passing.

- [ ] **Step 4: Add test-only hash helper**

In `crates/yach-backend/src/edit.rs`, expose a test-only helper near the existing private `sha256_hex` function:

```rust
#[cfg(test)]
pub(crate) fn sha256_hex_for_test(content: &str) -> String {
    sha256_hex(content.as_bytes())
}
```

- [ ] **Step 5: Implement `edit_access.rs`**

Replace the test-only skeleton in `crates/yach-backend/src/edit_access.rs` with the implementation plus the tests from Step 1 at the bottom:

```rust
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::edit_harness::{
    native_edit_apply_evidence_summary, native_edit_prepared_evidence_summary,
};
use crate::{
    NativeEditApplyResult, NativeEditEngine, NativeEditError, NativeEditEvidenceOutcome,
    NativeEditPolicy, NativeEditTransactionId, NativeEditTransactionRequest,
    NativePermissionActor, NativePermissionCapability, NativePermissionDecision,
    NativePermissionDecisionEngine, NativePermissionDecisionId, NativePermissionPolicy,
    NativePermissionRequest, NativePermissionRisk, NativePermissionTargetSummary,
    NativeResourceRoot, NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeToolRequestId,
    NativeTurnId, PreparedNativeEditTransaction,
};

static EDIT_PREVIEW_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEditPreviewId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditAccessContext {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
    pub permission_policy: NativePermissionPolicy,
    pub edit_policy: NativeEditPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeEditAccessReviewState {
    Allowed,
    NeedsUserApproval,
    AutoReviewUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditPreview {
    pub preview_id: NativeEditPreviewId,
    pub transaction_id: NativeEditTransactionId,
    pub permission_decision_id: NativePermissionDecisionId,
    pub review_state: NativeEditAccessReviewState,
    pub operation_count: usize,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
    pub diff_summary_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeEditAccessError {
    PermissionDenied { reason: String },
    Preview(NativeEditError),
    Apply(NativeEditError),
    PreviewNotFound,
    DecisionMismatch,
}

#[derive(Debug)]
struct PendingNativeEditPreview {
    context: NativeEditAccessContext,
    root: NativeResourceRoot,
    prepared: PreparedNativeEditTransaction,
    permission_decision_id: NativePermissionDecisionId,
}

#[derive(Debug, Default)]
pub struct NativeEditAccess {
    pending: BTreeMap<String, PendingNativeEditPreview>,
}

impl NativeEditAccess {
    pub fn prepare(
        &mut self,
        root: &NativeResourceRoot,
        request: NativeEditTransactionRequest,
        context: NativeEditAccessContext,
        log: &mut NativeSessionLog,
    ) -> Result<NativeEditPreview, NativeEditAccessError> {
        let permission_request = permission_request_from_edit(&request);
        let decision =
            NativePermissionDecisionEngine::decide(&permission_request, &context.permission_policy);
        log.record_permission_decision(
            context.session_id.clone(),
            context.turn_id.clone(),
            decision.summary(&permission_request, false),
        );

        let review_state = match &decision {
            NativePermissionDecision::Allowed { .. } => NativeEditAccessReviewState::Allowed,
            NativePermissionDecision::NeedsUserReview { reason, .. }
                if reason == "auto_review_unavailable_fallback_ask" =>
            {
                NativeEditAccessReviewState::AutoReviewUnavailable
            }
            NativePermissionDecision::NeedsUserReview { .. } => {
                NativeEditAccessReviewState::NeedsUserApproval
            }
            NativePermissionDecision::Denied { reason, .. } => {
                return Err(NativeEditAccessError::PermissionDenied {
                    reason: reason.clone(),
                });
            }
        };

        let prepared = NativeEditEngine::preview(root, request, &context.edit_policy)
            .map_err(NativeEditAccessError::Preview)?;
        let summary = native_edit_prepared_evidence_summary(&prepared);
        log.push(NativeSessionEvent::EditTransactionPrepared {
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_request_id: None::<NativeToolRequestId>,
            transaction_id: prepared.transaction_id.clone(),
            summary,
        });

        let preview_id = NativeEditPreviewId(next_edit_preview_id());
        let preview = NativeEditPreview {
            preview_id: preview_id.clone(),
            transaction_id: prepared.transaction_id.clone(),
            permission_decision_id: decision.decision_id(),
            review_state,
            operation_count: prepared.operation_count,
            diff_summary: prepared.diff_summary.clone(),
            diff_summary_truncated: prepared.diff_summary_truncated,
            diff_summary_bytes: prepared.diff_summary_bytes,
        };
        self.pending.insert(
            preview_id.0.clone(),
            PendingNativeEditPreview {
                context,
                root: root.clone(),
                prepared,
                permission_decision_id: preview.permission_decision_id.clone(),
            },
        );
        Ok(preview)
    }

    pub fn apply(
        &mut self,
        preview_id: &NativeEditPreviewId,
        decision_id: NativePermissionDecisionId,
        log: &mut NativeSessionLog,
    ) -> Result<NativeEditApplyResult, NativeEditAccessError> {
        let pending = self
            .pending
            .remove(&preview_id.0)
            .ok_or(NativeEditAccessError::PreviewNotFound)?;
        if pending.permission_decision_id != decision_id {
            self.pending.insert(preview_id.0.clone(), pending);
            return Err(NativeEditAccessError::DecisionMismatch);
        }
        let transaction_id = pending.prepared.transaction_id.clone();
        match NativeEditEngine::apply(&pending.root, pending.prepared, &pending.context.edit_policy) {
            Ok(result) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: pending.context.session_id,
                    turn_id: pending.context.turn_id,
                    tool_request_id: None::<NativeToolRequestId>,
                    transaction_id: Some(transaction_id),
                    outcome: NativeEditEvidenceOutcome::Completed,
                    reason: None,
                    summary: Some(native_edit_apply_evidence_summary(&result)),
                });
                Ok(result)
            }
            Err(error) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: pending.context.session_id,
                    turn_id: pending.context.turn_id,
                    tool_request_id: None::<NativeToolRequestId>,
                    transaction_id: Some(transaction_id),
                    outcome: NativeEditEvidenceOutcome::Failed,
                    reason: Some(native_edit_error_label(&error)),
                    summary: None,
                });
                Err(NativeEditAccessError::Apply(error))
            }
        }
    }

    pub fn reject(
        &mut self,
        preview_id: &NativeEditPreviewId,
        decision_id: NativePermissionDecisionId,
        log: &mut NativeSessionLog,
    ) -> Result<(), NativeEditAccessError> {
        let pending = self
            .pending
            .remove(&preview_id.0)
            .ok_or(NativeEditAccessError::PreviewNotFound)?;
        if pending.permission_decision_id != decision_id {
            self.pending.insert(preview_id.0.clone(), pending);
            return Err(NativeEditAccessError::DecisionMismatch);
        }
        log.push(NativeSessionEvent::EditTransactionFinished {
            session_id: pending.context.session_id,
            turn_id: pending.context.turn_id,
            tool_request_id: None::<NativeToolRequestId>,
            transaction_id: Some(pending.prepared.transaction_id),
            outcome: NativeEditEvidenceOutcome::Failed,
            reason: Some(String::from("user_rejected")),
            summary: Some(native_edit_prepared_evidence_summary(&pending.prepared)),
        });
        Ok(())
    }

    #[must_use]
    pub fn has_pending_preview(&self, preview_id: &NativeEditPreviewId) -> bool {
        self.pending.contains_key(&preview_id.0)
    }
}

fn permission_request_from_edit(request: &NativeEditTransactionRequest) -> NativePermissionRequest {
    let (operation, resource) = request.operations.first().map_or_else(
        || (String::from("empty_edit_transaction"), String::from("<none>")),
        |operation| match operation {
            crate::NativeEditOperation::ModifyTextFile { path, .. } => {
                (
                    String::from("modify_text_file"),
                    summarized_permission_resource(path),
                )
            }
            crate::NativeEditOperation::CreateTextFile { path, .. } => {
                (
                    String::from("create_text_file"),
                    summarized_permission_resource(path),
                )
            }
        },
    );
    NativePermissionRequest {
        request_id: next_edit_preview_id(),
        actor: NativePermissionActor::UserLocalUi,
        capability: NativePermissionCapability::EditTransaction,
        target: NativePermissionTargetSummary {
            operation,
            resource,
        },
        risk: NativePermissionRisk::WorkspaceWrite,
        requested_reviewer: None,
    }
}

fn summarized_permission_resource(path: &str) -> String {
    let parsed = std::path::Path::new(path);
    if parsed.is_absolute() {
        return String::from("<absolute_path>");
    }
    if parsed
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return String::from("<path_traversal>");
    }
    if path == ".yach" || path.starts_with(".yach/") {
        return String::from("<metadata_path>");
    }
    if path.trim().is_empty() {
        return String::from("<empty_path>");
    }
    path.to_owned()
}

fn next_edit_preview_id() -> String {
    let next = EDIT_PREVIEW_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("edit-preview-{next}")
}

fn native_edit_error_label(error: &NativeEditError) -> String {
    match error {
        NativeEditError::HashMismatch { .. } => String::from("hash_mismatch"),
        NativeEditError::HunkNotFound { .. } => String::from("hunk_not_found"),
        NativeEditError::HunkAmbiguous { .. } => String::from("hunk_ambiguous"),
        NativeEditError::Io { .. } => String::from("io_error"),
        _ => String::from("edit_apply_failed"),
    }
}
```

Important implementation note: `NativeEditEngine::apply` is crate-private today. `edit_access.rs` lives in the same crate, so this does not make apply public.

- [ ] **Step 6: Wire the module**

Modify `crates/yach-backend/src/lib.rs`:

```rust
mod edit_access;
pub use edit_access::*;
```

- [ ] **Step 7: Run focused edit tests**

Run:

```bash
just dev cargo test -p yach-backend edit_access::tests -- --nocapture
just dev cargo test -p yach-backend edit_harness -- --nocapture
```

Expected: PASS. The test helper is crate-visible under `#[cfg(test)]`, so `edit_access` unit tests in the same crate can call `crate::sha256_hex_for_test`.

- [ ] **Step 8: Commit Task 2**

Run:

```bash
git add crates/yach-backend/src/edit.rs crates/yach-backend/src/edit_access.rs crates/yach-backend/src/edit_harness.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add backend local edit access facade"
```

---

### Task 3: Protocol Events For Local Edit

**Files:**
- Modify: `crates/yach-proto/src/lib.rs`
- Modify: `crates/yach-ui/src/lib.rs`

- [ ] **Step 1: Add failing protocol tests**

Add these tests to `crates/yach-proto/src/lib.rs` inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn local_edit_events_round_trip_as_jsonl() {
    let prepare = ClientEvent::LocalEditPrepareRequested {
        request_id: String::from("local-edit-request-1"),
        operation: LocalEditOperationInput::ModifyTextFile {
            path: String::from("src/lib.rs"),
            expected_sha256: String::from("abc123"),
            find: String::from("old"),
            replace: String::from("new"),
        },
    };

    let line = prepare.to_jsonl().expect("encode prepare");
    let decoded = ClientEvent::from_jsonl(&line).expect("decode prepare");
    assert_eq!(decoded, prepare);
    assert!(line.contains("\"type\":\"local_edit_prepare_requested\""));

    let preview = ServerEvent::LocalEditPreviewReady {
        request_id: String::from("local-edit-request-1"),
        preview: LocalEditPreviewSummary {
            preview_id: String::from("edit-preview-1"),
            transaction_id: String::from("edit-transaction-1"),
            permission_decision_id: String::from("permission-decision-1"),
            path: String::from("src/lib.rs"),
            operation: String::from("modify_text_file"),
            review_state: LocalEditReviewState::NeedsUserApproval,
            diff_summary: String::from("-old\n+new\n"),
            diff_summary_truncated: false,
        },
    };

    let line = preview.to_jsonl().expect("encode preview");
    let decoded = ServerEvent::from_jsonl(&line).expect("decode preview");
    assert_eq!(decoded, preview);
    assert!(line.contains("\"type\":\"local_edit_preview_ready\""));
}

#[test]
fn ui_handshake_exposes_local_edit_capability() {
    let handshake = default_ui_handshake();

    assert!(handshake.supports(Capability::LocalEdit));
}
```

Add the new protocol types to the test module imports:

```rust
LocalEditDecision, LocalEditFinishedOutcome, LocalEditOperationInput, LocalEditPreviewSummary,
LocalEditReviewState,
```

- [ ] **Step 2: Run the failing protocol tests**

Run:

```bash
just dev cargo test -p yach-proto local_edit -- --nocapture
```

Expected: FAIL because the new protocol types and events do not exist.

- [ ] **Step 3: Add local edit DTOs and events**

In `crates/yach-proto/src/lib.rs`, add `LocalEdit` to `Capability`:

```rust
    LocalEdit,
```

Add these protocol DTOs near the dialog DTOs:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalEditOperationInput {
    ModifyTextFile {
        path: String,
        expected_sha256: String,
        find: String,
        replace: String,
    },
    CreateTextFile {
        path: String,
        content: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEditDecision {
    Apply,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEditReviewState {
    Allowed,
    NeedsUserApproval,
    AutoReviewUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalEditPreviewSummary {
    pub preview_id: String,
    pub transaction_id: String,
    pub permission_decision_id: String,
    pub path: String,
    pub operation: String,
    pub review_state: LocalEditReviewState,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEditFinishedOutcome {
    Applied,
    Rejected,
    Denied,
    Failed,
}
```

Add these variants to `ClientEvent`:

```rust
    LocalEditPrepareRequested {
        request_id: String,
        operation: LocalEditOperationInput,
    },
    LocalEditDecisionSubmitted {
        preview_id: String,
        permission_decision_id: String,
        decision: LocalEditDecision,
    },
```

Add these variants to `ServerEvent`:

```rust
    LocalEditPreviewReady {
        request_id: String,
        preview: LocalEditPreviewSummary,
    },
    LocalEditFinished {
        preview_id: Option<String>,
        outcome: LocalEditFinishedOutcome,
        message: String,
    },
```

Add `Capability::LocalEdit` to `default_ui_handshake()`. Do not add it to `default_rpc_handshake()` unless a later CLI/RPC client implements it.

Update `crates/yach-ui/src/lib.rs` in the `UiCapabilities::supports` match so the new enum variant is exhaustive and the alpha UI can negotiate the feature:

```rust
            Capability::LocalEdit => true,
```

Add this assertion to `alpha_profile_matches_proto_handshake`:

```rust
        assert!(capabilities.supports(Capability::LocalEdit));
        assert!(handshake.supports(Capability::LocalEdit));
```

- [ ] **Step 4: Run protocol tests**

Run:

```bash
just dev cargo test -p yach-proto local_edit -- --nocapture
just dev cargo test -p yach-proto ui_handshake -- --nocapture
just dev cargo test -p yach-ui alpha_profile_matches_proto_handshake -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git add crates/yach-proto/src/lib.rs crates/yach-ui/src/lib.rs
git commit -m "feat: add local edit protocol events"
```

---

### Task 4: Native Backend Runner Wiring

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write failing backend runner tests**

Add these tests to `crates/yach-backend/src/native_runner.rs` inside the existing `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn native_runner_prepares_and_applies_local_edit() {
    use tokio::sync::mpsc;
    use yach_proto::{
        BackendEvent, ClientEvent, LocalEditDecision, LocalEditFinishedOutcome,
        LocalEditOperationInput, ServerEvent,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let session_path = temp.path().join("session.jsonl");
    std::fs::write(temp.path().join("file.txt"), "hello\n").expect("write fixture");

    let (client_tx, client_rx) = mpsc::unbounded_channel();
    let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_native_dogfood_loop(
        client_rx,
        backend_tx,
        NativeDogfoodRunnerConfig {
            session_path,
            provider: None,
            project_root: Some(temp.path().to_path_buf()),
        },
    ));

    client_tx
        .send(ClientEvent::LocalEditPrepareRequested {
            request_id: String::from("local-edit-request-1"),
            operation: LocalEditOperationInput::ModifyTextFile {
                path: String::from("file.txt"),
                expected_sha256: crate::sha256_hex_for_test("hello\n"),
                find: String::from("hello"),
                replace: String::from("goodbye"),
            },
        })
        .expect("send prepare");

    let preview = loop {
        match backend_rx.recv().await.expect("backend event") {
            BackendEvent::Server(ServerEvent::LocalEditPreviewReady { preview, .. }) => {
                break preview;
            }
            _ => {}
        }
    };

    client_tx
        .send(ClientEvent::LocalEditDecisionSubmitted {
            preview_id: preview.preview_id.clone(),
            permission_decision_id: preview.permission_decision_id.clone(),
            decision: LocalEditDecision::Apply,
        })
        .expect("send apply");

    let outcome = loop {
        match backend_rx.recv().await.expect("backend event") {
            BackendEvent::Server(ServerEvent::LocalEditFinished { outcome, .. }) => {
                break outcome;
            }
            _ => {}
        }
    };

    assert_eq!(outcome, LocalEditFinishedOutcome::Applied);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("file.txt")).unwrap(),
        "goodbye\n"
    );

    drop(client_tx);
    handle.await.expect("runner exits");
}

#[tokio::test]
async fn native_runner_rejects_stale_local_edit_decision() {
    use tokio::sync::mpsc;
    use yach_proto::{
        BackendEvent, ClientEvent, LocalEditDecision, LocalEditFinishedOutcome, ServerEvent,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let session_path = temp.path().join("session.jsonl");
    let (client_tx, client_rx) = mpsc::unbounded_channel();
    let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_native_dogfood_loop(
        client_rx,
        backend_tx,
        NativeDogfoodRunnerConfig {
            session_path,
            provider: None,
            project_root: Some(temp.path().to_path_buf()),
        },
    ));

    client_tx
        .send(ClientEvent::LocalEditDecisionSubmitted {
            preview_id: String::from("missing"),
            permission_decision_id: String::from("permission-decision-missing"),
            decision: LocalEditDecision::Apply,
        })
        .expect("send stale decision");

    let outcome = loop {
        match backend_rx.recv().await.expect("backend event") {
            BackendEvent::Server(ServerEvent::LocalEditFinished { outcome, .. }) => {
                break outcome;
            }
            _ => {}
        }
    };

    assert_eq!(outcome, LocalEditFinishedOutcome::Failed);

    drop(client_tx);
    handle.await.expect("runner exits");
}
```

- [ ] **Step 2: Run failing runner tests**

Run:

```bash
just dev cargo test -p yach-backend native_runner_prepares_and_applies_local_edit -- --nocapture
just dev cargo test -p yach-backend native_runner_rejects_stale_local_edit_decision -- --nocapture
```

Expected: FAIL because the runner does not handle the new events.

- [ ] **Step 3: Add runner state and imports**

In `crates/yach-backend/src/native_runner.rs`, extend the `yach_proto` import:

```rust
    LocalEditDecision, LocalEditFinishedOutcome, LocalEditOperationInput,
    LocalEditPreviewSummary, LocalEditReviewState,
```

Extend the crate import:

```rust
    NativeEditAccess, NativeEditAccessContext, NativeEditAccessError,
    NativeEditAccessReviewState, NativeEditError, NativeEditHunk, NativeEditOperation,
    NativeEditPolicy, NativeEditPreviewId, NativeEditTransactionRequest, NativePermissionDecisionId,
    NativePermissionPolicy,
```

Add the project root field to `NativeDogfoodRunnerConfig`:

```rust
pub struct NativeDogfoodRunnerConfig {
    pub session_path: PathBuf,
    pub provider: Option<NativeProviderDogfoodConfig>,
    pub project_root: Option<PathBuf>,
}
```

Inside `run_native_dogfood_loop`, create the facade before the loop:

```rust
    let NativeDogfoodRunnerConfig {
        session_path,
        provider,
        project_root,
    } = config;
    let project_root = project_root
        .or_else(|| std::env::current_dir().ok())
        .and_then(|root| NativeResourceRoot::project(root).ok());
    let store = NativeJsonlSessionStore::new(session_path.clone());
    send_native_initial_state(&tx, &session_path, provider.as_ref());
    let mut edit_access = NativeEditAccess::default();
    let mut local_edit_index = 0_u64;
```

Remove the original destructuring and `store` initialization that this snippet replaces. Also add `project_root: Option<PathBuf>` to `NativeDogfoodRunnerConfig`, and set `project_root: None` in existing test/config literals that are not exercising local edit. The runner defaults to the process working directory when no explicit root is provided.

- [ ] **Step 4: Advertise local edit capability**

In `send_native_initial_state`, add `Capability::LocalEdit` to the native backend handshake:

```rust
vec![
    Capability::PromptStreaming,
    Capability::PromptCancellation,
    Capability::LocalEdit,
],
```

- [ ] **Step 5: Handle prepare and decision events**

Add these match arms in `run_native_dogfood_loop` before the ignored event arm:

```rust
            ClientEvent::LocalEditPrepareRequested {
                request_id,
                operation,
            } => {
                local_edit_index = local_edit_index.saturating_add(1);
                handle_local_edit_prepare(
                    &tx,
                    &store,
                    project_root.as_ref(),
                    &mut edit_access,
                    request_id,
                    operation,
                    local_edit_index,
                );
            }
            ClientEvent::LocalEditDecisionSubmitted {
                preview_id,
                permission_decision_id,
                decision,
            } => {
                handle_local_edit_decision(
                    &tx,
                    &store,
                    &mut edit_access,
                    preview_id,
                    permission_decision_id,
                    decision,
                );
            }
```

Add the helper functions near `handle_native_prompt`:

```rust
fn handle_local_edit_prepare(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    project_root: Option<&NativeResourceRoot>,
    edit_access: &mut NativeEditAccess,
    request_id: String,
    operation: LocalEditOperationInput,
    local_edit_index: u64,
) {
    let Some(root) = project_root else {
        send_local_edit_finished(
            tx,
            None,
            LocalEditFinishedOutcome::Failed,
            "local edit unavailable: project root not available",
        );
        return;
    };
    let mut log = store.load().unwrap_or_default();
    let context = NativeEditAccessContext {
        session_id: NativeSessionId(String::from("default")),
        turn_id: NativeTurnId(format!("local-edit-{local_edit_index}")),
        permission_policy: NativePermissionPolicy::default_local_edit(),
        edit_policy: NativeEditPolicy::conservative(),
    };
    let path = local_edit_operation_path(&operation);
    let operation_label = local_edit_operation_label(&operation);
    let request = native_edit_request_from_proto(operation);
    match edit_access.prepare(root, request, context, &mut log) {
        Ok(preview) => {
            if let Err(error) = log.write_to_file(store.path()) {
                send_local_edit_finished(
                    tx,
                    Some(preview.preview_id.0),
                    LocalEditFinishedOutcome::Failed,
                    &format!("local edit evidence persist failed: {error}"),
                );
                return;
            }
            let review_state = proto_review_state(preview.review_state);
            let summary = LocalEditPreviewSummary {
                preview_id: preview.preview_id.0,
                transaction_id: preview.transaction_id.0,
                permission_decision_id: preview.permission_decision_id.0,
                path,
                operation: operation_label,
                review_state,
                diff_summary: preview.diff_summary,
                diff_summary_truncated: preview.diff_summary_truncated,
            };
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditPreviewReady {
                request_id,
                preview: summary,
            }));
        }
        Err(NativeEditAccessError::PermissionDenied { reason }) => {
            let _ = log.write_to_file(store.path());
            send_local_edit_finished(tx, None, LocalEditFinishedOutcome::Denied, &reason);
        }
        Err(error) => {
            let _ = log.write_to_file(store.path());
            send_local_edit_finished(
                tx,
                None,
                LocalEditFinishedOutcome::Failed,
                &format!("local edit preview failed: {}", local_edit_access_error_label(&error)),
            );
        }
    }
}

fn handle_local_edit_decision(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    edit_access: &mut NativeEditAccess,
    preview_id: String,
    permission_decision_id: String,
    decision: LocalEditDecision,
) {
    let mut log = store.load().unwrap_or_default();
    let preview_id = NativeEditPreviewId(preview_id);
    let decision_id = NativePermissionDecisionId(permission_decision_id);
    let result = match decision {
        LocalEditDecision::Apply => edit_access
            .apply(&preview_id, decision_id, &mut log)
            .map(|_| LocalEditFinishedOutcome::Applied),
        LocalEditDecision::Reject => edit_access
            .reject(&preview_id, decision_id, &mut log)
            .map(|()| LocalEditFinishedOutcome::Rejected),
    };
    match result {
        Ok(outcome) => {
            if let Err(error) = log.write_to_file(store.path()) {
                send_local_edit_finished(
                    tx,
                    Some(preview_id.0),
                    LocalEditFinishedOutcome::Failed,
                    &format!("local edit evidence persist failed: {error}"),
                );
            } else {
                send_local_edit_finished(tx, Some(preview_id.0), outcome, "local edit finished");
            }
        }
        Err(error) => {
            let _ = log.write_to_file(store.path());
            send_local_edit_finished(
                tx,
                Some(preview_id.0),
                LocalEditFinishedOutcome::Failed,
                &format!("local edit decision failed: {}", local_edit_access_error_label(&error)),
            );
        }
    }
}
```

Add these conversion helpers:

```rust
fn native_edit_request_from_proto(operation: LocalEditOperationInput) -> NativeEditTransactionRequest {
    let operation = match operation {
        LocalEditOperationInput::ModifyTextFile {
            path,
            expected_sha256,
            find,
            replace,
        } => NativeEditOperation::ModifyTextFile {
            path,
            expected_sha256,
            hunks: vec![NativeEditHunk { find, replace }],
        },
        LocalEditOperationInput::CreateTextFile { path, content } => {
            NativeEditOperation::CreateTextFile { path, content }
        }
    };
    NativeEditTransactionRequest {
        operations: vec![operation],
    }
}

fn local_edit_operation_path(operation: &LocalEditOperationInput) -> String {
    match operation {
        LocalEditOperationInput::ModifyTextFile { path, .. }
        | LocalEditOperationInput::CreateTextFile { path, .. } => path.clone(),
    }
}

fn local_edit_operation_label(operation: &LocalEditOperationInput) -> String {
    match operation {
        LocalEditOperationInput::ModifyTextFile { .. } => String::from("modify_text_file"),
        LocalEditOperationInput::CreateTextFile { .. } => String::from("create_text_file"),
    }
}

fn proto_review_state(state: NativeEditAccessReviewState) -> LocalEditReviewState {
    match state {
        NativeEditAccessReviewState::Allowed => LocalEditReviewState::Allowed,
        NativeEditAccessReviewState::NeedsUserApproval => LocalEditReviewState::NeedsUserApproval,
        NativeEditAccessReviewState::AutoReviewUnavailable => {
            LocalEditReviewState::AutoReviewUnavailable
        }
    }
}

fn send_local_edit_finished(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    preview_id: Option<String>,
    outcome: LocalEditFinishedOutcome,
    message: &str,
) {
    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
        preview_id,
        outcome,
        message: message.to_owned(),
    }));
}

fn local_edit_access_error_label(error: &NativeEditAccessError) -> &'static str {
    match error {
        NativeEditAccessError::PermissionDenied { .. } => "permission_denied",
        NativeEditAccessError::Preview(NativeEditError::HashMismatch { .. })
        | NativeEditAccessError::Apply(NativeEditError::HashMismatch { .. }) => "hash_mismatch",
        NativeEditAccessError::Preview(NativeEditError::HunkNotFound { .. })
        | NativeEditAccessError::Apply(NativeEditError::HunkNotFound { .. }) => "hunk_not_found",
        NativeEditAccessError::Preview(_) => "preview_failed",
        NativeEditAccessError::Apply(_) => "apply_failed",
        NativeEditAccessError::PreviewNotFound => "preview_not_found",
        NativeEditAccessError::DecisionMismatch => "decision_mismatch",
    }
}
```

Update ignored event matches to include no local edit variants because they are handled explicitly.

- [ ] **Step 6: Keep provider-visible tools unchanged**

Add this assertion to the existing provider advertising test that validates native provider requests with read-only tools:

```rust
assert!(
    advertising
        .tools
        .iter()
        .all(|tool| tool.name != "edit" && tool.name != "write")
);
```

Use the nearest existing test that inspects `advertising.tools`; do not add a new provider tool.

- [ ] **Step 7: Run backend runner tests**

Run:

```bash
just dev cargo test -p yach-backend native_runner_prepares_and_applies_local_edit -- --nocapture
just dev cargo test -p yach-backend native_runner_rejects_stale_local_edit_decision -- --nocapture
just dev cargo test -p yach-backend provider_tool_advertising -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit Task 4**

Run:

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "feat: wire native local edit protocol"
```

---

### Task 5: TUI Local Edit Flow

**Files:**
- Modify: `crates/yach-ui/src/slash_commands.rs`
- Modify: `crates/yach-ui/src/app.rs`

This first TUI flow is intentionally narrow and explicit:

- `/edit` starts a local edit composer.
- The composer asks for operation kind, path, and content/find/replace through local TUI state.
- The backend returns a preview with a truncated diff summary.
- The TUI review surface supports apply/reject.
- No extension or provider can initiate this path in this task.

- [ ] **Step 1: Add failing slash command tests**

Modify `crates/yach-ui/src/slash_commands.rs` tests:

```rust
#[test]
fn completion_includes_executable_commands() {
    let matches = match_slash_commands("/");
    let names = matches.iter().map(|cmd| cmd.name).collect::<Vec<_>>();

    for expected in [
        "/quit",
        "/exit",
        "/clear",
        "/model",
        "/session",
        "/fork",
        "/thinking",
        "/perf",
        "/edit",
        "/help",
    ] {
        assert!(names.contains(&expected));
    }
}
```

Add:

```rust
#[test]
fn parser_accepts_edit_command() {
    assert_eq!(
        parse_slash_command("/edit"),
        SlashParseResult::Command(SlashAction::Edit)
    );
}
```

- [ ] **Step 2: Implement `/edit` parsing**

Add `Edit` to `SlashAction`:

```rust
    Edit,
```

Add this command before `/help` in `SLASH_COMMANDS`:

```rust
    SlashCommand {
        name: "/edit",
        description: "Edit a local file",
        action: SlashAction::Edit,
    },
```

Run:

```bash
just dev cargo test -p yach-ui slash_commands -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Add TUI state tests**

Add these tests to `crates/yach-ui/src/app.rs` inside the existing test module:

```rust
fn local_edit_connected_event() -> BackendEvent {
    BackendEvent::Connected {
        negotiated: NegotiatedCapabilities::from_handshakes(
            &default_ui_handshake(),
            &Handshake::new("yach-native-dogfood", vec![Capability::LocalEdit]),
        ),
    }
}

#[test]
fn edit_command_requires_backend_capability() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx);
    app.handle_backend_event(connected_event());
    app.set_prompt_text("/edit");

    app.submit_input();

    assert_eq!(app.status_message, "local edit unavailable");
}

#[test]
fn edit_command_opens_composer_when_supported() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx);
    app.handle_backend_event(local_edit_connected_event());
    app.set_prompt_text("/edit");

    app.submit_input();

    assert!(matches!(
        app.mode,
        AppMode::LocalEditCompose {
            step: LocalEditComposeStep::Kind,
            ..
        }
    ));
}

#[test]
fn local_edit_preview_enters_review_mode() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx);
    app.handle_backend_event(local_edit_connected_event());

    app.handle_server_event(ServerEvent::LocalEditPreviewReady {
        request_id: String::from("local-edit-request-1"),
        preview: yach_proto::LocalEditPreviewSummary {
            preview_id: String::from("edit-preview-1"),
            transaction_id: String::from("edit-transaction-1"),
            permission_decision_id: String::from("permission-decision-1"),
            path: String::from("src/lib.rs"),
            operation: String::from("modify_text_file"),
            review_state: yach_proto::LocalEditReviewState::NeedsUserApproval,
            diff_summary: String::from("-old\n+new\n"),
            diff_summary_truncated: false,
        },
    });

    assert!(matches!(app.mode, AppMode::LocalEditReview { .. }));
    assert_eq!(app.status_message, "review local edit");
}

#[test]
fn local_edit_auto_review_unavailable_is_visible() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx);
    app.handle_backend_event(local_edit_connected_event());

    app.handle_server_event(ServerEvent::LocalEditPreviewReady {
        request_id: String::from("local-edit-request-1"),
        preview: yach_proto::LocalEditPreviewSummary {
            preview_id: String::from("edit-preview-1"),
            transaction_id: String::from("edit-transaction-1"),
            permission_decision_id: String::from("permission-decision-1"),
            path: String::from("src/lib.rs"),
            operation: String::from("modify_text_file"),
            review_state: yach_proto::LocalEditReviewState::AutoReviewUnavailable,
            diff_summary: String::from("-old\n+new\n"),
            diff_summary_truncated: false,
        },
    });

    assert!(matches!(app.mode, AppMode::LocalEditReview { .. }));
    assert_eq!(
        app.status_message,
        "auto-review unavailable; user approval required"
    );
}

#[test]
fn local_edit_finished_returns_to_normal_mode() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx);
    app.handle_backend_event(local_edit_connected_event());
    app.mode = AppMode::LocalEditReview {
        preview: LocalEditReview {
            preview_id: String::from("edit-preview-1"),
            permission_decision_id: String::from("permission-decision-1"),
            path: String::from("src/lib.rs"),
            operation: String::from("modify_text_file"),
            review_state: yach_proto::LocalEditReviewState::NeedsUserApproval,
            diff_summary: String::from("-old\n+new\n"),
            diff_summary_truncated: false,
        },
        selected: LocalEditReviewAction::Apply,
    };

    app.handle_server_event(ServerEvent::LocalEditFinished {
        preview_id: Some(String::from("edit-preview-1")),
        outcome: yach_proto::LocalEditFinishedOutcome::Applied,
        message: String::from("local edit finished"),
    });

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(app.status_message, "local edit finished");
}
```

- [ ] **Step 4: Add compose/review state types**

In `crates/yach-ui/src/app.rs`, extend `AppMode`:

```rust
    LocalEditCompose {
        step: LocalEditComposeStep,
        draft: LocalEditDraft,
    },
    LocalEditReview {
        preview: LocalEditReview,
        selected: LocalEditReviewAction,
    },
```

Add these types near `PendingDialog`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalEditComposeStep {
    Kind,
    Path,
    ExpectedSha256,
    Find,
    Replace,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalEditOperationKind {
    Modify,
    Create,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LocalEditDraft {
    kind: Option<LocalEditOperationKind>,
    path: String,
    expected_sha256: String,
    find: String,
    replace: String,
    content: String,
    buffer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalEditReview {
    preview_id: String,
    permission_decision_id: String,
    path: String,
    operation: String,
    review_state: LocalEditReviewState,
    diff_summary: String,
    diff_summary_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalEditReviewAction {
    Apply,
    Reject,
}
```

- [ ] **Step 5: Open composer from `/edit`**

In `submit_input`, add an arm before `/help`:

```rust
            SlashParseResult::Command(SlashAction::Edit) => {
                self.clear_input();
                self.open_local_edit_composer();
                return;
            }
```

Add:

```rust
    fn open_local_edit_composer(&mut self) {
        if self.backend_busy() {
            self.status_message = String::from("wait for current response before editing");
            return;
        }
        if !self.supports(Capability::LocalEdit) {
            self.status_message = String::from("local edit unavailable");
            return;
        }
        self.mode = AppMode::LocalEditCompose {
            step: LocalEditComposeStep::Kind,
            draft: LocalEditDraft::default(),
        };
        self.status_message = String::from("choose edit kind");
    }
```

- [ ] **Step 6: Handle local edit server events**

In `handle_server_event`, add:

```rust
            ServerEvent::LocalEditPreviewReady { preview, .. } => {
                let status_message = match preview.review_state {
                    LocalEditReviewState::Allowed => "local edit pre-approved",
                    LocalEditReviewState::NeedsUserApproval => "review local edit",
                    LocalEditReviewState::AutoReviewUnavailable => {
                        "auto-review unavailable; user approval required"
                    }
                };
                self.mode = AppMode::LocalEditReview {
                    preview: LocalEditReview {
                        preview_id: preview.preview_id,
                        permission_decision_id: preview.permission_decision_id,
                        path: preview.path,
                        operation: preview.operation,
                        review_state: preview.review_state,
                        diff_summary: preview.diff_summary,
                        diff_summary_truncated: preview.diff_summary_truncated,
                    },
                    selected: LocalEditReviewAction::Apply,
                };
                self.status_message = String::from(status_message);
            }
            ServerEvent::LocalEditFinished {
                outcome, message, ..
            } => {
                self.mode = AppMode::Normal;
                self.status_message = if message.is_empty() {
                    format!("local edit {outcome:?}")
                } else {
                    message
                };
            }
```

- [ ] **Step 7: Add key handling for compose and review**

Extend the mode dispatcher where key handlers are called:

```rust
            AppMode::LocalEditCompose { .. } => self.handle_local_edit_compose_key(key, modifiers),
            AppMode::LocalEditReview { .. } => self.handle_local_edit_review_key(key, modifiers),
```

Add:

```rust
    fn handle_local_edit_compose_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        if matches!(key, KeyCode::Enter) {
            self.advance_local_edit_compose();
            return;
        }
        let AppMode::LocalEditCompose { step, draft } = &mut self.mode else {
            return;
        };
        match key {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
                self.status_message = String::from("local edit cancelled");
            }
            KeyCode::Char('1') if *step == LocalEditComposeStep::Kind => {
                draft.kind = Some(LocalEditOperationKind::Modify);
                *step = LocalEditComposeStep::Path;
                draft.buffer.clear();
                self.status_message = String::from("enter path");
            }
            KeyCode::Char('2') if *step == LocalEditComposeStep::Kind => {
                draft.kind = Some(LocalEditOperationKind::Create);
                *step = LocalEditComposeStep::Path;
                draft.buffer.clear();
                self.status_message = String::from("enter path");
            }
            KeyCode::Backspace => {
                draft.buffer.pop();
            }
            KeyCode::Char(ch) => {
                draft.buffer.push(ch);
            }
            _ => {}
        }
    }

    fn handle_local_edit_review_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        match key {
            KeyCode::Esc | KeyCode::Char('r') => self.submit_local_edit_review(LocalEditDecision::Reject),
            KeyCode::Enter | KeyCode::Char('a') => {
                self.submit_local_edit_review(LocalEditDecision::Apply);
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                if let AppMode::LocalEditReview { selected, .. } = &mut self.mode {
                    *selected = match selected {
                        LocalEditReviewAction::Apply => LocalEditReviewAction::Reject,
                        LocalEditReviewAction::Reject => LocalEditReviewAction::Apply,
                    };
                }
            }
            _ => {}
        }
    }
```

Import these protocol types from `yach_proto`:

```rust
LocalEditDecision, LocalEditOperationInput, LocalEditReviewState,
```

- [ ] **Step 8: Add compose advancement and event submission**

Add:

```rust
    fn advance_local_edit_compose(&mut self) {
        let mut should_submit = false;
        {
            let AppMode::LocalEditCompose { step, draft } = &mut self.mode else {
                return;
            };
            match step {
                LocalEditComposeStep::Kind => {
                    self.status_message = String::from("choose 1 modify or 2 create");
                }
                LocalEditComposeStep::Path => {
                    draft.path = draft.buffer.trim().to_owned();
                    draft.buffer.clear();
                    if draft.path.is_empty() {
                        self.status_message = String::from("path required");
                    } else if matches!(draft.kind, Some(LocalEditOperationKind::Modify)) {
                        *step = LocalEditComposeStep::ExpectedSha256;
                        self.status_message = String::from("enter expected sha256");
                    } else {
                        *step = LocalEditComposeStep::Content;
                        self.status_message = String::from("enter file content");
                    }
                }
                LocalEditComposeStep::ExpectedSha256 => {
                    draft.expected_sha256 = draft.buffer.trim().to_owned();
                    draft.buffer.clear();
                    *step = LocalEditComposeStep::Find;
                    self.status_message = String::from("enter text to find");
                }
                LocalEditComposeStep::Find => {
                    draft.find = draft.buffer.clone();
                    draft.buffer.clear();
                    *step = LocalEditComposeStep::Replace;
                    self.status_message = String::from("enter replacement text");
                }
                LocalEditComposeStep::Replace => {
                    draft.replace = draft.buffer.clone();
                    draft.buffer.clear();
                    should_submit = true;
                }
                LocalEditComposeStep::Content => {
                    draft.content = draft.buffer.clone();
                    draft.buffer.clear();
                    should_submit = true;
                }
            }
        }
        if should_submit {
            self.submit_local_edit_prepare();
        }
    }

    fn submit_local_edit_prepare(&mut self) {
        let draft = match &self.mode {
            AppMode::LocalEditCompose { draft, .. } => draft.clone(),
            _ => return,
        };
        let Some(kind) = draft.kind.clone() else {
            self.status_message = String::from("choose edit kind");
            return;
        };
        let operation = match kind {
            LocalEditOperationKind::Modify => LocalEditOperationInput::ModifyTextFile {
                path: draft.path.clone(),
                expected_sha256: draft.expected_sha256.clone(),
                find: draft.find.clone(),
                replace: draft.replace.clone(),
            },
            LocalEditOperationKind::Create => LocalEditOperationInput::CreateTextFile {
                path: draft.path.clone(),
                content: draft.content.clone(),
            },
        };
        let request_id = format!("local-edit-request-{}", self.local_edit_request_counter);
        self.local_edit_request_counter = self.local_edit_request_counter.saturating_add(1);
        if self.send_client_event(ClientEvent::LocalEditPrepareRequested {
            request_id,
            operation,
        }) {
            self.mode = AppMode::Normal;
            self.status_message = String::from("preparing local edit");
        }
    }

    fn submit_local_edit_review(&mut self, decision: LocalEditDecision) {
        let (preview_id, permission_decision_id) = match &self.mode {
            AppMode::LocalEditReview { preview, .. } => (
                preview.preview_id.clone(),
                preview.permission_decision_id.clone(),
            ),
            _ => return,
        };
        if self.send_client_event(ClientEvent::LocalEditDecisionSubmitted {
            preview_id,
            permission_decision_id,
            decision,
        }) {
            self.status_message = String::from("submitting local edit decision");
        }
    }
```

Add a `local_edit_request_counter: u64` field to `App` and initialize it to `0` in constructors/test helpers.

- [ ] **Step 9: Render compose/review overlays**

In the render mode match, add:

```rust
                AppMode::LocalEditCompose { step, draft } => {
                    render_local_edit_compose_overlay(frame, step, draft);
                }
                AppMode::LocalEditReview { preview, selected } => {
                    render_local_edit_review_overlay(frame, preview, *selected);
                }
```

Add overlay renderers near `render_dialog_overlay`:

```rust
fn render_local_edit_compose_overlay(
    frame: &mut ratatui::Frame<'_>,
    step: &LocalEditComposeStep,
    draft: &LocalEditDraft,
) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = centered_rect(70, 45, frame.area());
    Clear.render(popup_area, frame.buffer_mut());
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Local edit")
        .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let inner = block.inner(popup_area);
    block.render(popup_area, frame.buffer_mut());

    let prompt = match step {
        LocalEditComposeStep::Kind => "1 modify file, 2 create file",
        LocalEditComposeStep::Path => "Path",
        LocalEditComposeStep::ExpectedSha256 => "Expected sha256",
        LocalEditComposeStep::Find => "Find",
        LocalEditComposeStep::Replace => "Replace",
        LocalEditComposeStep::Content => "Content",
    };
    let lines = vec![
        Line::from(prompt.to_owned()),
        Line::raw(""),
        Line::from(draft.buffer.clone()),
    ];
    Paragraph::new(lines).render(inner, frame.buffer_mut());
}

fn render_local_edit_review_overlay(
    frame: &mut ratatui::Frame<'_>,
    preview: &LocalEditReview,
    selected: LocalEditReviewAction,
) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = centered_rect(80, 70, frame.area());
    Clear.render(popup_area, frame.buffer_mut());
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Review edit")
        .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let inner = block.inner(popup_area);
    block.render(popup_area, frame.buffer_mut());

    let apply_style = if selected == LocalEditReviewAction::Apply {
        Style::new().fg(Color::Black).bg(Color::Green)
    } else {
        Style::new().fg(Color::Green)
    };
    let reject_style = if selected == LocalEditReviewAction::Reject {
        Style::new().fg(Color::Black).bg(Color::Red)
    } else {
        Style::new().fg(Color::Red)
    };
    let mut lines = vec![
        Line::from(format!("{} {}", preview.operation, preview.path)),
        Line::raw(""),
    ];
    match preview.review_state {
        LocalEditReviewState::Allowed => {
            lines.push(Line::from("pre-approved by policy"));
            lines.push(Line::raw(""));
        }
        LocalEditReviewState::AutoReviewUnavailable => {
            lines.push(Line::from("auto-review unavailable; user approval required"));
            lines.push(Line::raw(""));
        }
        LocalEditReviewState::NeedsUserApproval => {}
    }
    lines.push(Line::from(preview.diff_summary.clone()));
    if preview.diff_summary_truncated {
        lines.push(Line::raw(""));
        lines.push(Line::from("diff summary truncated"));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(" Apply ", apply_style),
        Span::raw("  "),
        Span::styled(" Reject ", reject_style),
    ]));
    Paragraph::new(lines).render(inner, frame.buffer_mut());
}
```

Make sure `AppMode::Normal | ...` render fallback excludes the two local edit modes because they are rendered explicitly.

- [ ] **Step 10: Run UI tests**

Run:

```bash
just dev cargo test -p yach-ui edit_command -- --nocapture
just dev cargo test -p yach-ui local_edit -- --nocapture
```

Expected: PASS.

- [ ] **Step 11: Commit Task 5**

Run:

```bash
git add crates/yach-ui/src/app.rs crates/yach-ui/src/slash_commands.rs
git commit -m "feat: add tui local edit review flow"
```

---

### Task 6: Cross-Crate Verification And Cleanup

**Files:**
- Modify only files needed for compile/test cleanup from Tasks 1-5.

- [ ] **Step 1: Run formatting**

Run:

```bash
just fmt
```

Expected: PASS and no manual formatting drift.

- [ ] **Step 2: Run workspace tests**

Run:

```bash
just test
```

Expected: PASS.

- [ ] **Step 3: Run lint**

Run:

```bash
just lint
```

Expected: PASS. If lint fails for large enum variants introduced in proto or app state, box the large field rather than allowing the lint. If lint fails for too many lines/arguments in helper functions, split helpers by conversion or event sending responsibility.

- [ ] **Step 4: Verify provider replay still ignores local edit evidence**

Run:

```bash
just dev cargo test -p yach-backend native_provider_messages_from_log -- --nocapture
just dev cargo test -p yach-backend provider_tool_advertising -- --nocapture
```

Expected: PASS and no test advertising an edit/write provider tool.

- [ ] **Step 5: Verify protocol JSONL compatibility for new events**

Run:

```bash
just dev cargo test -p yach-proto local_edit_events_round_trip_as_jsonl -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit verification fixes**

If Steps 1-5 changed files, commit them:

```bash
git add crates/yach-backend/src crates/yach-proto/src crates/yach-ui/src
git commit -m "test: verify native local edit access"
```

If no files changed, skip this commit.

---

## Execution Notes

- Default local edit permission mode is `Ask`; config file plumbing is out of scope for this implementation.
- `AutoReview` is first-class in enums/evidence but falls back to user review with `auto_review_unavailable_fallback_ask`.
- `Allow` and `Deny` must be covered in backend unit tests even though the initial TUI exposes the default `Ask` path only.
- Extension self-approval denial is represented in the generic permission engine now so future extension tools cannot accidentally bypass core policy.
- The TUI composer is intentionally simple. It is a real working path, not a provider tool and not a hidden built-in write tool.
- The backend runner uses the current process working directory as the native edit root for dogfood. If native launch project context becomes available in runner config before this plan is executed, prefer that configured project root over `std::env::current_dir()`.

## Self-Review

- Spec coverage:
  - Generic permission model: Task 1.
  - Backend-owned local edit facade with pending prepared transactions: Task 2.
  - Protocol local edit events and capability: Task 3.
  - Native runner enforcement, evidence persistence, stale preview failures, provider advertising unchanged: Task 4.
  - TUI first client with compose, preview, apply, reject states: Task 5.
  - Verification across workspace, provider replay, protocol JSONL: Task 6.
- Deferred by design:
  - Config file implementation for permission profiles.
  - Actual auto-review reviewer/subagent runtime.
  - CLI/RPC local edit client.
  - Extension-owned mutation tools.
  - Sandbox/process/network permissions.
