# Provider Connections and API-Key Setup

**Date:** 2026-08-03
**Status:** Accepted
**Prior work:** model-catalog hydration slices 1-3, especially provider-native
model discovery and the key-truthful `/model` picker. Board: “Provider/model
product surface — auth/connect flows, provider config, model discovery.”

## Problem

yach can discover and switch models once one provider has already been
configured, but provider setup still begins outside the product with
`YACH_RIG_*` environment variables. A first-time user must know provider labels,
credential variable names, endpoint shapes, and model variables before the TUI
can do useful work. The existing picker also identifies a model only by
provider/model, which cannot distinguish two accounts or two compatible
endpoints using the same provider shape.

The catalog and discovery layers now know how to list models truthfully. The
missing product layer is a durable connection identity, secure credential
storage, and an in-session setup flow that makes those discovered models
available without silently changing the user’s active model.

## Cohort evidence (source-verified, 2026-08-03)

- **Pi** keeps login credentials separate from model configuration and changes
the active model after login only when there was no known model. Providers
advertise API-key and OAuth capabilities. This is the closest behavioral match:
connecting is additive, not an implicit model switch.
- **OpenCode** lets provider integrations advertise their authentication
methods and refreshes provider availability after a successful `/connect`.
Its credential map is provider-ID keyed, which is too narrow for multiple
accounts or endpoints.
- **Crush** gives the picker a good continuation: selecting an unconfigured
model opens authentication, validates, then resumes the original selection.
It writes credentials and OAuth tokens into a mode-0600 JSON file, which yach
will not copy.
- **OMP** gives credentials durable row identities, permits multiple
credentials per provider, and separates a non-mutating discovery credential
lookup from request-time credential selection. Its rotation, ranking, usage,
and refresh machinery are later concerns, not first-slice requirements.
- **Claude Code** keeps `/login` and `/model` separate. That simple vocabulary
remains understandable even when the picker offers a shortcut into setup.

## Owner decisions

1. The primary entry point is a TUI-first `/connect` flow.
2. API keys go into the operating system credential store. yach must not fall
back to plaintext credential persistence when that backend is unavailable.
3. A provider may have multiple connections. Every connection has an opaque,
stable generated ID and an optional human label.
4. The first slice supports API-key authentication for Anthropic, OpenAI, and
OpenAI-compatible endpoints. ChatGPT subscription/OAuth is deferred to its own
lifecycle slice.
5. A successful connection becomes available to discovery immediately but
never changes the active or default model. Connecting a provider for later use
or future role/subagent routing is valid.
6. Existing `YACH_RIG_*` behavior remains a noninteractive compatibility path.
Environment credentials are never copied into the system credential store.

## Design

### Connection identity and metadata

A connection identifies the configuration needed to construct one provider
client, excluding its secret:

```rust
pub struct ProviderConnection {
    pub id: ConnectionId,
    pub provider: ProviderKind,
    pub label: Option<String>,
    pub base_url: Option<String>,
    pub authentication: ConnectionAuth,
    pub state: ConnectionState,
}

pub enum ProviderKind {
    Anthropic,
    OpenAi,
    OpenAiCompatible,
    ChatGptSubscription,
}

pub enum ConnectionAuth {
    ApiKey { source: CredentialSource },
    ChatGptSubscription { token_dir: PathBuf },
}

pub enum CredentialSource {
    System,
    Environment,
}

pub enum ConnectionState {
    PendingCredential,
    Ready,
}
```

Stored `ConnectionId` values are generated UUIDs and are the canonical
identity. The one transient startup connection uses the reserved
`environment` ID. `CredentialSource::Environment` and
`ConnectionAuth::ChatGptSubscription` are valid only for that read-only
environment record: the registry rejects either in persisted metadata. An
environment API key remains in the transient runtime adapter, while the
subscription record retains its configured token-directory path. Slice 1 thus
represents every existing runtime shape without pretending to implement
subscription setup.
Labels are presentation only: they
may be changed or duplicated without breaking model references. Ambiguous
labels are rendered with the provider and a short ID. Provider/model strings
remain catalog identities; a selectable runtime target becomes
`(connection_id, provider, model_id)`.

The persisted registry is machine-managed `~/.yach/connections.json`, separate
from the human/project policy file `config.json` and from `models.toml` catalog
corrections:

```json
{
  "schema": "yach.connections.v1",
  "connections": [
    {
      "id": "1af6…",
      "provider": "anthropic",
      "label": "work",
      "authentication": { "kind": "api_key", "source": "system" },
      "state": "ready"
    },
    {
      "id": "99c2…",
      "provider": "openai-compatible",
      "label": "local gateway",
      "base_url": "https://gateway.example/v1",
      "authentication": { "kind": "api_key", "source": "system" },
      "state": "ready"
    }
  ]
}
```

The registry uses an advisory cross-process lock and an atomic write with
restrictive user-only permissions (mode 0600 on Unix). It preserves the previous
valid file on parse or write failure. It contains no API keys, access tokens, or
secret-manager output. First-slice connections are user-global only; project
connection declarations and project defaults are deferred. Labels are bounded
to 80 Unicode scalar values. Base URLs are bounded to 2,048 bytes, must use
`http` or `https`, and reject user info, query strings, and fragments so
credentials cannot accidentally enter metadata. Normalization removes trailing
slashes but does not invent or strip a provider-specific path.

The registry accepts at most 64 stored connections. A file exceeding that cap
is malformed rather than partially loaded, and `/connect` rejects a 65th create
with a bounded status. This cap bounds both persisted state and discovery work.

### Credential storage and resolution

The credential store is a backend interface, not TUI logic:

```rust
pub trait CredentialStore: Send + Sync {
    fn put(&self, connection: &ConnectionId, secret: &ProviderSecret)
        -> Result<(), CredentialError>;
    fn get(&self, connection: &ConnectionId)
        -> Result<Option<ProviderSecret>, CredentialError>;
    fn remove(&self, connection: &ConnectionId)
        -> Result<(), CredentialError>;
}
```

Production uses the platform credential service under service `yach` and an
account derived only from the stable connection ID. Tests use an in-memory
implementation. `ProviderSecret` has redacted `Debug`, is never serializable,
and replaces raw `String` credentials in debug-derived adapter configuration.
The request adapter exposes the bytes only at the provider-client construction
boundary.

Credential-store calls can block or display an operating-system prompt. The
native async loop therefore executes them through a blocking-task boundary;
no credential access runs on the Tokio worker that handles rendering or
provider-stream events.

Every complete `YACH_RIG_*` startup shape—including
`chatgpt-subscription`—is represented as one read-only transient environment
connection. It remains the startup-active provider and outranks stored metadata
for that invocation. It is visible in connection/model surfaces as
“Environment,” cannot be renamed or removed, and is never persisted. Stored
connections are additive availability; they do not replace an
environment-selected provider merely because they exist.

A missing or locked system credential is a connection status, not absence:
`credential unavailable`. `/connect` can repair it by replacing the secret.
When no platform backend is available, setup fails with actionable guidance to
use the existing environment path; plaintext fallback is prohibited.

### `/connect` flow

`/connect` opens a connection list with `Add connection` plus every stored and
transient environment connection. Selecting a stored connection offers:

- replace API key while preserving the ID;
- rename or clear its optional label;
- remove it after confirmation.

Adding a connection is a backend-driven generic-dialog sequence:

1. Choose Anthropic, OpenAI, or OpenAI-compatible.
2. Enter an optional label.
3. For OpenAI-compatible, enter and normalize its API base URL.
4. Enter the API key through a masked secret-input dialog.
5. Validate with the existing bounded provider-native model-listing path.
6. Persist the secret and metadata, refresh discovery, and report success.

The TUI knows how to render selection, ordinary input, confirmation, and masked
secret input; it does not contain provider-specific setup rules. The backend
owns a small connection-flow state machine and advances it from typed dialog
responses. Cancelling at any step has no side effects.

Validation does not send a paid completion. Authentication, malformed URL,
network, and provider response failures remain typed and redact the submitted
secret. The first slice requires successful `/models` validation for a new
OpenAI-compatible connection. Compatible endpoints without model listing keep
working through the existing environment path; supporting interactive manual
model declaration is a later product decision rather than an unverified secret
save.

Creation uses a durable pending record so a fallible keyring rollback cannot
create an invisible secret. Every create, repair, replace, rename, and remove
first acquires an advisory cross-process lock for that stable connection ID,
then reloads and rechecks its registry state. The ID lock remains held across
all keyring and registry steps, preventing replace/remove or repair/remove races.
Registry writes additionally take the global registry-file lock so mutations of
different IDs cannot lose one another.

Creation proceeds:

1. atomically persist metadata as `pending_credential`;
2. write the credential;
3. atomically mark the record `ready`.

Failure before step 2 leaves no secret. Failure in step 2 or 3 leaves a visible
pending record that `/connect` can repair or remove; startup never uses pending
records for discovery. Removal deletes the credential first; if metadata
removal then fails, the visible unavailable entry remains repairable.
Replacing a key validates the candidate before acquiring the ID lock, then
rechecks the record under the lock before overwriting the previous secret; it
does not change metadata or connection identity.

### Discovery, picker, and activation

Opening `/model` retains the current lazy behavior. The backend resolves ready
credentials and runs provider-native discovery with at most eight requests in
flight. The registry's 64-connection cap, existing per-provider timeout and
2,048-result/ID bounds, and a 4,096-row aggregate snapshot cap bound the work
and retained result. Aggregate truncation follows deterministic
connection/model order, never drops the exact active row, and reports one
bounded status. The current stale/active list remains visible while refresh is
pending. One failed connection does not suppress successful peers.

Every returned `ModelInfo` carries `connection_id` and a backend-resolved
connection display label. Picker ordering is:

1. the active `(connection, model)` target;
2. other models on the active connection;
3. remaining connections ordered by provider, display label, and stable ID;
4. models ordered by ID within each connection.

Known non-generation filtering and catalog metadata joining remain supply-side
behavior from catalog slice 3. Catalog lookup still keys on provider/model; the
connection ID selects credentials and endpoint only.

Selecting a model sends its exact connection ID and model ID. The backend
resolves that credential, constructs the adapter, applies that model’s catalog
profile, and switches the current session only after all steps succeed. Failure
leaves the prior active adapter/model intact. A → B → A must restore each
connection’s endpoint, secret, context window, output budget, and output-token
parameter.

The active target is connection-aware everywhere: initial `BackendState` and
`ModelChanged` confirmations carry the connection ID alongside provider/model,
and the picker compares the exact tuple. Two connections exposing the same
provider/model therefore never both render as current.

A successful `/connect` refreshes availability and returns to the connection
list. If setup was entered as a continuation from an unavailable picker row, it
may reopen `/model` focused on the original target, but selection remains an
explicit Enter action. This slice does not add or mutate a persisted default
model.

Replacing the credential on the active connection is allowed only while idle.
The backend validates and fully constructs a candidate adapter first, writes
the new key, then swaps the in-memory active configuration with no later
fallible step; the next prompt uses the replacement. Removing the active
connection is rejected with guidance to select another connection first.
Renaming it changes display metadata and active-state presentation only.

### Protocol and ownership

The protocol extends the existing generic-dialog path rather than building a
second connection-management UI protocol:

- a negotiated `ProviderConnections` capability;
- `ConnectionsRequested`, which asks the backend to open the root `/connect`
  dialog;
- existing select/input/confirm dialogs for the list and management steps;
- a masked secret-input dialog kind and a distinct
  `DialogResponse::Secret { value: SubmittedSecret }`;
- `connection_id` on selectable `ModelInfo`, detailed model selection, initial
  `BackendState`, and `ModelChanged` confirmation.

Dialog option values carry stable connection IDs, so labels remain presentation
only. Success/failure uses the existing status event, and the backend reopens a
fresh root dialog after a mutation. The backend owns connection metadata,
credential access, validation, discovery, and active adapter replacement.
`yach-cli` supplies filesystem/platform implementations and translates the
legacy environment configuration into a transient connection. `yach-ui` owns
rendering and input state only.

No secret enters session JSONL, status text, transcript rows, catalog caches,
model metadata, debug output, or connection JSON. Protocol transports carry a
submitted secret only in the distinct direct dialog response needed to reach
the backend; that variant's `Debug` and any transport/session logging render
only `[REDACTED]`. The TUI clears both its secret buffer and pending dialog state
on submit or cancel.

`SubmittedSecret` is deliberately serializable for the direct wire hop, exposes
its value only to the backend conversion into non-serializable
`ProviderSecret`, and implements redacted `Debug`; the enclosing client event's
derived `Debug` is therefore redacted too. Session/event persistence explicitly
skips the secret response rather than serializing it for later replay.

## Failure behavior

- Missing `connections.json`: start with no stored connections and no warning.
  Malformed/unreadable registry: keep the environment provider usable, show one
  bounded setup warning, and do not overwrite the bad file.
- Credential-store unavailable/locked/denied: show the affected connection as
  unavailable; do not delete metadata or fall back to plaintext.
- Validation failure: keep the wizard open at the relevant field, preserve no
new metadata, and render only typed redacted guidance.
- Discovery timeout/failure: retain other connections and the active target;
show per-connection failure status without exposing response bodies or keys.
- Model activation failure: retain the old active adapter and model atomically.
- Pending credential record on startup: display it as repairable, but never
  resolve it for discovery or activation.
- Active-connection removal: reject it until another connection/model is
  active. Active-key replacement either installs both the stored key and
  candidate adapter while idle or preserves the prior active adapter.
- Busy provider turn: `/connect` may inspect connections, but mutation and model
  activation are rejected until the turn is idle, matching other backend-owned
state transitions.
- Missing `ProviderConnections` negotiation: `/connect` remains visible but
returns a clear unsupported status without sending an event.

## Validation

1. Registry unit tests: schema round-trip, stored UUID and reserved environment
ID/auth-source rules, the 64-record cap, cross-process-safe atomic writes,
restrictive permissions, malformed-file preservation, URL/label bounds,
duplicate-label display, and pending-record recovery after every injected
write/keyring failure. Race tests interleave replace/remove and repair/remove
from separate store handles and prove no orphaned secret or lost metadata.
2. Credential tests through an in-memory store: put/get/remove, unavailable
backend, missing key, replacement preserving ID and the old key on failed
validation, plus no secret in `Debug` or serialized metadata.
3. Protocol round-trips for connection requests and connection-aware models;
secret-dialog tests prove masking, redacted `DialogResponse` and complete
`ClientEvent` debug output, skipped persistence, and buffer clearing on submit
and cancellation.
4. Backend state-machine tests for create/cancel/repair/rename/remove and the
invariant that successful connection does not change the active model. Replacing
an active key changes the next request's credential; removing the active
connection is rejected.
5. Discovery tests with multiple connections: partial failure isolation,
active-first ordering, exact connection IDs, duplicate provider/model IDs,
deterministic ascending model-ID order, and bounded/redacted statuses.
6. Runtime regression: select A → B → A and assert endpoint, credential, model,
context window, output budget, and output-token parameter all restore.
7. Restart acceptance with one temporary registry and fake credential store:
run A creates a connection and exits; run B loads it, lists it, discovers its
models, explicitly activates one, and completes a streaming prompt.
8. Live TUI smoke with an in-memory credential store and local mock provider:
connect, observe newly discovered models, explicitly select one, and complete a
streaming prompt.
9. Existing `just fmt-check`, `just lint`, `just test`, `just check`, evaluator
oracle gate, and startup profile. Credential access and discovery remain off the
first-render path. The 125-cell sweep remains the recorded pre-release gate,
not a per-slice requirement.

## Risks

- **Secret leakage through derived debug or protocol logging.** Mitigated by a
non-serializable redacted runtime secret, a serializable-but-debug-redacted
one-hop wire wrapper, skipped session persistence, and negative tests over
rendered/debug output.
- **Partial persistence or mutation races across keyring and filesystem.**
Mitigated by per-ID cross-process locks spanning each transaction, global
registry-write locking, state rechecks, durable pending records, and injected
failure/race tests.
- **Picker latency multiplied by connections.** Discovery is lazy, bounded to
64 configured connections, eight in flight, 2,048 results per provider, and a
4,096-row immutable snapshot; failures remain isolated and stale/active choices
remain visible while it runs.
- **Protocol and runner scope.** Connection identity touches the CLI, backend,
protocol, and TUI. The implementation plan must land the typed domain/store
before rewiring selection, then prove the complete interactive path before
cleanup.

## Non-goals

- ChatGPT subscription, OAuth browser/device flows, refresh tokens, or logout
semantics beyond deleting an API key.
- Credential rotation, ranking, usage-aware selection, or multiple secrets
inside one connection.
- Provider roles, subagent routing, fallback chains, or per-task model policy.
- Project-scoped connections, team credential sharing, remote vaults, or secret
sync.
- Persisted default-model/default-connection changes.
- Manual interactive setup for OpenAI-compatible endpoints without `/models`.
- Removing `YACH_RIG_*`; it remains the CI/headless/noninteractive path.

## Slices

1. **API-key connections, end to end.** Registry and system credential store,
`/connect`, connection-aware multi-provider discovery/picker, atomic current-
session activation, and the live local-provider smoke described above.
2. **Subscription/OAuth lifecycle.** Provider-advertised auth methods, OAuth
storage/refresh/logout, and ChatGPT subscription migration.
3. **Defaults and routing policy.** Persistent default connection/model, roles,
subagents, and fallback/rotation only after their consumers define the required
semantics.

Each slice lands separately with its own focused verification record. Slice 1
is the implementation target for this design.
