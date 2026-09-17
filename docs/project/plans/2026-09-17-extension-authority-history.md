# Extension Authority History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete extension consent evidence without synthetic session identities, fix duplicate discovery of aliased install stores, and make extension diagnostics truthful on the wire and at process exit.

**Architecture:** One per-extension user-owned JSON document commits current authority and retained decisions under a stable sidecar lock. Existing CLI and lifecycle callers share that store, while activation keeps a read-only fail-closed grant query. Discovery deduplicates identical sources rather than extension IDs; existing session-event schemas remain unchanged.

**Tech Stack:** Rust 2024, serde/serde_json, fs2, uuid, std filesystem, existing Tokio runner; no new dependencies. Use the repository's `just` development environment.

**Spec:** `docs/project/specs/2026-09-16-extension-capability-contract-design.md`

Implements its 2026-09-17 amendment, especially Evidence onward.

**Source:** plane:YACH-10

## Global Constraints

- Consent is not enforcement. Do not imply that Yach observes or restricts a subprocess's undeclared socket/process use.
- User state alone grants authority. Discovery deduplication must preserve stored scope; project records never become user records.
- Existing session-event schemas and replay remain unchanged. No synthetic session, turn, user name, or authenticated-human identity is recorded.
- Surface is `cli` or `lifecycle`; do not label arbitrary RPC traffic as a verified TUI/human.
- No new dependencies, generic audit framework, cross-process host-control service, or audit-browser feature.
- All new writes use `yach.extension-authority.v1`. Recognizing existing unversioned grant data for migration is required; dual writers and unaudited mutation APIs are prohibited.
- Existing file-scoped extensions require no grant or new state writes.
- History is retained without automatic pruning. Activation performs no audit write; tool calls perform no new audit work.
- `unwrap`, `expect`, `panic`, and lint suppression are forbidden by workspace policy. `panic_in_result_fn` is denied: use Result-returning test bodies with explicit errors, or existing non-Result assertion conventions, not panic assertions inside Result bodies.
- Never change process-global HOME in a test. Use `ExtensionAuthorityStore::in_home` or a child process with a private environment. New tests use isolated directories and deterministic synchronization, not sleeps.
- Use `just dev cargo ...`, `just test`, `just lint`, and `just fmt-check`. Skip validation in parallel subagents; the integration owner runs it after edits settle.
- Tests defend observable contracts, not wording/source text/field forwarding. Delete in-scope wording pins instead of updating their literals.
- The previous plan remains historical. Old `yach#wc3k` and parent `yach#528c` remain open until this replacement evidence is verified and published; never claim closure just because this plan exists.

## File Structure and Ownership

| File | Responsibility / task |
| --- | --- |
| `crates/yach-backend/src/extension_capability.rs` | Capability domain types, public mutation/query entrypoints and messages; Task 1 |
| `crates/yach-backend/src/extension_capability/store.rs` (new) | Versioned document, legacy parsing, invariant validation, locking and durable replacement; Task 1 |
| `crates/yach-backend/src/extension.rs` | Migrate path-based test caller in Task 1; manifest discovery fix in Task 3 |
| `crates/yach-cli/src/main.rs` | Explicit audit surface in Task 1; shared install-store loading and failure exit in Task 3 |
| `crates/yach-backend/src/runner/extension_state.rs` | Explicit audit surface in Task 1; blocking lifecycle scheduling/ordering and error semantics in Task 2 |
| `crates/yach-cli/tests/extension_capability.rs` | Replace global-HOME/forged-validation proof with real CLI/store checks in Tasks 1–2; discovery regressions in Task 3 |
| `crates/yach-cli/tests/rpc_matrix.rs` | Real native lifecycle grant/revoke and retained evidence; Task 2 |
| `crates/yach-backend/src/runner.rs` | Keep and exercise real production turn-policy regression; Task 2 only if a genuine test seam needs adjustment |
| `docs/extensions.md` | Authority format, migration, revoke/failure behavior, CLI/TUI distinctions; Tasks 1, 3, 4 |
| `docs/protocol/yach-proto-v0.md` | Existing extension messages and nullable fields; Task 4 |

Keep serialization/storage private under the capability module; do not spread the schema into CLI, UI, session logs or config.toml. `lib.rs` already glob-reexports the capability module; confirm that before adding redundant exports. Rust LSP was unavailable during planning; use it for references if execution has a server, otherwise search all crates before changing exported signatures.

## Execution Dependencies

Execute Tasks 1 → 2 → 3 → 4. Storage and caller signature cutover are one coherent task, not a compiling store that no production caller uses. Task 2 owns the caller integration proof and lifecycle scheduling. Task 3 is independently reviewable but shares main.rs/test files, so do not run its edits concurrently with Tasks 1–2. Documentation can be drafted independently, but its final state follows implemented behavior.

Each task gets focused red/green verification and a fileset commit. Run the full suite, formatter/linter and performance gate once on the combined result, then a whole-branch review. Save immutable base revisions before mutation for scoped review. Use fresh jj changes above the committed spec and plan via starting-a-change; do not jump to trunk and strand these documents.

### Task 1: Commit authority and decision history atomically

**Files:** create `crates/yach-backend/src/extension_capability/store.rs`; modify `extension_capability.rs`, the actual callers in CLI `main.rs` and runner `extension_state.rs`, and existing storage tests in `extension.rs` and CLI `tests/extension_capability.rs`. Update `docs/extensions.md` for changed on-disk and revoke behavior.

**Interfaces:** keep capability derivation, grant shape and confirmation messages. Define these public domain/store signatures (method bodies belong in this task, not later):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDecisionSurface { Cli, Lifecycle }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionAuthorityError {
    HomeUnavailable, InvalidId, InvalidDocument, UnsupportedSchema,
    UnsafePath, UnsafePermissions, Io, DurabilityUnknown,
}

#[derive(Debug, Clone)]
pub struct ExtensionAuthorityStore { home: std::path::PathBuf }

impl ExtensionAuthorityStore {
    pub fn for_current_user() -> Result<Self, ExtensionAuthorityError>;
    pub fn in_home(home: &std::path::Path) -> Self;
    pub fn load_grant(&self, id: &str)
        -> Result<Option<ExtensionCapabilityGrant>, ExtensionAuthorityError>;
    pub fn grant_requested(&self, id: &str, version: &str,
        tools: &[ExtensionToolContribution], surface: ExtensionDecisionSurface)
        -> Result<Option<ExtensionCapabilityGrant>, ExtensionAuthorityError>;
    pub fn revoke_grant(&self, id: &str, surface: ExtensionDecisionSurface)
        -> Result<bool, ExtensionAuthorityError>;
}
```

Method declarations above are contracts, not stubs to commit. Add `Display`/`Error` for the error enum. Keep `load_grant(id) -> Option<ExtensionCapabilityGrant>` as the production fail-closed query. Update `grant_requested(id, version, tools, surface)` and `revoke_grant(id, surface)` to delegate to `for_current_user()` with the typed error. Remove exported raw `store_grant`/`remove_grant` and their path helpers; no compatibility mutation aliases. Path-injected tests use `in_home`, not synthetic ids derived from arbitrary temp filenames.

Private document shape:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityDocument {
    schema: String,
    extension_id: String,
    current: Option<ExtensionCapabilityGrant>,
    legacy_baseline: Option<ExtensionCapabilityGrant>,
    history: Vec<AuthorityDecision>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthorityAction { Grant, Revoke }
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityDecision {
    operation_id: String,
    recorded_at: String,
    action: AuthorityAction,
    reason: String,
    surface: ExtensionDecisionSurface,
    before: Option<ExtensionCapabilityGrant>,
    after: Option<ExtensionCapabilityGrant>,
}
```

- [ ] **Step 1 — Write behavioral storage regressions before implementation.** Add a private test fixture owning a UUID-named temporary home with Drop cleanup; its `store()` returns `ExtensionAuthorityStore::in_home`, and `document(id)` reads JSON from `.yach/extensions/<id>.json`. No production authority serialization is exposed for test convenience. Example non-Result test using existing assert-then-let-else convention:

```rust
#[test]
fn revoke_retains_evidence_but_denies_activation() {
    let home = TestHome::new();
    let store = home.store();
    let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
    let granted = store.grant_requested("example.net", "1.0", &tools,
        ExtensionDecisionSurface::Cli);
    assert!(matches!(granted, Ok(Some(_))));
    assert_eq!(store.revoke_grant("example.net", ExtensionDecisionSurface::Cli), Ok(true));
    assert_eq!(store.load_grant("example.net"), Ok(None));
    let doc = home.document("example.net");
    assert!(doc["current"].is_null());
    assert_eq!(doc["history"][0]["after"]["approved"], serde_json::json!(["uses_network"]));
    assert_eq!(doc["history"][1]["before"], doc["history"][0]["after"]);
    assert!(doc["history"][1]["after"].is_null());
    assert_eq!(doc["history"][1]["reason"], "extension_capability_revoke");
}
```

Implement TestHome locally with `fs::create_dir_all` checked before use and a stable path accessor; `tool` already exists in the capability tests. Distinct additional contracts: repeat revoke records null→null and returns false; repeat grant retains both decisions; no-capability trust creates no document; restart reload denies a retained revoked document; malformed document mutation returns error without changing bytes; unknown `schema` cannot masquerade as legacy. Invalid ids must still protect a sentinel outside the grants directory.

- [ ] **Step 2 — Observe red.** Run `just dev cargo test -p yach-backend --lib extension_capability`. Initially the history regression may fail to compile against missing types; after introducing the model it must fail on missing behavioral implementation. Do not count a compile-only red as proof of the atomicity cases.

- [ ] **Step 3 — Implement format and invariant validation.** Parse JSON once to inspect schema presence, then decode either the exact unversioned legacy shape or versioned document. Explicit `schema: null` is not legacy. Require document fields even when their values can be null (Serde Option alone accepts missing fields; check object keys or use a required-nullable deserializer). Validate filename/id equality, nonempty approved grants, unique valid operation UUIDs, exact action/reason pairs and ordered before/after continuity from legacy_baseline to current. New grant timestamps use existing UTC helper; legacy timestamps are retained as recorded metadata, not recertified historical truth. Unknown capability names, action/surface values and inconsistent documents fail closed.

The new validator walks borrowed history entries and compares values; avoid cloning/replaying every grant into a second history. Only a mutation needs to clone prior current authority into a new decision. Loading for activation returns current authority after validation, without migration writes.

- [ ] **Step 4 — Implement one durable transaction.** Follow `UserConfigStore::update`/`write_document`, with the authority spec's additional parent-directory durability requirement. Use a stable `<id>.json.lock` sidecar, `fs2::FileExt::lock_exclusive`, reload after locking, sibling UUID `create_new` temp file, file sync, atomic rename, then parent sync. Validate private user-state files/directories and reject symlink authority/lock paths; Unix `O_NOFOLLOW` on opened final components avoids an avoidable check/open race. Handle directory-creation races by verifying the resulting directory, not accepting arbitrary AlreadyExists. Never delete lock files while another process may hold them. Keep home path semantics consistent with existing user state (a resolved user home may itself be a legitimate symlink).

Use a private commit phase seam for fault tests, not an exported filesystem abstraction. Define `CommitPhase::{BeforeRename, AfterRename}` and `fn commit_document(path: &Path, bytes: &[u8], phase_hook: impl Fn(CommitPhase) -> io::Result<()>) -> Result<(), ExtensionAuthorityError>`; normal calls use `|_| Ok(())`. Injected BeforeRename failure removes the temporary and leaves original bytes; injected AfterRename failure maps to DurabilityUnknown with the new complete document readable. Exercise real file operations on both sides of the hook. Check write/file-sync/rename errors through their real error paths too. Unsupported directory sync has the platform-documented weaker guarantee; any other error after rename is uncertainty, not Io/rollback.

- [ ] **Step 5 — Migrate and cut over all callers.** Legacy grant reads remain read-only. On explicit mutation import the legacy object as baseline, append the new decision, and replace once. There are no separate migration markers or writable legacy files. CLI passes Cli; runner passes Lifecycle. Remove raw writes/deletes and migrate their tests. Existing `extension.rs` test near `capability_block_reason_with_grant` should construct a temporary home/store, then reload after revoke rather than relying on file deletion. Replace the CLI integration test's global-HOME mutation and forged `ToolValidation::Allowed` proof now with a real CLI grant/revoke/restart persistence test using isolated child environments. Task 2 adds native host activation/restart coverage; retain the production turn-policy regression throughout, and do not claim the Task 1 CLI test alone proves host execution.

Add cross-process concurrency coverage using test subprocesses, not just threads: spawn two instances of a narrowly filtered test helper that both mutate the same in_home store, each writing a fixed number of decisions. Join both and verify every operation id is unique, every before/after transition is continuous, and final current matches the last decision. The helper is gated by test-only environment and launched with `current_exe --exact <helper-name>`; the parent never mutates global environment. Do not add a production worker command.

- [ ] **Step 6 — Green, docs and commit.** Run focused backend tests and build/test affected CLI callers. Update docs/extensions.md to say revoke retains a document with no current authority, explain legacy baseline and durability-unknown errors, and identify the private JSON audit artifact. Commit only this task's actual files with `jj commit <files> -m 'Persist extension authority and decisions atomically'`. No old raw writer should remain in crate-wide reference search.

### Task 2: Prove CLI and native lifecycle evidence at the real boundaries

**Files:** `crates/yach-backend/src/runner/extension_state.rs`, `crates/yach-cli/tests/extension_capability.rs`, `crates/yach-cli/tests/rpc_matrix.rs`; retain existing production turn-policy regression in runner.rs. CLI main.rs only for an error-message correction if typed durability handling requires it.

**Interfaces:** consumes Task 1 public store and explicit `ExtensionDecisionSurface`; keeps all ClientEvent/ServerEvent definitions unchanged. Add a blocking revoke scheduler matching existing trust/reload scheduling, not a second storage implementation.

- [ ] **Step 1 — Add native RPC proof before altering lifecycle scheduling.** Extend existing RpcChild constructor internally to accept an optional fixture package root, set `YACH_EXTENSION_PACKAGE_ROOTS` after its inherited-environment scrub, and preserve existing constructors by delegating to that one implementation. Use native backend (`None`), not the fixture runner; no provider credentials or network requests are needed. `FirstRenderCompleted` drives extension discovery. Wait with bounded event-channel deadlines until a diagnostic snapshot shows the fixture blocked before sending trust:

```rust
child.send(&ClientEvent::ExtensionLifecycleRequested {
    request_id: "audit-trust".into(),
    action: yach_proto::ExtensionLifecycleAction::Trust,
    selector: "example.capability-network".into(),
});
child.wait_for(|event| matches!(event,
    ServerEvent::ExtensionLifecycleFinished { request_id, outcome, .. }
    if request_id == "audit-trust"
        && *outcome == yach_proto::ExtensionLifecycleOutcome::Completed));
child.send(&ClientEvent::ExtensionDiagnosticSnapshotRequested {
    request_id: "after-trust".into(), selector: Some("example.capability-network".into()),
});
```

Assert the returned record is active with fetch_url registered and network capability approved. Read the child's private authority JSON: one lifecycle grant decision and no invented session/turn fields. Send correlated revoke, verify host becomes stopped/not active, current=null, and the revoke before-value contains the prior grant. Restart a child using the same test-owned home and confirm activation is blocked. Extend RpcChild ownership to preserve that home only for this explicit restart test; all ordinary matrix tests retain isolated owned home cleanup.

- [ ] **Step 2 — Add real CLI history/restart proof.** Use `Command::new(env!("CARGO_BIN_EXE_yach"))` with `.env("HOME", private_home)`, private user/project store overrides, `.env_remove("YACH_EXTENSION_PACKAGE_ROOTS")`, and a distinct cwd. Install the existing fixture, trust, inspect persisted JSON, run doctor in a new process, revoke, and inspect retained evidence. Test corrupt/unknown-format state using bytes written into the private home; assert mutation reports Failed and old bytes remain. Exit-code assertions become green in Task 3; do not assert erroneous zero status here as a contract. Validate all process outputs, not merely successful OS exit.

- [ ] **Step 3 — Make lifecycle storage blocking and ordering explicit.** File locks/fsync must run in spawn_blocking, including revoke; never hold a synchronous file lock across an async await. Within one native runner, use its existing activation snapshot mutex to serialize lifecycle transitions: acquire snapshot in the blocking task before grant/revoke storage mutation, release the file lock inside the store before calling reload/stop, then finish the correlated result. All trust/reload/revoke use consistent snapshot-first ordering. Reload still calls the real current authority gate, never a cached returned grant.

A successful persistence followed by failed reload reports both facts without undoing consent. DurabilityUnknown returns Failed plus the typed uncertainty message and does not reload. Cross-process CLI revoke changes future activation authority but does not promise to stop an already running external host. No global host registry or IPC is added.

- [ ] **Step 4 — Add deterministic lifecycle race coverage.** Use a test barrier at the lifecycle test seam to delay an older trust before its reload, complete revoke after trust's durable write, and prove no later reload can use that stale returned grant as permission. Serial snapshot ordering may prevent that interleaving in one runner; test the resulting sequence and separately exercise an external store revoke before the reload gate. Barriers are test-only; no timing sleeps or environment-based production toggles. Keep the production policy regression `a_granted_network_extension_tool_is_advertised_and_allowed_in_a_turn` so activation proof does not substitute for provider advertising/authorization.

- [ ] **Step 5 — Run focused checks, smoke, commit.** Run `just dev cargo test -p yach --test rpc_matrix rpc_extension`, `just dev cargo test -p yach --test extension_capability`, and `just dev cargo test -p yach-backend --lib a_granted_network_extension_tool`. Launch the actual rebuilt CLI and native RPC with a temporary home to observe persisted history and revoke behavior; remove the throwaway smoke fixture afterward. Commit the actual modified files with `jj commit <files> -m 'Verify extension decision evidence through CLI and native lifecycle'`.

### Task 3: Deduplicate aliased discovery sources and report CLI failure

**Files:** CLI `main.rs`, backend `extension.rs`, CLI `tests/extension_capability.rs`, docs/extensions.md.

**Interfaces:** add path-injected CLI loader `loaded_extension_install_records_from_paths(user: &Path, project: &Path) -> Result<Vec<ExtensionInstallRecord>, String>`. Existing `loaded_extension_install_records()` resolves the two scope paths and delegates. Stored `ExtensionInstallRecord.scope` is untouched. No new backend root-precedence policy.

- [ ] **Step 1 — Reproduce with real subprocess tests.** Install fixture once with child cwd==HOME, then doctor/trust. Repeat using a symlink cwd alias on Unix. Before the fix both doctor cases return Failed/catalog_error. Add genuine conflict fixture with a second physical package declaring the same ID; it must remain Failed and, after the exit fix, status 1. Preserve distinct IDs declaring the same tool as another negative case. For shared-store records explicitly marked Project, assert discovery preserves Project scope rather than relabelling User.

- [ ] **Step 2 — Fix store aliasing once.** Resolve paths once. Load the user path once; if project path identifies that same existing store, do not reload. Canonical path equality handles symlink aliases; on Unix also compare metadata dev/ino for hard-link aliases. Missing stores remain empty; an unrelated malformed or unreadable project store must not be hidden by failed identity lookup. Equality of two missing literal paths may skip an empty second load; do not conflate two identity errors with equality.

```rust
fn loaded_extension_install_records_from_paths(
    user: &Path, project: &Path,
) -> Result<Vec<ExtensionInstallRecord>, String> {
    let mut records = load_install_records_at(user)?;
    if !same_install_store(user, project)? {
        records.extend(load_install_records_at(project)?);
    }
    Ok(records)
}
```

Implement local helpers `load_install_records_at(&Path) -> Result<Vec<ExtensionInstallRecord>, String>` using existing error labels and `same_install_store(&Path, &Path) -> Result<bool, String>` with the semantics above. Preserve existing single-scope loading used by unrelated install commands; do not widen this into generic filesystem deduplication.

- [ ] **Step 3 — Deduplicate manifest aliases after validation.** In `load_extension_package_root`, validate every discovered pointer's containment first. Retain a set of canonical manifest paths (and filesystem identity for hard-link aliases where available) for this root only, preserving first spelling/order for diagnostics. Skip a second reference only after validation, so an escaping pointer cannot hide behind another manifest. Do not merge separate roots or suppress catalog DuplicateExtensionId/DuplicateToolName. Tests: default yach.extension.json repeated in package.json, symlink to same in-root manifest, escaped pointer, genuine distinct-package conflicts.

- [ ] **Step 4 — Correct extension failure exit mapping.** Add explicit Failed match arms before the catch-all extension result arms:

```rust
Self::ExtensionDiagnostics { outcome: ExtensionDiagnosticsOutcome::Failed, .. }
| Self::ExtensionManagement { outcome: ExtensionManagementOutcome::Failed, .. } => 1,
```

The exact existing enum is `ExtensionDiagnosticsOutcome`. Successful/no-op outcomes remain 0, usage errors 2, all unrelated commands unchanged. Assert a real binary failed command returns 1 and still prints Failed; do not add a test that only echoes a constructed result's message.

- [ ] **Step 5 — Verify and commit.** Run affected backend manifest tests and `just dev cargo test -p yach --test extension_capability`, then repeat the original identical/symlink-home CLI smoke without overrides that mask the shared store. Record structured outcome and process exit. Update docs/extensions.md for failure exit behavior. Commit with `jj commit <files> -m 'Deduplicate shared extension discovery sources and fail CLI honestly'`.

### Task 4: Document the implemented protocol and verify the whole contract

**Files:** `docs/protocol/yach-proto-v0.md`, `docs/extensions.md`; no new protocol types or test-only documentation snapshots.

**Interfaces:** documents existing `ExtensionLifecycleRequested`, `ExtensionLifecycleFinished`, `ExtensionDiagnosticSnapshotRequested`, `ExtensionDiagnosticSnapshotUpdated`, and `ExtensionDiagnosticRecord` in yach-proto. No runtime interface changes.

- [ ] **Step 1 — Add exact wire contract.** Add these existing names to the modeled client/server event lists, then an extension lifecycle/diagnostics section:

| Event | Fields |
| --- | --- |
| `extension_lifecycle_requested` | request_id, action (stop/reload/trust/revoke), selector |
| `extension_lifecycle_finished` | request_id, action, selector, outcome (completed/not_found/not_active/failed), required message |
| `extension_diagnostic_snapshot_requested` | request_id, optional/nullable selector |
| `extension_diagnostic_snapshot_updated` | request_id, outcome (completed/not_found/failed), records, optional/nullable message |

Record fields: id, version, scope, package_root, manifest_path, source_ref, install_source, activation_state, generation, last_error_kind, last_error_summary, registered_tools, provider_visible_tools, capabilities, capability_grant. Read their actual Rust types before documenting optionality. Only claim order for Yach-produced native records, not for external capability arrays. Missing/null capability fields mean unknown; empty requested means no capabilities; empty grant means no current approval, including a retained revoked authority document. Selector whitespace becomes unfiltered; unmatched selector returns not_found with empty records. Lifecycle capability negotiation is client gating, not a substitute for authority checks.

- [ ] **Step 2 — Correct user documentation.** Distinguish CLI fresh package/install scan from TUI live activation snapshot, and say both expose capability fields instead of identical diagnostics. Keep the prominent consent-not-enforcement statement. Document private history inspection, no automatic deletion on revoke, legacy provenance, and the uncertainty distinction according to implemented error semantics. Preserve the protocol note's existing stability caveat; do not promise wire ordering or authenticated human identity.

- [ ] **Step 3 — Full proof on combined branch.** Run `just test`, `just lint`, `just fmt-check`, then the repository-prescribed CI Clippy reproduction:

```bash
nix shell nixpkgs#rustup nixpkgs#pkg-config nixpkgs#openssl --command bash -c 'export PATH="$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"; cargo clippy --all-targets --all-features -- -D warnings'
```

Run `just perf --filter 'extension/*'`. In a throwaway in-environment probe, measure repeated read-only load for equivalent granted documents with 1 and 1000 valid decisions; report elapsed/sample count and confirm no writes/locks on the activation read path. Generate histories through the public store, not hand-wave malformed benchmark inputs. The history probe is a measurement, not a new permanent benchmark API or arbitrary latency threshold. Remove it after recording results.

- [ ] **Step 4 — Final review and completion.** Commit the actual doc files with `jj commit docs/extensions.md docs/protocol/yach-proto-v0.md -m 'Document extension lifecycle and retained authority evidence'`. Request a whole-branch review covering state/history atomicity, legacy parser discrimination, native callers, real-policy advertising, revoke/read semantics, discovery scope preservation and truthful failures. Address Critical/Important findings with scoped re-review and rerun affected checks. Preserve all proof artifacts in the normal ignored execution workspace; no new status/roadmap files.

Publication requires `agent-github-publication-plan <bookmark> origin` and public-repository operator approval. Do not reuse expired approval or push main. After PR creation, close verified new-plan children, verify blockers, then close the new parent and the original yach#wc3k/yach#528c only when their amended acceptance is actually met. Update YACH-10 with evidence and the exact PR URL; completion does not imply merge. Inspect `jj log -r 'main..@'`, leave a clean described stack, and run standalone foreground `agent-checkpoint push` last. Include its printed reference and PR URL in the handoff.

## Coverage Map

| Spec requirement | Task / proof |
| --- | --- |
| Single current authority plus retained honest history | 1: document validation and revoke/restart tests |
| Stable lock, durable replacement, uncertainty | 1: cross-process writer and phase-failure tests |
| Legacy recognition without invented past decisions | 1: discriminator/migration/failure preservation tests |
| CLI and lifecycle surface distinction; no session schema changes | 1 signatures; 2 real CLI/RPC records |
| No stale returned grant used by activation | 2: lifecycle ordering and current-authority gate |
| File-scoped no-op and unchanged policy routing | 1 no-write test; 2 real production policy regression |
| Shared install-store identity and preserved scope | 3: identical/symlink/hard-link paths and genuine conflict controls |
| Repeated in-root manifest identity without escape bypass | 3: backend manifest tests |
| Nonzero extension Failed exits | 3: real CLI failure checks |
| Nullable diagnostic wire semantics and surface distinctions | 4: code-grounded docs, existing protocol roundtrip tests |
| No per-tool overhead and measured read impact | 4: extension perf gate plus history-load probe |
