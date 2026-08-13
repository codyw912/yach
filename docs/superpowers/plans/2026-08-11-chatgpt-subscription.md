# ChatGPT Subscription / OAuth Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development` to execute this plan task by task. Use `test-driven-development` for every behavior change and `requesting-code-review` after every task. Do not run project-wide format, lint, build, or test commands inside delegated tasks; the coordinating session runs them once after the end-to-end smoke works.

**Goal:** Let a user log in to ChatGPT subscription from `/connect` via rig's device flow, persist one managed connection at the fixed logical auth file, refresh/use stored tokens on every request, and treat remove as logout. Headless uses stored tokens only.

**Architecture:** Patch vendored `rig-core` first so auth writes are atomic/0600, every transaction holds `<auth_file>.lock`, every write is fenced, and completion yields typed `CompletionError::Auth`. Yach then owns probe/login/logout/activation around a public `AuthFileGuard`. Registry metadata lock is second; auth-file lock is first whenever both are needed.

**Tech stack:** Rust 2024 workspace; Tokio; Serde; existing `fs2` advisory locks; vendored Rig 0.41 (`[patch.crates-io]`); Ratatui generic dialogs.

**Accepted design:** `docs/superpowers/specs/2026-08-11-chatgpt-subscription-design.md`

## Cross-task contracts

- Logical auth path policy: `~/.yach/auth/chatgpt-subscription.json` in production; tests inject a temp-dir path via `ConnectionPolicy`.
- Distinct auth variants: `ChatGptSubscriptionEnvironment { token_dir }` (transient only; load/write reject) and `ChatGptSubscriptionManaged { auth_file, account_id }` (persisted Ready; no credential store).
- One managed subscription row. Second login updates that row in place (stable connection id).
- Lock order: auth-file lock, then registry lock. Never reversed.
- `allow_device_flow = true` only inside the dedicated login task. Probes, activation, request adapters, and headless are false.
- Login is two-phase: rig may commit the auth file; phase 2 (backend connection-flow runtime, never UI) writes/updates the row under both locks after `guard.authorize_account()`.
- `AuthEntryToken` and physical paths never serialize, log, or leave process. Protocol `AuthFailureSummary` is redacted.
- Message-string matching of `AuthError` is forbidden. Match typed variants only.
- No env migration. No overwrite-based re-auth: fenced delete, then device flow.
- Before modifying exported protocol, runner, adapter, or catalog symbols, use LSP references.

---

## Task 1: Vendored rig — typed errors, atomic writes, lock, fencing

**Files:**
- Modify: `vendor/rig-core/src/providers/internal/auth.rs`
- Modify: `vendor/rig-core/src/providers/chatgpt/auth/native.rs`
- Modify: `vendor/rig-core/src/providers/chatgpt/auth/mod.rs`
- Modify: `vendor/rig-core/src/providers/chatgpt/auth/wasm.rs`
- Modify: `vendor/rig-core/src/completion/request.rs`
- Modify: `vendor/rig-core/src/providers/chatgpt/mod.rs`

### Behavior

Add typed `AuthError` variants from the spec (`DeviceFlowDisabled`, `AuthBusy`, `AuthConflict`, `AccountMismatch`, `UnsafeAuthFile`, `UnsafeLockFile`, `RepairRequired`). Re-export auth types from the chatgpt provider module.

`write_auth_record` becomes atomic same-dir temp + 0600 + fsync + rename. Every write takes `ExpectedAuthEntry::{Absent, Present(AuthEntryToken)}`. No unfenced write path.

`PlatformAuthenticator` acquires `<auth_file>.lock` (open-or-create, regular-file, never unlinked) with try-lock + short wait → `AuthBusy`. Hold only around read/refresh/write and the device-flow final write — never across polling.

Per-transaction: re-resolve logical path (canonicalize parent), re-run final-entry policy. Symlink/non-regular → `UnsafeAuthFile`. Loose/restrictive regular mode → `fchmod` 0600 under lock.

Public `AuthFileGuard` + `AuthFileStat` + `AuthEntryToken` + `delete_if_unchanged`. Guard-aware `authorize_account()` uses the held lock (non-reentrant).

`CompletionError::Auth(AuthError)` owns the error. ChatGPT completion maps auth failures to that variant, not `ProviderError(err.to_string())`.

### Tests (RED then GREEN)

Focused `rig-core` tests named in the spec: 0600 after create/rewrite; no leftover temps; stale fencing → `AuthConflict`; contention → `AuthBusy`; lock entry rejects symlink/non-regular; ctime catches same-size rewrite; `AuthorizedAccount` has no token fields.

```bash
just dev cargo test -p rig-core --lib providers::chatgpt::auth
```

### Checkpoint

```bash
jj describe -m "feat(rig): lock, fence, and type ChatGPT auth writes"
jj new
```

---

## Task 2: Vendored rig — injectable base, bounded flow, account APIs, observer

**Files:** same chatgpt auth + `ChatGPTBuilder` / completion wrappers.

### Behavior

`.auth_base_url(...)` rewrites device-code, device-token, oauth-token, verify URL, and `redirect_uri`.

Device flow: per-request HTTP timeouts + absolute `timeout_at` from remaining budget before every await.

`authorize_account() -> Result<AuthorizedAccount, AuthError>` (redacted; `entry` token only).

`authorize_expected(expected_account: Option<&str>)` on the request path.

`login_device_flow_expecting(ExpectedAuthEntry::Absent, ...) -> Result<LoginCompletion, AuthError>`: never consumes cached auth; any present entry or fenced commit mismatch → `AuthConflict`; missing parseable account id is login failure.

Completion takes a sync observer receiving borrowed `AuthOutcomeSummary { account_id, refreshed }` on success only.

Disabled + no usable token → `DeviceFlowDisabled`.

### Tests

Fixture HTTP server via injected base: account id from fixture id token; disabled → `DeviceFlowDisabled`; `login_device_flow_expecting(Absent)` conflicts when an entry appears; hung request fails at request timeout; near-deadline request fails at deadline; all five endpoints honor injected base; mismatch from real completion path.

### Checkpoint

```bash
jj describe -m "feat(rig): injectable ChatGPT auth and typed login APIs"
jj new
```

---

## Task 3: Yach filesystem helper + connection policy

**Files:**
- Create: `crates/yach-connections/src/chatgpt_auth.rs` (or equivalent module)
- Modify: `crates/yach-connections/src/lib.rs`

### Behavior

Helper: `create_dir_all` auth root then 0700; canonicalize directory (parent symlinks accepted); final-entry no-follow policy matching spec. Regular non-0600 → tighten. Symlink → repair. Non-regular → manual repair. NotFound → pass.

`ConnectionPolicy { chatgpt_auth_file: PathBuf }` injected at store construction. Production stamps the logical home path; tests inject temp paths.

### Tests

Symlinked file → repair; directory → refusal; symlinked `~/.yach` parent → works; 0644 and 0400 → 0600; 0600 untouched; NotFound pass; root 0700.

### Checkpoint

```bash
jj describe -m "feat: add ChatGPT auth-file policy and path helper"
jj new
```

---

## Task 4: Managed subscription registry

**Files:**
- Modify: `crates/yach-connections/src/lib.rs`
- Modify: `crates/yach-connections/src/registry.rs`

### Behavior

Split `ConnectionAuth` / serde tags as specified. `validate_persisted` accepts only `Managed` for ChatGPT; environment shape fails load and write. Collection validation: two subscription rows is a load error.

Store methods: `create_managed_subscription`, `update_managed_account`, `remove` (no credential store). Store stamps policy logical path. Uniqueness check-and-insert / in-place account update under registry lock.

### Tests

Environment-shaped persisted record rejected; duplicate rows load-error; create Ready without credential write; second login same id; policy path stamped not trusted from draft; account id empty/overlong rejected.

### Checkpoint

```bash
jj describe -m "feat: persist one managed ChatGPT subscription connection"
jj new
```

---

## Task 5: Protocol + TUI dialogs for probe, device code, repair, adoption

**Files:**
- Modify: `crates/yach-proto/src/lib.rs` (and related)
- Modify: `crates/yach-ui` dialog/slash connect paths
- Modify: backend connection-flow runtime

Device-code events carry operation id. Device code is display-only: secret-safe `Debug`; test proves absence from Debug/log.

Dialogs: use existing / confirm / re-auth / repair / manual-repair / adoption / busy / retry. No token material on the wire.

### Checkpoint

```bash
jj describe -m "feat: add ChatGPT subscription connect dialogs"
jj new
```

---

## Task 6: Login operation, probe state machine, logout

**Files:** backend connection-flow + CLI provider_connections + runner.

### Behavior

`/connect` → ChatGPT runs the probe, not the device flow. Probe holds one guard for stat + optional `authorize_account`, then releases before dialogs. Commit fencing under both locks.

Login task: abortable, monotonic operation id, `allow_device_flow` true only here. Phase 2 in backend runtime. Cancelled generation cannot create a row. Re-auth: confirm, fenced delete, then flow. Logout: fenced delete then row; NotFound ok; unsafe lock blocks row removal.

### Tests

Fake OAuth server covering the spec matrix rows that `/connect` owns (create, cancel, leftover file, conflict, uniqueness, logout idempotence).

### Checkpoint

```bash
jj describe -m "feat: log in and out of ChatGPT subscription from /connect"
jj new
```

---

## Task 7: Activation, request path, health, headless

**Files:** `rig_adapter.rs`, runner retry/auth dispatch, provider config construction.

### Behavior

Activation runs `authorize_expected(row.account_id)` (device flow off) through the shared retry-schedule helper. Adapter carries expected account + generation lease.

Request path: `authorize_expected` inside completion; observer → `record_auth_health`; terminal `CompletionError::Auth` → `AuthFailureDetail` sidecar → `handle_auth_failure` only for unrecovered outcomes.

Headless: stored tokens only; failures name TUI `/connect`. Env `token_dir` path remains transient `ChatGptSubscriptionEnvironment` and is never persisted.

### Tests

Activation vs first-use separately; crash window → adoption; generation lease discard; sidecar redaction; transient refresh retries without re-login; TUI request paths never enable device flow; headless fast-fail.

### Checkpoint

```bash
jj describe -m "feat: activate and use managed ChatGPT subscription tokens"
jj new
```

---

## Task 8: Coordinating verification

Coordinating session only:

```bash
just dev cargo test -p rig-core --lib providers::chatgpt
just dev cargo test -p yach-connections
just dev cargo test -p yach-proto
just dev cargo test -p yach-backend chatgpt
just dev cargo test -p yach-cli chatgpt
just fmt
just clippy
```

Update `docs/project/next.md` and the board item only after the slice lands enough to change recommended next work.

Squash into reviewable commits; bookmark; do not push unless asked.
