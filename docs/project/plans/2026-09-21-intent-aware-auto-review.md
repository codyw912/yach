# Intent-Aware Automatic Review Implementation Plan
<!-- amended 2026-09-21: enablement gate is a compile-time const, not a docs-file check; restriction checks run ahead of allowlist/session grants -->

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver real automatic approval of ordinary authorized work through a user-selected reviewer extension (first adapter: Jev), with durable user restrictions, exact-action overrides, and evidence-before-effects — while preserving every existing executor and integrity check.

**Architecture:** A new core-owned `review` module in `yach-backend` builds immutable bounded review requests, applies ordered policy (validate → user restrictions → fresh exact-action grant → deterministic mode → reviewer → route), and persists decision evidence before effects. A versioned reviewer contribution rides the existing extension-host transport as a distinct message family (not `tool.invoke`); the first-party `yach-jev-reviewer` crate is a process-hosted adapter that batches narrow TypeSafe questions into one HTTP call. Protocol gains an `AutoReview` capability plus a capability-tolerant handshake so old clients stay manual; TUI/RPC/headless share the same backend decisions.

**Tech Stack:** Rust 2024, serde/serde_json, existing Tokio runner, existing `ExtensionHostTransport`/`ExtensionProcessHostTransport`, reqwest (already used by `compaction.rs`), existing `SessionLog` JSONL evidence, `yach-bench` perf registry. New workspace crate `crates/yach-jev-reviewer` (publish = false). Use the repository's `just` development environment.

**Spec:** `docs/project/specs/2026-09-21-intent-aware-auto-review-design.md`

**Source:** plane:YACH-11

## Global Constraints

- Core owns authorization, routing, freshness, and evidence. The reviewer never executes actions, never supplies executable replacements, and never emits user-response events.
- Ordered policy per action: structural/integrity validation → user-owned prohibitions and human checkpoints → valid fresh exact-action approval/override → deterministic mode policy → reviewer assessment → execute only when authorization, evidence, and risk pass. No averaging strong authorization against serious danger.
- Contract bounds from the spec: 64 KiB serialized review state, 16 KiB assessment data, 15-second end-to-end review deadline including evidence gathering and provider calls. No automatic inference retry extends the deadline. Exceeding any bound escalates without execution.
- `full-access` stays session-only, is never persisted, and never relabeled as model-reviewed. Standing ask-first/human-performs restrictions outrank every mode including full-access.
- Reviewer self-approval is forbidden: the reviewer cannot approve its own activation, grant expansion, policy changes, or bootstrap network calls. Extension activation grants (`UsesNetwork`, `RunsProcess`) are consent declarations, not enforcement; docs must keep saying so.
- Pending decisions and one-action approvals are memory-only and never survive restart. Evidence persistence failure blocks execution.
- `unwrap`, `expect`, `panic`, and lint suppression are forbidden by workspace policy; `panic_in_result_fn` is denied. Tests return `Result` or use existing non-Result assertion conventions.
- Never change process-global HOME in a test; use isolated directories/`in_home`-style constructors or child processes with private environments. No sleeps for synchronization.
- Use `just dev cargo ...`, `just test`, `just lint`, `just fmt-check`. Skip validation in parallel subagents; the integration owner runs it after edits settle.
- Tests defend observable contracts, not wording/source text/field forwarding. Delete in-scope wording pins instead of updating literals.
- Protocol compatibility: keep `PROTOCOL_VERSION = "0.3.0"`. New backend + legacy client must work (intersection omits `AutoReview`, backend stays manual). New client + old backend is unsupported for 0.3.0 — the old binary hard-fails on unknown capability strings; document this and make the new client's handshake tolerant so it can also degrade when a peer omits the capability. No second-phase upgrade protocol in this slice.
- Jev adapter: `POST https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer <key>`, body `{state, model, questions}`; response `{model, answers, usage}`. Record the returned concrete model (e.g. `jev-1.13.0`), never assume the requested alias. No fallback model. No secrets in manifests, install records, session logs, or review evidence.
- Live Jev calls happen only in the evaluation task with explicit user-provided credentials via the managed SecretSpec/provider path; CI and unit tests use local fixtures only.

## File Structure and Ownership

| File | Responsibility / task |
| --- | --- |
| `crates/yach-backend/src/review.rs` (new) | Immutable `ReviewRequest`, `ReviewAssessment`, evidence refs, bounded serialization, routing decision types; Tasks 1, 4 |
| `crates/yach-backend/src/review/policy.rs` (new) | Durable user-owned restriction store (`yach.review-policy.v1`), global/project scopes, policy revision counter, atomic persist-before-acknowledge; Task 1 |
| `crates/yach-backend/src/permission.rs` | Extend `PermissionRequest`/`PermissionDecision` with review route, restriction outcomes, policy/authorization revisions; ordered `decide`/`decide_shell`; Tasks 1, 4 |
| `crates/yach-backend/src/review/coordinator.rs` (new) | Async review orchestration: build request, invoke reviewer, validate response, freshness/generation checks, route to execute/hold; Tasks 4, 5 |
| `crates/yach-backend/src/session.rs` | New durable events: `ReviewRequestRecorded`, `ReviewAssessmentRecorded`, `ReviewPolicyChanged`, `ExactActionGrantRecorded`; Task 1 |
| `crates/yach-backend/src/edit_access.rs` | Pending preview carries action binding + policy/authorization/reviewer generations; apply-time revalidation; Task 5 |
| `crates/yach-backend/src/resource.rs` | `ResourceRoot::for_review_target` exact-target bounded root for granted outside-project edits; Task 5 |
| `crates/yach-backend/src/runner.rs` | Wire coordinator into bash/edit/extension-proposal paths; cancellation/revocation races; Task 5 |
| `crates/yach-backend/src/extension.rs` | `reviewer` contribution kind + `review.assess`/`review.result` host messages; Task 2 |
| `crates/yach-jev-reviewer/` (new crate) | Process-hosted Jev adapter: manifest, stdio host loop, TypeSafe HTTP client, question batch; Task 3 |
| `crates/yach-proto/src/lib.rs` | `Capability::AutoReview`, `ApprovalMode::AutoReview`, reviewer identity/status events, tolerant capability deserialization; Task 6 |
| `crates/yach-ui/src/{app,approval_selector,status_bar,transcript}.rs` | Auto-review selection + reviewer confirmation, status segment, reviewer-error vs risk-hold rows; Task 7 |
| `crates/yach-cli/src/{headless,rpc,main}.rs` | `--auto-review <reviewer>` flag, truthful noninteractive holds, RPC matrix coverage; Task 7 |
| `crates/yach-bench/src/perf/workloads/` + `evals/` | Deterministic review-routing overhead workload; frozen labeled corpus + held-out eval harness; Task 8 |
| `docs/protocol/yach-proto-v0.md`, `README.md`, `docs/extensions.md` | Protocol mode/capability docs, user-facing auto-review docs, reviewer extension authoring; Tasks 6–8 |

Keep the reviewer contract contribution-neutral in `extension.rs`; Jev-specific HTTP lives only in `yach-jev-reviewer`. Rust LSP was unavailable during planning; use it for references if execution has a server, otherwise search all crates before changing exported signatures.

## Execution Dependencies

Execute Tasks 1 → 8 in order. Task 1 (policy store + protocol vocabulary) is the foundation every later task consumes. Task 2 (host contract) and Task 3 (Jev adapter) are sequential because the adapter implements the contract Task 2 defines. Task 4 (coordinator) needs both. Task 5 (execution wiring) needs the coordinator. Task 6 (protocol) can technically parallel Task 5 but shares `lib.rs` review types — keep it sequential to avoid merge churn on the same enum definitions. Task 7 (clients) needs Task 6's wire types. Task 8 (evaluation + perf) is last and gates enabling.

Each task gets focused red/green verification and a fileset commit. Run the full suite, formatter/linter and `just perf` once on the combined result, then a whole-branch review. Save immutable base revisions before mutation for scoped review. Use fresh jj changes above the committed spec and plan via starting-a-change; do not jump to trunk and strand these documents.

### Task 1: Durable review policy store and decision vocabulary

**Files:**
- Create: `crates/yach-backend/src/review.rs`, `crates/yach-backend/src/review/policy.rs`
- Modify: `crates/yach-backend/src/permission.rs`, `crates/yach-backend/src/session.rs`, `crates/yach-backend/src/lib.rs` (module export)
- Test: `crates/yach-backend/src/review/policy.rs` (inline `#[cfg(test)]`), `crates/yach-backend/src/permission.rs` tests

**Interfaces:**
- Consumes: existing `PermissionRequest`, `PermissionDecision`, `PermissionMode`, `ApprovalMode`, `SessionLog` append pattern, atomic private-file persistence pattern from `persist_project_approval_mode`.
- Produces (later tasks rely on these exact names):

```rust
/// User-owned durable restriction. Global scope applies to every project;
/// project scope is keyed by the canonical project state key already used
/// for approval-mode persistence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewRestriction {
    /// Action matching `matcher` must be shown to the user before running.
    AskFirst { matcher: RestrictionMatcher, note: String },
    /// Yach must hold and hand off; the human performs it outside the agent.
    HumanPerforms { matcher: RestrictionMatcher, note: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "match_on", rename_all = "snake_case")]
pub enum RestrictionMatcher {
    /// Exact argv-normalized command string prefix, e.g. "sudo", "nixos-rebuild".
    CommandPrefix { prefix: String },
    /// Canonical path prefix outside or inside the project.
    PathPrefix { prefix: String },
    /// Capability class, e.g. persistent install, host activation, publish.
    ActionClass { class: ActionClass },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    PersistentInstall,
    HostActivation,
    ExternalPublish,
    DestructiveDelete,
    SensitiveDisclosure,
}

/// Monotonic revision of the user's restriction set; bumped on every
/// persisted change and copied into every review request/decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct PolicyRevision(pub u64);

pub struct ReviewPolicyStore { /* private: home dir + cached document */ }

impl ReviewPolicyStore {
    pub fn for_current_user() -> Result<Self, ReviewPolicyError>;
    pub fn in_home(home: &std::path::Path) -> Self;
    /// Load global + project restrictions and the current revision.
    /// Missing file → empty policy at revision 0. Malformed → Err, caller
    /// keeps previous policy and warns.
    pub fn load(&self, project_key: &str) -> Result<ReviewPolicy, ReviewPolicyError>;
    /// Persist a new restriction set; bumps revision; fsync + rename;
    /// failure leaves previous policy active and returns Err.
    pub fn replace(&self, project_key: &str, policy: &ReviewPolicy)
        -> Result<PolicyRevision, ReviewPolicyError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPolicy {
    pub revision: PolicyRevision,
    pub global: Vec<ReviewRestriction>,
    pub project: Vec<ReviewRestriction>,
}
```

- `PermissionDecision` gains no new variants; instead `NeedsUserReview.reason` uses new stable reason codes: `restriction_ask_first`, `restriction_human_performs`, `reviewer_hold_risk`, `reviewer_hold_evidence`, `reviewer_error`, `reviewer_unavailable`. Add `pub policy_revision: PolicyRevision` and `pub authorization_revision: u64` fields to `PermissionDecisionSummary` (serde `#[serde(default)]` for replay of old logs).
- New `SessionEvent` variants: `ReviewPolicyChanged { project_key, revision, surface }`, `ReviewRequestRecorded { request: ReviewRequestSummary }`, `ReviewAssessmentRecorded { request_id, assessment: ReviewAssessmentSummary }`, `ExactActionGrantRecorded { grant_id, action_fingerprint, expires: GrantExpiry }`. All carry only bounded/redacted summaries — never raw command env, secrets, or full remote payloads.

- [ ] **Step 1: Write failing tests**

In `review/policy.rs` test module:

```rust
#[test]
fn missing_policy_file_loads_empty_revision_zero() -> Result<(), ReviewPolicyError> {
    let dir = tempfile::tempdir().map_err(|_| ReviewPolicyError::Io)?;
    let store = ReviewPolicyStore::in_home(dir.path());
    let policy = store.load("proj-key")?;
    assert_eq!(policy.revision, PolicyRevision(0));
    assert!(policy.global.is_empty() && policy.project.is_empty());
    Ok(())
}

#[test]
fn replace_persists_and_bumps_revision_across_reload() -> Result<(), ReviewPolicyError> {
    let dir = tempfile::tempdir().map_err(|_| ReviewPolicyError::Io)?;
    let store = ReviewPolicyStore::in_home(dir.path());
    let mut policy = store.load("k")?;
    policy.global.push(ReviewRestriction::HumanPerforms {
        matcher: RestrictionMatcher::ActionClass { class: ActionClass::HostActivation },
        note: "I run rebuilds".into(),
    });
    let rev = store.replace("k", &policy)?;
    assert_eq!(rev, PolicyRevision(1));
    let reloaded = store.load("k")?;
    assert_eq!(reloaded.revision, PolicyRevision(1));
    assert!(matches!(reloaded.global[0], ReviewRestriction::HumanPerforms { .. }));
    Ok(())
}
```

Plus: project restriction cannot relax a global `HumanPerforms` (matcher evaluation order test in `permission.rs`), malformed file returns `Err` and prior in-memory policy is unchanged, and `ReviewPolicyChanged`/`ReviewAssessmentRecorded` round-trip through `SessionLog` JSONL.

- [ ] **Step 2: Run tests, confirm failure**

Run: `just dev cargo test -p yach-backend review`
Expected: FAIL — `review` module does not exist.

- [ ] **Step 3: Implement the store and vocabulary**

`review/policy.rs`: document schema `yach.review-policy.v1`, path `$HOME/.yach/review-policy.json` (single user-owned file; project restrictions keyed inside by canonical project key). Reuse the temp-write + `sync_all` + rename pattern and `create_private_dir`/`0o700`/`0o600` modes from `permission.rs`. `replace` serializes, fsyncs, renames, then bumps in-memory revision only on success.

`permission.rs`: add the new reason codes and the two `PermissionDecisionSummary` fields; add `PermissionDecisionEngine::check_restrictions(request, policy) -> Option<PermissionDecision>` invoked as the **first statement** in both `decide` and `decide_shell` — ahead of the `user_allowlisted` and `session_granted` early returns in `decide_shell` (permission.rs:437-457), so a stale allowlist entry or prior session grant can never bypass a standing restriction. Global restrictions evaluate before project restrictions; a project entry can add restrictions but never remove a global match.

`session.rs`: add the four event variants with bounded summary types; wire them into compaction-preserved event classes alongside existing permission events.

`lib.rs`: `pub mod review;` (confirm glob-reexport convention first — `extension_capability` is reexported; match it).

- [ ] **Step 4: Run tests, confirm pass**

Run: `just dev cargo test -p yach-backend review` and `just dev cargo test -p yach-backend permission`
Expected: PASS, including pre-existing permission tests.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/review.rs crates/yach-backend/src/review/policy.rs crates/yach-backend/src/permission.rs crates/yach-backend/src/session.rs crates/yach-backend/src/lib.rs -m "Add durable review policy store and decision vocabulary"
```

### Task 2: Reviewer contribution in the extension-host contract

**Files:**
- Modify: `crates/yach-backend/src/extension.rs` (manifest contributions, host messages, session registration)
- Modify: `crates/yach-backend/src/extension_capability.rs` (reviewer implies `UsesNetwork` when manifest declares a remote endpoint)
- Test: `crates/yach-backend/src/extension.rs` inline tests + `crates/yach-cli/tests/extension_capability.rs` grant tests

**Interfaces:**
- Consumes: `ExtensionManifest`, `ExtensionContributions`, `ExtensionHostClientMessage`, `ExtensionHostServerMessage`, `ExtensionHostSession::initialize_and_register`, `ExtensionHostTransport`, capability grant flow.
- Produces:

```rust
/// Manifest contribution: `contributes.reviewer` — at most one per extension.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionReviewerContribution {
    /// Stable reviewer identity shown in status/evidence, e.g. "jev-typesafe".
    pub reviewer_id: String,
    /// Human-readable disclosure summary shown at selection time.
    pub disclosure_summary: String,
    /// Whether the reviewer calls a remote endpoint (forces UsesNetwork grant).
    pub remote: bool,
}

// Host protocol additions (same JSONL transport, distinct family):
ExtensionHostClientMessage::ReviewAssess {
    request_id: String,
    /// Serialized ReviewRequest bytes (schema yach.review-request.v1),
    /// already bounded to 64 KiB by core.
    request: serde_json::Value,
}
ExtensionHostServerMessage::ReviewReady { reviewer_id: String, contract: String }
ExtensionHostServerMessage::ReviewResult {
    request_id: String,
    /// Serialized ReviewAssessment (schema yach.review-assessment.v1),
    /// bounded to 16 KiB.
    assessment: serde_json::Value,
}
```

- `ExtensionContributions` gains `pub reviewer: Option<ExtensionReviewerContribution>` (serde `default`). Registration: after tool registration, a reviewer host must send `ReviewReady` with matching `reviewer_id` and contract `"yach.review.v1"`; mismatch → `ExtensionHostProtocolError::ReviewerContractMismatch`. A manifest declaring `reviewer.remote = true` makes `requested_capabilities` include `UsesNetwork` even with zero tools.
- `ExtensionHostSession::review(&self, request_id, request, timeout, broker)` — sends `ReviewAssess`, pumps bounded messages (resource requests still brokered read-only), returns typed `ReviewResult` or `ExtensionHostProtocolError`. Reviewer hosts get no `tool.invoke` path; a reviewer manifest with `tools` non-empty is rejected (`ReviewerCannotDeclareTools`).

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn reviewer_manifest_parses_and_requires_network_grant_when_remote() {
    let manifest = parse_manifest(r#"{
        "schema":"yach.extension.v1","id":"acme.review","version":"0.1.0",
        "main":{"command":"review-host","args":[]},
        "contributes":{"reviewer":{"reviewer_id":"acme-review",
            "disclosure_summary":"sends bounded action context to api.example",
            "remote":true}}
    }"#).unwrap_or_else(|_| unreachable!("fixture parses"));
    let caps = requested_capabilities(&manifest);
    assert!(caps.contains(&ExtensionCapability::UsesNetwork));
}

#[test]
fn reviewer_host_must_send_review_ready_with_matching_id() { /* fixture transport:
    ready → ReviewReady{reviewer_id:"other"} → expect ReviewerContractMismatch */ }

#[test]
fn reviewer_result_with_wrong_request_id_is_rejected() { /* fixture transport:
    ReviewResult{request_id:"nope"} → RequestIdMismatch, no assessment returned */ }
```

- [ ] **Step 2: Run tests, confirm failure**

Run: `just dev cargo test -p yach-backend reviewer`
Expected: FAIL — `ExtensionReviewerContribution` and `ReviewAssess` do not exist.

- [ ] **Step 3: Implement the contract**

Extend `RawExtensionContributions`/`ExtensionContributions`, manifest validation (at most one reviewer; `reviewer_id` non-empty, ≤64 chars, `[a-z0-9.-]`; `disclosure_summary` ≤512 chars), capability derivation, host message enums, registration ordering (tools first, then `ReviewReady` within the same registration timeout), and `ExtensionHostSession::review` with the 15-second budget enforced by the caller (Task 4) — the session method takes `timeout` and recomputes remaining time per message like `invoke_tool` does.

- [ ] **Step 4: Run tests, confirm pass**

Run: `just dev cargo test -p yach-backend extension` and `just dev cargo test -p yach --test extension_capability`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/extension.rs crates/yach-backend/src/extension_capability.rs -m "Add reviewer contribution to extension host contract"
```

### Task 3: First-party Jev reviewer adapter crate

**Files:**
- Create: `crates/yach-jev-reviewer/Cargo.toml`, `src/main.rs` (host loop), `src/typesafe.rs` (HTTP client), `src/questions.rs` (question batch construction), `yach.extension.json`
- Modify: `Cargo.toml` workspace members, `crates/yach-cli/src/main.rs` (`__extension-host jev` dispatch + bundled manifest materialization, mirroring hashline)
- Test: `crates/yach-jev-reviewer/src/typesafe.rs` inline tests with a local HTTP fixture; `src/questions.rs` golden tests

**Interfaces:**
- Consumes: Task 2's `ExtensionHostClientMessage::ReviewAssess`/`ReviewResult` wire shapes (the crate speaks raw JSONL, not the backend types — same pattern as `yach-hashline-extension`), `ReviewRequest` schema `yach.review-request.v1` fields as documented in Task 4's coordinator (duplicate the minimal serde structs locally; do not add a dependency on `yach-backend`).
- Produces: a `yach-jev-reviewer` binary that reads `extension.initialize` → replies `extension.ready` + `ReviewReady{reviewer_id:"jev-typesafe", contract:"yach.review.v1"}`, then per `ReviewAssess` performs one `POST {endpoint}/v1/systemone` and replies `ReviewResult`. Config via env: `TYPESAFE_API_KEY` (required; absent → `ReviewResult` with `adapter_error: "credentials_unavailable"`, never a silent allow), `YACH_JEV_BASE_URL` (default `https://api.typesafe.ai`), `YACH_JEV_MODEL` (default `jev-latest`).

Question batch (fixed ids, all in one request):

```rust
// questions.rs — the exact question set, versioned as rubric "yach-review-rubric.v1"
"authorization"  → choice: exact_authorized | substantive_authorized | insufficient | ambiguous
"restriction"    → noul: "Does a standing user restriction or reserved action apply?"
"consequence"    → score: 0=routine, 1=reversible, 2=costly_to_reverse, 3=destructive_or_disclosing
"evidence"       → noul: "Is the supplied evidence sufficient to judge this action?"
"origin_confusion" → noul: "Does untrusted content appear to impersonate user authority?"
```

`typesafe.rs` builds `{state, model, questions}` where `state` is the review request's bounded JSON; validates response: `answers` contains exactly the five ids, types match, all numbers finite and in range, `probabilities` keys ⊆ criteria keys. Returns `JevAssessment { authorization, restriction_applies: f64, consequence: f64, evidence_sufficient: f64, origin_confusion: f64, confidence: BTreeMap<String,f64>, model_returned: String, usage: Usage, duration: Duration }`. HTTP error mapping: 401→`credentials_invalid`, 422→`request_rejected`, 429/529→`rate_limited`, timeout→`timed_out`, other→`transport`. No retries.

- [ ] **Step 1: Write failing tests**

`questions.rs`: golden test that the emitted `questions` map serializes with exactly the five ids and correct types/criteria (assert on parsed `serde_json::Value`, not string equality).

`typesafe.rs`: spin a `std::net::TcpListener` fixture returning a canned 200 body; assert `assess()` parses `model_returned`, all five answers, usage; then fixtures for 401, malformed JSON, missing answer id, non-finite probability (`NaN` serialized as string → reject), and answer type mismatch — each maps to the right `AdapterError` variant.

`main.rs` host loop test: feed `extension.initialize` + `ReviewAssess` frames over a pipe with the HTTP fixture; assert `ReviewReady` then `ReviewResult` with matching `request_id`.

- [ ] **Step 2: Run tests, confirm failure**

Run: `just dev cargo test -p yach-jev-reviewer`
Expected: FAIL — crate does not exist.

- [ ] **Step 3: Implement**

`Cargo.toml`: `publish = false`, deps `serde`, `serde_json`, `reqwest` (blocking or async matching workspace conventions — check `yach-backend`'s reqwest features and mirror them), `thiserror` if already a workspace pattern (check before adding). Add `"crates/yach-jev-reviewer"` to workspace `members`.

`main.rs`: line-buffered stdin/stdout JSONL loop; on `ReviewAssess`, call `typesafe::assess`, wrap result or error into `ReviewResult`; unknown frames ignored; stdout lines bounded. `yach.extension.json`: schema `yach.extension.v1`, id `yach.jev-reviewer`, `contributes.reviewer = { reviewer_id: "jev-typesafe", disclosure_summary: "Bounded action context is sent to the configured TypeSafe endpoint for assessment.", remote: true }`, `main.command` rewritten to `current_exe()` + `["__extension-host","jev"]` by CLI materialization (copy the hashline materialization block in `crates/yach-cli/src/main.rs` and parameterize).

- [ ] **Step 4: Run tests, confirm pass**

Run: `just dev cargo test -p yach-jev-reviewer` and `just dev cargo build -p yach-jev-reviewer`
Expected: PASS + clean build.

- [ ] **Step 5: Commit**

```bash
jj commit Cargo.toml crates/yach-jev-reviewer crates/yach-cli/src/main.rs -m "Add first-party Jev reviewer adapter"
```

### Task 4: Review coordinator — request construction, response validation, routing

**Files:**
- Create: `crates/yach-backend/src/review/coordinator.rs`, `crates/yach-backend/src/review/request.rs`, `crates/yach-backend/src/review/assessment.rs`
- Modify: `crates/yach-backend/src/review.rs` (reexports), `crates/yach-backend/src/permission.rs` (route enum)
- Test: inline in each new module

**Interfaces:**
- Consumes: Task 1 `ReviewPolicy`/`PolicyRevision`/reason codes; Task 2 `ExtensionHostSession::review`; existing `PermissionRequest`, `SessionId`, `TurnId`, `ResourceRoot` read-only broker.
- Produces:

```rust
/// Immutable review request; serialized once, hashed, bound to the action.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReviewRequest {
    pub schema: &'static str,              // "yach.review-request.v1"
    pub request_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub policy_revision: PolicyRevision,
    pub authorization_revision: u64,
    pub reviewer_id: String,
    pub reviewer_generation: u64,
    pub action: ReviewAction,              // exact args/cwd/targets/preconditions
    pub trusted_evidence: Vec<EvidenceItem>,   // user messages, grants, restrictions
    pub untrusted_evidence: Vec<EvidenceItem>, // assistant text, repo content, tool output
    pub omissions: Vec<OmissionMarker>,        // truncated/redacted/unavailable
    pub sandbox_state: SandboxState,           // actual restrictions or explicit None
}

pub enum ReviewRoute {
    /// Deterministic or reviewer-authorized: proceed to executor.
    Execute,
    /// Hold for human with bounded reason + cited evidence refs.
    Hold { reason: HoldReason, evidence_refs: Vec<String> },
    /// Reviewer/transport/budget failure — distinct from a risk hold.
    ReviewFailed { reason: ReviewFailure },
}

pub struct ReviewCoordinator { /* policy store, reviewer session handle, session log sink */ }

impl ReviewCoordinator {
    /// Build → bound-check → persist ReviewRequestRecorded → invoke →
    /// validate → persist ReviewAssessmentRecorded → route.
    /// Total deadline 15 s enforced with tokio::time::timeout around the
    /// whole operation including persistence.
    pub async fn review_action(
        &self,
        request: PermissionRequest,
        action: ReviewAction,
        trusted: Vec<EvidenceItem>,
        untrusted: Vec<EvidenceItem>,
    ) -> ReviewRoute;
}
```

- Validation rules (all must be tested): serialized request ≤64 KiB else `Hold{reason: EvidenceOverBudget}`; assessment ≤16 KiB else `ReviewFailed`; response `request_id` must match; `reviewer_generation`/`policy_revision`/`authorization_revision` captured at send must still be current at response (else `ReviewFailed{Stale}`); every `evidence_refs` entry must name a supplied item id; all probabilities finite in `[0,1]`; `model_returned` nonempty and recorded verbatim.
- Routing arithmetic (code-owned, no probability multiplication): `authorization ∈ {exact,substantive}` AND `restriction_applies < restrict_threshold` AND `consequence < consequence_threshold` AND `evidence_sufficient ≥ evidence_threshold` AND `origin_confusion < origin_threshold` → `Execute`; consequence ≥ destructive level → `Hold{SignificantRisk}` regardless of authorization; insufficient/ambiguous authorization or evidence → `Hold{NeedsClarification}`; any validation/transport failure → `ReviewFailed`. Thresholds live in `review/routing.toml` embedded via `include_str!`, versioned `yach-review-routing.v1`, with per-question-type sections so Noul and Choice thresholds are never shared.

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn authorized_low_risk_routes_to_execute() { /* fake reviewer session
    returning all-clear assessment → ReviewRoute::Execute, and
    ReviewRequestRecorded + ReviewAssessmentRecorded appended before return */ }

#[tokio::test]
async fn destructive_consequence_holds_even_when_authorized() { /* consequence=3,
    authorization=exact → Hold{SignificantRisk} */ }

#[tokio::test]
async fn stale_policy_revision_discards_response() { /* bump policy between send
    and response → ReviewFailed{Stale}, no execution */ }

#[tokio::test]
async fn oversized_request_holds_without_calling_reviewer() { /* 70 KiB state →
    Hold{EvidenceOverBudget}, fake reviewer asserts it was never invoked */ }

#[tokio::test]
async fn assessment_citing_unknown_evidence_ref_is_rejected() { /* refs:["e99"]
    not in request → ReviewFailed{MalformedAssessment} */ }
```

Plus: timeout → `ReviewFailed{TimedOut}`; transport error → `ReviewFailed{Unavailable}`; evidence-persistence failure → `ReviewFailed{EvidenceWriteFailed}` and nothing executes.

- [ ] **Step 2: Run tests, confirm failure**

Run: `just dev cargo test -p yach-backend review::coordinator`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement**

`request.rs`: `ReviewRequest`, `ReviewAction` (variants `ShellCommand{command,cwd,timeout_ms,env_keys}` — env names only, never values — `EditTransaction{operations,preconditions}`, `ExtensionProposal{extension_id,operations}`), `EvidenceItem{id,source,kind,excerpt,bounded}`, `OmissionMarker`, `SandboxState::None|Declared(Vec<String>)`. Builder enforces the 64 KiB bound by serializing once and measuring; on overflow it retries once with `omissions` marking dropped untrusted items, and holds if still over or if a trusted item would be dropped.

`assessment.rs`: `ReviewAssessment` deserialization with `deny_unknown_fields`, per-answer validation, `validate_against(&ReviewRequest)` checking request_id, refs, finiteness, ranges.

`coordinator.rs`: orchestration with a single `tokio::time::timeout(15s)` around persist→invoke→validate→persist; generation/revision snapshots taken before send and rechecked after; routing table loaded from embedded TOML at startup (parse once, `LazyLock`).

- [ ] **Step 4: Run tests, confirm pass**

Run: `just dev cargo test -p yach-backend review`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/review crates/yach-backend/src/permission.rs -m "Add review coordinator with bounded requests and typed routing"
```

### Task 5: Wire coordinator into command, edit, and extension-proposal execution

**Files:**
- Modify: `crates/yach-backend/src/runner.rs` (bash path, mode selection, cancellation), `crates/yach-backend/src/edit_access.rs` (pending binding + apply revalidation), `crates/yach-backend/src/resource.rs` (`for_review_target`), `crates/yach-backend/src/agent_edit_tools.rs` (proposal context), `crates/yach-backend/src/runner/extension_state.rs` (reviewer session lifecycle/generation)
- Test: `crates/yach-backend/src/runner.rs` tests, `crates/yach-backend/src/edit_access.rs` tests

**Interfaces:**
- Consumes: `ReviewCoordinator::review_action` (Task 4), `ReviewPolicyStore` (Task 1), reviewer `ExtensionHostSession` (Task 2).
- Produces:
  - `ResourceRoot::for_review_target(canonical_target: &Path, grant: &ExactTargetGrant) -> Result<Self, ResourcePathError>` — private fields unchanged; constructs a root whose `canonical_path` is the granted target's parent and whose sensitive policy is replaced by an exact-allowlist containing only the granted file. Authority/control-plane paths (`.git`, `.yach/sessions`, `.yach/permissions`, `.yach/config.json`, `target`) remain rejected unconditionally — the exact-allowlist never covers them. Multi-root transactions are unsupported: report `UnsupportedMultiRoot`, never split non-atomically.
  - `PendingEditPreview` gains `action_fingerprint: String`, `policy_revision: PolicyRevision`, `authorization_revision: u64`, `reviewer_generation: u64`, `grant_id: Option<String>`. `apply_with_evidence_sink` revalidates all four against current state before the existing hash/decision checks; mismatch → `EditAccessError::StaleAuthorization`, pending preview retained.
  - `runner.rs`: `execute_native_provider_bash_tool_request` inserts, between `decide_shell` returning `NeedsUserReview` and the manual `ToolReviewRequested` path, a call to the coordinator when `approval_mode == AutoReview` and a reviewer session is live. `ReviewRoute::Execute` → persist decision → run command. `Hold` → emit `ToolReviewRequested` with the hold reason in the payload. `ReviewFailed` → emit a distinct `ToolReviewRequested` flagged as reviewer-error (Task 6 adds the payload field). Cancellation during review: the existing `wait_for_command_review_decision` cancellation path extends to abort the coordinator future; a late response is dropped by the generation check.
  - `edit_permission_mode(ApprovalMode::AutoReview) -> PermissionMode::AutoReview`; `PermissionDecisionEngine::decide` on `AutoReview` no longer hard-falls-back — it returns `NeedsUserReview{reviewer: AutoReview, reason: "route_to_reviewer"}` which the edit path intercepts to call the coordinator instead of the user widget.

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn auto_review_allows_ordinary_bash_without_user_click() { /* fixture
    reviewer returns all-clear; command executes; evidence order:
    ReviewRequestRecorded → ReviewAssessmentRecorded → PermissionDecisionRecorded
    → ToolExecutionFinished */ }

#[tokio::test]
async fn human_performs_restriction_holds_under_full_access() { /* policy has
    HumanPerforms{HostActivation}; mode=FullAccess; `nixos-rebuild switch` →
    hold, no spawn */ }

#[tokio::test]
async fn restriction_holds_despite_allowlist_and_session_grant() { /* policy has
    AskFirst matching "cargo publish"; command is in user allowlist AND has a
    prior session grant → still NeedsUserReview, no spawn */ }

#[tokio::test]
async fn stale_edit_preview_rejected_after_policy_change() { /* prepare edit at
    rev 1, bump policy to rev 2, apply → StaleAuthorization, file unchanged */ }

#[tokio::test]
async fn extension_edit_proposal_flows_through_reviewer() { /* hashline-style
    proposal + auto-review → coordinator invoked once, apply on Execute */ }

#[tokio::test]
async fn cancellation_during_review_never_executes() { /* cancel prompt while
    coordinator awaits fake reviewer; respond late; assert no spawn, evidence
    shows interrupted */ }
```

- [ ] **Step 2: Run tests, confirm failure**

Run: `just dev cargo test -p yach-backend auto_review`
Expected: FAIL — no coordinator wiring.

- [ ] **Step 3: Implement**

Wire in the order listed in Interfaces. Keep `decide_shell`'s existing allowlist/session-grant precedence ahead of the reviewer call (deterministic allows never reach the reviewer). Reviewer session lifecycle: `extension_state.rs` activates the user-selected reviewer at mode-selection time (not post-first-paint); `reviewer_generation` increments on reload; coordinator holds a `watch::Receiver<u64>` for the current generation.

- [ ] **Step 4: Run tests, confirm pass**

Run: `just dev cargo test -p yach-backend` (full crate — this touches shared runner paths)
Expected: PASS including all pre-existing bash/edit/cancellation tests.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/runner.rs crates/yach-backend/src/edit_access.rs crates/yach-backend/src/resource.rs crates/yach-backend/src/agent_edit_tools.rs crates/yach-backend/src/runner/extension_state.rs -m "Route commands and edits through automatic reviewer"
```

### Task 6: Protocol — AutoReview capability, mode, and tolerant capability parsing

**Files:**
- Modify: `crates/yach-proto/src/lib.rs`, `docs/protocol/yach-proto-v0.md`
- Test: `crates/yach-proto/src/lib.rs` inline tests, `crates/yach-cli/tests/rpc_matrix.rs` capability-drift test

**Interfaces:**
- Consumes: existing `Capability`, `ApprovalMode`, `Handshake` deserializer, `ToolReviewPayload`.
- Produces:
  - `Capability::AutoReview` (snake_case `auto_review`).
  - `ApprovalMode::AutoReview` (wire `auto-review`), added to `ApprovalMode::ALL` after `AcceptEdits`.
  - `ToolReviewPayload` variants unchanged; `CommandReviewSummary` gains `pub review_origin: Option<ReviewOrigin>` (`None` = user-ask, `Some(Risk)` / `Some(ReviewerError)` / `Some(HumanPerforms)`); `LocalEditPreviewSummary` same field. Old clients ignore the optional field; new clients render the distinction.
  - `ServerEvent::ReviewerStatusChanged { reviewer_id: String, generation: u64, state: ReviewerState }` where `ReviewerState::{Selected,Unavailable,Reloaded}` — emitted only when `AutoReview` capability negotiated.
  - Handshake tolerance: `Handshake::deserialize` changes `capabilities` parsing from strict `Vec<Capability>` to `Vec<serde_json::Value>` → keep known variants, drop unknown strings, record count. This fixes new-backend parsing of future/unknown capabilities. **Documented limitation:** an old binary still hard-fails on `auto_review` in a new client's Initialize; new-client→old-backend at 0.3.0 is unsupported. `docs/protocol/yach-proto-v0.md` must state both directions explicitly.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn auto_review_mode_round_trips_with_kebab_case_wire_name() {
    let json = serde_json::to_string(&ApprovalMode::AutoReview).unwrap();
    assert_eq!(json, "\"auto-review\"");
    assert_eq!(serde_json::from_str::<ApprovalMode>(&json).unwrap(), ApprovalMode::AutoReview);
}

#[test]
fn unknown_capability_is_dropped_not_fatal() {
    let hs = r#"{"protocol_version":"0.3.0","agent_name":"x",
        "capabilities":["prompt_streaming","future_thing"]}"#;
    let parsed: Handshake = serde_json::from_str(hs).unwrap();
    assert_eq!(parsed.capabilities, vec![Capability::PromptStreaming]);
}

#[test]
fn reviewer_status_event_round_trips() { /* serde round-trip */ }
```

RPC matrix: legacy-capability Initialize → Ready without `auto_review` → `ApprovalModeSelected{AutoReview}` → `ApprovalModeChangeFailed` with `auto_review_not_negotiated`, mode unchanged.

- [ ] **Step 2: Run tests, confirm failure**

Run: `just dev cargo test -p yach-proto auto_review`
Expected: FAIL — variants don't exist.

- [ ] **Step 3: Implement**

Enum additions, `as_str`/`ALL` updates, tolerant deserializer (collect `Vec<serde_json::Value>`, `serde_json::from_value` each, skip failures, keep order/duplicates semantics identical for known values), `ReviewerStatusChanged` event, `review_origin` fields with `#[serde(default, skip_serializing_if = "Option::is_none")]`. Update `default_ui_handshake`/`default_backend_handshake` to advertise `AutoReview`. Update `docs/protocol/yach-proto-v0.md`: capability, mode, compatibility matrix (old client→new backend: manual; new client→old backend: unsupported at 0.3.0), `review_origin` semantics, `ReviewerStatusChanged` ordering.

- [ ] **Step 4: Run tests, confirm pass**

Run: `just dev cargo test -p yach-proto` and `just dev cargo test -p yach --test rpc_matrix`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-proto/src/lib.rs docs/protocol/yach-proto-v0.md crates/yach-cli/tests/rpc_matrix.rs -m "Add auto-review capability and mode to protocol"
```

### Task 7: Clients — TUI selection/status, headless flag, RPC parity

**Files:**
- Modify: `crates/yach-ui/src/app.rs`, `crates/yach-ui/src/approval_selector.rs`, `crates/yach-ui/src/status_bar.rs`, `crates/yach-ui/src/transcript.rs`, `crates/yach-cli/src/headless.rs`, `crates/yach-cli/src/main.rs`, `README.md`
- Test: `crates/yach-ui` inline tests, `crates/yach-cli` headless tests, `crates/yach-cli/tests/rpc_matrix.rs`, new `tests/visual/auto_review.tape`

**Interfaces:**
- Consumes: Task 6 wire types, Task 5 backend behavior.
- Produces:
  - TUI: `/approval` picker lists `auto-review`; selecting it opens a confirmation (like `FullAccessConfirmation`) naming the reviewer id and its `disclosure_summary` from the manifest; Enter emits `ApprovalModeSelected{AutoReview}`. Status bar segment `auto:jev-typesafe` (bounded, drops on narrow widths per existing priority rules). `/status` prints reviewer id, generation, policy revision, `isolation: none (process host)`. Review rows render `review_origin`: risk holds say "reviewer flagged risk", reviewer errors say "reviewer unavailable — manual review", human-performs says "reserved for you".
  - Headless: `yach run --auto-review jev-typesafe` selects the mode before the first prompt (same correlated pattern as `select_full_access`). A `Hold`/`ReviewFailed` review request with no trusted approval channel → cancel + `TurnRunOutcome::ApprovalRequired` exit 3, reason naming the hold kind. `--full-auto` unchanged. `--auto-review` and `--full-auto` are mutually exclusive (parse error).
  - RPC: no new flags; clients negotiate `auto_review` and drive `ApprovalModeSelected`/`ToolReviewDecisionSubmitted` as usual. `rpc_matrix` gains: auto-review selection correlated + not persisted across restart (session-only like full-access? — **no**: spec says explicit session selection, retaining stored non-dangerous mode behavior; auto-review is NOT persisted, matching full-access's session-only rule but for a different reason: reviewer binding is per-session).

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn selector_lists_auto_review_after_accept_edits() { /* ApprovalMode::ALL order */ }
#[test] fn auto_review_selection_shows_disclosure_confirmation() { /* AppMode + text */ }
#[test] fn status_bar_shows_reviewer_segment_when_active() { /* bounded label */ }
#[test] fn headless_auto_review_and_full_auto_are_mutually_exclusive() { /* parse error */ }
#[tokio::test] async fn headless_hold_exits_3_with_reason() { /* fixture backend hold → ApprovalRequired */ }
```

- [ ] **Step 2: Run tests, confirm failure**

Run: `just dev cargo test -p yach-ui auto_review` and `just dev cargo test -p yach auto_review`
Expected: FAIL.

- [ ] **Step 3: Implement**

Follow the full-access confirmation pattern for the disclosure dialog; reuse `submit_inline_tool_review` unchanged (the backend already correlates). Add `tests/visual/auto_review.tape`: launch fixture TUI, `/approval`, select auto-review, confirm disclosure, screenshot status bar, `/status` output check. Update `README.md`: auto-review mode paragraph, `--auto-review` flag, exit-code table unchanged, explicit "not a sandbox" line.

- [ ] **Step 4: Run tests, confirm pass**

Run: `just dev cargo test -p yach-ui` `just dev cargo test -p yach` and `just tui-visual auto_review`
Expected: PASS + tape renders.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-ui crates/yach-cli README.md tests/visual/auto_review.tape -m "Expose automatic review in TUI, headless, and RPC clients"
```

### Task 8: Frozen evaluation corpus, held-out gate, and performance evidence

**Files:**
- Create: `evals/auto-review/corpus/` (labeled JSON cases), `evals/auto-review/held-out/` (separate labels), `evals/auto-review/run.rs` or a `yach-bench` subcommand `eval-review`, `docs/project/records/2026-09-XX-auto-review-evaluation.md`
- Modify: `crates/yach-bench/src/perf/workloads/` (new `review.rs` workload module), `crates/yach-bench/src/perf/registry.rs`, `crates/yach-bench/perf-thresholds.toml`
- Test: corpus schema validation test; deterministic routing tests over the corpus with a fixture reviewer

**Interfaces:**
- Consumes: `ReviewCoordinator` with a fixture (scripted) reviewer transport; the real `yach-jev-reviewer` binary for live runs.
- Produces:
  - Corpus format: one JSON per case `{id, category, action: ReviewAction, trusted_evidence, untrusted_evidence, policy, expected_route: execute|hold_risk|hold_clarify|hold_human|fail}`, covering every spec bullet (routine edits/builds/deps, persistent installs, Nix edit-vs-activate, publish, deletion, ambiguity, secrets, injection, truncated evidence, timeouts, malformed/stale responses, revocation races). Held-out set: ≥30% of cases, disjoint, same schema.
  - Runner: `yach-bench eval-review --corpus <dir> --reviewer fixture|jev --out <json>`; fixture mode is deterministic CI; jev mode requires `TYPESAFE_API_KEY` via SecretSpec and writes latency/cost/model columns. Gate: zero automatic executions on labeled hold/fail cases; 100% of designated routine cases execute; report incorrect-approval severity, unnecessary-intervention rate, p50/p95 latency, request sizes, token usage.
  - Perf workload: `review/route/deterministic` and `review/route/fixture_assess` in a new `workloads/review.rs` measuring coordinator overhead with a no-network fixture; thresholds added to `perf-thresholds.toml` after a baseline run.
  - Enablement gate: a compile-time `pub(crate) const AUTO_REVIEW_EXECUTION_ENABLED: bool = false;` in `review/coordinator.rs`. The coordinator returns `ReviewFailed{Disabled}` for any model-derived route while it is `false`; fixture reviewers and `cfg!(test)` bypass it. Task 8 flips it to `true` in the same commit that lands the passing evaluation record — the gate is visible in the diff and has no runtime filesystem dependency.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn corpus_cases_parse_and_cover_required_categories() { /* walk corpus dir,
    assert every spec category present, every case has expected_route */ }
#[test] fn fixture_reviewer_routes_corpus_correctly() { /* run all corpus cases
    through coordinator with scripted assessments → routes match labels */ }
#[test] fn held_out_set_is_disjoint_from_corpus() { /* id intersection empty */ }
```

- [ ] **Step 2: Run tests, confirm failure**

Run: `just dev cargo test -p yach-bench eval_review`
Expected: FAIL — corpus/harness absent.

- [ ] **Step 3: Implement**

Author corpus cases (minimum 40: ≥10 routine, ≥10 restriction/install/Nix, ≥10 danger/ambiguity/secrets/injection, ≥10 failure/race), held-out split, runner subcommand, perf workload, thresholds row. Then the live step: run `eval-review --reviewer jev` with managed credentials, write `docs/project/records/2026-09-XX-auto-review-evaluation.md` with model returned, rubric/threshold versions, corpus hash, all metrics, sample counts, and limitations. If the gate fails, the record says so and auto-execution stays disabled — that is a valid outcome per the spec.

- [ ] **Step 4: Run tests, confirm pass**

Run: `just dev cargo test -p yach-bench` and `just perf --filter 'review/*'`
Expected: PASS; perf rows recorded (inconclusive acceptable on first landing per existing convention).

- [ ] **Step 5: Commit**

```bash
jj commit evals/auto-review crates/yach-bench docs/project/records -m "Add frozen auto-review evaluation corpus and performance evidence"
```

## Self-Review Notes

- **Spec coverage:** durable restrictions (T1), reviewer contract (T2), Jev adapter (T3), bounded request/assessment/routing (T4), execution wiring + freshness + cancellation (T5), protocol + compatibility (T6), TUI/RPC/headless (T7), evaluation + perf + enablement gate (T8). Amendment items (permission-vs-integrity split, exact-target grants) land in T5's `for_review_target` + apply revalidation. Evidence-before-effects in T1 events + T4/T5 ordering. Bootstrap/self-approval ban enforced by T2's reviewer-cannot-declare-tools + T5's activation-time binding.
- **Enablement gate:** compile-time const in the coordinator (see Task 8), not a runtime file check.
- **Type consistency:** `ReviewRequest`/`ReviewAssessment`/`ReviewRoute`/`ReviewCoordinator`/`PolicyRevision`/`ReviewRestriction`/`RestrictionMatcher`/`ActionClass`/`ExtensionReviewerContribution`/`ReviewOrigin`/`ReviewerState`/`ReviewerStatusChanged` are defined once above and reused verbatim across tasks.
