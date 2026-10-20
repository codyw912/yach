# Anthropic Auth Token

**Outcome:** linear:CEW-147 (kata `yach#q6rj`)

Status: approved by the owner, 2026-10-19 ("go ahead through to a PR"). Owner
decisions the same day: patch the vendored Rig so each request carries
exactly one credential header (no duplicate header for a service that is not
this gateway), and include stored connections in this change.

## Problem

Yach's Anthropic provider always authenticates with `x-api-key`. The vendored
Rig `AnthropicKey` turns every key into that header
(`vendor/rig-core/src/providers/anthropic/client.rs:45-64`). The host
subscription gateway (`http://omp-subscriptions.home.lan:4000`) answers
`/v1/messages` differently by header. Probed 2026-10-19 with
`claude-sonnet-4-5`:

| Headers sent | Result |
| --- | --- |
| `Authorization: Bearer <token>` | 200 |
| `x-api-key: <token>` | 403 |
| both | 200 |

Other clients reach the gateway because they send `ANTHROPIC_AUTH_TOKEN` as a
Bearer token. The M1 dogfood live cohort (CEW-147) needs Anthropic through
this gateway, so yach must be able to do the same.

A second gap blocks stored connections specifically. The gateway's
`GET /v1/models` returns `"has_more": null` and 693 rows. Rig's Anthropic
lister declares `has_more: bool`
(`vendor/rig-core/src/providers/anthropic/model_listing.rs:11-15`), so parsing
fails. Creating a connection runs discovery and treats a failure as fatal
(`crates/yach-cli/src/provider_connections.rs:652-655`), so a gateway
Anthropic connection could not be created even with the right header. The
parse failure follows from the declared type; it has not been run.

A third gap: a stored Anthropic connection cannot have a base URL at all.
`NewConnectionDraft::new` rejects one with `BaseUrlNotAllowed`
(`crates/yach-connections/src/lib.rs:365-369`).

## Goals

1. An Anthropic credential can be sent either as `x-api-key` (today's
   behavior, the default) or as `Authorization: Bearer`. Each request carries
   exactly one of the two.
2. The choice is available from the environment and from stored connections,
   alongside a custom base URL.
3. Every Anthropic request goes through the same choice: chat turns, model
   discovery, and `yach smoke-rig-anthropic`.
4. Direct Anthropic API use is unchanged for existing environments and
   stored connections.

## Non-goals

- Claude subscription OAuth (login, refresh, PKCE). Upstream PR #1905 covers
  that shape; this change only selects a header for a token the user already
  has.
- Bearer auth for any other provider.
- Filtering a gateway's model list to Anthropic models. The OpenAI-compatible
  path through the same gateway lists every row today; this keeps parity.
- A registry schema bump (see Compatibility).

## Design

### Vendored Rig

`AnthropicKey` becomes an enum:

```rust
pub enum AnthropicKey {
    /// Sent as `x-api-key`.
    ApiKey(String),
    /// Sent as `Authorization: Bearer`.
    AuthToken(String),
}
```

- `From<S: Into<String>>` keeps producing `ApiKey`, so every existing caller
  and test keeps today's header.
- `AnthropicKey::auth_token(value)` constructs the Bearer form.
- `into_header` maps `AuthToken` through the existing `make_auth_header`
  (`vendor/rig-core/src/client/mod.rs`), the same helper `BearerAuth` uses.

yach cannot substitute Rig's `BearerAuth` for this, because
`AnthropicBuilder` fixes the client's key type to `AnthropicKey`.

Model listing tolerates a missing or `null` `has_more` and treats it as the
last page: `#[serde(default)] has_more: Option<bool>`, ending the loop unless
it is `Some(true)`. Rig's OpenAI lister already relaxes gateway-omitted fields
the same way (`vendor/rig-core/src/providers/openai/model_listing.rs:15-21`).

Both edits carry a short `yach patch:` comment naming the reason, so the
vendor inventory planned in `yach#626n` can find them. Both are small and
suitable to offer upstream after that reconciliation; the key enum is the
shape #1905's `AuthSource` would also need.

### Backend configuration

`RigProviderConfig::Anthropic` gains `auth: AnthropicAuth`:

```rust
pub enum AnthropicAuth {
    ApiKey,
    AuthToken,
}
```

The field is required. Every construction site states the mode, so no site
can drop it silently.

One helper in `yach-backend` builds the Anthropic client builder from
`(api_key, auth, base_url)`. The three Anthropic call sites use it:

- the turn adapter (`crates/yach-backend/src/rig_adapter.rs:455`)
- the smoke command (`crates/yach-backend/src/rig_adapter.rs:983`), which
  also gains a base URL and stops ignoring `YACH_RIG_ANTHROPIC_BASE_URL`
- model discovery (`crates/yach-backend/src/model_discovery.rs:33`)

Compaction is unaffected. Its raw request with Bearer auth only serves
`RigProviderConfig::OpenAi` (`crates/yach-backend/src/compaction.rs:907-922`).

### Environment

`YACH_RIG_ANTHROPIC_AUTH_TOKEN` is the Bearer counterpart of
`YACH_RIG_ANTHROPIC_API_KEY`, matching `ANTHROPIC_AUTH_TOKEN`:

| `_API_KEY` | `_AUTH_TOKEN` | Result |
| --- | --- | --- |
| set | unset | `x-api-key`, as today |
| unset | set | `Authorization: Bearer` |
| set | set | configuration error naming both variables |
| unset | unset | today's missing-credential error, extended to name both variables |

Setting both is an error because neither choice is clearly what the user
meant. Every place that reads `YACH_RIG_ANTHROPIC_API_KEY` in
`crates/yach-cli/src/main.rs` goes through one resolver implementing this
table: the interactive provider, `smoke-rig-anthropic`, and the request-config paths.
The environment connection reports the mode it resolved (see Stored
connections).

### Stored connections

`ConnectionAuth` gains one variant:

```rust
#[serde(rename = "auth_token")]
AuthToken { source: CredentialSource },
```

- It is valid only for `ProviderKind::Anthropic`. `validate_persisted`
  accepts `AuthToken { source: System }` for Anthropic and rejects it for
  every other provider.
- `ApiKey` keeps its meaning. Existing registry entries have
  `"kind": "api_key"` and load unchanged.
- The environment connection
  (`crates/yach-cli/src/provider_connections.rs:89-95`) maps a resolved auth
  token to `AuthToken { source: Environment }`.
- The secret is stored and retrieved exactly as an API key is. Only the
  header changes.

`NewConnectionDraft` gains the auth mode and allows an optional base URL for
Anthropic. The URL goes through the same `normalize_base_url` as
OpenAI-compatible endpoints. OpenAI keeps rejecting a base URL.

Adapter construction from a connection (`adapter_for_parts`,
`crates/yach-cli/src/provider_connections.rs:1600`) maps the connection's auth
variant to `AnthropicAuth`. The other places that match `ConnectionAuth::ApiKey`
(`yach-cli` `provider_connections.rs` and `model_discovery_cache.rs`,
`yach-backend` `provider_connections.rs`) treat `AuthToken` like `ApiKey`,
except where the header mode matters.

### Add-connection flow

The dialog flow in `crates/yach-backend/src/provider_connections.rs` is the
only creation surface. For Anthropic it gains two steps after the label:

1. **Choose authentication:** "API key (x-api-key)" or "Auth token
   (Authorization: Bearer)". API key is the default choice.
2. **Base URL:** optional. Blank keeps the Anthropic API. The
   OpenAI-compatible step stays required as today.

The secret prompt names the selected mode ("Anthropic auth token" rather than
"API key"). Replacing or repairing the secret keeps the connection's mode.
Changing the mode or base URL means removing the connection and adding a new
one, the same as changing an OpenAI-compatible base URL today.

### Compatibility

- **Existing environments and connections:** unchanged headers, endpoints,
  and registry contents.
- **Downgrade:** the registry schema stays `yach.connections.v2`. An older
  binary reading a registry that holds a gateway Anthropic connection fails
  loudly: it rejects the unknown `auth_token` kind and the Anthropic base URL
  (`RegistryError::Malformed`). It does not send the token as `x-api-key`.
  Removing that connection restores the older binary. Pre-1.0, a loud
  downgrade failure is acceptable; a schema bump would make every upgraded
  registry unreadable by older binaries, including registries that never use
  the feature.

## Testing

Permanent tests, in the crates that own the behavior:

- **Header selection:** against the existing fixture HTTP server, API-key
  mode sends only `x-api-key` and auth-token mode sends only
  `Authorization: Bearer`. Checked for a chat turn and for model listing.
- **Listing:** a `/v1/models` body with `"has_more": null`, or without the
  field, returns its rows and stops after one page.
- **Environment resolution:** both variables set is an error naming both;
  neither set names both; each alone selects its mode.
- **Connections:** a v2 registry entry without the new variant loads as
  `ApiKey`; `AuthToken` round-trips; `AuthToken` on a non-Anthropic provider
  is rejected; an Anthropic draft accepts and normalizes a base URL; OpenAI
  still rejects one.
- **Flow:** the Anthropic add flow reaches the secret prompt in both modes,
  with and without a base URL.

## Live proof

`secretspec.toml` gains `YACH_RIG_ANTHROPIC_AUTH_TOKEN` in a new scope,
`anthropic-gateway`, next to the existing OpenAI-compatible `gateway` scope.
Three runs through `omp-subscriptions`, recorded in
`docs/project/records/`:

1. `yach smoke-rig-anthropic` with `YACH_RIG_ANTHROPIC_AUTH_TOKEN` and
   `YACH_RIG_ANTHROPIC_BASE_URL=http://omp-subscriptions.home.lan:4000`.
2. One completed `yach run` turn that calls a tool, following
   `docs/project/records/2026-09-30-session-diagnostics-gateway-smoke.md`,
   with the same Anthropic settings.
3. The `/connect` dialog driven over `yach rpc`: add an Anthropic connection
   with an auth token and the gateway base URL, activate one of its
   discovered models, and complete one prompt.

## Documentation

- `README.md` provider table (lines 179-181): add
  `YACH_RIG_ANTHROPIC_AUTH_TOKEN`, when to use it, and that it excludes
  `_API_KEY`.
- The README has no `/connect` section to extend. The `/connect` dialog
  itself shows the new choices, so no other README change.

## Done when

- Both header modes work from the environment and from stored connections,
  with and without a base URL, and no request carries both headers.
- Direct API behavior and existing registries are unchanged.
- The tests above pass, and the live proof record shows both gateway runs
  completing.
- `yach#626n` has the two vendor patches to inventory.
