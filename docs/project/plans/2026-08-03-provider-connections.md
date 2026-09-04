# Provider Connections and API-Key Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development` to execute this plan task by task. Use `test-driven-development` for every behavior change and `requesting-code-review` after every task. Do not run project-wide format, lint, build, or test commands inside delegated tasks; the coordinating session runs them once after the end-to-end smoke works.

**Goal:** Let a user create durable API-key provider connections through `/connect`, discover every connected provider's actual models, and explicitly activate an exact `(connection, model)` target without exposing credentials or changing the active model during setup.

**Architecture:** Add a small `yach-connections` crate for machine-managed metadata and platform credential storage. Extend the existing generic dialog protocol with a redacted secret response and one `ConnectionsRequested` event. The backend owns the `/connect` state machine and active-target invariants behind an async `ProviderConnectionRuntime` seam; the CLI implements that seam with catalog layers, Rig provider clients, the JSON registry, and Keyring. The model picker remains backend-fed but becomes connection-aware end to end.

**Tech stack:** Rust 2024 workspace; Tokio; Serde; UUID; Keyring 4 `v1` compatibility API; `fs2` advisory locks; `zeroize`; existing Rig 0.41 provider clients and Ratatui dialogs.

**Accepted design:** `docs/project/specs/2026-08-03-provider-connections-design.md`

**Dependency:** This work extends model-catalog slice 3 in PR #223. Execute on top of bookmark `model-catalog-slice3` until #223 merges; after merge, rebase onto `main`. Do not recreate or weaken slice-3 discovery, filtering, lazy-refresh, catalog-join, or deterministic model-ID behavior.

## Cross-task contracts

These names and boundaries are fixed so later tasks can build independently after their prerequisites land:

- Stored connection IDs are UUID strings. The sole transient environment connection uses reserved ID `environment`.
- `ProviderKind` includes Anthropic, OpenAI, OpenAI-compatible, and transient-only ChatGPT subscription. Persisted records accept only API keys with `CredentialSource::System`; the environment record represents API keys with `CredentialSource::Environment` or ChatGPT subscription with its token-directory path.
- Stored metadata has `pending_credential` and `ready` states. Discovery and activation ignore pending records; `/connect` exposes repair/remove actions. The registry accepts at most 64 stored connections.
- Secrets use two types: serializable `SubmittedSecret` only for the direct protocol hop, and non-serializable `ProviderSecret` everywhere after backend receipt. Both redact `Debug` and zero memory on drop.
- The protocol uses existing generic dialogs. It adds no parallel connection-list protocol: `ConnectionsRequested` asks the backend to open the root dialog; dialog option values carry stable IDs.
- `ModelInfo`, `BackendState`, `ModelSelectedDetailed`, and `ModelChanged` carry an optional connection ID for compatibility with non-native adapters. Native provider rows and confirmations always supply it.
- The connection runtime returns immutable `Arc<[CatalogModelEntry]>` availability snapshots and complete candidate `ProviderConfig` values. The runner alone swaps the active config.
- Setup never activates a connection. Replacing an active key reconstructs and swaps the active adapter while idle. Removing the active connection is rejected until another exact target is active.
- All registry and keyring work crosses `tokio::task::spawn_blocking`; provider HTTP work remains async.
- Every mutation holds a per-connection cross-process lock across its keyring and registry steps, rechecking metadata after lock acquisition. Registry writes also use one global file lock.
- Discovery permits at most eight provider requests in flight and retains at most 4,096 deterministic model rows across all connections while preserving the exact active row.
- Before modifying any exported protocol, runner, adapter, or catalog-facing symbol, use LSP references to enumerate every callsite; the file lists below are starting sets, not permission to skip symbol-aware migration.

---

## Task 1: Extend the protocol with redacted secrets and connection identity

**Files:**
- Modify: `crates/yach-proto/Cargo.toml`
- Modify: `crates/yach-proto/src/lib.rs`

### Step 1: Add failing protocol contract tests

Add focused tests beside the existing protocol round-trip tests:

1. `submitted_secret_debug_redacts_complete_client_event`
   - Construct `ClientEvent::DialogResolved` with `DialogResponse::Secret`.
   - Assert `format!("{event:?}")` contains `[REDACTED]` and does not contain the sentinel key.
   - Direct wire serialization/deserialization must recover the sentinel.
2. `secret_response_is_not_recordable`
   - Assert the explicit record/replay JSONL API returns no record for both a
     secret `ClientEvent` and its enclosing `TransportMessage`.
   - Assert ordinary client events remain recordable.
3. `connection_aware_model_state_round_trips`
   - Round-trip a `ModelInfo` with connection ID/display, `BackendState`,
     `ModelSelectedDetailed`, and `ModelChanged` with connection/provider/model.
   - Assert legacy JSON without the new optional fields still deserializes.
4. `provider_connections_capability_negotiates_only_when_both_peers_offer_it`.

Run the narrow tests and confirm RED because the variants/fields do not exist:

```bash
just dev cargo test -p yach-proto submitted_secret_debug_redacts_complete_client_event
just dev cargo test -p yach-proto connection_aware_model_state_round_trips
```

### Step 2: Implement the wire-safe secret type

Add `zeroize = "1.8"` to `yach-proto`.

Implement `SubmittedSecret` as a private `String` wrapper with:

- `new`, `is_empty`, and consuming `into_inner` methods;
- `Serialize`/`Deserialize` for the direct transport;
- manual `Debug` that emits only `[REDACTED]`;
- `Drop` that zeroizes the string;
- no `Display`, `AsRef<str>`, or public borrowed-value accessor.

Add:

```rust

DialogKind::SecretInput
DialogResponse::Secret { value: SubmittedSecret }
Capability::ProviderConnections
ClientEvent::ConnectionsRequested
```

Add `ProviderConnections` to `default_ui_handshake()` as the UI's offer, but not
to the default backend handshake: only native CLI session setup conditionally
claims backend support once a runtime exists.

The ordinary `Text` response remains unchanged.

Keep `to_jsonl` as direct wire encoding. Add a separately named record/replay
encoding path on `ClientEvent` and `TransportMessage` that returns no record for
`DialogResponse::Secret`; persistence code must never call the wire encoder.
This distinction permits transport while making recorder intent explicit.

### Step 3: Add the complete connection-aware model fields

Add backward-compatible optional fields:

- `ModelInfo.connection_id` and `ModelInfo.connection_display`;
- `BackendState.model_connection_id`;
- `ClientEvent::ModelSelectedDetailed.connection_id`;
- `ServerEvent::ModelChanged.connection_id` and `provider`.

Use `#[serde(default, skip_serializing_if = "Option::is_none")]` so legacy
adapters and recorded JSON remain readable. Update constructors and fixtures
with `None` where they represent non-native/legacy adapters. Provider/model
remain catalog identity; connection ID chooses credentials/endpoint and the
backend-resolved display field labels duplicate rows.

### Step 4: Verify GREEN

```bash
just dev cargo test -p yach-proto submitted_secret_debug_redacts_complete_client_event
just dev cargo test -p yach-proto connection_aware_model_state_round_trips
just dev cargo test -p yach-proto provider_connections_capability_negotiates_only_when_both_peers_offer_it
just dev cargo test -p yach-proto secret_response_is_not_recordable
```

### Step 5: Review and checkpoint

Review for accidental secret formatting/borrowing APIs and backward-compatible Serde defaults. Then:

```bash
jj describe -m "feat: add connection-aware and secret-safe protocol types"
jj new
```

---

## Task 2: Add the durable connection and credential domain

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/yach-connections/Cargo.toml`
- Create: `crates/yach-connections/src/lib.rs`
- Create: `crates/yach-connections/src/registry.rs`
- Create: `crates/yach-connections/src/credential.rs`

### Step 1: Add the crate and failing domain tests

Register `crates/yach-connections` in the workspace. Dependencies:

```toml
fs2 = "0.4.3"
keyring = "4.1.0"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
url = "2.5"
uuid = { version = "1", features = ["v4"] }
zeroize = "1.8"
```

Write tests first for:

- UUID generation and rejection of reserved/invalid stored IDs;
- persisted rejection of environment API-key sources and ChatGPT subscription auth;
- schema round-trip for ready and pending system-key records;
- missing file => empty registry, malformed file => typed error without overwrite;
- label trim/80-scalar bound and base-URL length/scheme/userinfo/query/fragment rules;
- trailing-slash normalization without inventing `/v1`;
- duplicate labels resolved to deterministic display labels with a short ID;
- rejection of a 65th stored connection and of a file containing more than 64 records;
- `ProviderSecret` debug redaction and zeroizing ownership API.

Run one representative test and confirm RED:

```bash
just dev cargo test -p yach-connections registry_rejects_transient_authentication
```

### Step 2: Implement domain values and validation

In `lib.rs`, implement:

- `ConnectionId` with `new_stored`, `environment`, `as_str`, and persisted validation;
- `ProviderKind`;
- `ConnectionAuth` and `CredentialSource`, including transient environment API-key and ChatGPT token-directory shapes;
- `ConnectionState`;
- `ProviderConnection`;
- `ProviderSecret` with redacted `Debug`, consuming exposure only at provider-client construction, and zeroize-on-drop;
- bounded, secret-free error enums and connection display-label helpers.

Do not derive `Debug` for a container until every secret-bearing member is proven redacted. Do not implement `Serialize` for `ProviderSecret`.

### Step 3: Add failing persistence-transaction tests

Define injectable traits before production implementations:

```rust
pub trait ConnectionMetadataStore: Send + Sync { /* load and locked mutations */ }
pub trait CredentialStore: Send + Sync { /* put/get/remove */ }
```

Use fakes and two independent store handles to test every transaction and race:

1. pending metadata write fails: no keyring write;
2. credential write fails: pending metadata remains visible;
3. ready-state write fails after credential write: pending metadata remains visible and repairable;
4. successful repair marks the same ID ready;
5. removal key succeeds but metadata removal fails: visible unavailable metadata remains;
6. candidate replacement validation failure never invokes `put` and preserves the old key;
7. interleaved replace/remove cannot remove metadata after installing an orphaned replacement key;
8. interleaved repair/remove cannot produce ready metadata without a credential;
9. rename/remove rechecks record existence under the same per-ID lock.

Confirm RED with:

```bash
just dev cargo test -p yach-connections ready_write_failure_leaves_repairable_pending_record
```

### Step 4: Implement the JSON registry and service

Implement `JsonConnectionMetadataStore` at an injected path:

- use a sibling global `.lock` file with `fs2::FileExt` for every atomic registry read-modify-write;
- use a stable per-connection lock file for every create/repair/replace/rename/remove; acquire it before reloading metadata and hold it across all keyring and registry steps;
- enforce one lock order: per-connection lock first, global registry lock only around the individual metadata mutation;
- reload inside each lock before deciding so concurrent processes do not act on stale state;
- write a same-directory temporary file, flush/sync it, set mode 0600 on Unix, rename atomically, then sync the parent directory where supported;
- never rewrite a malformed prior file and reject more than 64 records;
- preserve deterministic record ordering by stable ID.

Implement `ProviderConnectionStore` transaction methods over the two traits:

- `create_validated` locks the new ID and persists pending → key → ready;
- `repair_validated` locks/rechecks the pending ID and writes key → ready;
- `rename` locks/rechecks and changes metadata only;
- `remove` locks/rechecks and deletes key before metadata;
- `replace_validated` validates first, then locks/rechecks and writes the candidate key.

Implement `SystemCredentialStore` with Keyring service `yach` and account equal to the full stable connection ID. Map non-exhaustive Keyring errors to bounded typed categories; never include platform error debug strings in user-visible messages.

### Step 5: Verify focused persistence behavior

```bash
just dev cargo test -p yach-connections
```

The test suite must include two independent registry/service handles racing mutations, the 64-record cap, and a Unix-only 0600 assertion. It must not invoke the real platform credential store.

### Step 6: Review and checkpoint

Review atomicity, lock scope, symlink/path behavior, absence of secret serialization, and all injected failure points. Then:

```bash
jj describe -m "feat: add durable provider connection storage"
jj new
```

---

## Task 3: Add masked TUI input and the `/connect` entry point

**Files:**
- Modify: `crates/yach-ui/Cargo.toml`
- Modify: `crates/yach-ui/src/lib.rs`
- Modify: `crates/yach-ui/src/slash_commands.rs`
- Modify: `crates/yach-ui/src/app.rs`

### Step 1: Add failing UI behavior tests

Add tests proving observable behavior:

- `/connect` parses as a command without arguments;
- negotiated `ProviderConnections` sends exactly one `ConnectionsRequested` event;
- missing capability leaves the command visible but reports unsupported and sends nothing;
- secret dialog rendering displays one mask glyph per Unicode scalar and never the value;
- submit emits `DialogResponse::Secret`, not `Text`;
- submit and Esc cancellation clear/drop the secret buffer and pending dialog state;
- cursor insertion, backspace, delete, Home/End, and left/right operate on Unicode boundaries.

Confirm RED with targeted test commands.

### Step 2: Implement `/connect`

Add `SlashAction::Connect` and the slash-command descriptor. In command dispatch:

- reject arguments through the existing exact-command parser behavior;
- require negotiated `ProviderConnections`;
- send `ClientEvent::ConnectionsRequested` only while connected;
- do not put provider logic in `yach-ui`;
- add `ProviderConnections` to the exhaustive `UiCapabilities::supports`
  match; the backend still controls whether negotiation succeeds.

### Step 3: Implement a dedicated secret buffer

Do not reuse or clone the ordinary `TextArea` value. Add a private zeroizing secret-input state with a byte-index cursor constrained to character boundaries. Render only mask glyphs and cursor state. On submit, move the owned string into `SubmittedSecret`; on cancellation or replacement, zeroize/drop it.

Handle `DialogKind::SecretInput` through the existing pending-dialog overlay so connection setup remains backend-driven.

### Step 4: Verify GREEN

```bash
just dev cargo test -p yach-ui connect_command_requests_backend_flow
just dev cargo test -p yach-ui secret_dialog_masks_unicode_and_never_renders_value
just dev cargo test -p yach-ui secret_dialog_submit_and_cancel_clear_state
```

### Step 5: Review and checkpoint

Review rendered buffers, debug assertions, status strings, and every clone/format operation around the secret. Then:

```bash
jj describe -m "feat: add masked provider connection dialogs"
jj new
```

---

## Task 4: Add the backend connection runtime seam and dialog state machine

**Files:**
- Modify: `crates/yach-backend/Cargo.toml`
- Create: `crates/yach-backend/src/provider_connections.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-backend/src/runner.rs`

### Step 1: Define the runtime contract with failing tests

Add `yach-connections` as a dependency. Define object-safe async boundaries using boxed futures:

```rust
pub trait ProviderConnectionRuntime: Send + Sync {
    fn list(&self) -> ConnectionListFuture;
    fn cached_models(&self) -> Option<Arc<[CatalogModelEntry]>>;
    fn refresh_models(&self, active: Option<ActiveModelTarget>) -> ModelDiscoveryFuture;
    fn create(&self, draft: NewConnectionDraft, secret: ProviderSecret) -> ConnectionMutationFuture;
    fn repair(&self, id: ConnectionId, secret: ProviderSecret) -> ConnectionMutationFuture;
    fn replace(&self, id: ConnectionId, model: Option<String>, secret: ProviderSecret)
        -> ConnectionReplacementFuture;
    fn rename(&self, id: ConnectionId, label: Option<String>) -> ConnectionMutationFuture;
    fn remove(&self, id: ConnectionId) -> ConnectionMutationFuture;
    fn activate(&self, id: ConnectionId, model: String) -> ProviderActivationFuture;
}
```

Exact return structs must be secret-free and bounded. `replace` may return a fully built candidate `ProviderConfig` when the ID is active. The trait does not expose storage or keyring primitives to the UI.

Write failing fake-runtime tests for root-list rendering, add/cancel, validation failure, successful create returning to a fresh root list, repair, rename, confirmed remove, busy mutation rejection, active removal rejection, unrelated dialog responses being ignored, and a secret response leaving the session log byte-for-byte unchanged.

### Step 2: Implement a pure connection-flow reducer

In `provider_connections.rs`, represent the wizard explicitly:

- root list;
- provider choice;
- optional label;
- compatible base URL;
- secret entry;
- validating/mutating;
- stored-connection action;
- rename;
- remove confirmation;
- replace/repair secret.

Use fixed backend-owned dialog ID prefixes plus stable connection IDs in option values. A response is accepted only when its ID and expected response kind match the current state. Cancellation clears the draft and secrets without side effects.

### Step 3: Integrate async outcomes into the runner

Extend `RunnerConfig` with `Option<Arc<dyn ProviderConnectionRuntime>>`. In the native loop:

- handle `ConnectionsRequested` by opening the root dialog;
- route only connection-prefixed `DialogResolved` events to the reducer;
- start runtime futures without blocking the loop;
- poll connection operation and model-refresh completion in the existing
  `tokio::select!`;
- on `/model`, emit `cached_models()` immediately and independently start
  `refresh_models()`, then emit its generation-current result without requiring
  the picker to reopen;
- after every successful create/repair/replace/rename/remove, start the same refresh automatically;
- emit bounded `StatusUpdated` events and the next dialog;
- reject mutation while a provider turn is active;
- leave inspection/listing available while busy;
- assume capability negotiation was completed by CLI session setup before the
  runner starts; the runner must not advertise a second, ignored handshake.

No connection or credential loading occurs at runner construction or before
the first render event. Secret `DialogResolved` events are consumed directly by
the flow and are never appended to session/event logs.

### Step 4: Verify GREEN

```bash
just dev cargo test -p yach-backend provider_connection_flow_cancel_has_no_side_effects
just dev cargo test -p yach-backend provider_connection_flow_create_never_activates
just dev cargo test -p yach-backend provider_connection_flow_rejects_active_remove
just dev cargo test -p yach-backend secret_connection_response_never_enters_session_log
```

### Step 5: Review and checkpoint

Review state transitions, mismatched/stale dialog responses, busy-state races, future cancellation, and status redaction. Then:

```bash
jj describe -m "feat: orchestrate provider connection dialogs"
jj new
```

---

## Task 5: Implement the CLI runtime over Keyring, Rig, and catalog layers

**Files:**
- Modify: `crates/yach-cli/Cargo.toml`
- Create: `crates/yach-cli/src/provider_connections.rs`
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Modify: `crates/yach-backend/src/model_discovery.rs`

### Step 1: Replace raw adapter secrets first

Add failing tests that `RigProviderAdapterConfig` debug output never contains an Anthropic/OpenAI/OpenAI-compatible sentinel key. Change API-key fields from raw `String` to `ProviderSecret`. Expose the value only when calling the Rig builder. Keep ChatGPT subscription's token-directory configuration unchanged.

### Step 2: Add a lazy CLI runtime

Implement `CliProviderConnectionRuntime` with:

- registry path `~/.yach/connections.json` using the same HOME resolution convention as existing `~/.yach` files;
- `SystemCredentialStore` in production and injectable stores for tests/smoke;
- an optional transient environment connection made from every complete existing `YACH_RIG_*` shape: environment-sourced API keys remain in the transient adapter and ChatGPT subscription retains its token-directory path;
- cloned immutable catalog layers and the existing provider timeout/max-token/test-delay configuration;
- a generation-tagged availability cache shared as `Arc<[CatalogModelEntry]>`.

Construction records paths/configuration only. It performs no registry read, keyring call, or provider request.

Construct the runtime before `start_backend_session`, but keep its constructor
inert. The native backend handshake passed to `BackendEvent::Connected` includes
`ProviderConnections` only when this runtime exists; the default/non-native
backend handshake does not claim it. Add an integration test over the actual
native launch negotiation, not only protocol-level set intersection.

### Step 3: Implement list and mutation operations

Every registry/keyring operation runs inside `spawn_blocking`. The root list:

- shows Environment first when present;
- loads stored records in deterministic provider/display/ID order;
- marks pending or missing/locked credentials as unavailable/repairable;
- never includes platform error bodies.
- rejects creation once 64 stored records exist.

Creation/repair/replacement validation calls provider-native `/models` with the candidate secret and existing bounds/redaction. A successful empty model list authenticates but yields no selectable rows. Any validation failure leaves the wizard at the secret/base-URL step with typed guidance.

Every successful create/repair/replace/rename/remove increments the cache generation before returning; this prevents old-key or old-label discovery already in flight from publishing. The backend then calls `refresh_models` and publishes the fresh result without another `/model` open. Setup does not return a candidate active config. Active replacement validates and constructs the candidate config before keyring `put`, then returns that config for the runner's no-more-fallible-work swap.

### Step 4: Implement multi-connection discovery

The runtime exposes two distinct paths:

- `cached_models()` synchronously clones only the last immutable `Arc` snapshot
  and performs no I/O;
- `refresh_models()` resolves ready credentials through blocking tasks, consumes
  the deterministic maximum of 64 stored connections plus the one environment
  connection, and returns one newly completed generation-tagged snapshot.

Refresh behavior:

- run at most eight provider discoveries concurrently with the existing per-provider timeout and 2,048-result/ID bounds;
- isolate each connection's failure;
- keep ChatGPT subscription active-only because Rig model listing is unsupported;
- reuse slice-3 known-non-generation filtering and provider/model catalog join;
- sort connections by provider/display/ID and models by ascending ID;
- retain at most 4,096 rows across the immutable snapshot, report deterministic truncation, and never drop the exact active row;
- place the exact active tuple first;
- assign the exact connection ID/display label to every `CatalogModelEntry`/`ModelInfo`;
- publish/cache only if the result generation is still current.

Do not deep-clone model metadata on each prompt or compaction request.

### Step 5: Add provider-fixture tests and verify

Use local Anthropic/OpenAI-compatible fixtures and fake credential/metadata stores to prove:

- two connections exposing the same model produce two exact rows;
- one auth/timeout failure does not hide the successful peer;
- environment ChatGPT remains represented without persisted subscription auth;
- provider response bodies and submitted keys never enter errors/debug/status;
- every mutation invalidates the old cache and stale in-flight results cannot overwrite the new generation;
- replacement invalidates old-key discovery and refreshes without reopening
  `/model`;
- rename invalidates old-label rows and refreshes deterministic display/order
  without reopening `/model`;
- cached rows are emitted immediately while a fresh generation is later emitted
  during the same picker open;
- successful create/repair/remove refreshes and emits availability without
  reopening `/model`;
- 64 connections with eight in flight never retain more than 4,096 rows and preserve the exact active row;
- native launch negotiates `ProviderConnections` iff the inert runtime exists.

```bash
just dev cargo test -p yach-cli provider_connections::tests
```

### Step 6: Review and checkpoint

Review secret lifetimes, `spawn_blocking` boundaries, stale-generation races, ordering compatibility with slice 3, and startup laziness. Then:

```bash
jj describe -m "feat: resolve connected providers through catalog and keyring"
jj new
```

---

## Task 6: Make model activation and current state connection-aware

**Files:**
- Modify: `crates/yach-backend/src/runner.rs`
- Modify: `crates/yach-cli/src/provider_connections.rs`
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-ui/src/app.rs`
- Modify: `crates/yach-ui/src/model_selector.rs`
- Modify tests in those modules

### Step 1: Add failing duplicate-target UI tests

Create two model rows with identical provider/model IDs but different connection IDs. Assert:

- only the exact active connection row has `(current)`;
- moving and pressing Enter emits the selected row's exact connection ID;
- initial backend state and later `ModelChanged` update the same exact active tuple.

### Step 2: Add failing A → B → A runtime regression

Using two local compatible endpoints and distinct sentinel keys, explicitly select A/model, B/model, then A/model. For every transition assert the next request uses that target's:

- endpoint;
- credential;
- model ID;
- context window;
- output budget;
- output-token parameter spelling.

Also assert a failed activation retains the entire prior `ProviderConfig` and exact active tuple.

### Step 3: Implement atomic activation

Extend `ProviderConfig` with stable connection ID/display data. When a connection runtime exists, handle `ModelSelectedDetailed` only when both connection ID and model ID match an advertised current snapshot. Ask `ProviderConnectionRuntime::activate` for a fully resolved candidate; swap only on success and only while idle. When no connection runtime exists, preserve the existing provider/model-only selection path and `None` connection IDs for legacy/non-native adapters.

Emit `ModelChanged` with connection/provider/model identity, update `BackendState`, and ensure prompt/compaction/session-stat paths read metadata from the newly active config. Do not persist a default connection/model.

For active credential replacement, swap the already-built candidate config immediately after successful storage. Reject active removal. Rename updates display state without reconstructing the adapter.

### Step 4: Update picker current-row logic

Track `model_connection_id` separately from display labels. Native rows compare `(connection_id, model_id)` exactly; never infer current state from label, provider/model alone, or connection display name. When both the active state and a legacy row have no connection ID, retain the existing provider/model fallback so protocol compatibility does not disable non-native pickers.

### Step 5: Verify GREEN

```bash
just dev cargo test -p yach-ui model_selector_marks_only_exact_connection_current
just dev cargo test -p yach-backend connection_activation_failure_preserves_prior_config
just dev cargo test -p yach provider_connection_switch_a_b_a_restores_complete_config
```

### Step 6: Review and checkpoint

Review every constructor and adapter/non-native compatibility path; the required LSP reference searches happen before each exported-symbol change, not here after migration.

```bash
jj describe -m "feat: switch exact provider connection model targets"
jj new
```

### Task 6 completion ledger

- 2026-08-04: completed exact runtime activation-state integration; evidence and concerns: `docs/project/records/2026-08-04-task-6-provider-activation-report.md`.
- Verified the required UI, backend, and CLI commands; no Task 7 process tests were run.

---

## Task 7: Prove restart durability and the live TUI path

**Files:**
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-cli/src/headless.rs` only if shared outcome plumbing requires it
- Modify/create CLI integration-test support

### Step 1: Add a two-process durability acceptance test

Use a test-only subprocess helper and one temporary registry plus file-backed
fake credential store. Spawn the integration-test executable itself in a
child-helper mode; do not add a production plaintext credential option:

- Process A creates a connection, reports success, and exits.
- Process B starts with no in-memory state, opens `/connect`, lists the saved
  connection, discovers it, explicitly activates a model, and completes a
  streaming prompt against the local provider fixture.

The test must fail if a fresh process never reloads metadata, if key lookup uses a non-stable account, if the fixture credential is process-local, or if activation accidentally depends on Process A's cache.

### Step 2: Add a hidden live smoke mode

Follow the existing dialog/native-provider smoke-command pattern. Add one hidden `yach` smoke command that:

- starts an in-process local OpenAI-compatible `/models` plus streaming response fixture;
- injects a fresh temporary JSON registry and in-memory credential store;
- launches the real native runner and Ratatui UI;
- accepts the API key only through the masked dialog;
- records whether the fixture received the expected key/model and whether a prompt completed.

This is a real smoke harness, not a production fallback. It must use the same backend state machine, CLI runtime, discovery, picker, activation, and request adapter as normal execution.

### Step 3: Drive the smoke end to end

Launch the smoke under a PTY and exercise:

1. `/connect`;
2. Add connection → OpenAI-compatible;
3. label and fixture base URL;
4. type a Unicode-containing sentinel key and confirm only masks render;
5. observe success without active-model change;
6. `/model`, select the discovered exact row;
7. send a prompt and observe the expected streamed completion;
8. reopen `/connect` and remove only after switching away or confirm active removal is rejected.

Capture the command and observed outcome for the eventual measurement record. Never include the sentinel key in captured output.

### Step 4: Review and checkpoint

Review the smoke for production-code reuse and absence of fake success branches. Then:

```bash
jj describe -m "test: prove provider connections across restart and TUI"
jj new
```

### Task 7 completion ledger

- 2026-08-04: hardened restart acceptance after review; evidence and concerns: `docs/project/records/2026-08-04-task-7-provider-restart-report.md`.
- The bounded fixture proves A validation, B discovery, and B prompt use exactly three requests; child output is captured and rejects the test sentinel without reproducing it.
- 2026-08-05: fixture lifecycle follow-up uses nonblocking accepts and a caller-supplied overall deadline, then returns a completion receiver and worker handle. Restart acceptance uses a ten-second fixture deadline and an eleven-second parent wait; the missing-request regression uses 200 ms and a one-second parent wait. Parent assertions join the worker before reading observations; the focused missing-request regression, segmented reader regression, and restart acceptance each passed (1 passed, 110 filtered).
- 2026-08-05: absolute-deadline follow-up restores blocking mode on accepted sockets, passes the same `Instant` into every fixture read, and recomputes the socket timeout before each read, preventing slow partial requests from extending the overall deadline. A parent completion timeout sets a shutdown token, wakes an active socket, and joins the worker. The slow-drip, parent wake-and-join, missing-request, segmented-reader, and restart acceptance regressions each passed (1 passed, 111 filtered).

---

## Task 8: Final integration, review, and measurement

This task starts only after Task 7's live smoke succeeds.

### Step 1: Run formatter and focused static checks

```bash
just fmt
just fmt-check
just lint
just check
```

Fix only real findings. Do not weaken lints or add allowances.

### Step 2: Run the full suite

```bash
just test
```

Then rerun the exact restart and A → B → A tests after any fix.

### Step 3: Run behavioral gates

Run the exact evaluator and startup-profile commands used by the catalog arc:

```bash
just eval-validate
just dev cargo run -p yach-bench -- yach-tui-startup-profile-report --samples 10
```

Required observations:

- all evaluator oracles pass;
- startup profile still reaches first render without registry/keyring/provider work;
- opening `/connect` performs metadata/credential status work only then;
- opening `/model` performs discovery only then;
- no key or provider response body appears in captured output.

The 125-cell provider sweep remains deferred to the pre-release gate per the accepted owner decision; do not represent it as a missing slice result.

### Step 4: Request final whole-stack review

Review from the parent before this plan through `@`, with emphasis on:

- secret wire/debug/session-log leakage;
- keyring and registry failure recovery;
- active replacement/removal behavior;
- duplicate provider/model rows;
- stale async cache publication;
- legacy environment providers, especially ChatGPT subscription;
- first-render laziness.

Fix every Important/Critical finding and re-review until Ready.

### Step 5: Create the measurement record and update active planning

After all checks pass, create `docs/project/records/2026-08-XX-provider-connections-measurement.md` with exact commands, smoke outcome, test counts, startup observations, and deferred sweep policy. Update `docs/project/board.md` and `docs/project/next.md` only where completion changes current status or recommended next work.

### Step 6: Shape the publishable change

Inspect:

```bash
jj log -r 'main..@'
jj diff --from main --to @ --stat
```

Squash implementation/fix checkpoints into one reviewable feature change while preserving any prerequisite slice-3 commit as its own ancestor. Describe the feature intent, create a publishable bookmark, push with `jj git push`, open the PR, and report its full URL.
