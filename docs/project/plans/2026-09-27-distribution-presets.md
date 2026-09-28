# Distribution Presets and Reference Components Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make yach's model-facing opinions (the six project tools, the
baseline guidance, and the two bundled extensions) replaceable components,
selected by named presets (`minimal`, `full`), with bundled extensions that
users can remove and a core build that omits them.

**Architecture:**

- A backend `ComponentSet` value (enabled kernel components) is resolved once
  per session from user config or an ephemeral `--preset`, carried on
  `RunnerConfig`, and consulted in two places: tool routing and provider
  message assembly.
- Tool routing: the project tools leave the routable name list, the rig
  approval allowlist, and the registry together. With the tools absent, the
  existing replacement-bundle resolver already reports a missing target as a
  member failure (`ToolResolutionError::MissingBuiltIn`). The discarded bundle
  diagnostics become visible.
- Guidance moves from an unconditional first message into a conditional one
  owned by the `baseline-guidance` component.
- Preset state (`[preset]`, `[components]`, `[bundled]`) extends the existing
  TOML-preserving `UserConfigStore`. Bundled extension records stop being
  re-seeded on every read; a startup refresh repoints only existing records on
  a version change.
- `bundled-hashline` / `bundled-jev` cargo features gate the extension crates.

**Tech Stack:** Rust 2024 workspace (`yach-backend`, `yach` CLI crate at
`crates/yach-cli`), `toml_edit`, serde/serde_json, tokio. Integration tests
drive `yach rpc` and `yach extension` as child processes.

**Spec:** `docs/project/specs/2026-09-27-distribution-presets-design.md`

**Source:** plane:YACH-12

## Global Constraints

- Presets select components only. Applying a preset never changes approval
  mode, project trust, capability grants, shell allowlists, or
  `AUTO_REVIEW_EXECUTION_ENABLED`.
- Component names (exact): `project-tools`, `baseline-guidance`, `hashline`,
  `jev-reviewer`, `skill-index`. Preset names (exact): `minimal`, `full`.
  The word "profile" is never used for this concept in code, CLI, or docs.
- `minimal` = `bash` + `skill-index`. `full` = `bash`, `project-tools`,
  `baseline-guidance`, `hashline`, `jev-reviewer`, `skill-index`.
- `skill-index` is a reserved slot with no behavior in this plan; it is
  accepted in config and presets and reported by `yach component list` as
  "reserved (plane:YACH-14)".
- Bundled extension ids (exact): `yach.hashline`, `yach.jev-reviewer`.
  Component-to-id map: `hashline` → `yach.hashline`, `jev-reviewer` →
  `yach.jev-reviewer`.
- Config layout in `~/.yach/config.toml` (exact keys):
  `[preset] applied = "<name>"`; `[components] project-tools = <bool>`,
  `baseline-guidance = <bool>`; `[bundled] removed = [<ids>]`.
  `[components]` holds only booleans for known kernel components; unknown keys
  produce a diagnostic and are ignored (not a load failure).
- First apply of `full` preserves every existing bundled record's `enabled`.
- Removed bundled ids are never re-added by startup or `preset use`; only
  `yach extension install --bundled <id>` or `yach preset use <name> --reset`
  clears them.
- Upgrade refresh: the materialized version of a bundled record is the last
  path component of its `package_root`; on mismatch with `CARGO_PKG_VERSION`,
  re-materialize and repoint, keeping `enabled`. Never create records, never
  touch removed ids.
- `compaction.summary_prompt` is honored from user config only; the same key
  in `<project>/.yach/config.json` is ignored with a diagnostic.
- A preset or component naming a bundled extension not compiled into this build
  records the preference, reports `not compiled in`, and never fails startup.
- Existing replacement rules are unchanged
  (`2026-08-21-hashline-extension-bundle-design.md:215-222`).
- All seven built-in tool names (six project tools and `bash`) stay reserved
  against extension registration in every component set; disabling
  `project-tools` never frees a name for an undeclared extension tool.
- A replacement bundle that fails for any reason contributes no tools to the
  catalog: its member implementations are dropped too, so a failed bundle
  never leaks standalone tools, and the rig approval allowlist always covers
  every advertised name.
- Default build behavior (default features, no config) is byte-for-byte the
  same provider request as today: same tools, same guidance text.
- Skip formatters, linters, and project-wide test suites inside tasks; run only
  the named tests. Final verification runs `just fmt`, `just lint`, and
  `just dev cargo test -p yach-backend -p yach`.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/yach-backend/src/components.rs` (new) | `Component`, `Preset`, `ComponentSet`: names, preset membership, parsing | 1 |
| `crates/yach-backend/src/user_config.rs` | Parse/persist `[preset]`, `[components]`, `[bundled]` | 2 |
| `crates/yach-backend/src/lib.rs` | Re-export component types | 1 |
| `crates/yach-backend/src/tools.rs` | Registry constructor honoring `project-tools` | 3 |
| `crates/yach-backend/src/extension.rs` | Snapshot default registry from components; surface bundle diagnostics | 3 |
| `crates/yach-backend/src/runner.rs` | `RunnerConfig.components`; routable names, rig allowlist, guidance message | 3, 4 |
| `crates/yach-backend/src/runner/extension_state.rs` | Pass components into activation; diagnostics status line | 3 |
| `crates/yach-backend/src/compaction.rs` | User-scope `summary_prompt` | 5 |
| `crates/yach-backend/src/extension_install.rs` | Removable bundled records; `refresh_bundled` | 6 |
| `crates/yach-backend/src/bench_loop.rs` | New `RunnerConfig` field in literals | 3 |
| `crates/yach-cli/src/bundled.rs` (new) | Materialization, upgrade refresh, preset apply, not-compiled-in reporting | 6, 7 |
| `crates/yach-cli/src/main.rs` | `preset` / `component` commands; remove per-read seeding; features on host dispatch | 6, 7, 8 |
| `crates/yach-cli/src/headless.rs`, `crates/yach-cli/src/rpc.rs` | `--preset` ephemeral selection | 7 |
| `crates/yach-cli/Cargo.toml` | `bundled-hashline`, `bundled-jev` features | 8 |
| `crates/yach-cli/tests/presets.rs` (new) | RPC and CLI acceptance scenarios | 9 |
| `crates/yach-cli/tests/hashline_extension.rs` | Update re-seeding assertion | 6 |
| `README.md`, `docs/extensions.md`, `docs/presets.md` (new) | User docs | 10 |

`RunnerConfig` is built as a struct literal at ~80 sites (CLI, backend tests,
bench). Task 3 adds exactly one field, `components: ComponentSet`, and every
literal gets `components: ComponentSet::full()` except the production
constructors named in Task 3 and Task 7.

---

### Task 1: Component and preset vocabulary

**Files:**
- Create: `crates/yach-backend/src/components.rs`
- Modify: `crates/yach-backend/src/lib.rs` (add `mod components;` and
  `pub use components::{Component, ComponentSet, Preset};`)
- Test: inline `#[cfg(test)] mod tests` in `components.rs`

**Interfaces:**
- Produces:
  - `pub enum Component { ProjectTools, BaselineGuidance, Hashline, JevReviewer, SkillIndex }`
    with `pub const ALL: [Component; 5]`, `pub fn name(self) -> &'static str`,
    `pub fn parse(name: &str) -> Option<Component>`,
    `pub fn bundled_extension_id(self) -> Option<&'static str>` (Some for
    `Hashline`/`JevReviewer` only), `pub fn is_kernel(self) -> bool` (true for
    `ProjectTools`, `BaselineGuidance`, `SkillIndex`).
  - `pub enum Preset { Minimal, Full }` with `name`, `parse`,
    `pub fn components(self) -> &'static [Component]`.
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct ComponentSet { bits: u8 }`
    with `pub const fn full() -> Self`, `pub fn from_preset(Preset) -> Self`,
    `pub fn contains(self, Component) -> bool`,
    `pub fn with(self, Component, bool) -> Self`,
    `pub fn project_tools(self) -> bool`, `pub fn baseline_guidance(self) -> bool`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_membership_matches_the_spec() {
        let minimal = ComponentSet::from_preset(Preset::Minimal);
        assert!(!minimal.project_tools());
        assert!(!minimal.baseline_guidance());
        assert!(minimal.contains(Component::SkillIndex));
        assert!(!minimal.contains(Component::Hashline));

        let full = ComponentSet::from_preset(Preset::Full);
        for component in Component::ALL {
            assert!(full.contains(component), "{}", component.name());
        }
        assert_eq!(full, ComponentSet::full());
    }

    #[test]
    fn names_round_trip_and_reject_unknown() {
        for component in Component::ALL {
            assert_eq!(Component::parse(component.name()), Some(component));
        }
        assert_eq!(Component::parse("profile"), None);
        assert_eq!(Preset::parse("minimal"), Some(Preset::Minimal));
        assert_eq!(Preset::parse("full"), Some(Preset::Full));
        assert_eq!(Preset::parse("default"), None);
    }

    #[test]
    fn only_extension_components_map_to_bundled_ids() {
        assert_eq!(Component::Hashline.bundled_extension_id(), Some("yach.hashline"));
        assert_eq!(Component::JevReviewer.bundled_extension_id(), Some("yach.jev-reviewer"));
        assert_eq!(Component::ProjectTools.bundled_extension_id(), None);
        assert!(Component::BaselineGuidance.is_kernel());
        assert!(!Component::Hashline.is_kernel());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend components::tests`
Expected: FAIL to compile (`components` module does not exist).

- [ ] **Step 3: Implement**

```rust
//! Reference components and the presets that select them.
//! Design: docs/project/specs/2026-09-27-distribution-presets-design.md.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Component {
    ProjectTools,
    BaselineGuidance,
    Hashline,
    JevReviewer,
    SkillIndex,
}

impl Component {
    pub const ALL: [Component; 5] = [
        Self::ProjectTools,
        Self::BaselineGuidance,
        Self::Hashline,
        Self::JevReviewer,
        Self::SkillIndex,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ProjectTools => "project-tools",
            Self::BaselineGuidance => "baseline-guidance",
            Self::Hashline => "hashline",
            Self::JevReviewer => "jev-reviewer",
            Self::SkillIndex => "skill-index",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|component| component.name() == name)
    }

    #[must_use]
    pub const fn bundled_extension_id(self) -> Option<&'static str> {
        match self {
            Self::Hashline => Some("yach.hashline"),
            Self::JevReviewer => Some("yach.jev-reviewer"),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_kernel(self) -> bool {
        self.bundled_extension_id().is_none()
    }

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Minimal,
    Full,
}

impl Preset {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Full => "full",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Minimal, Self::Full]
            .into_iter()
            .find(|preset| preset.name() == name)
    }

    #[must_use]
    pub const fn components(self) -> &'static [Component] {
        match self {
            Self::Minimal => &[Component::SkillIndex],
            Self::Full => &Component::ALL,
        }
    }
}

/// Enabled components for one session. `bash` is kernel and always present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentSet {
    bits: u8,
}

impl ComponentSet {
    #[must_use]
    pub const fn full() -> Self {
        Self { bits: 0b1_1111 }
    }

    #[must_use]
    pub fn from_preset(preset: Preset) -> Self {
        preset
            .components()
            .iter()
            .fold(Self { bits: 0 }, |set, component| set.with(*component, true))
    }

    #[must_use]
    pub const fn contains(self, component: Component) -> bool {
        self.bits & component.bit() != 0
    }

    #[must_use]
    pub const fn with(self, component: Component, enabled: bool) -> Self {
        let bits = if enabled {
            self.bits | component.bit()
        } else {
            self.bits & !component.bit()
        };
        Self { bits }
    }

    #[must_use]
    pub const fn project_tools(self) -> bool {
        self.contains(Component::ProjectTools)
    }

    #[must_use]
    pub const fn baseline_guidance(self) -> bool {
        self.contains(Component::BaselineGuidance)
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend components::tests`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/components.rs crates/yach-backend/src/lib.rs -m "Add component and preset vocabulary"
```

---

### Task 2: Preset state in user config

**Files:**
- Modify: `crates/yach-backend/src/user_config.rs` (snapshot struct `:22-26`,
  `parse_snapshot` `:210-261`, new persist methods after `:128`)
- Test: inline tests in `user_config.rs` (reuse the `temp_store` helper used by
  `targeted_updates_preserve_unrelated_content` at `:428`)

**Interfaces:**
- Consumes: `Component`, `Preset`, `ComponentSet` (Task 1).
- Produces:
  - `UserConfigSnapshot` gains `pub preset_applied: Option<Preset>`,
    `pub component_overrides: BTreeMap<Component, bool>` (kernel components
    only), `pub bundled_removed: BTreeSet<String>`,
    `pub unknown_components: Vec<String>`.
  - `impl UserConfigSnapshot { pub fn kernel_components(&self) -> ComponentSet }`:
    starts from `ComponentSet::full()`, applies `component_overrides`.
  - `UserConfigStore::persist_component(&self, component: Component, enabled: bool) -> Result<(), UserConfigError>`
    (kernel components only; returns `Invalid` for extension components).
  - `UserConfigStore::persist_preset(&self, preset: Preset, reset: bool) -> Result<(), UserConfigError>`:
    writes `[preset] applied`, writes every kernel component's preset value into
    `[components]`, and when `reset` is true clears `[bundled] removed`.
  - `UserConfigStore::persist_bundled_removed(&self, id: &str, removed: bool) -> Result<(), UserConfigError>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn preset_tables_parse_and_unknown_components_are_diagnostics() {
    let (_directory, store) = temp_store("preset-parse");
    fs::write(
        store.path(),
        "[preset]\napplied = \"minimal\"\n\n[components]\nproject-tools = false\nlaser = true\n\n[bundled]\nremoved = [\"yach.hashline\"]\n",
    )
    .unwrap_or_default();
    set_private(store.path());
    let snapshot = store.load();
    assert!(snapshot.is_ok(), "{snapshot:?}");
    let Ok(snapshot) = snapshot else { return };
    assert_eq!(snapshot.preset_applied, Some(Preset::Minimal));
    assert!(!snapshot.kernel_components().project_tools());
    assert!(snapshot.kernel_components().baseline_guidance());
    assert!(snapshot.bundled_removed.contains("yach.hashline"));
    assert_eq!(snapshot.unknown_components, vec![String::from("laser")]);
}

#[test]
fn persist_preset_writes_components_preserving_unrelated_content_and_removed() {
    let (_directory, store) = temp_store("preset-persist");
    fs::write(
        store.path(),
        "# keep me\n[thinking]\ndefault = \"low\"\n\n[bundled]\nremoved = [\"yach.jev-reviewer\"]\n",
    )
    .unwrap_or_default();
    set_private(store.path());
    assert!(store.persist_preset(Preset::Minimal, false).is_ok());
    let raw = fs::read_to_string(store.path()).unwrap_or_default();
    assert!(raw.contains("# keep me"));
    assert!(raw.contains("default = \"low\""));
    let Ok(snapshot) = store.load() else { unreachable!("valid config") };
    assert_eq!(snapshot.preset_applied, Some(Preset::Minimal));
    assert!(!snapshot.kernel_components().project_tools());
    assert!(snapshot.bundled_removed.contains("yach.jev-reviewer"));

    assert!(store.persist_preset(Preset::Full, true).is_ok());
    let Ok(snapshot) = store.load() else { unreachable!("valid config") };
    assert!(snapshot.kernel_components().project_tools());
    assert!(snapshot.bundled_removed.is_empty(), "--reset clears removals");
}

#[test]
fn malformed_component_value_is_invalid() {
    let (_directory, store) = temp_store("preset-malformed");
    fs::write(store.path(), "[components]\nproject-tools = \"yes\"\n").unwrap_or_default();
    set_private(store.path());
    assert_eq!(store.load(), Err(UserConfigError::Invalid));
}
```

If the existing tests use a different helper to set `0600` permissions than
`set_private`, use that helper; `ensure_regular_private_or_missing` rejects
group/world-readable files.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend user_config::tests::preset`
Expected: FAIL to compile (fields and methods missing).

- [ ] **Step 3: Implement**

Add fields to `UserConfigSnapshot` and derive `Default` as today. In
`parse_snapshot`, after `model_default`:

```rust
let preset_applied = match document.get("preset") {
    None => None,
    Some(item) => {
        let table = item.as_table().ok_or(UserConfigError::Invalid)?;
        match table.get("applied") {
            None => None,
            Some(value) => {
                let raw = value.as_str().ok_or(UserConfigError::Invalid)?;
                Some(Preset::parse(raw).ok_or(UserConfigError::Invalid)?)
            }
        }
    }
};

let mut component_overrides = BTreeMap::new();
let mut unknown_components = Vec::new();
if let Some(item) = document.get("components") {
    let table = item.as_table().ok_or(UserConfigError::Invalid)?;
    for (key, value) in table {
        match Component::parse(key).filter(|component| component.is_kernel()) {
            Some(component) => {
                let enabled = value.as_bool().ok_or(UserConfigError::Invalid)?;
                component_overrides.insert(component, enabled);
            }
            None => unknown_components.push(key.to_owned()),
        }
    }
}

let mut bundled_removed = BTreeSet::new();
if let Some(item) = document.get("bundled") {
    let table = item.as_table().ok_or(UserConfigError::Invalid)?;
    if let Some(removed) = table.get("removed") {
        let array = removed.as_array().ok_or(UserConfigError::Invalid)?;
        for value in array {
            let id = value.as_str().ok_or(UserConfigError::Invalid)?;
            bundled_removed.insert(id.to_owned());
        }
    }
}
```

Persist methods, following `persist_thinking_default`:

```rust
pub fn persist_component(&self, component: Component, enabled: bool) -> Result<(), UserConfigError> {
    if !component.is_kernel() {
        return Err(UserConfigError::Invalid);
    }
    self.update(|document| {
        let components = table_mut(document.as_table_mut(), "components")?;
        components[component.name()] = value(enabled);
        Ok(())
    })
}

pub fn persist_preset(&self, preset: Preset, reset: bool) -> Result<(), UserConfigError> {
    let selected = ComponentSet::from_preset(preset);
    self.update(|document| {
        table_mut(document.as_table_mut(), "preset")?["applied"] = value(preset.name());
        let components = table_mut(document.as_table_mut(), "components")?;
        for component in Component::ALL.into_iter().filter(|c| c.is_kernel()) {
            components[component.name()] = value(selected.contains(component));
        }
        if reset {
            table_mut(document.as_table_mut(), "bundled")?.remove("removed");
        }
        Ok(())
    })
}

pub fn persist_bundled_removed(&self, id: &str, removed: bool) -> Result<(), UserConfigError> {
    self.update(|document| {
        let bundled = table_mut(document.as_table_mut(), "bundled")?;
        let mut ids: BTreeSet<String> = bundled
            .get("removed")
            .and_then(Item::as_array)
            .map(|array| array.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        if removed {
            ids.insert(id.to_owned());
        } else {
            ids.remove(id);
        }
        let mut array = toml_edit::Array::new();
        for id in ids {
            array.push(id);
        }
        bundled["removed"] = value(array);
        Ok(())
    })
}
```

`kernel_components`:

```rust
impl UserConfigSnapshot {
    #[must_use]
    pub fn kernel_components(&self) -> ComponentSet {
        self.component_overrides
            .iter()
            .fold(ComponentSet::full(), |set, (component, enabled)| set.with(*component, *enabled))
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend user_config::tests`
Expected: all `user_config` tests pass, including the pre-existing
`targeted_updates_preserve_unrelated_content`.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/user_config.rs -m "Persist preset and component state in user config"
```

---

### Task 3: `project-tools` component in tool routing

**Files:**
- Modify: `crates/yach-backend/src/runner.rs`:
  - `RunnerConfig` (`:119-146`) add `pub components: ComponentSet`; destructure
    at `:1118-1130`.
  - `provider_approved_tools()` (`:4096-4109`) → `provider_approved_tools(components: ComponentSet) -> Vec<String>`;
    call sites `:985`, `:1016` pass `config.components` (read before `config`
    is moved; bind `let components = config.components;` next to
    `let trace = config.trace.clone();`).
  - `ProviderPromptProjectRuntime` (`:821-836`) add `components: ComponentSet`;
    construction `:2095`; destructure `:9773-9781`.
  - `ProviderPromptRequest` (`:9822-9843`) and `ProviderAgentToolRound`
    (`:4757-4787`) add `components: ComponentSet`; thread it through.
  - `run_native_provider_one_agent_tool_round` (`:4971-4979`): build
    `routable_tool_names` from `project_tool_names(components)` + `"bash"`.
  - Replace `_replacement_bundle_diagnostics` (`:4984`) with a named binding.
    Emit each diagnostic once per session as
    `ServerEvent::StatusUpdated { message: format!("tool_replacement_bundle_inactive extension={} bundle={} member={} reason={}", ...) }`
    through `review_tx`. De-duplicate with a `HashSet<(String, String)>`
    stored on `ProviderPromptProjectRuntime` as `Arc<Mutex<HashSet<_>>>`.
  - **Failed-bundle members leave the catalog.** After resolution, drop every
    member tool (`member.tool`) of each bundle that produced a diagnostic
    from `routable_tool_names`, then resolve once more. Otherwise a failed
    bundle's members (for example `hashline_read`/`hashline_edit` under
    `minimal`) are advertised standalone, and rig rejects the request
    because they are not in `provider_approved_tools` (`rig_adapter.rs:821-833`).
    Put this in a helper on `ExtensionActivationSnapshot`:
    `pub fn resolve_provider_turn_catalog_dropping_failed_bundles<'a>(&self, policy, executable_tools) -> (ResolvedToolCatalog, Vec<ToolReplacementBundleDiagnostic>)`
    and call it from `run_native_provider_one_agent_tool_round` instead of
    `resolve_provider_turn_catalog`.
- Modify: `crates/yach-backend/src/extension.rs`, background activation
  status: after `activate_background_metadata_extensions` returns in
  `schedule_extension_background_activation`
  (`runner/extension_state.rs:158-216`), call
  `snapshot.resolve_provider_turn_catalog_dropping_failed_bundles` with the
  snapshot's own `turn_permission_policy`-equivalent (all active tools plus
  `project_tool_names(components)` + `bash`). Send one
  `tool_replacement_bundle_inactive ...` `StatusUpdated` per diagnostic,
  before the existing `extension_background_activation_finished` line, so
  users and RPC clients see the inactive bundle without prompting.
  `turn_permission_policy` lives in `runner.rs:4895`; make it `pub(crate)`
  and call it from `extension_state.rs` (a `runner` submodule).
- **Reserve the built-in names regardless of components.** Under `minimal`
  the six project tools are absent from the registry, so
  `ToolRegistry::extension_tool_rejection` (`tools.rs:2108-2132`) no longer
  rejects an extension tool named `read_text_file` as a duplicate. Add a fixed
  reserved list in `tools.rs`:
  `pub const BUILTIN_TOOL_NAMES: [&str; 7] = [<six project tools>, "bash"];`
  and make `extension_tool_rejection` return
  `ToolRegistrationError::DuplicateToolName { name }` for any name in it, even
  when the registry does not currently hold that definition. Replacement stays
  possible only through a declared bundle whose target is active.
- Modify: `crates/yach-backend/src/tools.rs`: add
  `ToolRegistry::for_components(components: ComponentSet) -> Self` beside
  `with_project_read_only_and_agent_edit_tools` (`:2076`).
- Modify: `crates/yach-backend/src/extension.rs`: `ExtensionActivationSnapshot`
  gains `pub fn for_components(components: ComponentSet) -> Self` (same as
  `Default` but with `ToolRegistry::for_components`), and
  `activate_background_metadata_extensions` (`:1309`) takes
  `components: ComponentSet` and starts from it.
- Modify: `crates/yach-backend/src/runner/extension_state.rs`
  (`schedule_extension_background_activation` `:137-165`) and the initial
  snapshot at `runner.rs:1287-1289`: pass `components`.
- Modify: every `RunnerConfig { ... }` literal: add
  `components: yach_backend::ComponentSet::full(),` (backend-internal literals
  use `crate::ComponentSet::full()`). Sites: `crates/yach-cli/src/main.rs`,
  `crates/yach-cli/src/headless.rs:359`, `crates/yach-cli/src/rpc.rs` (via
  `runner_config`), `crates/yach-backend/src/bench_loop.rs:176,410`, and the
  backend test literals in `runner.rs`. Find them with
  `grep -rn 'RunnerConfig {' crates`.
- Test: inline tests in `runner.rs` (near the existing
  `a_granted_network_extension_tool_is_advertised_and_allowed_in_a_turn`,
  `:33214`) and `extension.rs` (near
  `replacement_bundle_resolves_atomically_and_projects_member_contracts`,
  `:3634`).

**Interfaces:**
- Consumes: `ComponentSet` (Task 1).
- Produces:
  - `pub fn project_tool_names(components: ComponentSet) -> &'static [&'static str]`
    in `tools.rs`: the six names when enabled, `&[]` when disabled.
  - `ToolRegistry::for_components(ComponentSet) -> ToolRegistry`.
  - `ExtensionActivationSnapshot::for_components(ComponentSet) -> Self`.
  - `RunnerConfig.components: ComponentSet` (read by Task 4 and set by Task 7).

- [ ] **Step 1: Write the failing tests**

In `extension.rs` tests:

```rust
#[test]
fn replacement_bundle_is_inactive_when_project_tools_are_disabled() -> Result<(), String> {
    let mut snapshot = ExtensionActivationSnapshot::for_components(
        crate::ComponentSet::from_preset(crate::Preset::Minimal),
    );
    assert!(snapshot.registry.get("read_text_file").is_none());
    assert!(snapshot.registry.get("bash").is_some());
    let read_schema = ToolInputSchema::string_object(["path"], std::iter::empty::<&str>(), 4096);
    for (name, risk) in [
        ("hashline_read", ToolRisk::ReadsLocalContent),
        ("hashline_edit", ToolRisk::MutatesLocalState),
    ] {
        snapshot
            .registry
            .register_extension_tool(ToolDefinition::extension_tool_with_version(
                "example.hashline",
                Some("0.1.0"),
                name,
                "hashline tool",
                read_schema.clone(),
                risk,
                ProviderToolVisibility::Visible,
            ))
            .map_err(|error| format!("{error:?}"))?;
    }
    snapshot.replacement_bundles = vec![ActivatedToolReplacementBundle {
        extension_id: String::from("example.hashline"),
        extension_version: String::from("0.1.0"),
        bundle_id: String::from("hashline"),
        source: ToolReplacementSource::User,
        members: vec![
            ExtensionToolReplacementMember {
                builtin: String::from("read_text_file"),
                tool: String::from("hashline_read"),
                contract: ExtensionToolReplacementContract::Preserve,
            },
            ExtensionToolReplacementMember {
                builtin: String::from("edit_text_file"),
                tool: String::from("hashline_edit"),
                contract: ExtensionToolReplacementContract::Replace,
            },
        ],
    }];
    let policy = ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
        std::iter::empty::<&str>(),
        ["hashline_read"],
        ["hashline_edit"],
    );
    let (catalog, diagnostics) = snapshot.resolve_provider_turn_catalog(
        &policy,
        ["bash", "hashline_read", "hashline_edit"],
    );
    expect_equal(&catalog.implementation_name_for_provider_tool("read_text_file"), &None)?;
    expect_equal(&catalog.implementation_name_for_provider_tool("edit_text_file"), &None)?;
    expect_equal(&diagnostics.len(), &1)?;
    expect_equal(&diagnostics[0].member.as_deref(), &Some("read_text_file"))?;

    let (dropped, dropped_diagnostics) = snapshot
        .resolve_provider_turn_catalog_dropping_failed_bundles(
            &policy,
            ["bash", "hashline_read", "hashline_edit"],
        );
    expect_equal(&dropped.implementation_name_for_provider_tool("hashline_read"), &None)?;
    expect_equal(&dropped.implementation_name_for_provider_tool("hashline_edit"), &None)?;
    expect_equal(&dropped_diagnostics.len(), &1)
}

#[test]
fn builtin_names_stay_reserved_when_project_tools_are_disabled() {
    let mut registry = ToolRegistry::for_components(
        crate::ComponentSet::from_preset(crate::Preset::Minimal),
    );
    for name in crate::tools::BUILTIN_TOOL_NAMES {
        let result = registry.register_extension_tool(ToolDefinition::extension_tool_with_version(
            "example.squatter",
            Some("0.1.0"),
            name,
            "squats on a built-in name",
            ToolInputSchema::string_object(["path"], std::iter::empty::<&str>(), 4096),
            ToolRisk::ReadsLocalContent,
            ProviderToolVisibility::Visible,
        ));
        assert!(
            matches!(result, Err(ToolRegistrationError::DuplicateToolName { .. })),
            "{name} must stay reserved"
        );
    }
}
```

The expected member is the first missing builtin because
`resolve_provider_turn_catalog_with_replacements` fails on
`builtin_definition(&rule.builtin_name)?` (`tools.rs:2227`), producing
`ToolResolutionError::MissingBuiltIn { name }` that
`replacement_error_member` (`extension.rs:1087-1090`) reports. If the
permission policy constructor rejects an empty metadata list, use the policy
built by `turn_permission_policy(&snapshot.registry, &["hashline_read", "hashline_edit"])`.

In `runner.rs` tests, following the existing `RecordingProviderRequester`
pattern (`:13420-13440`) used by other one-round tests:

```rust
#[test]
fn minimal_components_advertise_only_bash() {
    let requests = run_recording_agent_round_with_components(
        crate::ComponentSet::from_preset(crate::Preset::Minimal),
    );
    let names = advertised_tool_names(&requests[0]);
    assert_eq!(names, vec![String::from("bash")]);
}

#[test]
fn full_components_advertise_the_seven_builtins() {
    let requests = run_recording_agent_round_with_components(crate::ComponentSet::full());
    let mut names = advertised_tool_names(&requests[0]);
    names.sort();
    assert_eq!(
        names,
        [
            "bash",
            "create_text_file",
            "edit_text_file",
            "list_project_paths",
            "project_path_info",
            "read_text_file",
            "search_project",
        ]
        .map(String::from)
        .to_vec()
    );
}

#[test]
fn approved_tools_follow_components() {
    assert_eq!(
        super::provider_approved_tools(crate::ComponentSet::from_preset(crate::Preset::Minimal)),
        vec![String::from("bash")]
    );
    assert_eq!(super::provider_approved_tools(crate::ComponentSet::full()).len(), 7);
}
```

Write the two helpers in the same test module:
`run_recording_agent_round_with_components(components) -> Vec<ProviderRequest>`
copies the setup of the nearest existing `run_native_provider_one_agent_tool_round`
test (for example the one at `:18847-18883`), sets `components` on the
`ProviderAgentToolRound`, uses a `RecordingProviderRequester` returning a
single text response, and returns the recorded requests.
`advertised_tool_names(&ProviderRequest) -> Vec<String>` parses
`request.approved_tool_advertising` with
`crate::tools::parse_provider_tool_advertising_extensions` and collects tool
names (return an empty vec when absent).

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend -- replacement_bundle_is_inactive_when_project_tools_are_disabled minimal_components_advertise_only_bash full_components_advertise_the_seven_builtins approved_tools_follow_components`
Expected: FAIL to compile (new APIs missing).

- [ ] **Step 3: Implement**

`tools.rs`:

```rust
const PROJECT_TOOL_NAMES: [&str; 6] = [
    "project_path_info",
    "read_text_file",
    "search_project",
    "list_project_paths",
    "edit_text_file",
    "create_text_file",
];

/// Reserved regardless of which components are enabled.
pub const BUILTIN_TOOL_NAMES: [&str; 7] = [
    "project_path_info",
    "read_text_file",
    "search_project",
    "list_project_paths",
    "edit_text_file",
    "create_text_file",
    "bash",
];

#[must_use]
pub fn project_tool_names(components: crate::ComponentSet) -> &'static [&'static str] {
    if components.project_tools() {
        &PROJECT_TOOL_NAMES
    } else {
        &[]
    }
}

impl ToolRegistry {
    #[must_use]
    pub fn for_components(components: crate::ComponentSet) -> Self {
        if components.project_tools() {
            Self::with_project_read_only_and_agent_edit_tools()
        } else {
            Self {
                definitions: vec![ToolDefinition::bash()],
            }
        }
    }
}
```

In `extension_tool_rejection` (`tools.rs:2108-2116`), before the existing
`self.get(&definition.name)` check:

```rust
if BUILTIN_TOOL_NAMES.contains(&definition.name.as_str()) {
    return Some(ToolRegistrationError::DuplicateToolName {
        name: definition.name.clone(),
    });
}
```

`extension.rs`, on `impl ExtensionActivationSnapshot`:

```rust
#[must_use]
pub fn resolve_provider_turn_catalog_dropping_failed_bundles<'a>(
    &self,
    permission_policy: &ToolPermissionPolicy,
    executable_tools: impl IntoIterator<Item = &'a str>,
) -> (ResolvedToolCatalog, Vec<ToolReplacementBundleDiagnostic>) {
    let executable_tools: Vec<&str> = executable_tools.into_iter().collect();
    let (catalog, diagnostics) =
        self.resolve_provider_turn_catalog(permission_policy, executable_tools.iter().copied());
    if diagnostics.is_empty() {
        return (catalog, diagnostics);
    }
    let failed: BTreeSet<(&str, &str)> = diagnostics
        .iter()
        .map(|d| (d.extension_id.as_str(), d.bundle_id.as_str()))
        .collect();
    let dropped: BTreeSet<&str> = self
        .replacement_bundles
        .iter()
        .filter(|b| failed.contains(&(b.extension_id.as_str(), b.bundle_id.as_str())))
        .flat_map(|b| b.members.iter().map(|m| m.tool.as_str()))
        .collect();
    let remaining = executable_tools.into_iter().filter(|name| !dropped.contains(name));
    let (catalog, _) = self.resolve_provider_turn_catalog(permission_policy, remaining);
    (catalog, diagnostics)
}
```

The second pass ignores its own diagnostics: with the failed bundle's members
gone, the same bundle fails again with "not executable", which adds nothing to
the first-pass diagnostics that are returned and surfaced.

`runner.rs`:

```rust
fn provider_approved_tools(components: ComponentSet) -> Vec<String> {
    crate::tools::project_tool_names(components)
        .iter()
        .copied()
        .chain(std::iter::once("bash"))
        .map(String::from)
        .collect()
}
```

Routable names in `run_native_provider_one_agent_tool_round`:

```rust
let mut routable_tool_names: Vec<String> = crate::tools::project_tool_names(components)
    .iter()
    .copied()
    .chain(std::iter::once("bash"))
    .map(String::from)
    .collect();
```

`extension.rs`: `impl Default for ExtensionActivationSnapshot` delegates to
`Self::for_components(crate::ComponentSet::full())`; `for_components` builds
the same struct with `registry: ToolRegistry::for_components(components)`.
`activate_background_metadata_extensions(package_records, config, components, trace)`
replaces `ExtensionActivationSnapshot::default()` at `:1314` with
`ExtensionActivationSnapshot::for_components(components)`; update its callers
(`extension_state.rs:159` and extension tests that call it, passing
`crate::ComponentSet::full()`).

Leave `run_native_provider_one_readonly_tool_round` (`:4638`) unchanged: it
advertises only `project_path_info` and is used by tests and the smoke path,
not by component-aware sessions.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend -- replacement_bundle minimal_components full_components approved_tools_follow_components`
Expected: the four new tests pass, and the existing
`replacement_bundle_resolves_atomically_and_projects_member_contracts` still
passes.

Then run: `just dev cargo test -p yach-backend --lib runner`
Expected: PASS (all literal sites compile with `ComponentSet::full()`).

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend crates/yach-cli/src -m "Route the project-tools component through tool advertising and approval"
```

---

### Task 4: `baseline-guidance` component

**Files:**
- Modify: `crates/yach-backend/src/runner.rs`:
  `provider_messages_from_log_with_static_context` (`:3915-3924`) gains a
  `components: ComponentSet` parameter; the guidance message is included only
  when `components.baseline_guidance()`. Update every caller: `:1971`
  (manual compaction; use the loop's `components`), `:4556`
  (`run_native_provider_one_tool_round_with_registry`; pass
  `ComponentSet::full()` there because it is the legacy one-tool path),
  `:5048`, `:5106`, `:5116`, `:5276`, `:5672` (agent round; pass the round's
  `components`), and the test callers (`:17812`, `:17874`, `:28888`, `:29006`,
  `:29165`, `:32468`, `:32551`) with `crate::ComponentSet::full()`.
- Test: inline in `runner.rs` beside the existing guidance test at `:17790-17830`.

**Interfaces:**
- Consumes: `RunnerConfig.components` threading from Task 3.
- Produces: `fn provider_messages_from_log_with_static_context(log, current_turn_id, context, components: ComponentSet) -> Vec<ProviderMessage>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn baseline_guidance_is_a_component() {
    let (log, turn_id) = single_user_turn_log("hello");
    let with = provider_messages_from_log_with_static_context(
        &log,
        &turn_id,
        &StaticContextBundle::default(),
        crate::ComponentSet::full(),
    );
    assert_eq!(with.len(), 2);
    assert!(with[0].content.contains("coding agent running in the yach harness"));

    let without = provider_messages_from_log_with_static_context(
        &log,
        &turn_id,
        &StaticContextBundle::default(),
        crate::ComponentSet::full().with(crate::Component::BaselineGuidance, false),
    );
    assert_eq!(without.len(), 1);
    assert_eq!(without[0].role, Role::User);
    assert_eq!(without[0].content, "hello");
}
```

`single_user_turn_log` builds the same `SessionLog` the test at `:17790-17800`
builds (one `EntryAppended` user entry); extract that setup into this helper
and use it from both tests.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend baseline_guidance_is_a_component`
Expected: FAIL to compile (arity mismatch).

- [ ] **Step 3: Implement**

```rust
fn provider_messages_from_log_with_static_context(
    log: &SessionLog,
    current_turn_id: &TurnId,
    context: &StaticContextBundle,
    components: ComponentSet,
) -> Vec<ProviderMessage> {
    let mut messages = Vec::new();
    if components.baseline_guidance() {
        messages.push(provider_baseline_guidance_message());
    }
    messages.extend(provider_messages_from_static_context(context));
    messages.extend(provider_messages_from_log(log, current_turn_id));
    messages
}
```

Update the doc comment on `PROVIDER_BASELINE_GUIDANCE` (`:3895-3901`) to add:
"Owned by the `baseline-guidance` component; omitted when that component is
disabled."

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend --lib runner`
Expected: PASS, including the existing guidance test at `:17790`.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/runner.rs -m "Make baseline guidance a component"
```

---

### Task 5: User-scope compaction `summary_prompt`

**Files:**
- Modify: `crates/yach-backend/src/compaction.rs`:
  - `CompactionConfig` (`:37-48`) add `#[serde(skip)] pub summary_prompt: Option<String>`
    (loaded text, never deserialized directly) and a private file struct field.
  - `load_for_project` (`:80-86`): read `summary_prompt` (a path string) only
    from the user file; if the project file contains it, set
    `pub project_summary_prompt_ignored: bool` to true.
  - `build_summary_prompt` (`:909-935`) → `build_summary_prompt(preparation, custom: Option<&str>)`.
- Modify: `crates/yach-backend/src/runner.rs:6504`: pass
  `run.config.summary_prompt.as_deref()`. When
  `run.config.project_summary_prompt_ignored` or the user prompt failed to
  load, send one `StatusUpdated` with
  `compaction_summary_prompt_ignored reason=<project_scope|unreadable|empty>`.
- Test: inline in `compaction.rs` beside the test at `:1401`.

**Interfaces:**
- Produces: `pub fn build_summary_prompt(preparation: &CompactionPreparation, custom: Option<&str>) -> String`;
  `CompactionConfig.summary_prompt: Option<String>`,
  `CompactionConfig.summary_prompt_error: Option<&'static str>`,
  `CompactionConfig.project_summary_prompt_ignored: bool`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn custom_summary_prompt_replaces_preamble_and_keeps_kernel_framing() {
    let preparation = summary_fixture_preparation(Some("prior anchored summary"), Some("keep API names"));
    let prompt = build_summary_prompt(&preparation, Some("Summarize tersely."));
    assert!(prompt.starts_with("Summarize tersely."));
    assert!(!prompt.contains("You are summarizing the earlier part"));
    assert!(prompt.contains("<previous-summary>\nprior anchored summary"));
    assert!(prompt.contains("User focus for this summary"));
    assert!(prompt.contains("<conversation>\n"));
    assert!(prompt.ends_with("\n</conversation>"));
}

#[test]
fn summary_prompt_is_user_scope_only() {
    let root = temp_config_root("summary-scope");
    let prompt_file = root.join("prompt.md");
    fs::write(&prompt_file, "User prompt.").unwrap_or_default();
    write_user_config(&root, &format!(r#"{{"compaction":{{"summary_prompt":"{}"}}}}"#, prompt_file.display()));
    write_project_config(&root, r#"{"compaction":{"summary_prompt":"/tmp/repo-prompt.md","keep_recent_tokens":5}}"#);
    let config = CompactionConfig::load_from_paths(Some(&root.join("user.json")), Some(&root.join("project.json")));
    assert_eq!(config.summary_prompt.as_deref(), Some("User prompt."));
    assert!(config.project_summary_prompt_ignored);
    assert_eq!(config.keep_recent_tokens, 5, "other project values still win");
}

#[test]
fn unreadable_or_empty_summary_prompt_falls_back() {
    let root = temp_config_root("summary-missing");
    write_user_config(&root, r#"{"compaction":{"summary_prompt":"/nonexistent/prompt.md"}}"#);
    let config = CompactionConfig::load_from_paths(Some(&root.join("user.json")), None);
    assert_eq!(config.summary_prompt, None);
    assert_eq!(config.summary_prompt_error, Some("unreadable"));
}
```

`summary_fixture_preparation` extracts the preparation built by the existing
test at `:1390-1400`. `temp_config_root`, `write_user_config`, and
`write_project_config` are small helpers in the test module writing
`user.json` / `project.json` under a temp dir. `load_from_paths` is the
testable core that `load_for_project` calls with `user_config_path()` and the
project path.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend compaction::tests::summary`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Keep precedence as today for every existing field (project file wins over user
file). Parse `summary_prompt` from the raw user JSON only
(`serde_json::Value` lookup `compaction.summary_prompt`), read the file with a
64 KiB cap, trim, and treat empty as `summary_prompt_error = Some("empty")`,
read failure as `Some("unreadable")`. Detect the key in the project JSON the
same way and set `project_summary_prompt_ignored`. Do not add
`summary_prompt` to the serde file struct, so a project value can never reach
`CompactionConfig.summary_prompt`.

`build_summary_prompt`:

```rust
pub fn build_summary_prompt(preparation: &CompactionPreparation, custom: Option<&str>) -> String {
    let mut prompt = match custom {
        Some(custom) => String::from(custom),
        None => {
            let mut prompt = String::from(
                "You are summarizing the earlier part of a coding session so work \
can continue in a smaller context. The conversation below is material to \
summarize, not a conversation to continue. Do not answer it and do not \
mention that you are summarizing.\n\nProduce a summary with exactly these \
sections:\n",
            );
            prompt.push_str(COMPACTION_SUMMARY_SCHEMA);
            prompt
        }
    };
    if let Some(previous_summary) = preparation.previous_summary.as_deref() {
        prompt.push_str("\n\n<previous-summary>\n");
        prompt.push_str(previous_summary);
        prompt.push_str(
            "\n</previous-summary>\n\nTreat the previous summary above as the \
current anchored summary: preserve still-true details, remove stale ones, \
and merge in new facts from the conversation below.",
        );
    }
    if let Some(focus) = preparation.focus_instructions.as_deref() {
        prompt.push_str("\n\nUser focus for this summary (in addition to the fixed sections): ");
        prompt.push_str(focus);
    }
    prompt.push_str("\n\n<conversation>\n");
    prompt.push_str(&preparation.serialized_conversation);
    prompt.push_str("\n</conversation>");
    prompt
}
```

Update the existing call in the test at `:1401` to pass `None`.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend compaction::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/compaction.rs crates/yach-backend/src/runner.rs -m "Add user-scope compaction summary_prompt override"
```

---

### Task 6: Removable bundled records and upgrade refresh

**Files:**
- Modify: `crates/yach-backend/src/extension_install.rs`:
  - `remove` (`:210-229`): drop the `BundledCannotRemove` rejection; delete the
    `BundledCannotRemove` variant (`:34`) and its label at
    `crates/yach-cli/src/main.rs:4486`.
  - Add `pub fn refresh_bundled(&mut self, source: &str, package_root: &Path) -> Result<bool, ExtensionInstallError>`:
    updates `package_root` of an existing `Bundled` record with that source,
    keeps `enabled`, returns `Ok(false)` without creating anything when no
    record exists.
  - Update test `bundled_install_preserves_disabled_state_across_package_updates`
    (`:462-490`): replace the `BundledCannotRemove` assertion with a
    successful removal.
- Create: `crates/yach-cli/src/bundled.rs` (declared `mod bundled;` in
  `main.rs`) holding the materialization moved out of `main.rs:4166-4300`:
  - `pub(crate) struct BundledPackage { pub component: Component, pub source: &'static str, pub dir_name: &'static str, pub host_arg: &'static str, pub manifest_json: fn() -> Option<&'static str> }`
    with `pub(crate) const BUNDLED: [BundledPackage; 2]` (`yach-hashline` /
    `hashline`, `yach-jev-reviewer` / `jev`). `manifest_json` returns `None`
    when the crate is not compiled in (Task 8 adds the cfg; here both return
    `Some`).
  - `pub(crate) fn materialize(home: &Path, package: &BundledPackage) -> io::Result<Option<PathBuf>>`
    (the existing `bundled_*_package_root` body, generalized; `None` when not
    compiled in).
  - `pub(crate) fn materialized_version(package_root: &Path) -> Option<&str>`:
    the last path component.
  - `pub(crate) fn refresh_on_upgrade(home: &Path, store: &mut ExtensionInstallStore, removed: &BTreeSet<String>) -> io::Result<bool>`:
    for each package whose record exists, is not in `removed`, and whose
    `materialized_version` differs from `env!("CARGO_PKG_VERSION")`,
    re-materialize and call `store.refresh_bundled`. Returns whether the store
    changed.
  - `pub(crate) fn install(home: &Path, store: &mut ExtensionInstallStore, package: &BundledPackage) -> io::Result<bool>`:
    materialize and `install_bundled` (used by preset apply and
    `extension install --bundled`).
- Modify: `crates/yach-cli/src/main.rs`:
  - Delete `ensure_bundled_hashline_install_record`,
    `ensure_bundled_jev_install_record`, `bundled_hashline_package_root`,
    `bundled_jev_package_root`, and all six call-site pairs listed below.
  - `installed_extension_records` (`:4745-4761`): replace the two ensure calls
    with one `bundled::refresh_on_upgrade` pass (load store, refresh, save only
    if changed; warn on stderr on error, as today).
  - `run_extension_remove_command` (`:4366-4385`), `run_extension_set_enabled_command`
    (`:4387-4418`), `loaded_extension_package_record` (`:4607-4625`),
    `extension_diagnostics_result` (`:4667-4692`): delete the ensure calls.
  - `run_extension_remove_command`: after a successful removal of a record
    whose `kind == Bundled`, call
    `UserConfigStore::for_current_user()?.persist_bundled_removed(source, true)`.
- Modify: `crates/yach-cli/tests/hashline_extension.rs`
  `bundled_hashline_package_lists_disables_and_reenables_through_cli`
  (`:369-434`): its first assertion (`extension_count=2`) depends on per-read
  seeding, which this task removes. Change that assertion to
  `extension_count=0` with no `yach.hashline` line, and end the test there
  with a `// Task 7 restores the full flow` comment. Task 7 prepends
  `yach preset use full` and restores the disable/doctor/enable steps.
- Test: inline tests in `extension_install.rs` and `bundled.rs`.

**Interfaces:**
- Consumes: `Component` (Task 1), `UserConfigStore::persist_bundled_removed`,
  `UserConfigSnapshot::bundled_removed` (Task 2).
- Produces: `ExtensionInstallStore::refresh_bundled`; module `bundled` with
  `BUNDLED`, `materialize`, `materialized_version`, `refresh_on_upgrade`,
  `install`.

- [ ] **Step 1: Write the failing tests**

`extension_install.rs`:

```rust
#[test]
fn bundled_records_are_removable_and_refresh_never_creates() -> Result<(), String> {
    let root = temp_package_root("bundled-refresh")?;
    let old = root.join("yach-hashline/0.1.0");
    let new = root.join("yach-hashline/0.2.0");
    for dir in [&old, &new] {
        expect_ok(fs::create_dir_all(dir))?;
    }
    let mut store = ExtensionInstallStore::default();
    assert_eq!(store.refresh_bundled("yach.hashline", &new), Ok(false));
    assert!(store.records.is_empty(), "refresh must not create a record");

    expect_ok(store.install_bundled("yach.hashline", &old, ExtensionInstallScope::User))?;
    expect_ok(store.set_enabled("yach.hashline", false))?;
    assert_eq!(store.refresh_bundled("yach.hashline", &new), Ok(true));
    let record = &store.records[0];
    assert!(record.package_root.ends_with("0.2.0"));
    assert!(!record.enabled, "refresh keeps the user's choice");

    expect_ok(store.remove("yach.hashline"))?;
    assert!(store.records.is_empty());
    Ok(())
}
```

`bundled.rs`:

```rust
#[test]
fn upgrade_refresh_repoints_existing_and_skips_removed() {
    let home = temp_home("bundled-upgrade");
    let stale = home.join(".yach/bundled/yach-hashline/0.0.1");
    let stale_jev = home.join(".yach/bundled/yach-jev-reviewer/0.0.1");
    for dir in [&stale, &stale_jev] {
        assert!(std::fs::create_dir_all(dir).is_ok());
    }
    let mut store = ExtensionInstallStore::default();
    assert!(store.install_bundled("yach.hashline", &stale, ExtensionInstallScope::User).is_ok());
    assert!(store.install_bundled("yach.jev-reviewer", &stale_jev, ExtensionInstallScope::User).is_ok());
    let removed = BTreeSet::from([String::from("yach.jev-reviewer")]);

    let changed = refresh_on_upgrade(&home, &mut store, &removed);
    assert!(matches!(changed, Ok(true)));
    let hashline = store.records.iter().find(|r| r.source == "yach.hashline");
    assert!(hashline.is_some_and(|r| materialized_version(&r.package_root) == Some(env!("CARGO_PKG_VERSION"))));
    assert!(hashline.is_some_and(|r| r.package_root.join("yach.extension.json").is_file()));
    let jev = store.records.iter().find(|r| r.source == "yach.jev-reviewer");
    assert!(jev.is_some_and(|r| r.package_root.ends_with("0.0.1")), "removed ids are untouched");

    let unchanged = refresh_on_upgrade(&home, &mut store, &removed);
    assert!(matches!(unchanged, Ok(false)), "second pass is a no-op");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend extension_install::tests && just dev cargo test -p yach bundled::tests`
Expected: FAIL to compile (`refresh_bundled`, `bundled` module missing).

- [ ] **Step 3: Implement**

Move the materialization body from `main.rs:4167-4212` into
`bundled::materialize`, parameterized by `home`, `dir_name`, `host_arg`, and
`manifest_json`. Keep the byte-compare, temp-file, `sync_all`, rename, and
`0700`/`0600` permission handling exactly as today. `refresh_bundled`:

```rust
pub fn refresh_bundled(&mut self, source: &str, package_root: &Path) -> Result<bool, ExtensionInstallError> {
    let Some(record) = self
        .records
        .iter_mut()
        .find(|record| record.source == source && record.kind == ExtensionInstallRefKind::Bundled)
    else {
        return Ok(false);
    };
    let package_root = fs::canonicalize(package_root).map_err(|_| ExtensionInstallError::StoreIo)?;
    if record.package_root == package_root {
        return Ok(false);
    }
    record.package_root = package_root;
    Ok(true)
}
```

Remove the `#[cfg(not(test))]` gating that existed only for the ensure calls.
`bundled.rs` functions take `home: &Path` so tests never read the real `HOME`;
production callers pass the `HOME` directory resolved the same way
`bundled_hashline_package_root` resolved it.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend extension_install::tests && just dev cargo test -p yach bundled::tests && just dev cargo test -p yach --test hashline_extension`
Expected: PASS (the hashline CLI test passes with its temporarily shortened
flow).

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src/extension_install.rs crates/yach-cli/src crates/yach-cli/tests/hashline_extension.rs -m "Make bundled extensions removable; refresh bundled records only on upgrade"
```

---

### Task 7: Preset and component commands, first-run apply, ephemeral `--preset`

**Files:**
- Modify: `crates/yach-cli/src/bundled.rs`: add
  - `pub(crate) struct PresetApplyReport { pub preset: Preset, pub installed: Vec<&'static str>, pub removed: Vec<&'static str>, pub not_compiled_in: Vec<&'static str>, pub preserved: Vec<&'static str> }`.
  - `pub(crate) fn apply_preset(home: &Path, config: &UserConfigStore, store_path: &Path, preset: Preset, reset: bool) -> io::Result<PresetApplyReport>`:
    1. `config.persist_preset(preset, reset)`;
    2. reload the snapshot for `bundled_removed`;
    3. for each `BUNDLED` package: if the preset includes its component and the
       id is not in `bundled_removed`: if no record exists, `install` it (or
       record `not_compiled_in` when `materialize` returns `None`); if a record
       exists, leave `enabled` as is and add it to `preserved`. If the preset
       excludes the component and a record exists, set `enabled = false`
       (never delete; removal is only the explicit `extension remove`) and
       add it to `removed`.
    4. save the store once.
  - `pub(crate) fn ensure_first_run(home: &Path, config: &UserConfigStore, store_path: &Path) -> io::Result<Option<PresetApplyReport>>`:
    when `config.load()?.preset_applied.is_none()`, `apply_preset(.., Preset::Full, false)`.
  - `pub(crate) fn session_components(config: &UserConfigStore, ephemeral: Option<Preset>) -> ComponentSet`:
    `ephemeral.map(ComponentSet::from_preset)` or the snapshot's
    `kernel_components()`; config load failure → `ComponentSet::full()` with a
    stderr warning.
  - `pub(crate) fn ephemeral_extension_filter(roots: Vec<ExtensionPackageRoot>, preset: Preset) -> Vec<ExtensionPackageRoot>`:
    drops roots whose `source_ref` is a bundled id whose component the preset
    excludes. Other roots pass through.
- Modify: `crates/yach-cli/src/main.rs`:
  - `CliArgs::from_args` (`:146-177`): route `preset` and `component`.
  - `Command` (`:187-223`): add
    `PresetList`, `PresetShow`, `PresetUse { preset: Preset, reset: bool }`,
    `ComponentList`, `ComponentSetEnabled { component: Component, enabled: bool }`,
    and `ExtensionInstallBundled { id: String }`.
  - `extension_install_command_from_args` (`:258-270`): `--bundled <id>` produces
    `ExtensionInstallBundled`; the command clears the id from
    `[bundled] removed` and calls `bundled::install`.
  - `Command::run` (`:358-405`): dispatch; `CommandResult` gains
    `Preset { lines: Vec<String>, failed: bool }` rendering
    `preset_action=use`, `preset=<name>`, `installed=`, `disabled=`,
    `preserved=`, `not_compiled_in=`, and for `minimal` the three
    approval-mode notes from the spec. `ComponentSetEnabled` for an extension
    component maps to `run_extension_set_enabled_command` with the bundled id;
    for a kernel component calls `persist_component`.
  - `yach component list` renders one line per component:
    `component name=<n> source=<kernel|bundled-extension> state=<enabled|disabled|removed|reserved> compiled_in=<true|false>`.
  - `usage_lines` (`:854`): add `preset`, `component`.
  - `runner_config` (`:4091-4115`) and `RunnerConfigInput` (`:4080-4089`):
    add `components: ComponentSet` and `ephemeral_preset: Option<Preset>`. The
    loader closure applies `bundled::ephemeral_extension_filter` when
    `ephemeral_preset` is `Some`.
  - Interactive TUI start (the `runner_config` call at `:4003`): call
    `bundled::ensure_first_run` before building the config; print a one-line
    stderr notice when it applied `full`; set
    `components: bundled::session_components(&store, None)`.
- Modify: `crates/yach-cli/src/headless.rs`: `RunOptions` (`:45-62`) add
  `pub preset: Option<Preset>`; `parse_run_args` accepts `--preset <name>`
  (error on unknown name). When `preset` is `Some`, skip `ensure_first_run`
  and never write config. The `RunnerConfig` literal at `:359` sets
  `components: bundled::session_components(&store, options.preset)` and the
  loader filter.
- Modify: `crates/yach-cli/src/rpc.rs`: `RpcOptions` (`:33-43`) add
  `pub preset: Option<Preset>`; `parse_rpc_args` accepts `--preset <name>`;
  `run_rpc` (`:283`) and its `runner_config` call (`:383`) follow the same
  rules as headless. Without `--preset`, call `ensure_first_run`.
- Modify: `crates/yach-cli/tests/hashline_extension.rs`: restore the full flow
  of `bundled_hashline_package_lists_disables_and_reenables_through_cli` with
  `yach preset use full` as the first command (asserting
  `installed=yach.hashline`), then the original list/disable/doctor/enable
  steps.
- Test: parser and command tests inline in `main.rs`, `headless.rs`, `rpc.rs`,
  and `bundled.rs`.

**Interfaces:**
- Consumes: Tasks 1, 2, 6.
- Produces: CLI surface `yach preset list|show|use <minimal|full> [--reset]`,
  `yach component list|enable <name>|disable <name>`,
  `yach extension install --bundled <id>`, `yach run --preset <name>`,
  `yach rpc --preset <name>`; `bundled::{apply_preset, ensure_first_run, session_components, ephemeral_extension_filter}`.

- [ ] **Step 1: Write the failing tests**

`bundled.rs`:

```rust
#[test]
fn first_apply_preserves_existing_disabled_record() {
    let home = temp_home("first-apply");
    let config = UserConfigStore::in_home(&home);
    let store_path = home.join(".yach/extensions.json");
    let mut store = ExtensionInstallStore::default();
    let package = &BUNDLED[0]; // hashline
    assert!(matches!(install(&home, &mut store, package), Ok(true)));
    assert!(store.set_enabled("yach.hashline", false).is_ok());
    assert!(store.save_to_path(&store_path).is_ok());

    let report = ensure_first_run(&home, &config, &store_path);
    assert!(matches!(&report, Ok(Some(r)) if r.preset == Preset::Full && r.preserved.contains(&"yach.hashline")));
    let Ok(store) = ExtensionInstallStore::load_from_path(&store_path) else { unreachable!() };
    let hashline = store.records.iter().find(|r| r.source == "yach.hashline");
    assert!(hashline.is_some_and(|r| !r.enabled), "existing choice preserved");
    assert!(store.records.iter().any(|r| r.source == "yach.jev-reviewer" && r.enabled));
    assert!(matches!(ensure_first_run(&home, &config, &store_path), Ok(None)), "runs once");
}

#[test]
fn preset_use_never_readds_removed_until_reset() {
    let home = temp_home("preset-removed");
    let config = UserConfigStore::in_home(&home);
    let store_path = home.join(".yach/extensions.json");
    assert!(config.persist_bundled_removed("yach.hashline", true).is_ok());

    let report = apply_preset(&home, &config, &store_path, Preset::Full, false);
    assert!(matches!(&report, Ok(r) if !r.installed.contains(&"yach.hashline")));
    let Ok(store) = ExtensionInstallStore::load_from_path(&store_path) else { unreachable!() };
    assert!(store.records.iter().all(|r| r.source != "yach.hashline"));

    let reset = apply_preset(&home, &config, &store_path, Preset::Full, true);
    assert!(matches!(&reset, Ok(r) if r.installed.contains(&"yach.hashline")));
}

#[test]
fn ephemeral_filter_drops_excluded_bundled_roots_only() {
    let roots = vec![
        root_with_source("yach.hashline"),
        root_with_source("yach.jev-reviewer"),
        root_with_source("example.third-party"),
    ];
    let kept = ephemeral_extension_filter(roots, Preset::Minimal);
    let sources: Vec<_> = kept.iter().filter_map(|r| r.source_ref.as_deref()).collect();
    assert_eq!(sources, vec!["example.third-party"]);
}
```

`main.rs` parser tests (append to the existing parser test group):

```rust
#[test]
fn preset_and_component_commands_parse() {
    assert_eq!(
        parse(["yach", "preset", "use", "minimal", "--reset"]),
        Command::PresetUse { preset: Preset::Minimal, reset: true }
    );
    assert_eq!(
        parse(["yach", "component", "disable", "project-tools"]),
        Command::ComponentSetEnabled { component: Component::ProjectTools, enabled: false }
    );
    assert_eq!(
        parse(["yach", "extension", "install", "--bundled", "yach.hashline"]),
        Command::ExtensionInstallBundled { id: String::from("yach.hashline") }
    );
    assert!(matches!(
        parse(["yach", "preset", "use", "profile"]),
        Command::UsageError { .. }
    ));
}
```

Use the parser test helper that already exists in the module (the one other
parser tests call to turn argv into a `Command`); name it `parse` locally if it
has a different name.

`headless.rs` / `rpc.rs`:

```rust
#[test]
fn run_accepts_preset() {
    let options = parse_run_args(&args(["--preset", "minimal", "--prompt", "hi"]));
    assert!(matches!(options, Ok(o) if o.preset == Some(Preset::Minimal)));
    assert!(parse_run_args(&args(["--preset", "tiny", "--prompt", "hi"])).is_err());
}

#[test]
fn rpc_accepts_preset() {
    let options = parse_rpc_args(&args(["--preset", "full"]));
    assert!(matches!(options, Ok(o) if o.preset == Some(Preset::Full)));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach -- bundled::tests preset_and_component_commands_parse run_accepts_preset rpc_accepts_preset`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Implement per the Files list. Two rules the implementation must keep:
- `session_components` never writes config.
- `ephemeral_preset` affects only the running session: it filters the loader's
  roots and sets `components`; `ensure_first_run` is not called and neither
  store is written.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach -- bundled::tests preset_and_component run_accepts_preset rpc_accepts_preset && just dev cargo test -p yach --test hashline_extension`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-cli -m "Add preset and component commands, first-run full, and ephemeral --preset"
```

---

### Task 8: `bundled-hashline` and `bundled-jev` cargo features

**Files:**
- Modify: `crates/yach-cli/Cargo.toml`:

```toml
[features]
default = ["bundled-hashline", "bundled-jev"]
bench = ["yach-backend/bench"]
bundled-hashline = ["dep:yach-hashline-extension"]
bundled-jev = ["dep:yach-jev-reviewer"]

[dependencies]
yach-hashline-extension = { version = "0.1.0", path = "../yach-hashline-extension", optional = true }
yach-jev-reviewer = { version = "0.1.0", path = "../yach-jev-reviewer", optional = true }
```

- Modify: `crates/yach-cli/src/main.rs:64-80`: gate each `__extension-host`
  branch with `#[cfg(feature = "bundled-hashline")]` /
  `#[cfg(feature = "bundled-jev")]`. When not compiled in, the dispatch arm
  prints `error=bundled extension not compiled in` to stderr and exits 1.
- Modify: `crates/yach-cli/src/bundled.rs`: `manifest_json` functions return
  `Some(yach_hashline_extension::MANIFEST_JSON)` under the feature, `None`
  otherwise (same for Jev).
- Modify: `crates/yach-cli/tests/hashline_extension.rs:146`: gate the test that
  reads `yach_hashline_extension::MANIFEST_JSON` with
  `#[cfg(feature = "bundled-hashline")]`, and gate
  `bundled_hashline_package_lists_disables_and_reenables_through_cli` the same
  way.
- Test: `bundled.rs` inline test compiled only without the feature, plus a
  build check.

**Interfaces:**
- Consumes: `bundled::BUNDLED`, `materialize` (Task 6), `PresetApplyReport`
  (Task 7).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(not(feature = "bundled-hashline"))]
#[test]
fn full_preset_reports_hashline_not_compiled_in() {
    let home = temp_home("not-compiled");
    let config = UserConfigStore::in_home(&home);
    let store_path = home.join(".yach/extensions.json");
    let report = apply_preset(&home, &config, &store_path, Preset::Full, false);
    assert!(matches!(&report, Ok(r) if r.not_compiled_in.contains(&"yach.hashline")));
    let Ok(snapshot) = config.load() else { unreachable!() };
    assert_eq!(snapshot.preset_applied, Some(Preset::Full), "preference recorded");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach --no-default-features --features bundled-jev bundled::tests::full_preset_reports_hashline_not_compiled_in`
Expected: FAIL (compile error from unconditional `yach_hashline_extension`
references, or the test fails because the report lists it as installed).

- [ ] **Step 3: Implement** per the Files list.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach --no-default-features --features bundled-jev bundled::tests`
Expected: PASS.

Run: `just dev cargo build -p yach --no-default-features`
Expected: builds. Then check that neither extension crate is linked:
`just dev cargo tree -p yach --no-default-features -e normal --prefix none | grep -E 'yach-(hashline-extension|jev-reviewer)'`
Expected: no output.

Run: `just dev cargo test -p yach bundled::tests`
Expected: PASS with default features.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-cli -m "Gate bundled extension crates behind default-on cargo features"
```

---

### Task 9: Acceptance scenarios over RPC and the CLI

**Files:**
- Create: `crates/yach-cli/tests/presets.rs`.
- Reuse (copy into the new file; integration test files do not share modules
  in this crate): from `crates/yach-cli/tests/rpc_review.rs`, `TestUnwrap`
  (`:19-42`), `TempDir` (`:964-989`), `RpcChild` (`:824-962`, spawn at `:833`),
  the connection plus model-activation sequence (`:60-143`), and the HTTP
  helpers `read_http_request` (`:777-813`) and `write_http_response`
  (`:815-822`). Add an optional `extra_args: &[&str]` parameter to
  `RpcChild::spawn` for `--preset`.
- Add a `CapturingOpenAiProvider` modeled on `MockOpenAiProvider`
  (`rpc_review.rs:400-484`) that answers `GET /models` like the original,
  answers each `POST /chat/completions` with a single text completion
  (`follow_up_sse` shape, `:496-503`), and records every POST body in
  `Arc<Mutex<Vec<serde_json::Value>>>`. Helpers:
  `advertised_tools(&Value) -> Vec<String>` (from `body["tools"][*]["function"]["name"]`)
  and `system_texts(&Value) -> Vec<String>` (from `body["messages"]` where
  `role == "system"`).

**Interfaces:**
- Consumes: the CLI behavior from Tasks 3-8.

Scenarios (one `#[test]` each; each gets its own `TempDir` project and home):

1. `full_first_run_matches_todays_request`: no config, no `--preset`. Prompt
   once. Assert the seven built-in tools are advertised (hashline activation
   is post-first-paint and may or may not have completed; assert the six
   project tool names as provider names are present, which holds either way
   because hashline replaces under the same names) and one system text contains
   `coding agent running in the yach harness`. Assert `HOME/.yach/config.toml`
   contains `applied = "full"`.
2. `minimal_preset_advertises_bash_and_no_guidance`: `yach preset use minimal`
   (as a CLI child with the same `HOME`), then RPC. Assert tools ==
   `["bash"]` and no system text contains `yach harness`.
3. `project_tools_disable_is_atomic`: `yach component disable project-tools`,
   prompt, assert tools == `["bash"]`; `yach component enable project-tools`,
   new session, assert all six return.
4. `minimal_plus_hashline_bundle_is_inactive_with_diagnostic`:
   `yach preset use minimal`, `yach extension install --bundled yach.hashline`,
   RPC with `FirstRenderCompleted`, wait for
   `extension_background_activation_finished`, prompt. Assert tools do not
   contain `read_text_file`, `edit_text_file`, `hashline_read`, or
   `hashline_edit`; the prompt completes (rig did not reject the request); and
   a `StatusUpdated` message starts with
   `tool_replacement_bundle_inactive extension=yach.hashline`, arriving before
   the prompt is sent.
5. Undeclared collision fails closed: covered by Task 3's
   `builtin_names_stay_reserved_when_project_tools_are_disabled` (the
   `minimal` case) and the existing manifest/registration tests that reject
   `project_path_info` and duplicate names (`extension.rs:3741-3748`). No RPC
   scenario.
6. `removed_bundled_extension_stays_removed`: `yach preset use full`,
   `yach extension remove yach.hashline`, start RPC twice, then
   `yach preset use full`. Assert `yach extension list` never shows
   `yach.hashline` and `config.toml` lists it under `[bundled] removed`.
7. `ephemeral_preset_writes_nothing`: snapshot `config.toml` and
   `extensions.json` bytes, run `yach rpc --preset minimal`, prompt, assert
   tools == `["bash"]`, exit, assert both files are byte-identical to the
   snapshot (or still absent).
8. `preset_changes_no_authority`: set approval mode via RPC
   `ApprovalModeChangeRequested` to `accept-edits` with persistence (reuse the
   flow in `rpc_matrix.rs:786-835`), run `yach preset use minimal`, restart,
   assert the persisted approval mode is unchanged. Assert
   `~/.yach/config.json` (allowlists) is byte-identical before and after.
9. Compaction summary prompt: covered by Task 5 unit tests; no RPC scenario.
10. `first_apply_preserves_disabled_hashline`: pre-create a user
    `extensions.json` with a disabled `yach.hashline` bundled record (use
    `yach preset use full` then `yach extension disable yach.hashline`, then
    delete the `[preset]` table from `config.toml` to simulate a pre-feature
    install). Start RPC. Assert `config.toml` has `applied = "full"` and
    `yach extension doctor yach.hashline` still reports `last_error_kind=disabled`.
11. `upgrade_refresh_repoints_bundled_record`: install hashline via preset,
    rewrite its `package_root` in `extensions.json` to a
    `.../yach-hashline/0.0.1` directory, run `yach extension list`, assert the
    record's `package_root` ends with the current version and `enabled` is
    unchanged.
12. `not_compiled_in_is_reported`: covered by Task 8's
    `--no-default-features` test; no default-feature RPC scenario.
13. Build check: covered by Task 8.

- [ ] **Step 1: Write the scenarios** above in `presets.rs`.

- [ ] **Step 2: Run to verify**

Run: `just dev cargo test -p yach --test presets`
Expected: all scenarios pass. If one fails, the defect is in the task that
owns that behavior (see the scenario's number against the spec's acceptance
list); fix it there and commit the fix with that task's files.

- [ ] **Step 3: Commit**

```bash
jj commit crates/yach-cli/tests/presets.rs -m "Add distribution preset acceptance scenarios"
```

---

### Task 10: Documentation

**Files:**
- Create: `docs/presets.md`: what presets and components are; the component
  table from the spec; `yach preset` / `yach component` /
  `yach extension install --bundled` / `--preset` usage; what `minimal` means
  for approval modes (the three bullets from the spec's "What `minimal` means"
  section, verbatim in meaning); removal and `--reset`; the
  `compaction.summary_prompt` user-scope rule; `cargo install yach
  --no-default-features`.
- Modify: `README.md`: in the Install section add one paragraph linking
  `docs/presets.md` and naming `--no-default-features`; in the config file
  list (`README.md:150-160`) add the `[preset]`, `[components]`, `[bundled]`
  tables of `~/.yach/config.toml`.
- Modify: `docs/extensions.md`: in the section that mentions the bundled
  `yach.hashline` package (`:52`), state that bundled extensions are
  removable, removal is remembered, and link `docs/presets.md`.
- Modify: `docs/README.md`: add `presets.md` to the index.

- [ ] **Step 1: Write the docs.**
- [ ] **Step 2: Check commands in the docs against `yach preset --help`
  output and the usage lines from Task 7.**
- [ ] **Step 3: Commit**

```bash
jj commit README.md docs/presets.md docs/extensions.md docs/README.md -m "Document distribution presets and components"
```

---

## Final verification

- [ ] `just fmt`
- [ ] `just lint`
- [ ] `just dev cargo test -p yach-backend -p yach`
- [ ] `just dev cargo test -p yach --no-default-features --features bundled-jev bundled::tests`
- [ ] `just dev cargo build -p yach --no-default-features`
- [ ] Manual smoke with a throwaway `HOME`:
  `HOME=$(mktemp -d) yach preset use minimal && HOME=... yach component list`
  shows `project-tools` disabled, `skill-index` reserved.
- [ ] Perf note for the PR body: the deterministic perf gate builds default
  features, so `binary/size_bytes` should be unchanged; the in-process
  `turn/scripted/*#alloc_bytes` rows may move by the size of the added
  `ComponentSet` field and the diagnostics de-duplication set. If they move,
  budget the measured value with a comment, per `perf-thresholds.toml`.

## Spec coverage

| Spec acceptance | Task |
|---|---|
| 1. `full` same as today | 3, 4, 9.1 |
| 2. `minimal` bash only, no guidance | 3, 4, 9.2 |
| 3. `project-tools` disable atomic | 3, 9.3 |
| 4. `minimal` + hashline inactive with diagnostic | 3, 9.4 |
| 5. Undeclared collision fails closed | 3 (reservation test), existing tests |
| 6. Removal persists | 6, 7, 9.6 |
| 7. Ephemeral preset writes nothing | 7, 9.7 |
| 8. Preset changes no authority | 7, 9.8 |
| 9. User-scope summary prompt | 5 |
| 10. First apply preserves choices | 7, 9.10 |
| 11. Upgrade refresh | 6, 9.11 |
| 12. Not compiled in reported | 8 |
| 13. Core build links neither crate | 8 |
