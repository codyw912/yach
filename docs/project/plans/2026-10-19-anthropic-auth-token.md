# Anthropic Auth Token Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an Anthropic credential be sent as `Authorization: Bearer`
instead of `x-api-key`, from the environment and from stored connections,
with a base URL, so yach can reach the host subscription gateway.

**Architecture:**
- `vendor/rig-core` gets two marked patches: `AnthropicKey` becomes an enum
  that renders exactly one auth header, and the Anthropic model lister
  accepts a missing or `null` `has_more`.
- `yach-backend` adds `AnthropicAuth` to `RigProviderConfig::Anthropic` and
  one `anthropic_client` helper that every Anthropic call site uses.
- `yach-cli` resolves the two environment variables through one pure
  function; `yach-connections` gains `CredentialKind` and an
  `ConnectionAuth::AuthToken` variant; the `/connect` flow asks Anthropic
  users for the mode and an optional base URL.

**Tech Stack:** Rust 2024, serde, reqwest via vendored `rig-core` 0.41.0.

**Spec:** `docs/project/specs/2026-10-19-anthropic-auth-token-design.md`
(approved 2026-10-19).

**Source:** linear:CEW-147

## Global Constraints

- Each request carries exactly one of `x-api-key` and `Authorization`.
  `x-api-key` stays the default everywhere a mode is not chosen.
- Environment: `YACH_RIG_ANTHROPIC_AUTH_TOKEN` excludes
  `YACH_RIG_ANTHROPIC_API_KEY`; both set is a configuration error naming
  both; neither set names both.
- Registry schema stays `yach.connections.v2`; entries with
  `"kind": "api_key"` load unchanged.
- `AuthToken` authentication is valid only for `ProviderKind::Anthropic`.
- OpenAI connections keep rejecting a base URL.
- Vendor edits carry a `// yach patch:` comment stating why.
- Workspace clippy: pedantic warn; `unwrap_used`, `expect_used`, `panic`
  deny. Tests follow neighbouring style (`assert!(x.is_ok()); let Ok(x) = x
  else { return };`).
- Run Rust through `just dev ...`, `just test`, `just lint`. One heavy job at
  a time on the shared host.
- Commit each task with `jj commit <paths> -m "<message>"`; never a bare
  `jj commit`.
- Line references are to `main` at `89c9d404`.

---

### Task 1: Vendored Rig patches

**Files:**
- Modify: `vendor/rig-core/src/providers/anthropic/client.rs:44-64`
- Modify: `vendor/rig-core/src/providers/anthropic/model_listing.rs:10-15,78-80`

**Interfaces:**
- Produces: `rig::providers::anthropic::client::AnthropicKey::{ApiKey(String), AuthToken(String)}`,
  `AnthropicKey::auth_token(impl Into<String>) -> AnthropicKey`;
  `From<S: Into<String>>` still yields `ApiKey`.

The vendor crate is outside the workspace; its behavior is tested through
`yach-backend` fixtures in Task 2.

- [ ] **Step 1: Replace the key type**

```rust
/// The credential a Messages-format client sends.
// yach patch: Anthropic-compatible gateways (and `ANTHROPIC_AUTH_TOKEN` in
// other clients) authenticate with `Authorization: Bearer`. Exactly one
// header is sent; the default stays `x-api-key`.
#[derive(Debug, Clone)]
pub enum AnthropicKey {
    /// Sent as `x-api-key`.
    ApiKey(String),
    /// Sent as `Authorization: Bearer`.
    AuthToken(String),
}

impl AnthropicKey {
    /// A token sent as `Authorization: Bearer` instead of `x-api-key`.
    pub fn auth_token(value: impl Into<String>) -> Self {
        Self::AuthToken(value.into())
    }
}

impl<S> From<S> for AnthropicKey
where
    S: Into<String>,
{
    fn from(value: S) -> Self {
        Self::ApiKey(value.into())
    }
}

impl ApiKey for AnthropicKey {
    fn into_header(self) -> Option<http_client::Result<(http::HeaderName, HeaderValue)>> {
        Some(match self {
            Self::ApiKey(key) => HeaderValue::from_str(&key)
                .map(|val| (HeaderName::from_static("x-api-key"), val))
                .map_err(Into::into),
            Self::AuthToken(token) => http_client::make_auth_header(token),
        })
    }
}
```

- [ ] **Step 2: Relax `has_more`**

```rust
#[derive(Debug, Deserialize)]
struct ListModelsResponse {
    data: Vec<ListModelEntry>,
    // yach patch: gateways that proxy the Messages API return
    // `"has_more": null` or omit it; treat that as the last page.
    #[serde(default)]
    has_more: Option<bool>,
    last_id: Option<String>,
}
```

and in `list_all`: `if page.has_more != Some(true) { break; }`.

- [ ] **Step 3: Build**

Run: `just dev cargo check -p yach-backend`
Expected: compiles (callers still pass strings, which convert to `ApiKey`).

- [ ] **Step 4: Commit**

```bash
jj commit vendor/rig-core/src/providers/anthropic -m "Add Bearer auth and lenient listing to vendored Anthropic client"
```

### Task 2: Backend auth mode and the shared client builder

**Files:**
- Modify: `crates/yach-backend/src/rig_adapter.rs` (`RigAnthropicSmokeConfig` 176-181,
  `RigProviderConfig` 200-207, adapter 454-466, `run_anthropic_smoke` 980-990, tests)
- Modify: `crates/yach-backend/src/model_discovery.rs:33-45`, tests 225-250, 438-472
- Modify: test constructors in `compaction.rs` (1498, 1540, 1896),
  `bench_loop.rs:254`, `runner.rs:26076`

**Interfaces:**
- Consumes: Task 1 `AnthropicKey`.
- Produces:
  `pub enum AnthropicAuth { ApiKey, AuthToken }` (`Clone, Copy, Debug, PartialEq, Eq`) in `rig_adapter`;
  `RigProviderConfig::Anthropic { api_key: ProviderSecret, auth: AnthropicAuth, base_url: Option<String> }`;
  `pub(crate) fn anthropic_client(secret: &ProviderSecret, auth: AnthropicAuth, base_url: Option<&str>) -> Result<anthropic::Client, rig::http_client::Error>`;
  `RigAnthropicSmokeConfig { api_key, auth: AnthropicAuth, base_url: Option<String>, model, timeout, max_tokens }`.

- [ ] **Step 1: Write the failing header tests**

In `model_discovery.rs` tests, generalize the fixture to take the expected
header line and response, then add:

```rust
#[tokio::test]
async fn anthropic_auth_token_sends_only_bearer_and_accepts_null_has_more() {
    let fixture = local_anthropic_models_fixture_with(
        AnthropicAuth::AuthToken,
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 82\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\"anthropic/m\",\"display_name\":\"M\"}],\"has_more\":null,\"object\":\"list\"}",
    );
    // assert discovery Ok with one model `anthropic/m`, and the fixture saw
    // `authorization: Bearer fixture-anthropic-sentinel` and no `x-api-key`.
}
```

The existing API-key fixture test also asserts no `authorization` line.
In `rig_adapter.rs`, add a twin of
`legacy_anthropic_request_without_native_envelope_uses_messages_wire_shape`
that sets `auth: AnthropicAuth::AuthToken` and asserts the captured header
contains `authorization: Bearer fixture-key` and no `x-api-key`; the existing
test gains the opposite assertions.

- [ ] **Step 2: Run to see them fail**

Run: `just dev cargo test -p yach-backend anthropic`
Expected: compile failure (`AnthropicAuth` undefined).

- [ ] **Step 3: Implement**

```rust
/// Which header carries an Anthropic credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnthropicAuth {
    /// `x-api-key`, the Anthropic API's own scheme.
    ApiKey,
    /// `Authorization: Bearer`, as `ANTHROPIC_AUTH_TOKEN` does elsewhere.
    AuthToken,
}

pub(crate) fn anthropic_client(
    secret: &ProviderSecret,
    auth: AnthropicAuth,
    base_url: Option<&str>,
) -> Result<anthropic::Client, rig::http_client::Error> {
    let mut builder = secret.with_exposed(|value| {
        anthropic::Client::builder().api_key(match auth {
            AnthropicAuth::ApiKey => anthropic::client::AnthropicKey::from(value),
            AnthropicAuth::AuthToken => anthropic::client::AnthropicKey::auth_token(value),
        })
    });
    if let Some(base_url) = base_url {
        builder = builder.base_url(base_url);
    }
    builder.build()
}
```

The turn adapter, `run_anthropic_smoke` (now wrapping its `String` key in a
`ProviderSecret` and honoring `base_url`), and `discover_provider_models`
call it. Every test constructor adds `auth: AnthropicAuth::ApiKey`.

- [ ] **Step 4: Run the tests**

Run: `just dev cargo test -p yach-backend anthropic`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend -m "Route every Anthropic client through one auth-aware builder"
```

### Task 3: Environment resolution

**Files:**
- Modify: `crates/yach-cli/src/main.rs` (`RigSmokeConfigError` 1391-1400,
  1463-1466, 1501-1504, 2492-2497, 3178-3240, `rig_config_error_message`
  3563-3574, tests 8190-8212, 6043)

**Interfaces:**
- Consumes: Task 2 `AnthropicAuth`, `RigAnthropicSmokeConfig`.
- Produces:
  `fn anthropic_credential(lookup: impl Fn(&'static str) -> Option<String>) -> Result<(ProviderSecret, AnthropicAuth), RigSmokeConfigError>`;
  `fn anthropic_credential_from_env()`;
  `RigSmokeConfigError::Conflicting(&'static str, &'static str)` and
  `RigSmokeConfigError::MissingOneOf(&'static str, &'static str)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn anthropic_credential_selects_header_from_exactly_one_variable() {
    let only = |name: &'static str, value: &'static str| {
        move |asked: &'static str| (asked == name).then(|| value.to_owned())
    };
    assert!(matches!(
        anthropic_credential(only("YACH_RIG_ANTHROPIC_API_KEY", "k")),
        Ok((_, AnthropicAuth::ApiKey))
    ));
    assert!(matches!(
        anthropic_credential(only("YACH_RIG_ANTHROPIC_AUTH_TOKEN", "t")),
        Ok((_, AnthropicAuth::AuthToken))
    ));
    let both = anthropic_credential(|_| Some(String::from("x")));
    assert!(both.is_err());
    let Err(both) = both else { return };
    assert_eq!(
        rig_config_error_message(&both),
        "env vars YACH_RIG_ANTHROPIC_API_KEY and YACH_RIG_ANTHROPIC_AUTH_TOKEN are both set; set only one"
    );
    let neither = anthropic_credential(|_| None);
    let Err(neither) = neither else { return };
    assert_eq!(
        rig_config_error_message(&neither),
        "missing required env var YACH_RIG_ANTHROPIC_API_KEY or YACH_RIG_ANTHROPIC_AUTH_TOKEN"
    );
    assert!(matches!(
        anthropic_credential(only("YACH_RIG_ANTHROPIC_AUTH_TOKEN", " ")),
        Err(RigSmokeConfigError::Empty("YACH_RIG_ANTHROPIC_AUTH_TOKEN"))
    ));
}
```

`provider_setup_error_copy_is_actionable` and
`unconfigured_launch_setup_error_requires_both_missing_env_and_no_stored_connections`
switch to `MissingOneOf` and the new copy.

- [ ] **Step 2: Run to see them fail**

Run: `just dev cargo test -p yach-cli anthropic_credential`
Expected: compile failure.

- [ ] **Step 3: Implement**

```rust
const ANTHROPIC_API_KEY_ENV: &str = "YACH_RIG_ANTHROPIC_API_KEY";
const ANTHROPIC_AUTH_TOKEN_ENV: &str = "YACH_RIG_ANTHROPIC_AUTH_TOKEN";

fn anthropic_credential(
    lookup: impl Fn(&'static str) -> Option<String>,
) -> Result<(ProviderSecret, AnthropicAuth), RigSmokeConfigError> {
    let (name, auth, value) = match (lookup(ANTHROPIC_API_KEY_ENV), lookup(ANTHROPIC_AUTH_TOKEN_ENV)) {
        (Some(_), Some(_)) => {
            return Err(RigSmokeConfigError::Conflicting(ANTHROPIC_API_KEY_ENV, ANTHROPIC_AUTH_TOKEN_ENV));
        }
        (Some(value), None) => (ANTHROPIC_API_KEY_ENV, AnthropicAuth::ApiKey, value),
        (None, Some(value)) => (ANTHROPIC_AUTH_TOKEN_ENV, AnthropicAuth::AuthToken, value),
        (None, None) => {
            return Err(RigSmokeConfigError::MissingOneOf(ANTHROPIC_API_KEY_ENV, ANTHROPIC_AUTH_TOKEN_ENV));
        }
    };
    if value.trim().is_empty() {
        return Err(RigSmokeConfigError::Empty(name));
    }
    Ok((ProviderSecret::new(value), auth))
}

fn anthropic_credential_from_env() -> Result<(ProviderSecret, AnthropicAuth), RigSmokeConfigError> {
    anthropic_credential(|name| std::env::var(name).ok())
}
```

Messages: `Conflicting(a, b)` → `env vars {a} and {b} are both set; set
only one`; `MissingOneOf(a, b)` → `missing required env var {a} or {b}`.
The three Anthropic env sites use `anthropic_credential_from_env()`;
`run_rig_anthropic_smoke` also reads `YACH_RIG_ANTHROPIC_BASE_URL`.

- [ ] **Step 4: Run the tests**

Run: `just dev cargo test -p yach-cli anthropic_credential provider_setup unconfigured_launch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-cli/src/main.rs -m "Resolve the Anthropic auth token from the environment"
```

### Task 4: Stored connection auth mode and Anthropic base URL

**Files:**
- Modify: `crates/yach-connections/src/lib.rs` (`ConnectionAuth` 194-213,
  `validate_persisted` 270-293, `NewConnectionDraft` 340-424, tests)
- Modify: `crates/yach-cli/src/provider_connections.rs` (89-125, 1583-1633)

**Interfaces:**
- Consumes: Task 2 `AnthropicAuth`.
- Produces:
  `pub enum CredentialKind { ApiKey, AuthToken }` (`Clone, Copy, Debug, Default, PartialEq, Eq`; default `ApiKey`);
  `ConnectionAuth::AuthToken { source: CredentialSource }` serialized `"kind": "auth_token"`;
  `ConnectionAuth::credential_kind(&self) -> CredentialKind` (`AuthToken` for that variant, else `ApiKey`);
  `NewConnectionDraft::with_credential_kind(self, CredentialKind) -> Result<Self, ValidationError>` (`UnsupportedProvider` unless Anthropic or `ApiKey`);
  `NewConnectionDraft::credential_kind(&self) -> CredentialKind`.

- [ ] **Step 1: Write the failing tests** (in `yach-connections`)

```rust
#[test]
fn anthropic_connections_accept_auth_token_and_base_url() {
    let draft = NewConnectionDraft::new(
        ProviderKind::Anthropic,
        None,
        Some(String::from(" http://gateway.example:4000/ ")),
    );
    assert!(draft.is_ok());
    let Ok(draft) = draft else { return };
    assert_eq!(draft.base_url(), Some("http://gateway.example:4000"));
    let draft = draft.with_credential_kind(CredentialKind::AuthToken);
    let Ok(draft) = draft else { return };
    let connection = draft.into_pending(ConnectionId::new_stored());
    assert_eq!(
        connection.authentication,
        ConnectionAuth::AuthToken { source: CredentialSource::System }
    );
    let json = serde_json::to_value(&connection.authentication);
    let Ok(json) = json else { return };
    assert_eq!(json, serde_json::json!({"kind": "auth_token", "source": "system"}));
    let ready = ProviderConnection { state: ConnectionState::Ready, ..connection };
    assert!(ready.validate_persisted().is_ok());
}

#[test]
fn auth_token_is_rejected_outside_anthropic_and_openai_keeps_rejecting_base_url() {
    // OpenAi draft: with_credential_kind(AuthToken) is Err(UnsupportedProvider);
    // OpenAi draft with a base URL is Err(BaseUrlNotAllowed);
    // a Ready OpenAi connection with AuthToken fails validate_persisted.
}

#[test]
fn legacy_api_key_entry_reads_as_api_key() {
    // deserialize {"kind":"api_key","source":"system"} → ApiKey; credential_kind() == ApiKey
}
```

- [ ] **Step 2: Run to see them fail**

Run: `just dev cargo test -p yach-connections`
Expected: compile failure.

- [ ] **Step 3: Implement**

- `NewConnectionDraft` gains `credential: CredentialKind`; `new` defaults it
  to `ApiKey` and, for Anthropic, normalizes an optional base URL with
  `normalize_base_url`.
- `into_pending` writes `AuthToken { source: System }` for `AuthToken`.
- `validate_persisted` accepts `AuthToken { source: System }` only when the
  provider is Anthropic, then re-normalizes through
  `NewConnectionDraft::new(...)` as today.
- `yach-cli`: `adapter_for_parts` takes a `CredentialKind`; Anthropic maps it
  to `AnthropicAuth`. `adapter_for_draft` passes `draft.credential_kind()`,
  `adapter_for_connection` passes `connection.authentication.credential_kind()`.
  `EnvironmentConnection::new` reports `AuthToken { source: Environment }`
  for `AnthropicAuth::AuthToken`.

- [ ] **Step 4: Run the tests**

Run: `just dev cargo test -p yach-connections && just dev cargo test -p yach-cli provider_connections`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-connections crates/yach-cli/src/provider_connections.rs -m "Store an Anthropic auth mode and base URL on connections"
```

### Task 5: `/connect` flow for Anthropic

**Files:**
- Modify: `crates/yach-backend/src/provider_connections.rs` (dialog IDs 16-30,
  `ConnectionFlowStateTag` 392-410, `ConnectionFlowState` 412-, dispatch
  695-730, `submit_label` 1019-1056, `submit_base_url` 1058-1072,
  `dialog_matches_state` 1340-, `dialog_for_state` 1410-1540, tests)

**Interfaces:**
- Consumes: Task 4 `CredentialKind`, `NewConnectionDraft::with_credential_kind`.
- Produces: dialog `provider-connection:anthropic-auth` (select: `api_key`,
  `auth_token`); state tag `ChoosingAnthropicAuth`;
  `EnteringBaseUrl { provider, label, credential: CredentialKind }`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn anthropic_flow_chooses_auth_token_and_optional_base_url() {
    // add → provider "anthropic" → label "Gateway" → expect
    // "provider-connection:anthropic-auth"; select "auth_token" → expect
    // "provider-connection:base-url" whose title is "Anthropic base URL";
    // submit "http://gateway.example:4000" → expect
    // "provider-connection:secret:create" with prompt "Enter the auth token".
}

#[test]
fn anthropic_flow_blank_base_url_keeps_api_key_defaults() {
    // ... select "api_key", submit "" → secret dialog prompt "Enter the API key".
}
```

Both use the module's `begin_root` and `only_dialog` helpers, as
`provider_connection_flow_uses_domain_validation_for_compatible_base_urls` does.

- [ ] **Step 2: Run to see them fail**

Run: `just dev cargo test -p yach-backend provider_connection_flow anthropic_flow`
Expected: FAIL (label goes straight to the secret dialog).

- [ ] **Step 3: Implement**

- `submit_label` for Anthropic → `ChoosingAnthropicAuth { label }`.
- Selecting `api_key`/`auth_token` → `EnteringBaseUrl { provider: Anthropic, label, credential }`.
- `submit_base_url` for Anthropic treats blank as `None`, builds the draft,
  applies `with_credential_kind`. OpenAI-compatible stays required, with
  `credential: ApiKey`.
- Base-URL dialog title: `"Anthropic base URL"` / prompt
  `"Optional; leave empty for the Anthropic API"` for Anthropic, unchanged
  otherwise. Create-secret prompt: `"Enter the auth token"` when the draft
  is `AuthToken`.

- [ ] **Step 4: Run the tests**

Run: `just dev cargo test -p yach-backend provider_connection`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/provider_connections.rs -m "Ask for the Anthropic auth mode and base URL in /connect"
```

### Task 6: Docs, credentials, full gates, live proof

**Files:**
- Modify: `README.md:88-92,176-181`
- Modify: `secretspec.toml`
- Create: `docs/project/records/2026-10-19-anthropic-auth-token-gateway-smoke.md`

- [ ] **Step 1: README and secretspec**

README table row:
`| YACH_RIG_ANTHROPIC_AUTH_TOKEN | — | Bearer token instead of an API key, for gateways that expect Authorization: Bearer (like ANTHROPIC_AUTH_TOKEN). Set this or YACH_RIG_ANTHROPIC_API_KEY, not both. |`
and the API-key row reads "Required for the default provider unless
`YACH_RIG_ANTHROPIC_AUTH_TOKEN` is set."

```toml
YACH_RIG_ANTHROPIC_AUTH_TOKEN = { description = "Anthropic Messages gateway auth token (host subscription gateway)" }

[scopes.anthropic-gateway]
secrets = ["YACH_RIG_ANTHROPIC_AUTH_TOKEN"]
```

- [ ] **Step 2: Gates**

Run: `just fmt-check && just lint && just test`
Expected: all pass.

- [ ] **Step 3: Live proof**

```bash
YACH_RIG_ANTHROPIC_BASE_URL=http://omp-subscriptions.home.lan:4000 \
YACH_RIG_ANTHROPIC_MODEL=claude-sonnet-4-5 \
secretspec run --provider omp-subscriptions --scope anthropic-gateway \
  --reason "yach anthropic auth token gateway smoke" -- \
  just dev cargo run -q -p yach -- smoke-rig-anthropic
```

then one `yach run` turn in a scratch project with
`YACH_RIG_PROVIDER=anthropic`, the same base URL and model, and a prompt
that lists and reads a file. Record commands, outcomes, and session
summary in the record; state that the `/connect` path was covered by tests,
not a live run.

- [ ] **Step 4: Commit**

```bash
jj commit README.md secretspec.toml docs/project/records/2026-10-19-anthropic-auth-token-gateway-smoke.md -m "Document the Anthropic auth token and record the gateway smoke"
```
