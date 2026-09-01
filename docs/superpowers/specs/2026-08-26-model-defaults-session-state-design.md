# Model Defaults And Session Model State Design

Status: accepted 2026-08-26 — owner decisions incorporated

## Problem

Yach currently persists the last successful interactive model activation in
`~/.yach/active-model.json`. The record contains a connection UUID and model ID,
and the native runner restores it after first render. This makes one
machine-global mutable value serve two different roles:

- the user's default model for a new session; and
- the active model of an existing session.

The distinction matters. A temporary `/model` change must not rewrite future
sessions, while changing a global default must not rewrite an existing
transcript's execution context. Existing assistant entries record provider and
model metadata, but the session log has no explicit setting event from which to
project the active model before or between turns.

The recent thinking-level implementation proves the required ownership split:
a global default seeds new sessions, explicit changes are durable session
evidence, and resumed session evidence wins. Model state needs the same split,
with an additional connection-resolution problem.

## Decisions

1. The user model default and the session-active model are separate state.
2. `~/.yach/active-model.json` is retired through an idempotent migration; it is
   not renamed into the new configuration format.
3. Human configuration uses TOML at `~/.yach/config.toml`. This slice introduces
   one typed user-config boundary and migrates the existing thinking default to
   it. It does not absorb catalog overrides from `models.toml`, connection
   records, permissions, project trust, or theme configuration.
4. An ordinary picker model-row activation changes only the current session.
   A separate selectable footer action, `Activate and save as default`, performs
   both operations. Model search has no prefixed save-default shortcut.
5. If no user default exists, the first successful picker activation also
   attempts to establish the default automatically.
6. Connection labels remain mutable, non-unique presentation. An optional,
   immutable connection key is the user-facing configuration identity when a
   provider has multiple connections. Human config never requires a connection
   UUID.
7. A configured or session target that cannot be resolved fails closed. Yach
   keeps the requested target visible and presents an actionable selection or
   repair path; it never silently substitutes another provider, connection, or
   model.
8. Session activation and default persistence are separate commits. If session
   activation succeeds but the default write fails, the session remains active,
   the prior default remains unchanged, and the partial outcome is reported.
9. Explicit CLI overrides on resume become the active session model and append
   the same durable session event as an interactive activation. They are not
   unrecorded one-turn exceptions.
10. Core owns resolution, activation, session evidence, default persistence,
    and failure classification. TUI, headless, and RPC callers express intent;
    they do not duplicate precedence or storage logic.

## State Model

Four concepts remain distinct.

### Configured default

A user-owned preference used to initialize a session that has no durable model
setting. It contains a canonical provider ID, model ID, and optional connection
key plus source provenance. It is not historical evidence.

### Session target

The exact target selected for one transcript: canonical provider ID, model ID,
and required `ConnectionId`. Stored connections use their UUID; transient
environment/bootstrap credentials use the reserved `environment` identity.
Stored targets also carry an optional immutable key snapshot. A session target
is therefore never provider-only and cannot rebind merely because the set of
connections changes. The key snapshot is diagnostic only for session replay.

### Active runtime target

The credential-bearing `ProviderConfig` currently installed in the runner. It
is derived from a resolved target and never persisted with secrets. Runtime
activation is valid only when it agrees with the session projection.

### Turn execution metadata

The provider/model metadata recorded on assistant entries describes what an
actual request used. It remains distinct from the configured default and the
session setting. This slice does not introduce a generic route/catalog layer;
if a future provider route uses an API model identity different from the
session's canonical model ID, the route identity belongs in per-turn provider
metadata rather than in the user preference.

## Connection Identity

`ConnectionId` remains the opaque UUID identity for credentials, mutations,
active runtime state, and exact session replay.

Add `ConnectionKey`, an optional persisted field on `ProviderConnection`:

- canonical syntax: `^[a-z][a-z0-9_-]{0,63}$`;
- `environment` is reserved;
- uniqueness is enforced within one provider, including retired keys;
- a key may be assigned once to a stored connection and cannot be renamed;
- labels remain independently renameable and may collide;
- removing a connection retires its key permanently; the registry retains a
  tombstone so later connections cannot acquire it;
- the transient environment connection cannot have a key.

The connection registry advances to a v2 document that contains active
connections plus retired `(provider, key)` tombstones. Loading v1 treats every
existing connection as unkeyed; the first successful mutation writes v2. Key
assignment and retirement occur under the registry's existing global mutation
lock and atomic-write/durability path, so concurrent processes cannot assign the
same key or lose a tombstone. Tombstones contain no credential or endpoint
metadata. The registry retains them indefinitely subject to the registry's
overall document-size safety bound; exceeding that bound rejects further keyed
mutations rather than permitting identity reuse.

The registry continues to own connection metadata. `config.toml` references a
key but does not define connections.

A single usable connection for a provider resolves without a key. If more than
one usable connection can serve the provider, configuration must name a key.
Provider-only resolution succeeds only when it identifies exactly one usable
connection. When the selected connection already has a key, a newly saved
default includes it even if provider-only resolution would currently be unique;
omitting `connection` is only for the ordinary unkeyed one-connection case.

The connection domain remains tolerant of existing registries containing
multiple unkeyed connections. They load with a categorical `key_required`
diagnostic instead of invalidating the whole registry. The connection UI adds a
one-time `Set configuration key` action. New creation of a second stored
connection for the same provider requires keys for both the existing and new
connection before the second connection becomes ready. No automatic key is
invented from a mutable label.

Session logs store UUIDs, not only keys. A removed connection therefore leaves
an old session unresolved even if its retired key remains visible in evidence.
The user must make an explicit model selection to append a new session target.

Adding a second ready connection can turn an existing provider-only default
into `connection_key_required`. Yach reports that transition and fails closed
for new sessions; it does not guess that the formerly unique connection should
remain selected. Assigning keys alone does not rewrite the default—the user
selects a keyed target and saves it explicitly.

## User TOML Boundary

The path is `~/.yach/config.toml`:

```toml
[thinking]
default = "high"

[model.default]
provider = "openai-codex"
model = "gpt-5.6-sol"
# Required only when provider-only resolution is ambiguous.
connection = "personal"
```

`provider` and `model` are required nonempty bounded strings. `connection` is an
optional `ConnectionKey`. Unknown tables and keys are preserved. The absence of
`connection` means “resolve exactly one currently usable connection for this
provider”; it never means an unspecified session target.

One backend-owned `UserConfigStore` replaces section-specific JSON readers and
writers. It uses `toml_edit` for document-preserving parse and targeted updates,
and `fs2` for the same advisory exclusive-lock convention already used by the
connection registry. Unrelated fields, comments, and ordering remain intact. A
targeted update must not deserialize and rewrite the whole document through a
lossy typed serializer.

Storage behavior:

- parent directory is created private (`0700` on Unix);
- config, temporary, and lock files are created `0600` on Unix;
- existing symlinks and non-regular config or lock paths are rejected;
- on Unix, an existing config or lock file must be owned by the effective user
  and have no group/other permission bits; otherwise loading or mutation returns
  `unsafe_permissions`;
- on platforms where equivalent ownership inspection is unavailable, Yach
  applies its strongest supported user-private creation mode and reports that
  the stronger Unix ownership guarantee is unavailable rather than claiming it;
- updates take an exclusive lock, re-read under the lock, parse, modify one
  typed path, write and fsync a same-directory temporary file, rename it, and
  sync the parent directory where supported;
- malformed TOML, wrong field types, unsafe paths, lock failures, and write or
  durability failures are categorical diagnostics;
- a malformed present TOML file never silently falls back to legacy JSON.

A malformed user TOML file makes the default state `unresolved(invalid_config)`
and disables legacy/default/environment fallback for a session that needs those
layers. It does not invalidate a higher-precedence explicit CLI target or an
exact projected session UUID target; those may activate while the backend also
publishes the config diagnostic. Saving a default remains unavailable until the
file is repaired.

The model-default slice does not add project model defaults. Project trust,
restriction/merging rules, and repository overrides require a later layered
configuration design. Existing `~/.yach/models.toml` and `.yach/models.toml`
remain catalog metadata override files, not default-selection files.

## Durable Session Evidence

Add one append-only event:

```text
SessionModelChanged {
    session_id,
    target: {
        provider,
        model,
        connection_id, # stored UUID or reserved "environment"
        connection_key_snapshot?,
    },
    source: default | explicit | cli_override | bootstrap | legacy_projection | fork_inherited,
}
```

The event is appended only after provider activation succeeds. Before swapping
the live provider, the runner appends and fsyncs the event. If append fails, the
candidate is discarded, the previous provider remains active, and the model
change fails. If append succeeds and the process exits before the in-memory
swap, resume projects and restores the appended target.

Projection is last-event-wins across the complete log. Model setting events:

- are not provider transcript messages;
- are not removed or hidden by compaction checkpoints;
- survive masking and transcript rendering changes;
- do not require an assistant entry to exist;
- allow a model change between turns to be resumed exactly.

Old logs without a model event remain valid. A non-empty legacy log first tries
to infer its target from the last assistant entry carrying provider/model
metadata. Inference succeeds only when provider/model resolves to exactly one
usable connection; the runner then activates it and appends a
`legacy_projection` event. Ambiguity, missing connection, or unavailable model
fails closed into selection. An empty log follows ordinary new-session
precedence. A legacy resume never silently adopts a newer user default merely
because the explicit event did not exist in the old schema.

A future fork inherits the projected target at the fork point. If a fork copies
the parent prefix, the relevant event is already present. If it creates a new
log plus lineage, it appends one `fork_inherited` event before accepting a
prompt. The child never consults a mutable parent session at execution time.

## Resolution And Precedence

### New or empty session

1. Explicit invocation/CLI target.
2. User TOML default.
3. Explicit environment/provider bootstrap.
4. Unconfigured state.

### Existing or resumed session

1. Explicit invocation/CLI target.
2. Last projected session target.
3. User TOML default, but only when the log has no model-setting event.
4. Explicit environment/provider bootstrap.
5. Unconfigured state.

An explicit override is resolved and activated through the normal backend path,
then appended with source `cli_override`. It never updates the user default
unless the caller separately requests that operation.

Resolution is exact and fail closed:

- a key must match the configured provider;
- provider-only resolution must find exactly one usable connection;
- a session UUID must resolve to that exact stored connection;
- connection metadata must be ready and its credential/auth state usable;
- an authoritative available-model result that excludes the model is
  `model_unavailable`;
- inability to refresh or prove availability is `availability_unknown`, not
  permission to choose another model;
- no rule falls through from an explicit but unresolved target to a lower
  precedence source.

Categorical unresolved reasons include `invalid_config`, `connection_missing`,
`connection_key_required`, `connection_not_ready`, `authentication_unavailable`,
`model_unavailable`, and `availability_unknown`.

## Startup And Session Switching

Model restoration stays off the first-render critical path. The first frame may
show `resolving model`, but the backend does not accept a prompt until target
resolution reaches either an active or explicit unresolved state.

On startup or transcript switch:

1. Clear the previous session's active runtime target from the new session cell;
   it must never leak across transcripts.
2. Load and project the selected session log.
3. Resolve by the precedence above.
4. Activate asynchronously.
5. Append an initial session event only when the selected log lacked one or an
   explicit override was supplied.
6. Publish the active/default state or a structured selection-required failure.

A failed resume activation leaves the selected transcript active but provider
execution unavailable. The requested target remains visible. The normal TUI
opens the model picker after availability data arrives; headless exits with a
setup failure; RPC emits a structured selection-required event. Repairing the
connection or explicitly selecting another model is required before prompting.

## Activation And Default Commit Ordering

All callers use one backend operation with an intent:

- `session_only`;
- `session_and_default`.

The backend performs:

1. Validate and resolve the exact target.
2. Activate a credential-bearing candidate without installing it.
3. Append and fsync `SessionModelChanged`.
4. Install the candidate as the active runtime target.
5. Publish the correlated session activation success.
6. If requested—or if no configured default exists—update `config.toml`.
7. Publish default success or a categorical partial failure.

There is deliberately no claimed atomic transaction across the session log and
user config. If step 6 fails, steps 3–5 remain valid: the session model stays
active, the prior/default-absent config state remains, and UI/status says
`model changed; default not saved: <category>`. The next explicit save retries.

A default is never written for a target that cannot be represented without an
ambiguous connection. A first successful environment selection may establish a
provider/model-only default only when that target resolves uniquely. Otherwise
the session activates and the default part reports `connection_key_required`.

## Protocol

Backend state exposes two separate structured values:

- `session_model`: `resolving`, `active`, or `unresolved`, carrying the exact
  resolved target or requested target plus bounded reason metadata;
- `default_model`: `absent`, `resolved`, or `unresolved`, carrying bounded
  source and reason metadata.

Do not retain parallel scalar fields as a second model-state representation.
Migrate TUI, headless, RPC, status, and tests to the structured state in one
cutover.

Add `Capability::ModelState`. Under that capability, replace
`ModelSelectedDetailed` with one correlated `ModelActivationRequested` carrying
the exact target and `ModelActivationIntent`, and replace the current
`ModelChanged` / `ModelChangeFailed` pair with a correlated result that
separately reports `session_activation` and `default_update`. `StateUpdated`
carries the structured session/default projections above. The protocol version
advances with this clean cutover; no deprecated model-selection alias remains.

Activation success is correlated independently from default-update outcome so
the partial-success rule is representable. A selection-required server event
contains the requested target and categorical reason; it contains no credential,
raw config content, or provider response body.

Clients that have not negotiated the new model-state capability cannot request
save-as-default. Ordinary legacy model selection is removed with the protocol
version cutover rather than retained as an alias.

## TUI

The model picker renders session and default state independently:

- `●` means the exact active session target;
- `◆` means the resolved user default;
- both markers appear when they are the same target;
- a legend spells out `active` and `default` so color or glyph alone is not
  required;
- an unresolved default appears in the picker header with its requested
  provider/model/key and reason, even when no row can match it;
- when filtering produces no model rows, the action row is disabled and Enter
  reports the existing no-match status rather than reusing a stale highlight.

The filtered model rows are followed by one visually separated action row:

- `Activate and save as default` applies to the highlighted model row;
- Up/Down moves model focus and then the action-row focus without discarding the
  highlighted model; model selection and action focus are separate state;
- Enter on a model row activates it for this session;
- Enter on the footer action activates the retained highlighted model and
  requests a user-default update;
- typing while the footer action is selected returns focus to model filtering
  and appends the character, so no ordinary search key is reserved;
- `Esc` closes without changing either state.

When no default exists, the footer explains that the first successful model-row
activation will also establish one. The backend remains authoritative for this
rule; the TUI does not infer it from stale local state.

Activation failure keeps both markers unchanged. Activation success plus
default-write failure moves only the active marker and displays the bounded
partial-failure status. Picker selection is unavailable during an active prompt,
matching current behavior.

## Headless And RPC Parity

`yach run --model` is an explicit session override within the invocation's
resolved provider/connection context. On resume it appends `SessionModelChanged`
with source `cli_override`. It does not save a global default. A later CLI design
may add explicit provider/connection-key flags; this slice does not overload a
model string with a new compound grammar.

Headless setup uses the same backend resolver and fails before prompting when an
explicit or projected target is unresolved. Outcome documents report requested
and active target identity without secrets.

RPC uses the same correlated selection intent and structured active/default
state as the TUI. RPC clients may request `session_and_default` only after
capability negotiation. Default writes are host-user authority, never repository
or provider authority.

## Migration

Migration is idempotent and field-specific.

### Existing thinking default

When `config.toml` has no `[thinking].default`, import the known
`thinking.default` from `~/.yach/config.json`; if absent, use the existing latest
legacy project-preference recovery. Write directly to TOML. Stop reading the
JSON default after a successful TOML write. Delete `config.json` only when it
contains no unknown/unmigrated fields; otherwise leave it with an actionable
legacy-file diagnostic.

### `active-model.json`

When TOML has no model default:

1. Read the old UUID/model record tolerantly.
2. Resolve the UUID to a ready connection and canonical provider.
3. If the referenced connection has a key, write provider/model/key.
4. Otherwise, if that provider has exactly one usable connection, write
   provider/model without `connection`.
5. If the UUID is stale, the connection is unusable, or ambiguity requires a
   missing key, report the categorical issue and leave the old file intact.
6. Delete the old file only after the TOML write and durability confirmation.

When a valid TOML model default already exists, it is authoritative and the old
active-model file is obsolete; remove the old file through the same safe-path
checks. A later successful explicit default save also supersedes and removes an
unmigrated legacy file.

Migration preserves only the old file's actual meaning: the default for a future
session. It cannot reconstruct historical session targets and never fabricates
session events for old completed turns.

After migration coverage lands, remove `remembered_selection`,
`remember_selection`, every active-model reader/writer/deletion path, and the
first-render restore branch that consumes them.

## Failure Invariants

- Failed activation changes neither session evidence, live provider, nor user
  default.
- Failed session-event persistence does not install the activated candidate.
- Failed default persistence does not roll back a committed session activation.
- An unresolved explicit/session/default target never falls through to another
  target.
- Switching transcripts clears the prior active provider before the new target
  is resolved.
- A connection label rename cannot affect config or session resolution.
- A connection key cannot be changed after assignment.
- Removing a connection cannot silently rebind old sessions; its configuration
  key remains retired.
- Malformed TOML never causes legacy or environment state to overwrite it.
- Migration never deletes a legacy source before its target write is durable.
- Provider-visible transcripts ignore model-setting/config events; provider
  metadata on actual turns remains authoritative for requests that ran.
- No configuration file contains credentials or grants repository-controlled
  shell/environment authority.

## Scope

This design includes:

1. Immutable optional connection keys and one-time assignment UX.
2. Typed document-preserving user TOML storage.
3. Thinking-default migration from JSON to TOML.
4. User model default load/update and legacy active-model migration.
5. Explicit session model-setting event and projection.
6. Backend-owned precedence, activation, partial-failure, startup, resume, and
   transcript-switch behavior.
7. Structured protocol state and correlated selection intents.
8. TUI active/default markers and a separate save-default action row.
9. Headless and RPC convergence on the same activation path.
10. Changed-contract tests and behavioral smokes for the cases below.

## Non-Goals

- Project or repository model defaults.
- General layered configuration merging or project trust.
- Moving connection definitions or credentials into TOML.
- Moving catalog overrides from `models.toml`.
- Connection-key renaming or automatic keys derived from labels.
- Silent model fallback or provider substitution.
- A generic provider route/catalog identity redesign.
- Durable prompt-inbox admission, optimistic client IDs, or remote-service
  reconnect semantics.
- Provider delivery-certainty evidence or broader crash-retry machinery.
- A full frozen turn envelope beyond the session target and existing per-turn
  provider metadata.
- Fork/session-tree implementation beyond the inheritance rule.
- Roles, subagents, provider extension contributions, or model routing policy.

## Verification

Required changed-contract coverage:

1. A new session with a valid user default activates it and appends one initial
   session event.
2. A resumed session's last model event wins over a changed user default.
3. `Enter` changes only the session when a default exists.
4. The picker save-default action changes the session and default.
5. The first successful `Enter` with no default attempts both changes.
6. Activation failure changes neither state.
7. Session append failure keeps the previous provider active.
8. Default write failure keeps the new session target active and the old default
   unchanged.
9. Explicit resume override appends a durable event and does not save a default.
10. Session switch cannot execute with the previous session's provider target.
11. A missing UUID/key, ambiguous provider, unready credential, unavailable
    model, and unknown availability each produce the correct fail-closed reason.
12. Label rename leaves default and session resolution unchanged.
13. Key assignment is one-time and unique within a provider.
14. Existing multiple unkeyed connections load with repair diagnostics.
15. TOML targeted writes preserve unrelated tables, keys, comments, and order.
16. Unsafe paths, malformed TOML, lock failure, and durability failure do not
    clobber config.
17. Valid `active-model.json` migrates once and is deleted only after durable
    TOML write.
18. Stale or unnameably ambiguous legacy targets remain intact with actionable
    diagnostics.
19. Thinking default migrates to TOML without changing resumed-session
    precedence.
20. Provider transcript projection ignores model-setting events.
21. Compaction does not change the projected session target.
22. Protocol JSONL covers absent, resolved, unresolved, active-only, and partial
    default-update states.
23. TUI rendering distinguishes active and default rows and retains `d` as query
    input.
24. Normal-TUI smoke: select a temporary session model, restart and observe it
    restored; save another default, start a new session and observe the new
    default; resume the first session and observe its prior target.
25. RPC/headless smoke: explicit override on a resumed session persists and an
    unresolved projected target fails before a prompt is sent.
