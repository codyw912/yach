# ChatGPT Subscription / OAuth Lifecycle

**Date:** 2026-08-11
**Status:** designed (owner, 2026-08-11; advisory rounds plus repeated
focused spec reviews — findings absorbed through successive revisions;
see `.superpowers/sdd/2026-08-11-chatgpt-subscription/`)
**Prior work:** provider connections
(`2026-08-03-provider-connections-design.md`; this slice is its deferred
"Subscription/OAuth lifecycle" item), file credential store
(`2026-08-05-file-credential-store-design.md`), model discovery slice 3
(active-only fallback for subscription).
Board: provider product surface.

## Problem

`chatgpt-subscription` works today only as a transient environment
provider: `YACH_RIG_CHATGPT_TOKEN_DIR` points at a token directory owned
by something else, the connection registry rejects persisted
subscription auth, and there is no way to log in, inspect, or log out
from yach itself.

Rig 0.41 ships the protocol lifecycle (`providers/chatgpt/auth/
native.rs`): device-code login with a `DeviceCodeHandler` callback, a
token cache file, expiry checks, and transparent refresh with
write-back. What yach lacks is the product surface — plus rig-level
gaps below, each verified against the vendored source.

## Rig gaps (verified against vendored 0.41)

1. **No permission or atomicity discipline.** `write_auth_record` is
   `create_dir_all` + `std::fs::write`: permissions are 0666 masked by
   the process umask (typically 0644 — potentially group/world-readable
   tokens), no atomic rename.
2. **Existing-file parse discipline blocks recovery.**
   `read_auth_record` treats only NotFound as empty; any unparseable
   existing file errors before the device flow starts. Yach must never
   pre-create a placeholder; corrupt state needs a yach-side repair
   path.
3. **No test seam, no public account view, untyped errors.** Endpoints
   are hardcoded `auth.openai.com` constants (including the
   `redirect_uri` form field built from `CHATGPT_AUTH_BASE`) with an
   internally constructed `reqwest::Client` that has no per-request
   timeout; `mod auth` is private; `authorize()` returns
   `Result<(), AuthError>`; the device-flow-disabled case is an untyped
   `AuthError::Message(String)`.
4. **No transactional boundary around auth.** `auth_context()` performs
   read → maybe-refresh → write internally with no hook, and rig
   stringifies `AuthError` inside the completion path — so callers
   cannot hold a lock around just the auth transaction, cannot enforce
   an expected account at request time, and cannot map typed auth
   failures before they become strings.
5. **Cancellation is structural, not explicit.** The poll loop has no
   cancellation seam and its 15-minute timeout is checked only BETWEEN
   polls — a hung HTTP await can outlast it. Aborting the driving task
   stops at await points but cannot roll back synchronous I/O: after
   the token exchange, rig's blocking write may commit the auth file.
   Cancellation guarantees "no registry row," never "no file."

## Owner Decisions (2026-08-11)

1. **Login lives inside `/connect`** (probe state machine → device flow
   when warranted), one mental model for all auth.
2. **Dedicated auth file**
   `~/.yach/auth/chatgpt-subscription.json`, owned by rig's
   `Authenticator`; fixed logical path, not user-configurable.
3. **Removal deletes tokens** — ordered two-step (file, then row);
   within removal, a crash never leaves tokens behind once the row is
   gone. Remove = logout. Active-connection removal stays rejected.
4. **Headless uses stored tokens only.** `allow_device_flow = false`
   everywhere except the dedicated login task. A separate
   headless-surfaces slice is queued (owner 2026-08-11).
5. **No env migration** (standing ruling: manual repair beats a
   one-use code path).
6. **One subscription connection, enforced cross-process.**
7. **Picker stays active-only** (slice-3 ruling stands).

## Design

### Vendored rig patch (load-bearing, upstreamable)

Seven changes to `vendor/rig-core` (the existing `[patch.crates-io]`
mechanism), all intended for upstream:

1. **Atomic + secure writes.** `write_auth_record`: temp file in the
   same directory with explicit 0600, write, fsync, rename. Every
   write, including first create and refresh write-back.
2. **Cross-process auth lock + per-transaction path revalidation inside
   every auth transaction.** The patched `PlatformAuthenticator`
   acquires `<auth_file>.lock` (flock-style) internally around every
   read/refresh/write transaction and around the device flow's final
   write — the lock lives where the file is owned, so every process is
   serialized by construction and rotating refresh tokens cannot
   interleave. Every auth transaction ALSO re-resolves the configured
   logical path (canonicalize, following legitimate parent symlinks)
   and re-runs the final-entry policy (a symlink or non-regular entry
   planted or retargeted AFTER adapter construction fails the
   transaction with typed `UnsafeAuthFile`; a regular file with loose
   permissions is TIGHTENED to 0600 in place under the lock —
   metadata-only, no error — matching the probe helper's rule; only
   ambiguous/trap states error) — so a long-lived rig client can never
   follow a replacement symlink on a later turn. Lock entry discipline mirrors the registry's lock files:
   open-or-create, regular-file validation, never unlinked (stable
   inode). Acquisition is try-lock with a short bounded wait;
   contention yields typed `AuthError::AuthBusy` (never a silent
   block). The lock is never held across polling or a turn.
3. **Public guard API with guard-aware authorization.** A public
   `AuthFileGuard` (plus `AuthFileStat`/`AuthEntryToken` types) lets yach
   acquire the SAME lock rig uses internally, inspect entry state
   (metadata only — existence, file type, mode, device+inode+mtime+
   ctime+size; never contents), and perform
   `delete_if_unchanged(token)` — so yach's probe, fencing, and delete
   paths share rig's lock protocol instead of duplicating it. The guard
   is the ONLY way to hold the lock across a verify-then-act sequence:
   `guard.authorize_account()` performs the exact read/refresh
   transaction under the HELD lock (no re-acquisition — the lock is
   non-reentrant) and returns `AuthorizedAccount` plus the
   post-transaction entry token. `AuthEntryToken` = resolved physical
   path + file type + device + inode + mtime + ctime + size (metadata
   only; yach never reads token contents). ctime catches in-place
   same-size rewrites that mtime can miss; the resolved-path component
   makes a parent-symlink retarget detectable. On Windows (rig supports
   it) the token's identity uses the platform's volume/file-index
   equivalents and there are no mode bits — secure-write semantics there
   are "atomic rename + private-directory ACL," documented and tested
   per-platform with Unix assertions cfg-gated.
4. **Fenced writes, explicitly represented.** Every auth-file write
   takes an `ExpectedAuthEntry::{Absent, Present(AuthEntryToken)}` —
   there is NO unfenced write path. Under the lock the current state is
   re-checked; a mismatch fails with typed `AuthError::AuthConflict`
   instead of overwriting. This closes the silent-overwrite race
   between a completed login in one process and probe/delete consent
   based on older state in another.
5. **Injectable auth base URL.** One builder option
   (`.auth_base_url(...)`, default the real host) rewriting ALL of:
   device-code URL, device-token URL, OAuth token URL, the user-facing
   verify URL, and the `redirect_uri` form field. Production never sets
   it.
6. **Public redacted account view + typed errors + expected-account
   enforcement + absent-expected login + full outcome envelope.**
   Re-export the auth error type publicly. Add:
   - `authorize_account() -> Result<AuthorizedAccount, AuthError>` with
     `AuthorizedAccount { account_id: Option<String>, refreshed: bool,
     entry: AuthEntryToken }` — no token material; the entry token feeds
     fenced operations. Also available guard-aware
     (`guard.authorize_account()`).
   - **An auth-outcome envelope on the completion path, with ONE
     ownership contract:** the patched chatgpt completion takes a
     synchronous observer callback that receives a BORROWED, redacted
     `AuthOutcomeSummary { account_id: Option<&str>, refreshed: bool }`
     for successful auth, while failures propagate as the owned
     `AuthError` return exactly as today (no dual delivery; `AuthError`
     contains non-Clone IO/HTTP causes, so the observer never receives
     errors). The consuming point is the chatgpt completion wrapper in
     `rig_adapter.rs`, which converts the borrowed summary into the
     runner's health call before returning.
   - `authorize_expected(expected_account: Option<&str>)` used by the
     REQUEST path: the auth transaction enforces the expected account
     and returns typed outcomes/errors from the exact transaction the
     request uses (no preflight/second-call race). Mismatch → typed
     `AuthError::AccountMismatch { expected, actual: Option<String>,
     entry: AuthEntryToken }` — `actual` is an option because a valid
     cached token can carry no parseable identity; the entry token makes
     a subsequent adoption fenceable.
   - The device flow used by yach's login task is
     `login_device_flow_expecting(ExpectedAuthEntry::Absent, ...) ->
     Result<LoginCompletion, AuthError>`, where
     `LoginCompletion { account_id: String, entry: AuthEntryToken }` is
     the redacted completion payload phase 2 compares against (the
     device-code callback only ever carries code/URL). A completed
     exchange with no parseable account id is a login FAILURE (the
     nonempty-identity invariant), not a `None` payload. The flow never
     consumes cached auth — if the initial read finds ANY entry, or the
     fenced commit finds the state changed, it fails with
     `AuthError::AuthConflict` (which yach routes back through the probe
     state machine). This closes the race where another process's
     completed login lands between yach's absent probe and the flow's
     first read: without it, rig would silently adopt that login.
   - Typed variants: `DeviceFlowDisabled` (disabled + no usable token),
     `AuthBusy`, `AuthConflict`, `AccountMismatch`,
     `UnsafeAuthFile { kind: UnsafeEntryKind, entry: Option<AuthEntryToken> }`
     (per-transaction path revalidation failure;
     `UnsafeEntryKind::{Symlink, NonRegular}` — the token is `Some` for
     a symlink, which yach may fenced-unlink, and `None` for a
     non-regular entry, which yach must never delete; the kind→surface
     mapping is exhaustive: `Symlink` → fenced repair,
     `NonRegular` → `ManualRepairRequired`), `UnsafeLockFile`
     (the lock entry itself is a symlink or non-regular file — distinct
     from UnsafeAuthFile because yach must NEVER unlink an untrusted
     lock entry another process may hold; the only outcome is
     manual-repair refusal naming the path, on every surface), and
     `RepairRequired { kind: RepairKind, detail: String,
     entry: AuthEntryToken }` —
     raised at the auth-transaction boundary for corrupt/unreadable
     PRESENT entries. The Io/Json cause is CLASSIFIED into the bounded
     `RepairKind::{Corrupt, Unreadable, UnsafeEntry, MissingIdentity}`
     (`MissingIdentity` is produced at the yach mapping layer for a
     valid token with no parseable account claim, not by rig) and then
     DISCARDED — `detail` is a bounded (≤200 chars), redacted display
     string derived from the cause, and the original error is not
     retained (keeps `AuthError` clone-free and the surface redacted).
     The under-lock entry token travels with the error so repair
     deletion stays fenceable. (`UnsafeLockFile` carries NO token —
     nothing about the entry is trusted.) Yach matches typed
     variants only — message-string matching is forbidden (lint-level
     review point).
7. **Bounded flow with absolute deadline + typed completion errors.**
   The internally constructed client gets per-request timeouts, AND the
   whole device flow is wrapped in an absolute `timeout_at` deadline
   computed from the remaining budget before every await — a request
   begun near expiry fails at the deadline, not one request-timeout past
   it. Separately, rig's completion path currently stringifies auth
   failures (`CompletionError::ProviderError(err.to_string())`); the
   patch adds a typed `CompletionError::Auth(AuthError)` variant OWNING
   the auth error, so `rig_adapter.rs` extracts it before any
   stringification and builds `AuthFailureDetail` from the typed value
   (the extraction point: the SSE completion wrapper's error mapping).

Patch tests: account id round-trips from a fixture id token; disabled →
`DeviceFlowDisabled`; contention → `AuthBusy`; stale fencing →
`AuthConflict`; account mismatch → `AccountMismatch` from the real
completion path (with `actual: None` when the token lacks identity);
guard-aware `authorize_account` works under a held guard and returns the
post-transaction entry token; `login_device_flow_expecting(Absent)`
fails with `AuthConflict` when an entry appears before its initial read
or before its commit and never consumes cached auth;
`ExpectedAuthEntry` has no unfenced write path; an in-place same-size
rewrite with preserved mtime is caught (ctime); `AuthorizedAccount` has
no token fields (construction-level); write→read round-trip; 0600 after
create and rewrite; no temp files left; all five endpoints honor the
injected base; a hung fixture request fails at the request timeout; a
request begun just before the deadline fails AT the deadline; the lock
entry rejects symlinks/non-regular files and is never unlinked; the
guard API's stat/delete-if-unchanged round-trip under concurrency;
Windows builds compile the cfg-gated token identity and secure-write
path (Unix-only assertions gated).

### Two distinct locks (summary)

- **Registry metadata lock (existing):** the metadata store's global
  file lock (`registry.rs` mutation lock) — cross-process. All registry
  mutations for this slice run inside it, including the subscription
  uniqueness check-and-insert and same-row re-auth update.
- **Auth-file lock (new, rig-owned):** `<auth_file>.lock`, acquired
  inside rig's auth transactions and by yach through the public
  `AuthFileGuard`. Lock ordering when both are needed (commit, logout):
  auth-file lock first, then registry lock — never reversed.

### Filesystem safety helper

The AUTHORITATIVE path safety lives in rig's per-transaction
revalidation (patch item 2) — a long-lived client can never follow a
symlink or loose-mode replacement planted after adapter construction.
The yach-side helper exists for UX-time decisions (probe dialogs,
fencing tokens, deletes) and is invoked at connection creation, probe,
activation (early, for fast clear errors), and before every delete:

1. `create_dir_all` the auth root, then set 0700 on Unix (an existing
   looser directory is tightened — its only purpose is token files).
2. `fs::canonicalize` the now-existing directory. Parent symlinks are
   followed and ACCEPTED (Nix-managed homes legitimately symlink
   `~/.yach`). The canonical path is runtime-only.
3. **Logical vs. physical, named precisely:** the registry persists the
   LOGICAL absolute path (`~/.yach/auth/chatgpt-subscription.json`);
   policy equality compares logical paths only; and RIG RECEIVES THE
   LOGICAL path at adapter construction
   (`.auth_file(logical)`) — rig canonicalizes the parent inside every
   auth transaction (patch item 2), so a parent-symlink retarget is
   observed on the next transaction by construction. The PHYSICAL
   (canonicalized) path appears only inside a live `AuthFileGuard` and
   its `AuthEntryToken` — never stored, never handed to a long-lived
   client.
4. Final-entry policy only (parent components are not policed), and all
   checks/act operations are no-follow racesafe: the entry is opened
   with `O_NOFOLLOW`-equivalent semantics and validated via `fstat` on
   the resulting descriptor; tightening uses `fchmod` on that
   descriptor; reads (rig's) happen through the same no-follow open;
   the lock file itself is opened no-follow as well. A check-then-use
   race against a non-cooperating process replacing the entry therefore
   cannot redirect operations — the descriptor pins the checked inode.
   The policy outcomes:
   - symlink → repair dialog (unlink is safe — removes the link, not
     the target — but still requires explicit confirmation);
   - directory or other non-regular entry → manual-repair refusal
     naming the path (yach never `rm -r`s);
   - regular file with ANY mode other than 0600 → normalize to 0600 via
     `fchmod` on the no-follow descriptor (metadata-only; no
     confirmation; status-logged). This covers loose modes (0644 —
     world-readable tokens) and restrictive ones (0400/0200/000 — which
     would break refresh or read paths) uniformly; there is no
     mode-based error state for regular files;
   - regular 0600 file, or NotFound → pass.

Tests: symlinked file → repair dialog; directory entry → refusal;
symlinked `~/.yach` parent → resolves and works; 0644 → tightened;
0600 → untouched; NotFound → pass; root mode 0700; helper runs on the
activation path (not just connect/delete); parent retarget between
probe and commit → fencing conflict, not silent adoption.

### Login operation protocol

- The connection-flow runtime owns a dedicated abortable tokio task per
  login attempt and its `JoinHandle`.
- Each attempt gets a monotonically increasing **operation id**.
  Device-code prompts, completions, failures, and cancel
  acknowledgements carry it; the dialog discards any event whose id is
  not current — a queued success from a cancelled attempt can never
  create a registry row.
- The login task polls WITHOUT holding the auth-file lock (no lock is
  ever held across the up-to-15-minute flow, so a second process is
  never starved; rig takes the lock internally for the short fenced
  commit write).
- rig's synchronous `DeviceCodeHandler` only posts the code + URL to
  the UI event channel with the operation id. The device code is
  display-only secret material: the event/payload types get secret-safe
  `Debug` impls, and a test proves the code is absent from Debug/log
  output while remaining renderable in the dialog.
- `allow_device_flow` is TRUE only inside this task. Probes,
  activations, and all request adapters run with it false — a normal
  TUI request must never trigger a 15-minute device poll outside
  `/connect`.
- **Two-phase completion:** phase 1 (the task) completes the flow — the
  auth file may commit (rig's fenced write). Phase 2 is owned by the
  **backend connection-flow runtime** (which holds the registry store
  and accepts the current operation generation — the UI never touches
  storage; it receives only dialog/status events): on a
  current-generation success, the runtime acquires `AuthFileGuard`,
  verifies the committed file's account via `guard.authorize_account()`
  against what the flow reported, acquires the registry lock,
  creates/updates the row, releases both. Cancellation before phase 2
  guarantees **no NEW row and no metadata update** — scoped precisely:
  for a fresh create, no row exists at all; for a re-authentication,
  the EXISTING row is preserved (login never deletes a row), but its
  auth file may already be deleted/replaced — that dead/stale row
  surfaces as "re-login required" health and is recoverable through
  `/connect`, with the activation identity check catching any account
  divergence on next use. A leftover file is the probe's
  existing-login case.

### Probe state machine

`/connect` → ChatGPT subscription runs the probe, not the device flow:

1. Filesystem helper (above). Rejections route to their dialogs; the
   helper's entry token is retained for fencing.
2. Existence and probe under ONE guard (the lock is non-reentrant):
   acquire `AuthFileGuard`, `guard.stat()` the PHYSICAL path, and if an
   entry is present call `guard.authorize_account()` under the SAME
   held guard; release the guard before presenting any dialog. (Yach
   never reads contents.) NotFound at stat → device flow (login
   operation). An entry that vanishes between stat and the guard-aware
   authorize is `AuthConflict` (state changed), never
   `DeviceFlowDisabled`-as-revoked.
3. Present → `authorize_account()` probe (device flow disabled):
   - Ok(Some(account), refreshed, entry) → confirmation dialog: "Use
     existing login for `<account>`?" / Re-authenticate / cancel.
   - Ok(None, ..) → not adoptable (identity is the consent anchor):
     "existing login is missing its account identity" →
     Re-authenticate / cancel.
   - Err(DeviceFlowDisabled) → present but refresh rejected: "existing
     login expired or revoked (account unknown)" → Re-authenticate /
     cancel.
   - Err(Http) / Err(Message) / Err(AuthBusy) → transient/provider/
     contention failure: "could not check the existing login" → Retry /
     Cancel. Never treated as absent. (Residual Io/Json causes that are
     not entry corruption — lock-file I/O, canonicalize failure — also
     land here; present-but-corrupt entries never surface as bare
     Io/Json because rig classifies them into `RepairRequired` at the
     boundary.)
   - Err(RepairRequired { kind, detail, entry }) → corrupt-file repair
     dialog directly ("Delete and re-authenticate" / cancel), fenced by
     the carried entry token and displaying the bounded `detail`
     explanation. No helper re-run — the token already identifies the
     entry.
   - Err(UnsafeAuthFile { kind, .. }) → match the pinned
     `UnsafeEntryKind`: `Symlink` → repair dialog (fenced unlink, using
     the carried token); `NonRegular` → manual-repair refusal naming
     the path. (A change between helper and probe is one way here;
     race test.)
   - Err(UnsafeLockFile) → manual-repair refusal naming the lock path.
   - Err(AuthConflict) → state changed under us: re-probe and
     re-present current state. Never a generic retry-overwrite.
4. **Commit fencing:** on "Use existing login", the row is persisted
   only inside a single critical section holding BOTH locks (guard
   first, then registry): re-authorize via `guard.authorize_account()`
   (the guard-aware form — no lock re-acquisition) and compare account
   id and entry token against the consented values; a mismatch
   re-presents the dialog with the NEW account (never persists the
   stale one).
5. **Refresh-before-confirmation, resolved:** a probe that refreshes
   writes back through the patched atomic path. That is token RENEWAL
   of the same account — the designed lifecycle — not the
   account-changing replacement the consent rule governs. The consent
   rule applies to re-authenticate paths only.

### Registry change

`ProviderKind::ChatGptSubscription` stops being transient-only:

1. **Distinct variants** (type-level separation):
   - `ChatGptSubscriptionEnvironment { token_dir: PathBuf }` —
     transient environment record only; rejected in persisted metadata
     both by the write path AND by `validate_persisted`/collection
     validation on load (a hand-edited environment-shaped record fails
     to load; negative load test retained).
   - `ChatGptSubscriptionManaged { auth_file: PathBuf, account_id: String }`
     — persisted shape: LOGICAL fixed path, nonempty bounded (≤128
     chars) account id captured at successful login. A token exchange
     without a parseable account id is a login failure.
   The legacy `chatgpt_subscription` tag never persisted, so the split
   migrates nothing.
2. **Managed rows bypass the credential transaction.** A managed
   subscription row is created directly in `ConnectionState::Ready` —
   its credential is the auth file, which rig has already written —
   via dedicated store operations (`create_managed_subscription`,
   `update_managed_account`, `remove`). No `CredentialStore` write or
   removal is attempted for this kind, and the pending/ready two-write
   API-key transaction does not apply.
3. **Fixed path is injected policy.** A `ConnectionPolicy` value
   (logical auth-file path) is passed to `ProviderConnectionStore` at
   construction alongside the existing injected storage traits and
   threaded to adapter construction as the LOGICAL path (rig
   `.auth_file(logical)`; rig canonicalizes per transaction — physical
   paths appear only inside live guards/tokens, never in a long-lived
   client). Domain validation checks shape (nonempty, bounded account
   id; well-formed path); canonical equality is checked against the
   injected policy's LOGICAL path; the store stamps the policy path
   onto `Managed` records at creation rather than trusting drafts.
   Tests inject temp-dir paths.
4. **Uniqueness** is check-and-insert inside the registry's global
   locked mutation. A second login is re-authentication of the existing
   row: on success the row's `account_id` is updated in place under the
   same global lock (one metadata write, stable connection id, no
   delete+create). **Crash window:** if the new token file commits but
   the metadata update fails/crashes, the row shows account A while the
   file holds account B — caught by the activation identity check
   (lifecycle below), which blocks use until an explicit
   adoption/re-auth resolves it.
5. **Load-time collection validation:** a hand-edited registry with two
   subscription rows is a load error naming the conflict, not a silent
   pick-one.

### Lifecycle

- **Use:** every request's auth runs through the patched
  `authorize_expected(expected_account)` inside the completion path —
  rig takes the auth-file lock internally, re-resolves and re-validates
  the path per transaction (a symlink or non-regular entry planted
  after activation fails with typed `UnsafeAuthFile` → repair surface;
  a loosened mode is tightened under the lock and the request
  proceeds),
  enforces the expected account, and reports the exact transaction's
  outcome through the envelope (the runner observes SUCCESS as well as
  failure, which is what makes `Ok`/`Refreshed` health truthful).
  Valid token → proceed; expired + refresh token → refresh (atomic
  fenced write-back); transient refresh failure (Http/Message) →
  existing retryable coarse kind with sidecar — NOT re-login guidance;
  refresh rejected (DeviceFlowDisabled) → typed re-login guidance;
  `AccountMismatch` → block + fenced adoption dialog; `AuthConflict` →
  re-probe and re-present; `AuthBusy` → bounded wait then typed busy
  status.
- **Activation identity check:** activating a managed subscription
  connection RUNS a typed `authorize_expected(row.account_id)`
  transaction (device flow disabled) as part of activation — before the
  runtime reports `Activated` — so wrong-account, corrupt, revoked,
  unsafe, and no-identity states produce their typed activation-time
  outcomes immediately rather than at first request; activation is
  tested separately from first use. Activation's transient failures
  (Http/Message from refresh) run through a shared retry-schedule
  helper extracted from the request path (same delays and transient
  classification as `provider_request_with_retry`, but generic over the
  operation — the activation path calls `authorize_expected` under it
  and never issues a provider request); the terminal outcome is the
  typed provider failure with the auth sidecar. The active adapter
  then carries the row's `account_id` as the expected account for every
  request; a mismatch (crash window, external replacement) invalidates
  the active adapter until an explicit adoption/re-auth resolves it. **Adoption is
  fenced:** the mismatch outcome carries the post-auth entry token for
  the actual account, and adoption runs in the same guard→registry
  two-lock critical section as probe/login commits — re-authorize under
  the guard, require the account AND token to still match the consented
  values, then `update_managed_account` under the registry lock. A
  change between dialog and adoption is `AuthConflict` → re-present,
  never a blind row update. A mismatch whose actual account is
  unparseable (`actual: None`) routes to repair/re-auth, never
  adoption.
- **Re-authentication (canonical rule; every re-auth choice routes
  here):** confirm first; acquire `AuthFileGuard`; fencing-check the
  consented entry token; `delete_if_unchanged(token)`. Token mismatch
  (including a consented-present entry having vanished) →
  `AuthConflict`: re-probe and re-present — never proceed as if the
  delete happened. Deletion I/O error → dialog stays open with the
  error. Only after successful deletion launch the device flow. Forced
  by rig semantics — a valid existing token makes rig skip login — and
  by consent: secret state is never overwritten without explicit
  confirmation, and never overwritten at all (fenced deletion precedes
  the flow, so a failed delete cannot strand a half-replaced file).
  Overwrite-based re-auth does not exist anywhere in yach.
- **Logout:** ordered two-step under both locks (guard for the fenced
  delete, then the registry lock for the row — that acquisition order,
  never reversed). Logout tolerates NotFound (the goal state already
  holds; remove completes idempotently) — unlike re-authentication,
  which treats a vanished consented entry as a conflict because it is
  about to start a NEW login. A crash between the steps leaves a dead
  row surfacing as "re-login required" that a repeated remove completes
  idempotently — but never leaves tokens once the row is gone.
- **Creation ordering (dual rule):** the flow writes the auth file
  (rig-internal lock + fencing), then phase 2 writes the row (both
  locks). A crash between leaves a valid auth file with no row;
  recovery is the probe's explicit existing-login confirmation, never
  silent adoption.
- **Headless:** stored tokens only; helper + guard + identity check
  apply identically; failure guidance names the TUI path; no device
  flow, ever.
- **Status:** account identity is persisted (the row's `account_id`);
  health is runtime-only. The protocol has two methods on
  `ProviderConnectionRuntime` (the runner calls both — it constructs
  adapters and observes every auth outcome via the patch's outcome
  envelope):
  - `record_auth_health(ConnectionId, AuthHealth)` for status-only
    outcomes, where `AuthHealth { status: Ok | Refreshed | Busy,
    generation: u64 }`. The runtime owns an in-memory map keyed by
    `ConnectionId` and applies only current-generation updates.
  - `handle_auth_failure(ConnectionId, AuthFailureDetail) ->
    Option<AuthFailureEffect>` for the dialog-driving outcomes — it
    carries the full internal sidecar (INCLUDING the entry token and
    repair detail; this is the in-process channel, never serialized) and
    RETURNS the effect the runner then triggers, or `None` when the
    detail's generation is stale (discarded — no dialog, no status
    change; tested). Effects:
    `OpenAdoptionDialog { actual: String, entry }` (only when `actual`
    is `Some` — an unparseable actual can never be adopted),
    `OpenRepairDialog { repair: RepairDetail, entry }` (fenced; also
    for `Mismatch` with `actual: None` — the adapter synthesizes
    `RepairDetail { kind: RepairKind::MissingIdentity, detail: "the
    stored login has no account identity" }` at mapping time, so one
    effect shape covers it — repair/re-auth, not adoption),
    `ShowManualRepair { path_label: String }` (redacted label, no entry
    token; for `UnsafeLockFile` and non-deletable entries),
    `ReProbe`, `ShowBusy`, `ShowReLoginGuidance`. The runner→flow call
    is the auth-outcome match arm in the request/activation layer,
    which dispatches the returned effect into the connection flow.
  - **Generation lease:** adapter construction/activation takes the
    connection's current generation FROM the runtime
    (`generation_for(ConnectionId)`), and the adapter carries it
    through each request outcome; removal, re-auth, AND every
    fenced adoption/managed-account update bump the generation, which
    invalidates all outstanding leases — stale outcomes from before the
    change (including pre-adoption outcomes from the old account) are
    discarded by both runtime methods, and a new lease is issued only
    at the next activation.
  The `/connect` list keeps the established no-provider-I/O fast path:
  persisted account id plus "not checked" until a real auth attempt
  reports otherwise.

### Error transport

Auth failures stay typed end to end, with an explicit redaction
contract. `ProviderError` derives Debug/Clone/Eq/Serialize/Deserialize
today, so the typed detail CANNOT embed rig's `AuthEntryToken` (it
carries the resolved physical home path — a local-filesystem secret —
and platform identity types that must not cross protocol, log, or
persistence boundaries). The split:

- **Coarse persisted/protocol surface:** `ProviderError` gains
  `auth: Option<AuthFailureSummary>` where
  `AuthFailureSummary { reason: AuthFailureReason, account: Option<String> }`
  is fully serializable and redacted (no paths, no tokens, no file
  metadata). `AuthFailureReason::{ReLoginRequired, Mismatch,
  RepairRequired, ManualRepairRequired, Conflict, Busy, Transient}` —
  `RepairRequired` covers deletable failure states (corrupt/unreadable
  entries, unsafe entries that are safe to unlink such as symlinks);
  `ManualRepairRequired` covers what yach must never delete:
  `UnsafeLockFile` and non-regular `UnsafeAuthFile` (directories etc.).
- **Coarse kind mapping (pinned for every reason):**
  `ReLoginRequired | Mismatch | RepairRequired | ManualRepairRequired`
  → `ProviderErrorKind::Authentication`; `Busy | Conflict` →
  `ProviderErrorKind::ProviderInternal` (retryable; the typed reason,
  not the kind, drives re-probe vs busy-wait); `Transient` → the
  existing retryable kind for the underlying cause
  (`Network`/`ProviderInternal`).
- **Internal sidecar (never serialized):** the adapter hands the runner
  the full `AuthFailureDetail { reason, generation: u64,
  repair: Option<RepairDetail>, entry: Option<AuthEntryToken>,
  actual_account: Option<String> }` — `RepairDetail { kind: RepairKind,
  detail: String }` preserves rig's classification and bounded display
  text through the `CompletionError::Auth` extraction mapping — through
  an internal error WRAPPER threaded through the retry loop
  (`ProviderStreamAttempt`/equivalent) — chosen over a direct runtime
  call precisely so dialog/status effects dispatch ONLY for the
  terminal unrecovered outcome — a retried-and-recovered attempt never
  opens a dialog, and its success reports through the observer, whose
  `refreshed: bool` maps to `AuthHealth` `Ok` (cached) or `Refreshed`
  (write-back happened) — no third value exists. The `generation` field is the adapter's lease (below), so
  both runtime methods can discard stale failures. The entry token and
  physical path stay off every serialized/logged surface; a test proves
  it.
- **Transient classification:** auth-transaction transport failures
  (Http/Message from refresh) map to their EXISTING retryable coarse
  kinds (`ProviderErrorKind::Network`/`ProviderInternal`) with the
  sidecar attached — so the existing retry loop (`provider_error_is_transient`)
  works unchanged, while the sidecar still informs the status surface.
  `Authentication` as a coarse kind is reserved for ReLoginRequired/
  Mismatch/RepairRequired/ManualRepairRequired.
- **Runner dispatch (match on reason, never strings):**
  `ReLoginRequired` → "re-login required → `/connect` → ChatGPT
  subscription" with the connection identified (the adapter knows its
  `ConnectionId` at construction — the runner derives identity from the
  active `ProviderConfig`); `Mismatch` → `handle_auth_failure` with the
  detail (fenced adoption dialog); `RepairRequired` →
  `handle_auth_failure` (fenced repair dialog with the carried
  classification + detail); `ManualRepairRequired` →
  `handle_auth_failure` (manual-repair refusal naming the redacted path
  label); `Conflict` → re-probe and re-present; `Busy` → bounded wait
  then typed busy status; `Transient` → retry surface.
  The tiered keyword classifier stays for transport errors; this is the
  typed channel for auth state.

### State matrix (auth file × entry point)

| File state | `/connect` | Activation (TUI) | Headless use | Remove |
|---|---|---|---|---|
| Absent | device flow | "not logged in: `/connect`" | setup error, TUI guidance | row removed (NotFound ignored) |
| Valid | confirm account (fenced) / re-auth | identity check, works | identity check, works | fenced delete, row removed |
| Valid, no parseable identity | re-auth offered (not adoptable) | repair/re-auth guidance | repair/re-auth guidance | as valid |
| Valid, wrong account vs row | confirm shows actual account | block + adoption dialog (fenced) | block + typed guidance | as valid |
| Expired, refreshable | probe refreshes (renewal), confirm | refresh under lock, works | same | as valid |
| Expired, refresh transient failure | retry/cancel (never treated as absent) | bounded retry → provider failure | bounded retry → provider failure | as valid |
| Expired, revoked | re-auth confirmation (account unknown) | typed re-login guidance | typed re-login guidance | as valid |
| Corrupt (Io/Json) | repair dialog (confirm fenced delete) | typed repair guidance | typed repair guidance | deletion proceeds (contents irrelevant) |
| Symlink | repair dialog (confirm unlink) | activation-time repair dialog (`authorize_expected` runs before `Activated`) | typed repair guidance | unlink requires confirmation |
| Directory / non-regular | manual-repair refusal | activation-time manual-repair refusal | manual-repair refusal | manual-repair refusal |
| Lock entry unsafe (symlink/non-regular) | manual-repair refusal (never unlinked by yach) | same — typed `UnsafeLockFile` | same | row removal BLOCKS with manual-repair guidance (removing the row first would orphan the token file with no way to retry logout); after manual repair, repeat remove |
| Replaced after activation (symlink/retarget) | n/a (helper at probe) | next request fails typed `UnsafeAuthFile` (rig re-validates per transaction) | same | helper re-checks at delete |
| Mode changed after activation (0644 or 0400/000) | normalized at probe | next request normalizes under the lock and proceeds (status-logged) | same | normalized at delete check |
| AuthConflict (state changed mid-flow) | re-probe, re-present | re-probe, re-present | bounded retry → typed conflict status | re-probe, re-present |
| Auth lock contended | retry/cancel (typed `AuthBusy`) | bounded wait → typed busy | bounded wait → typed busy | bounded wait → typed busy |

### Testing

- Fake OAuth server (via the injected base URL; the repo's tiny HTTP
  fixture pattern) covering: full device flow → row created, file 0600,
  atomic; cancel while polling → no file, no row; cancel/failure after
  token commit but before phase-2 → file may remain, no row, and the
  next `/connect` surfaces the existing-login confirmation; a queued
  completion from a cancelled operation id cannot create a row; phase-2
  runs in the backend connection-flow runtime (never the UI), holds
  both locks, and re-verifies account + fencing via guard-aware
  authorization; expired + refresh
  → fenced write-back preserves 0600 and reports `refreshed`;
  `Ok(None)` → not adoptable; refresh-rejected → re-auth confirmation;
  refresh-503 → retry branch (probe) and provider-failure (activation),
  never re-login guidance; corrupt file → repair dialog; hung fixture
  request → request-timeout failure inside the poll budget; a request
  begun just before the deadline fails AT the deadline; two fixture
  processes cannot interleave a refresh rotation (auth lock) or
  double-create a row (registry lock); concurrent login in a second
  process while one is polling → no starvation (poll holds no lock),
  commits serialized + fenced; a cross-process login completing between
  absent probe and flow start → `AuthConflict` from the absent-expecting
  login, re-probe, never silent adoption; stale-fencing login commit →
  `AuthConflict`, no overwrite; re-auth delete against a vanished or
  replaced entry → `AuthConflict`, re-probe, no device flow; adoption
  against a post-dialog file change → `AuthConflict`, re-present; second
  login → same-row account update, stable connection id; crash after
  token commit before metadata update → activation blocks with the
  adoption dialog; adoption updates the row only inside the
  guard→registry critical section; account change on re-auth →
  adoption/re-activation surfacing; removal under both locks, idempotent
  on NotFound, deletion error keeps the dialog open; hand-edited
  duplicate registry → load error; hand-edited environment-shaped persisted record → load
  rejection; non-regular directory entry → refusal without `rm -r`;
  0644 AND 0400/000 files → normalized to 0600 at probe; helper runs on activation,
  not just connect/delete; a symlink/parent-retarget planted BETWEEN
  two requests on one active adapter fails the second request with
  typed `UnsafeAuthFile` (rig re-validates per transaction), while a
  0644 or 0400 mode is normalized under the lock and the request
  proceeds (status-logged); activation transient failures use the
  extracted retry-schedule helper (same delays as the request path,
  no provider request issued); the
  auth-outcome envelope reports successful cached and refreshed auth to
  the runner (health `Ok`/`Refreshed` are observable, not just
  failures); corrupt-file request failure maps to `RepairRequired`
  (never Transient); transient auth transport failure keeps its
  retryable coarse kind AND the sidecar (the existing retry loop works
  unchanged); the internal sidecar's entry token and physical path
  never reach serialized/logged/persisted error surfaces (redaction
  test); dialog effects dispatch only for the TERMINAL unrecovered
  outcome — a retried-and-recovered attempt opens no dialog and reports
  through the observer (`refreshed` → health `Refreshed`, else `Ok`); repair errors carry the under-lock entry token so the repair
  dialog's fenced delete works without re-statting; rig's completion
  path yields typed `CompletionError::Auth` (no stringification) and
  rig_adapter extracts the typed error into `AuthFailureDetail`; an
  unsafe LOCK entry (symlink/non-regular) fails typed `UnsafeLockFile`
  with manual-repair refusal on every surface, and yach never unlinks
  it — row removal BLOCKS until the lock entry is manually repaired,
  after which a repeated remove deletes the auth file before the row
  (file-first ordering preserved); cancelled re-authentication
  preserves the existing row (dead-file state surfaces as
  re-login-required and recovers via `/connect`); the probe's
  helper→authorize race surfaces `UnsafeAuthFile`/`RepairRequired` and
  re-presents current state; adoption bumps the generation and
  pre-adoption outcomes are discarded; generation lease invalidation —
  an outcome carrying a pre-re-auth generation is discarded by both
  runtime methods; parent-symlink retarget between probe and commit → fencing conflict;
  device code absent from Debug/log output; TUI request paths never
  enable the device flow; headless fast-fail with TUI guidance.
- Connection-flow protocol round-trips and redaction: no token material
  in events, logs, dialog debug output, or `AuthorizedAccount`
  (construction-level).

## Non-goals

- Browser-flow OAuth (device flow only — works over SSH).
- Headless login/setup (separate headless-surfaces slice).
- Multiple subscription accounts.
- Codex CLI auth-file interop (rig's default is `~/.config/chatgpt/
  auth.json`, not `~/.codex/auth.json`; no interop either way).
- Subscription-side model discovery (active-only stands).
- Env-to-connection migration.

## Follow-ups queued by this design

- Headless-surfaces slice (owner 2026-08-11): interactive-only auth and
  related surfaces for headless/CI, designed holistically.
- Upstream the rig patch (atomic writes, auth-lock transactions +
  guard API + fencing, endpoint injection, redacted account view,
  expected-account enforcement, typed errors, bounded deadline).
