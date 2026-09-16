# Extension Capability Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an extension declare that a tool uses the network or runs a
process, and require an explicit user grant before that extension activates.

**Architecture:** The native `ToolRisk` enum already has `UsesNetwork` and
`RunsProcess`; this widens the extension-facing risk vocabulary onto them,
stops the permission policy denying network outright, derives an extension's
capability set from its manifest tool declarations, and gates activation on a
user-home grant that stores the approved capability set. Registration is
tightened so the host cannot register a tool surface that differs from the
manifest the user consented to.

**Tech Stack:** Rust 2024, `serde`/`serde_json`, `tokio` (existing runner),
`jj` for commits. No new dependencies.

**Spec:** `docs/project/specs/2026-09-16-extension-capability-contract-design.md`

**Source:** plane:YACH-10

## Global Constraints

- This workspace forbids `unwrap()` and `expect()`, including in tests.
  Use `let Some(x) = … else { … }`, `let Ok(x) = … else { … }`, or
  `assert!(matches!(…))`. Existing tests show the pattern.
- Run project commands through `just`: `just test`, `just lint`,
  `just fmt-check`. For ad hoc cargo use `just dev cargo …`. Never run bare
  `cargo` outside the devenv shell.
- `just lint` runs `toolchain-check` first; leave the toolchain pins alone.
- The contract is a declaration, not enforcement. No comment, message, or
  doc string added by this plan may claim that Yach prevents or detects an
  extension using the network without declaring it. The host is an ordinary
  subprocess with the agent's privileges.
- User home is the only place a capability grant may be written. Repository
  content must never grant capability.
- Existing file-scoped extensions, including the bundled hashline extension,
  must activate unchanged and with no grant.
- Each task ends green: `just test` and `just lint` pass before its commit.
- Commit with `jj commit <paths> -m "…"`, naming paths explicitly. Never a
  bare `jj commit`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/yach-backend/src/extension.rs` | Extension risk vocabulary, manifest parsing, host registration, activation | Modify |
| `crates/yach-backend/src/tools.rs` | Native risk allowlist, permission policy | Modify |
| `crates/yach-backend/src/extension_capability.rs` | **New.** Capability set derivation, grant record load/store, subset check | Create |
| `crates/yach-backend/src/lib.rs` | Module declaration and re-exports | Modify |
| `crates/yach-cli/src/main.rs` | Diagnostics output, grant/revoke subcommand | Modify |
| `crates/yach-ui/src/slash_commands.rs` | `/extension-trust` command | Modify |
| `crates/yach-ui/src/app.rs` | Slash dispatch for the new command | Modify |

`extension_capability.rs` is new rather than more code in `extension.rs`,
which is already over 4,700 lines. Capability derivation, the grant record,
and the subset check are one responsibility with a small interface, so they
belong together and can be tested without a running host.

---

### Task 1: Widen the extension risk vocabulary

Adds the two risk variants and their manifest strings. After this task an
extension can *declare* network or process risk; nothing grants it yet, so
registration still rejects it at the native allowlist. That keeps this task
independently reviewable.

**Files:**
- Modify: `crates/yach-backend/src/extension.rs:80-84` (`ExtensionToolRisk`)
- Modify: `crates/yach-backend/src/extension.rs:168-176` (`From` impl)
- Modify: `crates/yach-backend/src/extension.rs:2460-2467` (`parse_tool_risk`)
- Test: `crates/yach-backend/src/extension.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `ExtensionToolRisk::UsesNetwork`, `ExtensionToolRisk::RunsProcess`;
  manifest strings `"uses_network"` and `"runs_process"`;
  `From<ExtensionToolRisk> for ToolRisk` maps them to `ToolRisk::UsesNetwork`
  and `ToolRisk::RunsProcess`.

- [ ] **Step 1: Write the failing test**

Add to the existing `mod tests` in `crates/yach-backend/src/extension.rs`:

```rust
#[test]
fn manifest_parses_network_and_process_tool_risks() {
    let manifest = serde_json::json!({
        "schema": "yach.extension/v1",
        "id": "capability-fixture",
        "version": "1.0.0",
        "main": { "command": "fixture-host" },
        "contributes": {
            "tools": [
                {
                    "name": "fetch_url",
                    "description": "Fetch a URL.",
                    "risk": "uses_network",
                    "provider_visible": true
                },
                {
                    "name": "run_helper",
                    "description": "Run a helper process.",
                    "risk": "runs_process",
                    "provider_visible": true
                }
            ]
        }
    });

    let parsed = parse_extension_manifest(&manifest.to_string());
    assert!(parsed.is_ok(), "manifest should parse: {parsed:?}");
    let Ok(manifest) = parsed else { return };
    let risks: Vec<ExtensionToolRisk> = manifest
        .contributes
        .tools
        .iter()
        .map(|tool| tool.risk)
        .collect();
    assert_eq!(
        risks,
        vec![
            ExtensionToolRisk::UsesNetwork,
            ExtensionToolRisk::RunsProcess
        ]
    );
    assert_eq!(ToolRisk::from(risks[0]), ToolRisk::UsesNetwork);
    assert_eq!(ToolRisk::from(risks[1]), ToolRisk::RunsProcess);
}
```

If `parse_extension_manifest` is not the exact name in scope, find the
manifest entry point with
`grep -n 'fn parse_extension_manifest\|fn parse_manifest' crates/yach-backend/src/extension.rs`
and use that name with its real signature. Do not invent one.

- [ ] **Step 2: Run the test to verify it fails**

Run: `just dev cargo test -p yach-backend --lib manifest_parses_network_and_process`
Expected: FAIL — `UnsupportedToolRisk { risk: "uses_network" }`.

- [ ] **Step 3: Add the variants**

In `ExtensionToolRisk` (around line 80):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionToolRisk {
    ReadsLocalMetadata,
    ReadsLocalContent,
    MutatesLocalState,
    /// Declares that the tool contacts the network. A declaration the user
    /// grants at activation; it is not enforced. See
    /// docs/project/specs/2026-09-16-extension-capability-contract-design.md.
    UsesNetwork,
    /// Declares that the tool runs a process.
    RunsProcess,
}
```

In the `From` impl (around line 168), add:

```rust
            ExtensionToolRisk::UsesNetwork => Self::UsesNetwork,
            ExtensionToolRisk::RunsProcess => Self::RunsProcess,
```

In `parse_tool_risk` (around line 2460), add before the `_` arm:

```rust
        "uses_network" => Ok(ExtensionToolRisk::UsesNetwork),
        "runs_process" => Ok(ExtensionToolRisk::RunsProcess),
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `just dev cargo test -p yach-backend --lib manifest_parses_network_and_process`
Expected: PASS.

- [ ] **Step 5: Fix the exhaustive matches the compiler names**

Run: `just dev cargo build -p yach-backend 2>&1 | grep -E '^error' -A 6`

Every non-exhaustive match on `ExtensionToolRisk` is a decision, not
boilerplate. For each one the compiler reports, read the surrounding
function and decide whether the new variants belong with the mutating case
or need their own arm. Do not add a catch-all `_` arm — it would silently
absorb future variants.

- [ ] **Step 6: Verify and commit**

Run: `just test && just lint && just fmt-check`
Expected: all pass.

```bash
jj commit crates/yach-backend/src/extension.rs \
  -m "Add network and process risks to the extension vocabulary"
```

---

### Task 2: Let the permission policy allow network risk

`ToolPermissionPolicy` hardcodes `UsesNetwork` to denied in both
authorization and provider advertising, so the risk variant alone cannot
produce a callable tool.

**Files:**
- Modify: `crates/yach-backend/src/tools.rs:1898-1904` (policy struct)
- Modify: `crates/yach-backend/src/tools.rs:1981-2006` (`authorize`, `allows_provider_advertising`)
- Modify: `crates/yach-backend/src/tools.rs:2088-2098` (extension registration allowlist)
- Test: `crates/yach-backend/src/tools.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `ExtensionToolRisk::UsesNetwork` from Task 1.
- Produces: `ToolPermissionPolicy` field `network_execution: BTreeSet<String>`
  and a builder method `with_network_execution(names: &[&str]) -> Self`
  following the existing `with_*` builders in this file. Find their exact
  shape with `grep -n 'pub fn with_' crates/yach-backend/src/tools.rs` and
  match it.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/yach-backend/src/tools.rs`:

```rust
#[test]
fn network_risk_is_allowed_only_when_allowlisted() {
    let definition = ToolDefinition::extension_tool_with_version(
        "capability-fixture",
        Some(String::from("1.0.0")),
        String::from("fetch_url"),
        String::from("Fetch a URL."),
        ToolInputSchema::object_with_no_additional_properties(Vec::new()),
        ToolRisk::UsesNetwork,
        ProviderToolVisibility::Visible,
    );

    let denied = ToolPermissionPolicy::deny_all();
    assert_eq!(
        denied.authorize(&definition),
        ToolPermissionState::Denied,
        "network risk must not be allowed by default"
    );

    let allowed = ToolPermissionPolicy::deny_all().with_network_execution(&["fetch_url"]);
    assert_eq!(allowed.authorize(&definition), ToolPermissionState::Allowed);
    assert!(allowed.allows_provider_advertising(&definition));
}
```

`ToolDefinition::extension_tool_with_version` and
`ToolInputSchema::object_with_no_additional_properties` are used at
`crates/yach-backend/src/extension.rs:1812`; confirm the exact argument
order and schema constructor with
`grep -n 'fn extension_tool_with_version' -A 12 crates/yach-backend/src/tools.rs`
and
`grep -n 'fn object_with_no_additional_properties' -A 6 crates/yach-backend/src/tools.rs`,
then match them. Adjust the fixture to the real signatures rather than
changing the production code to fit this test.

- [ ] **Step 2: Run the test to verify it fails**

Run: `just dev cargo test -p yach-backend --lib network_risk_is_allowed_only_when_allowlisted`
Expected: FAIL — no method `with_network_execution`.

- [ ] **Step 3: Add the allowlist and route the risk through it**

Add the field to the struct:

```rust
pub struct ToolPermissionPolicy {
    fixture_execution: BTreeSet<String>,
    metadata_advertising: BTreeSet<String>,
    content_advertising: BTreeSet<String>,
    agent_edit_advertising: BTreeSet<String>,
    process_execution: BTreeSet<String>,
    network_execution: BTreeSet<String>,
}
```

Add a builder matching the existing `with_*` methods' shape. In `authorize`,
replace the combined arm so network consults its allowlist:

```rust
            ToolRisk::RunsProcess => self.process_execution.contains(&definition.name),
            ToolRisk::UsesNetwork => self.network_execution.contains(&definition.name),
            ToolRisk::MutatesLocalState => false,
```

Keep `MutatesLocalState` denied in `authorize`: it is gated by the edit
transaction path, not this allowlist, and this task must not change edit
behavior. In `allows_provider_advertising`, give `UsesNetwork` its own arm
consulting `network_execution`, leaving `FixtureSafe` denied.

- [ ] **Step 4: Widen the extension registration allowlist**

At `crates/yach-backend/src/tools.rs:2088`, the risk allowlist admits three
variants. Admit all five:

```rust
        if !matches!(
            definition.risk,
            ToolRisk::ReadsLocalMetadata
                | ToolRisk::ReadsLocalContent
                | ToolRisk::MutatesLocalState
                | ToolRisk::UsesNetwork
                | ToolRisk::RunsProcess
        ) {
```

`FixtureSafe` stays rejected for extension tools.

- [ ] **Step 5: Run the test to verify it passes**

Run: `just dev cargo test -p yach-backend --lib network_risk_is_allowed_only_when_allowlisted`
Expected: PASS.

- [ ] **Step 6: Verify and commit**

Run: `just test && just lint && just fmt-check`
Expected: all pass. If a test asserting that extension registration rejects
network risk now fails, it pinned the old restriction — read it, confirm
that is what it pins, and delete it rather than re-pinning the new text.

```bash
jj commit crates/yach-backend/src/tools.rs \
  -m "Allow allowlisted network-risk tools through the permission policy"
```

---

### Task 3: Derive capability sets and store grants

The new module: what capabilities a manifest requests, what the user
approved, and whether the first is a subset of the second. No activation
wiring yet, so this is testable without a host process.

**Files:**
- Create: `crates/yach-backend/src/extension_capability.rs`
- Modify: `crates/yach-backend/src/lib.rs` (add `mod extension_capability;` and re-export)
- Test: `crates/yach-backend/src/extension_capability.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `ExtensionToolRisk` from Task 1.
- Produces:
  - `ExtensionCapability` — `enum { UsesNetwork, RunsProcess }`, with
    `as_str(&self) -> &'static str` returning `"uses_network"` /
    `"runs_process"`.
  - `fn requested_capabilities(tools: &[ExtensionToolContribution]) -> BTreeSet<ExtensionCapability>`
  - `struct ExtensionCapabilityGrant { approved: BTreeSet<ExtensionCapability>, version_at_grant: String, granted_at: String }`
  - `fn grant_path(extension_id: &str) -> Option<PathBuf>`
  - `fn load_grant(extension_id: &str) -> Option<ExtensionCapabilityGrant>`
  - `fn store_grant(extension_id: &str, grant: &ExtensionCapabilityGrant) -> io::Result<()>`
  - `fn remove_grant(extension_id: &str) -> io::Result<()>`
  - `fn missing_capabilities(requested: &BTreeSet<ExtensionCapability>, grant: Option<&ExtensionCapabilityGrant>) -> BTreeSet<ExtensionCapability>`

  Later tasks call `requested_capabilities`, `load_grant`, and
  `missing_capabilities`; the CLI task calls `store_grant` and
  `remove_grant`.

- [ ] **Step 1: Write the failing tests**

Create `crates/yach-backend/src/extension_capability.rs` with the tests
first:

```rust
#[cfg(test)]
mod tests {
    use super::{
        ExtensionCapability, ExtensionCapabilityGrant, missing_capabilities,
        requested_capabilities,
    };
    use crate::extension::{ExtensionToolContribution, ExtensionToolRisk};
    use std::collections::BTreeSet;

    fn tool(name: &str, risk: ExtensionToolRisk) -> ExtensionToolContribution {
        ExtensionToolContribution {
            name: String::from(name),
            description: String::from("fixture"),
            risk,
            provider_visible: true,
        }
    }

    fn grant(approved: &[ExtensionCapability]) -> ExtensionCapabilityGrant {
        ExtensionCapabilityGrant {
            approved: approved.iter().copied().collect(),
            version_at_grant: String::from("1.0.0"),
            granted_at: String::from("2026-09-16T00:00:00Z"),
        }
    }

    #[test]
    fn only_network_and_process_risks_request_capability() {
        let tools = vec![
            tool("read", ExtensionToolRisk::ReadsLocalContent),
            tool("edit", ExtensionToolRisk::MutatesLocalState),
            tool("fetch", ExtensionToolRisk::UsesNetwork),
        ];
        assert_eq!(
            requested_capabilities(&tools),
            BTreeSet::from([ExtensionCapability::UsesNetwork]),
            "file-scoped risks must not require a grant"
        );
    }

    #[test]
    fn file_scoped_extensions_request_nothing() {
        let tools = vec![tool("read", ExtensionToolRisk::ReadsLocalContent)];
        assert!(requested_capabilities(&tools).is_empty());
    }

    #[test]
    fn a_widened_capability_set_is_not_covered_by_an_older_grant() {
        // The grant stores the approved set, so widening is caught even
        // though the manifest version string is unchanged.
        let approved = grant(&[ExtensionCapability::UsesNetwork]);
        let requested =
            BTreeSet::from([ExtensionCapability::UsesNetwork, ExtensionCapability::RunsProcess]);
        assert_eq!(
            missing_capabilities(&requested, Some(&approved)),
            BTreeSet::from([ExtensionCapability::RunsProcess])
        );
    }

    #[test]
    fn a_narrowed_capability_set_stays_covered() {
        let approved =
            grant(&[ExtensionCapability::UsesNetwork, ExtensionCapability::RunsProcess]);
        let requested = BTreeSet::from([ExtensionCapability::UsesNetwork]);
        assert!(missing_capabilities(&requested, Some(&approved)).is_empty());
    }

    #[test]
    fn no_grant_means_everything_requested_is_missing() {
        let requested = BTreeSet::from([ExtensionCapability::UsesNetwork]);
        assert_eq!(missing_capabilities(&requested, None), requested);
    }

    #[test]
    fn requesting_nothing_needs_no_grant() {
        assert!(missing_capabilities(&BTreeSet::new(), None).is_empty());
    }

    #[test]
    fn a_grant_round_trips_through_json() {
        let original = grant(&[ExtensionCapability::UsesNetwork]);
        let encoded = serde_json::to_string(&original);
        assert!(encoded.is_ok(), "grant should serialize: {encoded:?}");
        let Ok(encoded) = encoded else { return };
        assert!(
            encoded.contains("uses_network"),
            "capabilities serialize as snake_case strings: {encoded}"
        );
        let decoded: Result<ExtensionCapabilityGrant, _> = serde_json::from_str(&encoded);
        assert!(decoded.is_ok(), "grant should deserialize: {decoded:?}");
        let Ok(decoded) = decoded else { return };
        assert_eq!(decoded.approved, original.approved);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `just dev cargo test -p yach-backend --lib extension_capability`
Expected: FAIL — the module does not exist yet.

- [ ] **Step 3: Implement the module**

Above the test module in the same file:

```rust
//! Extension capability declarations and the user grants that admit them.
//!
//! A capability is a *declaration* the kernel records and gates activation
//! on. It is not enforced: the extension host is an ordinary subprocess with
//! the agent's privileges and can use the network whether or not it declared
//! doing so. See
//! `docs/project/specs/2026-09-16-extension-capability-contract-design.md`.

use std::collections::BTreeSet;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::extension::{ExtensionToolContribution, ExtensionToolRisk};

/// A capability an extension may request. Only risks that reach outside the
/// project require a grant; the file-scoped risks do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCapability {
    UsesNetwork,
    RunsProcess,
}

impl ExtensionCapability {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UsesNetwork => "uses_network",
            Self::RunsProcess => "runs_process",
        }
    }
}

/// The capability set a manifest's tools request, derived rather than
/// declared separately so the summary cannot understate the tools.
#[must_use]
pub fn requested_capabilities(
    tools: &[ExtensionToolContribution],
) -> BTreeSet<ExtensionCapability> {
    tools
        .iter()
        .filter_map(|tool| match tool.risk {
            ExtensionToolRisk::UsesNetwork => Some(ExtensionCapability::UsesNetwork),
            ExtensionToolRisk::RunsProcess => Some(ExtensionCapability::RunsProcess),
            ExtensionToolRisk::ReadsLocalMetadata
            | ExtensionToolRisk::ReadsLocalContent
            | ExtensionToolRisk::MutatesLocalState => None,
        })
        .collect()
}

/// A recorded user grant.
///
/// The approved set is authoritative. `version_at_grant` is display
/// metadata and is never compared: the manifest version is an unvalidated
/// string unrelated to contents, so an extension could widen its
/// capabilities without changing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionCapabilityGrant {
    pub approved: BTreeSet<ExtensionCapability>,
    pub version_at_grant: String,
    pub granted_at: String,
}

/// Capabilities requested but not approved. Empty means activation may
/// proceed.
#[must_use]
pub fn missing_capabilities(
    requested: &BTreeSet<ExtensionCapability>,
    grant: Option<&ExtensionCapabilityGrant>,
) -> BTreeSet<ExtensionCapability> {
    match grant {
        None => requested.clone(),
        Some(grant) => requested.difference(&grant.approved).copied().collect(),
    }
}

/// Grants live in user home only: repository content may restrict authority
/// but must never grant it.
#[must_use]
pub fn grant_path(extension_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".yach")
            .join("extensions")
            .join(format!("{extension_id}.json")),
    )
}
```

Implement `load_grant`, `store_grant`, and `remove_grant` over `grant_path`.
`load_grant` returns `None` for a missing or unreadable record — a
corrupt grant must not be treated as approval. `store_grant` creates the
directory and writes the file with mode `0o600` and the directory `0o700`,
matching `crates/yach-backend/src/session_store.rs:147-180`; read that
function and follow it. `remove_grant` treats a missing file as success.

- [ ] **Step 4: Declare the module**

In `crates/yach-backend/src/lib.rs`, add `mod extension_capability;` beside
the other module declarations and re-export the public items in the same
style as the neighbouring `pub use` lines.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `just dev cargo test -p yach-backend --lib extension_capability`
Expected: PASS — all seven.

- [ ] **Step 6: Verify and commit**

Run: `just test && just lint && just fmt-check`

```bash
jj commit crates/yach-backend/src/extension_capability.rs crates/yach-backend/src/lib.rs \
  -m "Derive extension capability sets and persist user grants"
```

---

### Task 4: Bind host registration to the manifest

Today activation passes only the tool *count* into registration
(`crates/yach-backend/src/extension.rs:1224`) and the loop accepts the
host's name and risk verbatim
(`crates/yach-backend/src/extension.rs:1800`). Without this, a metadata-only manifest could be
granted silently and then register a network tool — consent for a surface
the host never had to honor. This task must land before activation gating,
or that gate is cosmetic.

**Files:**
- Modify: `crates/yach-backend/src/extension.rs:1766-1830` (`initialize_and_register`)
- Modify: `crates/yach-backend/src/extension.rs:1219-1230` (call site)
- Modify: `crates/yach-backend/src/extension.rs` (`ExtensionHostProtocolError`)
- Test: `crates/yach-backend/src/extension.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `ExtensionToolContribution` (existing).
- Produces: `initialize_and_register` takes the manifest's declared tools —
  `&[ExtensionToolContribution]` — in place of `expected_tool_count: usize`.
  Two new `ExtensionHostProtocolError` variants:
  `UndeclaredTool { name: String }` and
  `ToolRiskMismatch { name: String, declared: ExtensionToolRisk, registered: ExtensionToolRisk }`.

- [ ] **Step 1: Write the failing tests**

There are existing tests driving a fixture host through
`initialize_and_register`; find them with
`grep -n 'initialize_and_register' crates/yach-backend/src/extension.rs` and
follow the nearest one's transport fixture pattern exactly. Add:

`FakeExtensionHostTransport::new` (defined at
`crates/yach-backend/src/extension.rs:2841`) takes a sequence of server
messages, which is all these tests need:

```rust
    fn declared_tool(name: &str, risk: ExtensionToolRisk) -> ExtensionToolContribution {
        ExtensionToolContribution {
            name: String::from(name),
            description: String::from("fixture"),
            risk,
            provider_visible: true,
        }
    }

    fn register_message(name: &str, risk: ExtensionToolRisk) -> ExtensionHostServerMessage {
        ExtensionHostServerMessage::ToolRegister {
            name: String::from(name),
            description: String::from("fixture"),
            risk,
            provider_visible: true,
            input_schema: ToolInputSchema::object_with_no_additional_properties(Vec::new()),
        }
    }

    fn ready_message() -> ExtensionHostServerMessage {
        ExtensionHostServerMessage::Ready {
            protocol: String::from("yach.extension-host.v2"),
            extension_id: String::from("capability-fixture"),
        }
    }

    #[test]
    fn a_host_cannot_register_a_tool_its_manifest_does_not_declare() {
        // Consent is computed from the manifest, so a registration outside
        // it would spend approval obtained for a different surface.
        let declared = vec![declared_tool("read_thing", ExtensionToolRisk::ReadsLocalContent)];
        let transport = FakeExtensionHostTransport::new([
            Ok(ready_message()),
            Ok(register_message("fetch_url", ExtensionToolRisk::UsesNetwork)),
        ]);
        let mut session = ExtensionHostSession::new(
            String::from("capability-fixture"),
            transport,
            64 * 1024,
        );
        let mut registry = ToolRegistry::default();

        let result = session.initialize_and_register(
            &mut registry,
            Some("1.0.0"),
            &declared,
            Duration::from_secs(1),
        );

        assert!(
            matches!(
                result,
                Err(ExtensionHostProtocolError::UndeclaredTool { ref name })
                    if name == "fetch_url"
            ),
            "expected UndeclaredTool, got {result:?}"
        );
        assert!(
            registry.resolve_extension_tool("fetch_url").is_none(),
            "an undeclared tool must not reach the registry"
        );
    }

    #[test]
    fn a_host_cannot_register_a_declared_tool_with_a_different_risk() {
        // The manifest says metadata; the host claims network. Accepting
        // this would let an extension take a silent file-scoped activation
        // and then hold network capability.
        let declared = vec![declared_tool("peek", ExtensionToolRisk::ReadsLocalMetadata)];
        let transport = FakeExtensionHostTransport::new([
            Ok(ready_message()),
            Ok(register_message("peek", ExtensionToolRisk::UsesNetwork)),
        ]);
        let mut session = ExtensionHostSession::new(
            String::from("capability-fixture"),
            transport,
            64 * 1024,
        );
        let mut registry = ToolRegistry::default();

        let result = session.initialize_and_register(
            &mut registry,
            Some("1.0.0"),
            &declared,
            Duration::from_secs(1),
        );

        assert!(
            matches!(
                result,
                Err(ExtensionHostProtocolError::ToolRiskMismatch { ref name, .. })
                    if name == "peek"
            ),
            "expected ToolRiskMismatch, got {result:?}"
        );
    }
```

`ExtensionHostSession::new`, `ToolRegistry`'s constructor, the registry
lookup used above, and `ToolInputSchema`'s constructor must match this
codebase. Check each against the neighbouring test and
`crates/yach-backend/src/extension.rs:1219-1230`, and adapt the fixture —
never change production signatures to fit a test.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `just dev cargo test -p yach-backend --lib a_host_cannot_register`
Expected: FAIL — registration currently accepts both.

- [ ] **Step 3: Change the signature and validate each registration**

Replace the `expected_tool_count: usize` parameter with
`declared_tools: &[ExtensionToolContribution]`. Inside the loop, after
receiving a `ToolRegister`, look the name up among the declared tools:

```rust
            let Some(declared) = declared_tools.iter().find(|tool| tool.name == name) else {
                return Err(ExtensionHostProtocolError::UndeclaredTool { name });
            };
            if declared.risk != risk {
                return Err(ExtensionHostProtocolError::ToolRiskMismatch {
                    name,
                    declared: declared.risk,
                    registered: risk,
                });
            }
```

Iterate `0..declared_tools.len()` so the count still bounds the loop. Update
the call site at line 1224 to pass `&record.manifest.contributes.tools`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `just dev cargo test -p yach-backend --lib a_host_cannot_register`
Expected: PASS.

- [ ] **Step 5: Verify and commit**

Run: `just test && just lint && just fmt-check`
Expected: all pass, including the existing hashline activation tests — the
bundled extension's manifest and registrations already agree, so it must be
unaffected. If it fails, the validation is wrong, not the fixture.

```bash
jj commit crates/yach-backend/src/extension.rs \
  -m "Reject host registrations that diverge from the extension manifest"
```

---

### Task 5: Gate activation on the grant

**Files:**
- Modify: `crates/yach-backend/src/extension.rs:1116-1180` (`activate_background_metadata_extensions`)
- Modify: `crates/yach-backend/src/extension.rs:801-861` (`reload_extension_from_record`)
- Modify: `crates/yach-backend/src/extension.rs:555-580` (`ExtensionActivationErrorKind` if a reason string is added)
- Test: `crates/yach-backend/src/extension.rs` (inline `mod tests`)

**There are two activation entry points, and both spawn hosts.**
`activate_background_metadata_extensions` runs at startup;
`reload_extension_from_record` is reached from `/extension-reload` and
increments `host_start_count` then calls `activate_extension_host_record`
directly (`crates/yach-backend/src/extension.rs:859-861`). They already
duplicate the same three guards — project trust, activation event, empty
tools. Adding the consent check to only the first would leave
`/extension-reload` as a bypass of the entire contract, so this task adds
one shared check used by both.

**Interfaces:**
- Consumes: `requested_capabilities`, `load_grant`, `missing_capabilities`
  from Task 3.
- Produces: `fn capability_block_reason(manifest: &ExtensionManifest) -> Option<String>`
  — `Some(message)` when the manifest requests ungranted capabilities,
  `None` when activation may proceed. Both entry points call it before
  spawning. The message names each missing capability and the grant command.

- [ ] **Step 1: Write the failing test**

Follow the existing activation tests (`grep -n 'activate_background_metadata_extensions' crates/yach-backend/src/extension.rs`)
for the package-record fixture shape, and set `HOME` to a temp directory so
no real grant is read:

```rust
    #[test]
    fn an_extension_requesting_network_without_a_grant_is_policy_blocked() {
        // Activation is synchronous and cannot prompt, so it refuses with a
        // diagnostic the user can act on, exactly as project-scope
        // extensions are refused today.
        let home = std::env::temp_dir().join(format!("yach-cap-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        // SAFETY NOTE: this test sets HOME. If the surrounding test module
        // runs tests in parallel against a shared HOME, follow whatever
        // serialization the neighbouring HOME-dependent tests use (look for
        // an existing lock or `#[serial]`-style guard) rather than
        // introducing a new mechanism.
        unsafe { std::env::set_var("HOME", &home) };

        let record = /* package record whose manifest declares one
            uses_network tool; build it with the same helper the
            neighbouring activation tests use */ todo_build_record();
        let snapshot = activate_background_metadata_extensions(
            std::slice::from_ref(&record),
            ExtensionBackgroundActivationConfig::default(),
            None,
        );

        assert_eq!(
            snapshot.host_start_count, 0,
            "consent is checked before spawn, so no host may start"
        );
        let Some(diagnostic) = snapshot.diagnostics.first() else {
            panic!("expected an activation diagnostic");
        };
        assert!(
            matches!(
                diagnostic.error_kind(),
                Some(ExtensionActivationErrorKind::PolicyBlocked)
            ),
            "expected PolicyBlocked, got {diagnostic:?}"
        );
        assert!(
            diagnostic.message().contains("uses_network"),
            "the diagnostic must name the missing capability: {diagnostic:?}"
        );
    }

    #[test]
    fn reloading_an_extension_cannot_bypass_the_capability_gate() {
        // `/extension-reload` reaches reload_extension_from_record, which
        // spawns a host on its own path. If the consent check lived only in
        // the startup path, reloading would activate an ungranted network
        // extension.
        let home = std::env::temp_dir().join(format!("yach-cap-reload-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        unsafe { std::env::set_var("HOME", &home) };

        let record = /* same helper as above: manifest declaring one
            uses_network tool, no grant on disk */ todo_build_record();
        let mut snapshot = ExtensionActivationSnapshot::default();
        let before = snapshot.host_start_count;

        let diagnostic = snapshot.reload_extension_from_record(
            &record,
            ExtensionBackgroundActivationConfig::default(),
            None,
        );

        assert_eq!(
            snapshot.host_start_count, before,
            "reload must check consent before spawning a host"
        );
        assert!(
            matches!(
                diagnostic.error_kind(),
                Some(ExtensionActivationErrorKind::PolicyBlocked)
            ),
            "expected PolicyBlocked on reload, got {diagnostic:?}"
        );
    }

    #[test]
    fn a_file_scoped_extension_activates_without_a_grant() {
        // The three local risks are untouched by this contract, so an
        // existing extension must not acquire a new activation gate.
        let record = /* same helper, manifest declaring one
            reads_local_content tool */ todo_build_record();
        let snapshot = activate_background_metadata_extensions(
            std::slice::from_ref(&record),
            ExtensionBackgroundActivationConfig::default(),
            None,
        );
        assert!(
            snapshot
                .diagnostics
                .first()
                .is_none_or(|d| d.error_kind().is_none()),
            "a file-scoped extension must not be policy-blocked"
        );
    }
```

Replace both `todo_build_record()` calls with the real package-record
fixture: find it with
`grep -n 'ExtensionPackageRecord {' crates/yach-backend/src/extension.rs | head`
and reuse the nearest activation test's builder, changing only the declared
tool's risk. `error_kind()` and `message()` are placeholders for whatever
accessors `ExtensionActivationDiagnostic` actually exposes — check the
struct and use its real fields or methods. The `host_start_count` assertion
is what proves the pre-spawn ordering; keep it in whatever form compiles.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `just dev cargo test -p yach-backend --lib without_a_grant` and
`just dev cargo test -p yach-backend --lib cannot_bypass_the_capability_gate`
Expected: FAIL on both — neither activation path checks consent yet. The
reload failure is the one that matters most: it is the path a user reaches
from `/extension-reload`.

- [ ] **Step 3: Add one shared consent check and call it from both paths**

Add a free function next to the other activation helpers:

```rust
/// The reason activation must be refused, or `None` when the manifest's
/// requested capabilities are covered by a recorded grant.
///
/// Both activation entry points call this before spawning a host. Keeping
/// it in one place is the point: a second copy is how `/extension-reload`
/// would drift into bypassing the contract.
fn capability_block_reason(manifest: &ExtensionManifest) -> Option<String> {
    let requested =
        crate::extension_capability::requested_capabilities(&manifest.contributes.tools);
    if requested.is_empty() {
        return None;
    }
    let grant = crate::extension_capability::load_grant(&manifest.id.0);
    let missing = crate::extension_capability::missing_capabilities(&requested, grant.as_ref());
    if missing.is_empty() {
        return None;
    }
    let names: Vec<&str> = missing.iter().map(ExtensionCapability::as_str).collect();
    Some(format!(
        "extension requests ungranted capabilities: {}. Grant with `/extension-trust {}`.",
        names.join(", "),
        manifest.id.0
    ))
}
```

`ExtensionManifest` is the manifest type on `ExtensionPackageRecord`;
confirm its real name with
`grep -n 'pub manifest:' crates/yach-backend/src/extension.rs | head -2`
and use that.

In `activate_background_metadata_extensions`, after the existing
project-trust, activation-event, and empty-tools guards and **before**
`host_start_count` is incremented:

```rust
        if let Some(reason) = capability_block_reason(&record.manifest) {
            diagnostic.mark_blocked(ExtensionActivationErrorKind::PolicyBlocked, &reason);
            snapshot.diagnostics.push(diagnostic);
            continue;
        }
```

In `reload_extension_from_record`, at the same position — after its
empty-tools guard at `crates/yach-backend/src/extension.rs:854-857` and
before `host_start_count` is incremented at line 859:

```rust
        if let Some(reason) = capability_block_reason(&record.manifest) {
            diagnostic.mark_blocked(ExtensionActivationErrorKind::PolicyBlocked, &reason);
            self.diagnostics.push(diagnostic.clone());
            return diagnostic;
        }
```

Note the two paths differ in how they record a diagnostic — one pushes to a
snapshot and continues the loop, the other pushes a clone and returns it.
Follow each function's existing guards rather than unifying that.

Match `mark_blocked`'s real signature — check whether it takes `&str` or
`String` and adapt.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `just dev cargo test -p yach-backend --lib _a_grant`
Expected: PASS.

- [ ] **Step 5: Verify and commit**

Run: `just test && just lint && just fmt-check`

```bash
jj commit crates/yach-backend/src/extension.rs \
  -m "Block activation of extensions with ungranted capabilities"
```

---

### Task 6: Surface capabilities and grant state in diagnostics

Activation is the only authority point, so being unable to see what an
extension may do makes that consent hollow. Diagnostics currently list tool
names and no risk at all.

**Files:**
- Modify: `crates/yach-cli/src/main.rs:510-523` (diagnostic rendering)
- Modify: `crates/yach-backend/src/extension.rs` (diagnostic struct, if capabilities must be carried)
- Test: `crates/yach-cli/src/main.rs` or the backend diagnostic test, wherever the existing diagnostic assertions live

**Interfaces:**
- Consumes: `requested_capabilities`, `load_grant` from Task 3.
- Produces: diagnostic output containing a `capabilities:` line and a
  `grant:` line. Later tasks do not depend on the exact wording.

- [ ] **Step 1: Write the failing test**

`ExtensionDiagnosticRecord::render_line` is a pure `&self -> String`
(`crates/yach-cli/src/main.rs:862`), so it tests directly. Add to the test
module in `crates/yach-cli/src/main.rs`:

```rust
    #[test]
    fn diagnostics_report_declared_capabilities_and_grant_state() {
        // Activation is the only authority point, so a user who cannot see
        // what an extension may do has not meaningfully consented.
        let record = ExtensionDiagnosticRecord {
            // Build from the neighbouring test's fixture if one exists;
            // otherwise fill every field explicitly. Declare one
            // uses_network tool and leave the grant absent.
            ..capability_diagnostic_fixture()
        };

        let line = record.render_line();
        assert!(
            line.contains("capabilities=uses_network"),
            "declared capabilities must be visible: {line}"
        );
        assert!(
            line.contains("capability_grant=none"),
            "grant state must be visible: {line}"
        );
    }

    #[test]
    fn a_file_scoped_extension_reports_no_capabilities() {
        let record = ExtensionDiagnosticRecord {
            ..file_scoped_diagnostic_fixture()
        };
        let line = record.render_line();
        assert!(
            line.contains("capabilities=none"),
            "a file-scoped extension requests nothing: {line}"
        );
    }
```

`capability_diagnostic_fixture` and `file_scoped_diagnostic_fixture` are
yours to write in the test module: construct `ExtensionDiagnosticRecord`
with every field set (the struct is at
`crates/yach-cli/src/main.rs:507-524`), differing only in the declared
capability set. The two field names asserted above —
`capabilities=` and `capability_grant=` — are the contract this task adds
to `render_line`; keep them consistent between the test and the
implementation.

- [ ] **Step 2: Run to verify it fails**

Run: `just dev cargo test -p yach-cli`
Expected: FAIL on the new assertion.

- [ ] **Step 3: Render capabilities and grant state**

Add the two lines to the diagnostic rendering. Keep the wording factual:
state what the extension declared and whether it is granted. Do not describe
a granted capability as "sandboxed", "restricted", or "limited to" — the
contract does not enforce.

- [ ] **Step 4: Run to verify it passes**

Run: `just dev cargo test -p yach-cli`

- [ ] **Step 5: Verify and commit**

Run: `just test && just lint && just fmt-check`

```bash
jj commit crates/yach-cli/src/main.rs crates/yach-backend/src/extension.rs \
  -m "Show declared capabilities and grant state in extension diagnostics"
```

---

### Task 7: Grant and revoke commands with evidence

**Files:**
- Modify: `crates/yach-ui/src/slash_commands.rs` (`/extension-trust`)
- Modify: `crates/yach-ui/src/app.rs` (dispatch)
- Modify: `crates/yach-cli/src/main.rs` (CLI subcommand)
- Modify: `crates/yach-backend/src/session.rs` or the permission evidence path (grant evidence)
- Test: inline `mod tests` in each modified crate

**Interfaces:**
- Consumes: `store_grant`, `remove_grant`, `requested_capabilities` from
  Task 3.
- Produces: `/extension-trust <id>` grants; `/extension-revoke <id>`
  removes. Both record permission-decision evidence with reason
  `extension_capability_grant` / `extension_capability_revoke`.

- [ ] **Step 1: Write the failing tests**

`/extension-trust` takes an argument, so follow the existing
argument-accepting commands — `/extension-stop` and `/extension-reload`
parse and dispatch at `crates/yach-ui/src/slash_commands.rs:186-200` and
`crates/yach-ui/src/app.rs:3344-3363`. Mirror that pattern exactly, and add
a parser test alongside `parser_accepts_extension_stop_selector_argument`.

Then a backend test asserting the grant round trip: granting writes the
approved set, a subsequent `missing_capabilities` is empty, and revoking
makes it non-empty again. Point `HOME` at a temp directory.

- [ ] **Step 2: Run to verify they fail**

Run: `just dev cargo test -p yach-ui --lib extension_trust && just dev cargo test -p yach-backend --lib grant`

- [ ] **Step 3: Implement the commands**

Add the slash commands, their dispatch arms, and the CLI subcommand. The
grant path computes requested capabilities from the manifest, writes the
grant, and records evidence. Revocation removes the record and records
evidence.

- [ ] **Step 4: Run to verify they pass**

- [ ] **Step 5: Verify and commit**

Run: `just test && just lint && just fmt-check`

```bash
jj commit crates/yach-ui/src/slash_commands.rs crates/yach-ui/src/app.rs \
  crates/yach-cli/src/main.rs crates/yach-backend/src/session.rs \
  -m "Add extension capability grant and revoke commands"
```

---

### Task 8: End-to-end proof and documentation

**Files:**
- Create: a fixture extension under the existing extension test fixtures —
  find them with `grep -rn 'yach.extension.json' crates/ --include=*.rs -l`
- Modify: `docs/` extension documentation, wherever the extension contract
  is described (`grep -rln 'extension' docs/*.md docs/**/*.md`)

- [ ] **Step 1: Write the end-to-end test**

A fixture extension declaring a `uses_network` tool:
blocked without a grant, activates after one, its tool is advertised and
callable, and the grant survives a simulated restart by being re-read from
disk.

- [ ] **Step 2: Run to verify it fails, then implement any glue, then verify it passes**

- [ ] **Step 3: Document the contract**

Document declaring a network or process tool, granting, revoking, and
reading diagnostics. State plainly that the declaration is consented to, not
enforced, and that an extension can use the network without declaring it —
undetected by Yach.

- [ ] **Step 4: Smoke test against the real binary**

The capability path is user-facing, so prove it outside unit tests. Use the
TUI visual harness (`tests/visual/`, and see `just tui-visual`) or
`yach rpc` against a fixture extension, and record what you observed in the
commit message.

Harness note: `tests/visual/hardening.tape` settles on timing and asserts
with screenshots rather than `Wait+Screen`, because the older
`tests/visual/session.tape` waits on `/no model/`, a string its fixture no
longer renders. `Wait+Screen` itself works — it was probed against both
ordinary shell output and alternate-screen content — so prefer it where the
string you wait for is one the current fixture actually prints, and fall back
to timing plus screenshots otherwise.

- [ ] **Step 5: Verify and commit**

Run: `just test && just lint && just fmt-check`

```bash
jj commit <fixture paths> <doc paths> \
  -m "Prove the extension capability contract end to end"
```

---

## Verification

- `just test`, `just lint`, `just fmt-check` after every task.
- Reproduce CI clippy with `just lint-ci` before publishing.
- The deterministic perf gate runs on the pull request. This plan adds a
  per-extension grant read at activation, which touches the
  `extension/activation/*` and `startup/*` workloads. If a row regresses,
  measure it with `just perf --filter 'extension/*'`, and follow the
  thresholds file's convention: name the affected row with its measured
  figure and a comment, rather than widening a glob.

## Out of scope

Per-call review of extension network or process calls; enforcement or
detection of undeclared use; signing or pinning; any OS-sandbox claim; the
in-process `ToolExecutor` embedder seam.
