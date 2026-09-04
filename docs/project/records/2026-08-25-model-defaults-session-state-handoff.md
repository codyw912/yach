# Model Defaults And Session State Handoff

Date: 2026-08-25
Status: superseded by accepted design; implementation not started

Resolution: `docs/project/specs/2026-08-26-model-defaults-session-state-design.md`
settles the remaining identity, configuration, session, failure, migration, and
picker questions. This handoff remains the exploration record.

## Purpose

This record preserves the configuration and model-selection architecture work
from the 2026-08-24/25 session so it can resume on another machine without
reconstructing decisions from chat.

The immediate trigger was `~/.yach/active-model.json`. The file is legitimate
durable state today, but it conflates two concepts that need different owners:

- the user's default model for a new session; and
- the active model of a particular session.

The target architecture removes that conflation rather than moving the current
file unchanged into a new config format.

## Current Yach Behavior

`crates/yach-cli/src/provider_connections.rs` owns
`~/.yach/active-model.json`. The versioned JSON record contains a connection
UUID and model ID. The CLI reads it tolerantly, writes it through a temporary
file plus rename, and deletes it when its referenced connection is removed.

`ProviderConnectionRuntime::remembered_selection` and
`remember_selection` expose the backend seam. The runner restores the target
after first render and persists every successful activation.

This makes the last successfully activated target a machine-global mutable
selection. It does not distinguish "use this for this transcript" from "make
this the default for future transcripts." Session provider metadata records the
provider/model on assistant entries, but there is no explicit model-change
session event from which the active session setting can be projected.

The recent reasoning-level work has the desired ownership split: explicit
thinking changes are session evidence, a resumed session's recorded value wins,
and the user config default applies to new sessions.

## Owner Decisions

The following decisions are accepted direction, not open questions:

1. **Session-active model and user default are separate state.** The standalone
   `active-model.json` is not part of the long-term architecture.
2. **Existing default:** an ordinary `/model` picker selection changes only the
   current session. The picker must also offer an explicit action to save the
   selected target as the user default; a `d` key is the current UI candidate.
3. **No existing default:** the first successful model selection both activates
   the session target and establishes the user default automatically.
4. **Connection naming is optional.** A user with one connection for a provider
   must not be forced to name it. A stable connection name becomes necessary
   only when the user has, or plans to have, multiple connections for the same
   provider.
5. **Model defaults are user-owned for now.** Repository/project model defaults
   are deferred until the wider layered-config and project-trust design is
   complete.
6. **The long-term human configuration format will not be JSON or JSONC.** TOML
   is the selected direction. Exact table/key names remain a schema-design
   detail.

## Intended Semantics

### New session

A new session resolves the user-configured default. If no default exists, the
existing explicit environment/provider bootstrap may remain the fallback until
the user successfully selects a model; that first selection then writes the
default.

The default is not rewritten merely because the user temporarily changes the
session model.

### Existing or resumed session

The session log must contain explicit model-setting evidence. Loading or
resuming projects that evidence to obtain the active connection/provider/model.
The session value wins over the user default because changing a global default
must not rewrite an existing transcript's execution context.

A model change during a session appends a model-change event after successful
activation. Forking should inherit the active projected setting at the fork
point.

### Picker

With a default already configured:

- Enter activates the highlighted target for this session and appends session
  evidence.
- `d` activates it and explicitly replaces the user default.
- The picker should identify the current session target and current user
  default separately; one marker cannot represent both states.

With no configured default, the first successful Enter selection is both the
session target and the new default. Failure must not persist either state.

## Configuration Direction

The eventual configuration work should replace section-specific JSON readers
and writers with one typed TOML boundary. A candidate shape, not yet frozen, is:

```toml
[model.default]
provider = "openai-codex"
model = "gpt-5.6-sol"
# connection = "personal" # only needed when provider resolution is ambiguous
```

Human configuration should not normally expose a connection UUID. Provider and
model identify the common one-connection case. When several connections can
serve the same provider, `connection` names a stable user-facing connection
identity. Existing connection labels are presentation-only and can be
non-unique, so the implementation must decide whether to add an immutable key
or strengthen label uniqueness before treating a name as configuration
identity.

The typed boundary must preserve unrelated TOML fields on targeted updates and
produce categorical diagnostics. User config may grant user authority;
eventual project config may restrict authority but must not grant it. Project
trust, merging, and repository overrides are deliberately outside this first
model-default slice.

## Resolution And Failure Invariants

These are the recommended safe defaults for the next design pass; they were not
separately owner-ratified in the source session:

- Proposed new-session precedence: explicit CLI target, then user default, then
  explicit environment bootstrap.
- Proposed resume precedence: explicit CLI target, then projected session
  target, then user default, then explicit environment bootstrap.
- An explicit session or configured target that no longer resolves should
  produce an actionable error and selection path. It should not silently swap
  providers, connections, or models.
- A CLI override on resume should become the session's active model and append
  the corresponding session event, rather than becoming an unrecorded one-turn
  exception.
- Provider-only connection resolution is valid only when it identifies exactly
  one usable connection. Ambiguity requires a stable connection name.

## Migration Direction

The cutover should be idempotent and preserve intent:

1. Add explicit session model-change evidence and projection first.
2. Add the typed user TOML model default and `/model` save-default action.
3. On startup, when no TOML model default exists, inspect
   `active-model.json` once. Import a still-valid target as the user default.
4. Delete `active-model.json` only after the TOML write succeeds. If it is stale
   or cannot be represented without resolving connection ambiguity, report the
   issue and leave it intact rather than silently choosing another target.
5. Remove `remembered_selection` / `remember_selection` and every standalone
   active-model reader, writer, and deletion path after migration coverage is
   complete.

The old file cannot recover historical per-session settings. Migration only
preserves its actual old meaning: the target used when starting the next
session.

## Cohort Research: Directional Evidence Only

The owner explicitly ruled that the cohort is directional evidence, not a
source of truth for Yach's design.

- Codex uses layered TOML with trust-aware project configuration. Session/thread
  state carries model, provider, and reasoning settings. This supports TOML and
  the defaults/session split.
- Claude Code uses strict JSON settings, allows temporary model/effort changes,
  and restores session state. Its ownership split is useful; its format is not.
- Current OpenCode uses JSONC and stores active session state separately. The
  protected `v2` rewrite branch (observed at `895eff09b01ce524a4878a65096adfd5191b7d78`)
  still uses JSONC (`cli.json`) with lock/atomic-write machinery and separates
  XDG config, state, data, cache, and SQLite session storage. It is pre-stable
  (`@next`; tracked by issue #36279), so it is not evidence for a final Yach
  schema.
- Pi's `bigrefactor` branch (observed at
  `90c017b05bfe5ce935e4cac3173ae0ec4e3bdcb8`) still uses JSON settings. Its
  valuable pattern is versioned JSONL session migration with explicit
  `model_change` and `thinking_level_change` entries, including branch-aware
  projection.
- OMP uses YAML configuration and explicit session change entries. The state
  split is relevant; YAML is not selected for Yach.

Sources:

- <https://github.com/anomalyco/opencode/tree/v2>
- <https://github.com/anomalyco/opencode/issues/36279>
- <https://github.com/badlogic/pi-mono/tree/bigrefactor>

## Remaining Design Decisions

The next session should resolve these before implementation:

1. Stable connection identity: immutable optional key versus unique mutable
   label, including rename behavior and multiple same-provider migration.
2. Exact TOML schema, update library, file location, permissions, and
   preservation behavior.
3. Exact protocol/session event shape and projection rules for old logs,
   forks, compaction, and transcript switching.
4. Exact unavailable-target UX and whether the proposed fail-closed resolution
   rules above are accepted.
5. CLI/headless/RPC model override parity and whether explicit resume overrides
   append a durable event.
6. Picker interaction details and markers for active versus default targets.
7. One-time migration behavior when the old UUID target is valid but cannot yet
   be named without user input.

## Recommended Implementation Order

1. Write and approve a focused model-default/session-setting design spec.
2. Add session event/projection support and changed-contract tests.
3. Introduce the TOML config boundary for the model default only; do not fold in
   project trust or permission policy at the same time.
4. Update TUI, headless, CLI, and RPC callers through one backend-owned model
   activation path.
5. Add the idempotent `active-model.json` migration and then remove the old
   global remembered-selection seam.
6. Smoke new-session selection, temporary session switching, save-as-default,
   resume, fork, missing connection, and migration behavior.

No implementation was started during this architecture exploration.
