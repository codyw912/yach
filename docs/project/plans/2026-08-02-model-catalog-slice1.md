# Model Catalog Slice 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `yach-catalog` crate with baked models.dev-derived data, override files, per-field provenance, and the five stopgap consumers rewired — cost and provenance landing in the outcome document.

**Architecture:** `yach-catalog` is a leaf crate consumed by `yach-cli` ONLY in this slice: the CLI resolves a `ModelProfile` at config-construction time and feeds existing backend structs (`RigProviderAdapterConfig`, a new optional catalog-models list for the picker). The backend keeps its own types; no backend→catalog dependency. Spec: `docs/project/specs/2026-08-02-model-catalog-hydration-design.md`.

**Tech Stack:** Rust workspace (edition 2024, publishes to crates.io); jj (not raw git); `just dev <cmd>` wraps the nix dev shell; serde/serde_json + toml for data.

## Global Constraints

- Run every cargo command as `just dev cargo <...>` from /Users/cody/dev/yach.
- Strict clippy: `-D warnings`, `panic!` banned even in tests (use `assert!`/`unreachable!`), `#[expect]` over `#[allow]`, cognitive complexity max 15, 100-line functions max, max 3 bools per struct.
- `just dev cargo fmt` after edits; `--check` clean before every commit.
- Exact-match edits only (no perl / multi-line sed).
- `jj commit -m "..."` per task; no AI attribution; `jj st` lists only intended files before each commit.
- New-crate metadata matches siblings exactly: `version = "0.1.0"`, `edition = "2024"`, `license = "MIT"`, `repository = "https://github.com/codyw912/yach"`, a one-line `description`, `[lints] workspace = true`.
- Behavior floor: with no catalog data for a model and no env overrides, every number must equal today's values (context 200_000, output budget 32_000, spelling `max_tokens`). No regression for unknown models — enforced by test.
- Offline invariant: nothing in this slice performs network I/O at runtime. The snapshot generator is a build-time tool run by a `just` recipe.
- Env semantics are unchanged for anyone setting the vars today; what changes is their role (override, not sole source).

---

### Task 1: `yach-catalog` crate — types and per-field resolution

**Files:**
- Create: `crates/yach-catalog/Cargo.toml`, `crates/yach-catalog/src/lib.rs`
- Modify: `Cargo.toml` (workspace members: add `"crates/yach-catalog"` in alphabetical order)

**Interfaces (produced; Tasks 2–5 consume):**
```rust
pub enum CatalogSource { Baked { snapshot_date: String }, Override { scope: OverrideScope }, EnvOverride, Default }
pub enum OverrideScope { User, Project }
pub struct Sourced<T> { pub value: T, pub source: CatalogSource }
pub enum OutputTokensParam { MaxTokens, MaxCompletionTokens }
pub struct CostRates { pub input: f64, pub output: f64, pub cache_read: Option<f64>, pub cache_write: Option<f64> } // per million tokens
pub struct ModelProfile {
    pub context_window: Sourced<u64>,
    pub output_ceiling: Sourced<u64>,
    pub output_tokens_param: Sourced<OutputTokensParam>,
    pub display_name: Sourced<String>,
    pub cost: Option<Sourced<CostRates>>,
}
pub struct CatalogEntry { /* one model's optional fields, all Option<_> */ }
pub struct Catalog { /* snapshot_date + provider -> model -> CatalogEntry */ }
pub struct Overrides { /* same shape, from one models.toml */ }
pub struct EnvOverrides { pub context_window: Option<u64>, pub max_tokens: Option<u64>, pub output_tokens_param: Option<OutputTokensParam> }
pub fn resolve(provider: &str, model: &str, baked: &Catalog, user: Option<&Overrides>, project: Option<&Overrides>, env: &EnvOverrides) -> ModelProfile;
pub fn effective_output_budget(profile: &ModelProfile, env_max_tokens: Option<u64>) -> Sourced<u64>;
pub fn baked_catalog() -> &'static Catalog; // parses the embedded data once (OnceLock)
```
(Field-by-field: exact struct definitions below.)

- [ ] **Step 1: Crate skeleton**

`crates/yach-catalog/Cargo.toml`:

```toml
[package]
name = "yach-catalog"
version = "0.1.0"
edition = "2024"
license = "MIT"
repository = "https://github.com/codyw912/yach"
description = "Layered model-metadata catalog for yach"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"

[lints]
workspace = true
```

Add `"crates/yach-catalog"` to the workspace `members` list (alphabetical: after `yach-bench`, before `yach-cli`).

- [ ] **Step 2: Write the failing resolution tests first**

In `crates/yach-catalog/src/lib.rs`, start with the test module (the types don't exist yet — this is the RED step):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn baked_with(provider: &str, model: &str, entry: CatalogEntry) -> Catalog {
        let mut catalog = Catalog::empty("2026-08-02");
        catalog.insert(provider, model, entry);
        catalog
    }

    #[test]
    fn unknown_model_resolves_to_the_behavior_floor() {
        let catalog = Catalog::empty("2026-08-02");
        let profile = resolve("openai-compatible", "mystery-model", &catalog, None, None, &EnvOverrides::default());
        assert_eq!(profile.context_window.value, 200_000);
        assert!(matches!(profile.context_window.source, CatalogSource::Default));
        assert_eq!(profile.output_ceiling.value, 32_000);
        assert!(matches!(profile.output_ceiling.source, CatalogSource::Default));
        assert!(matches!(profile.output_tokens_param.value, OutputTokensParam::MaxTokens));
        assert_eq!(profile.display_name.value, "mystery-model");
        assert!(profile.cost.is_none());
    }

    #[test]
    fn baked_data_overrides_the_floor_and_carries_snapshot_provenance() {
        let catalog = baked_with("anthropic", "claude-haiku-4-5", CatalogEntry {
            context_window: Some(200_000),
            output_ceiling: Some(64_000),
            display_name: Some(String::from("Claude Haiku 4.5")),
            cost: Some(CostRates { input: 1.0, output: 5.0, cache_read: Some(0.1), cache_write: Some(1.25) }),
            output_tokens_param: None,
        });
        let profile = resolve("anthropic", "claude-haiku-4-5", &catalog, None, None, &EnvOverrides::default());
        assert_eq!(profile.output_ceiling.value, 64_000);
        assert!(matches!(&profile.output_ceiling.source, CatalogSource::Baked { snapshot_date } if snapshot_date == "2026-08-02"));
        assert!(profile.cost.is_some());
    }

    #[test]
    fn env_beats_override_beats_baked_per_field() {
        let catalog = baked_with("anthropic", "m", CatalogEntry { context_window: Some(100_000), output_ceiling: Some(50_000), ..CatalogEntry::default() });
        let project = Overrides::from_toml_str("[anthropic.m]\ncontext_window = 150000\n").unwrap();
        let env = EnvOverrides { context_window: Some(180_000), ..EnvOverrides::default() };
        let profile = resolve("anthropic", "m", &catalog, None, Some(&project), &env);
        // env wins for the field it sets…
        assert_eq!(profile.context_window.value, 180_000);
        assert!(matches!(profile.context_window.source, CatalogSource::EnvOverride));
        // …and does NOT shadow other fields: baked still supplies the ceiling.
        assert_eq!(profile.output_ceiling.value, 50_000);
        assert!(matches!(profile.output_ceiling.source, CatalogSource::Baked { .. }));
    }

    #[test]
    fn project_override_beats_user_override() {
        let catalog = Catalog::empty("2026-08-02");
        let user = Overrides::from_toml_str("[p.m]\ncontext_window = 111\n").unwrap();
        let project = Overrides::from_toml_str("[p.m]\ncontext_window = 222\n").unwrap();
        let profile = resolve("p", "m", &catalog, Some(&user), Some(&project), &EnvOverrides::default());
        assert_eq!(profile.context_window.value, 222);
        assert!(matches!(profile.context_window.source, CatalogSource::Override { scope: OverrideScope::Project }));
    }

    #[test]
    fn effective_budget_is_min_of_ceiling_and_cohort_default_unless_env_set() {
        let mut profile_small = resolve("p", "m", &Catalog::empty("d"), None, None, &EnvOverrides::default());
        profile_small.output_ceiling = Sourced { value: 8_192, source: CatalogSource::Baked { snapshot_date: String::from("d") } };
        assert_eq!(effective_output_budget(&profile_small, None).value, 8_192);
        let profile_big = resolve("p", "m", &Catalog::empty("d"), None, None, &EnvOverrides::default());
        assert_eq!(effective_output_budget(&profile_big, None).value, 32_000);
        assert_eq!(effective_output_budget(&profile_big, Some(64_000)).value, 64_000);
        assert!(matches!(effective_output_budget(&profile_big, Some(64_000)).source, CatalogSource::EnvOverride));
    }

    #[test]
    fn catalog_json_tolerates_unknown_fields() {
        let json = r#"{ "snapshot_date": "2026-08-02", "providers": { "anthropic": { "models": { "m": { "context_window": 10, "future_field": {"x": 1} } } } } }"#;
        let catalog = Catalog::from_json_str(json).unwrap();
        assert_eq!(catalog.entry("anthropic", "m").unwrap().context_window, Some(10));
    }
}
```

- [ ] **Step 3: Run to verify RED**

Run: `just dev cargo test -p yach-catalog`
Expected: compile failure (types not defined).

- [ ] **Step 4: Implement the types and resolution**

Implement in the same file (single-responsibility crate; split into `types.rs`/`resolve.rs` modules only if lib.rs passes ~400 lines):

```rust
//! Layered model-metadata catalog: baked snapshot -> user/project
//! overrides -> env overrides, resolved per FIELD with provenance.
//! Design: docs/project/specs/2026-08-02-model-catalog-hydration-design.md
//! The schema tolerates unknown fields so future layers (fetched,
//! discovered, quirks) are additive.

use std::collections::BTreeMap;
use serde::Deserialize;

pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
pub const DEFAULT_OUTPUT_BUDGET: u64 = 32_000;

#[derive(Debug, Clone, PartialEq)]
pub enum CatalogSource {
    Baked { snapshot_date: String },
    Override { scope: OverrideScope },
    EnvOverride,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideScope { User, Project }

#[derive(Debug, Clone, PartialEq)]
pub struct Sourced<T> { pub value: T, pub source: CatalogSource }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputTokensParam { MaxTokens, MaxCompletionTokens }

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CostRates {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CatalogEntry {
    pub context_window: Option<u64>,
    pub output_ceiling: Option<u64>,
    pub output_tokens_param: Option<OutputTokensParam>,
    pub display_name: Option<String>,
    pub cost: Option<CostRates>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderEntry {
    #[serde(default)]
    models: BTreeMap<String, CatalogEntry>,
    /// Per-provider error dialect id (the spec-carved home for the
    /// tiered-classifier item). Data optional; no consumer in this slice.
    #[serde(default)]
    error_dialect: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    snapshot_date: String,
    #[serde(default)]
    providers: BTreeMap<String, ProviderEntry>,
}

impl Catalog {
    #[must_use]
    pub fn empty(snapshot_date: &str) -> Self {
        Self { snapshot_date: String::from(snapshot_date), providers: BTreeMap::new() }
    }
    pub fn insert(&mut self, provider: &str, model: &str, entry: CatalogEntry) {
        self.providers.entry(String::from(provider)).or_insert_with(|| ProviderEntry { models: BTreeMap::new() })
            .models.insert(String::from(model), entry);
    }
    #[must_use]
    pub fn entry(&self, provider: &str, model: &str) -> Option<&CatalogEntry> {
        self.providers.get(provider).and_then(|p| p.models.get(model))
    }
    /// Fallback for aggregator shapes: find the model id under any baked
    /// provider. First match in provider name order; provenance still
    /// records the snapshot. Used when the configured provider has no
    /// entry (openai-compatible aggregators serve other vendors' models).
    #[must_use]
    pub fn entry_by_model_id(&self, model: &str) -> Option<&CatalogEntry> {
        self.providers.values().find_map(|p| p.models.get(model))
    }
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
    #[must_use]
    pub fn snapshot_date(&self) -> &str { &self.snapshot_date }
    /// The provider's error-dialect id, when the data carries one.
    /// Spec-carved home; the classifier design is a separate item.
    #[must_use]
    pub fn provider_error_dialect(&self, provider: &str) -> Option<&str> {
        self.providers.get(provider).and_then(|p| p.error_dialect.as_deref())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Overrides {
    #[serde(flatten)]
    providers: BTreeMap<String, BTreeMap<String, CatalogEntry>>,
}

impl Overrides {
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
    #[must_use]
    pub fn entry(&self, provider: &str, model: &str) -> Option<&CatalogEntry> {
        self.providers.get(provider).and_then(|p| p.get(model))
    }
}

#[derive(Debug, Clone, Default)]
pub struct EnvOverrides {
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub output_tokens_param: Option<OutputTokensParam>,
}

#[must_use]
pub fn resolve(
    provider: &str,
    model: &str,
    baked: &Catalog,
    user: Option<&Overrides>,
    project: Option<&Overrides>,
    env: &EnvOverrides,
) -> ModelProfile {
    let baked_entry = baked.entry(provider, model).or_else(|| baked.entry_by_model_id(model));
    let user_entry = user.and_then(|o| o.entry(provider, model));
    let project_entry = project.and_then(|o| o.entry(provider, model));
    let snapshot = baked.snapshot_date().to_owned();

    // Per-field: env > project > user > baked > default.
    let field = |env_value: Option<u64>, pick: fn(&CatalogEntry) -> Option<u64>, default: u64| -> Sourced<u64> {
        if let Some(value) = env_value {
            return Sourced { value, source: CatalogSource::EnvOverride };
        }
        if let Some(value) = project_entry.and_then(pick) {
            return Sourced { value, source: CatalogSource::Override { scope: OverrideScope::Project } };
        }
        if let Some(value) = user_entry.and_then(pick) {
            return Sourced { value, source: CatalogSource::Override { scope: OverrideScope::User } };
        }
        if let Some(value) = baked_entry.and_then(pick) {
            return Sourced { value, source: CatalogSource::Baked { snapshot_date: snapshot.clone() } };
        }
        Sourced { value: default, source: CatalogSource::Default }
    };

    let context_window = field(env.context_window, |e| e.context_window, DEFAULT_CONTEXT_WINDOW);
    let output_ceiling = field(None, |e| e.output_ceiling, DEFAULT_OUTPUT_BUDGET);
    // (env max_tokens applies at effective_output_budget, not to the ceiling)

    let output_tokens_param = if let Some(value) = env.output_tokens_param {
        Sourced { value, source: CatalogSource::EnvOverride }
    } else if let Some(value) = project_entry.and_then(|e| e.output_tokens_param) {
        Sourced { value, source: CatalogSource::Override { scope: OverrideScope::Project } }
    } else if let Some(value) = user_entry.and_then(|e| e.output_tokens_param) {
        Sourced { value, source: CatalogSource::Override { scope: OverrideScope::User } }
    } else if let Some(value) = baked_entry.and_then(|e| e.output_tokens_param) {
        Sourced { value, source: CatalogSource::Baked { snapshot_date: snapshot.clone() } }
    } else {
        Sourced { value: OutputTokensParam::MaxTokens, source: CatalogSource::Default }
    };

    let display_name = [
        (project_entry, CatalogSource::Override { scope: OverrideScope::Project }),
        (user_entry, CatalogSource::Override { scope: OverrideScope::User }),
        (baked_entry, CatalogSource::Baked { snapshot_date: snapshot.clone() }),
    ]
    .into_iter()
    .find_map(|(entry, source)| entry.and_then(|e| e.display_name.clone()).map(|value| Sourced { value, source }))
    .unwrap_or(Sourced { value: String::from(model), source: CatalogSource::Default });

    let cost = [
        (project_entry, CatalogSource::Override { scope: OverrideScope::Project }),
        (user_entry, CatalogSource::Override { scope: OverrideScope::User }),
        (baked_entry, CatalogSource::Baked { snapshot_date: snapshot }),
    ]
    .into_iter()
    .find_map(|(entry, source)| entry.and_then(|e| e.cost.clone()).map(|value| Sourced { value, source }));

    ModelProfile { context_window, output_ceiling, output_tokens_param, display_name, cost }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelProfile {
    pub context_window: Sourced<u64>,
    pub output_ceiling: Sourced<u64>,
    pub output_tokens_param: Sourced<OutputTokensParam>,
    pub display_name: Sourced<String>,
    pub cost: Option<Sourced<CostRates>>,
}

/// The per-turn output budget: an explicit env value wins verbatim;
/// otherwise min(model ceiling, cohort default 32k) — preserving current
/// behavior for large models and fixing models whose ceiling is below it.
#[must_use]
pub fn effective_output_budget(profile: &ModelProfile, env_max_tokens: Option<u64>) -> Sourced<u64> {
    if let Some(value) = env_max_tokens {
        return Sourced { value, source: CatalogSource::EnvOverride };
    }
    let value = profile.output_ceiling.value.min(DEFAULT_OUTPUT_BUDGET);
    Sourced { value, source: profile.output_ceiling.source.clone() }
}
```

(If the closure-with-fn-pointer `field` helper fights the borrow checker over `snapshot.clone()`, lower it to a small private function taking the three entries and snapshot as arguments — keep the precedence order identical. If clippy's cognitive-complexity lint objects to `resolve`, split the display-name/cost pickers into private helpers.)

- [ ] **Step 5: GREEN + gate**

Run: `just dev cargo test -p yach-catalog` (all pass), `just dev cargo fmt -p yach-catalog && just dev cargo fmt --check -p yach-catalog`, `just dev cargo clippy -p yach-catalog --all-targets`, and `just dev cargo clippy --workspace --all-targets` (member addition compiles everywhere).

- [ ] **Step 6: Commit**

```bash
jj commit -m "feat: yach-catalog crate — layered per-field model-metadata resolution"
```

---

### Task 2: Baked snapshot — generator binary, committed data, `baked_catalog()`

**Files:**
- Create: `crates/yach-catalog/src/bin/snapshot.rs`, `crates/yach-catalog/data/catalog.json` (generated, committed)
- Modify: `crates/yach-catalog/src/lib.rs` (`baked_catalog()`), `crates/yach-catalog/Cargo.toml` (bin needs no extra deps; generator reads a DOWNLOADED file — no network in the binary either), `justfile` (recipe)

**Interfaces:**
- Consumes: `Catalog::from_json_str` (Task 1).
- Produces: `pub fn baked_catalog() -> &'static Catalog` (Tasks 3–5 consume); `just catalog-snapshot` recipe.

- [ ] **Step 1: The generator**

`crates/yach-catalog/src/bin/snapshot.rs` — reads a models.dev `api.json` from a PATH ARGUMENT (the recipe downloads it; the binary itself does no network I/O), filters to an allowlist, emits the catalog schema:

```rust
//! Generate the baked catalog from a downloaded models.dev api.json.
//! Usage: snapshot <api.json path> <output path> <snapshot-date>
//! Providers are allowlisted to what yach can drive; the schema is the
//! catalog's own, not models.dev's, so upstream drift breaks THIS tool
//! loudly instead of the runtime quietly.

use std::collections::BTreeMap;

const PROVIDER_ALLOWLIST: &[&str] = &["anthropic", "openai", "alibaba", "deepseek", "nvidia", "fireworks-ai"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output), Some(date)) = (args.next(), args.next(), args.next()) else {
        return Err("usage: snapshot <api.json> <output> <snapshot-date>".into());
    };
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&input)?)?;
    let mut providers = BTreeMap::new();
    for name in PROVIDER_ALLOWLIST {
        let Some(models_in) = raw.get(name).and_then(|p| p.get("models")).and_then(|m| m.as_object()) else {
            continue;
        };
        let mut models = BTreeMap::new();
        for (id, model) in models_in {
            let entry = serde_json::json!({
                "context_window": model.pointer("/limit/context"),
                "output_ceiling": model.pointer("/limit/output"),
                "display_name": model.get("name"),
                "cost": model.get("cost").map(|cost| serde_json::json!({
                    "input": cost.get("input"),
                    "output": cost.get("output"),
                    "cache_read": cost.get("cache_read"),
                    "cache_write": cost.get("cache_write"),
                })),
            });
            models.insert(id.clone(), entry);
        }
        providers.insert((*name).to_owned(), serde_json::json!({ "models": models }));
    }
    let catalog = serde_json::json!({
        "snapshot_date": date,
        "source": "models.dev api.json",
        "providers": providers,
    });
    std::fs::write(&output, serde_json::to_string_pretty(&catalog)?)?;
    println!("wrote {output}: {} providers", catalog["providers"].as_object().map_or(0, serde_json::Map::len));
    Ok(())
}
```

(The allowlist covers yach's drivable providers plus the vendors whose models the Zen aggregator serves — `entry_by_model_id` needs those baked to resolve aggregator cells. Null-valued fields deserialize as `None` through `CatalogEntry`'s `Option`s; that is deliberate.)

- [ ] **Step 2: The recipe**

Add to the justfile, following its comment style:

```just
# Regenerate the baked model catalog from models.dev (build-time tool;
# the runtime never fetches). Review the data diff like any change.
catalog-snapshot:
  curl -sf https://models.dev/api.json -o /tmp/models-dev-api.json
  cargo run -p yach-catalog --bin snapshot -- /tmp/models-dev-api.json crates/yach-catalog/data/catalog.json "$(date +%F)"
```

- [ ] **Step 3: Generate and commit the data**

Run: `just dev just catalog-snapshot` (or run the two commands directly with `just dev`). Then verify: `jq -r '.providers | keys | join(", ")' crates/yach-catalog/data/catalog.json` lists the allowlisted providers present in models.dev, and `jq -c '.providers.anthropic.models["claude-haiku-4-5"]' crates/yach-catalog/data/catalog.json` shows `context_window: 200000, output_ceiling: 64000` with cost rates.

- [ ] **Step 4: Embed and expose**

In lib.rs:

```rust
static BAKED: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();

/// The catalog baked into this build (release floor). Parsing happens
/// once; the data is committed and repo-reviewed, so a parse failure is
/// a build defect — surfaced loudly at first use, never mid-session.
#[must_use]
pub fn baked_catalog() -> &'static Catalog {
    BAKED.get_or_init(|| {
        Catalog::from_json_str(include_str!("../data/catalog.json"))
            .unwrap_or_else(|error| unreachable!("committed catalog data must parse: {error}"))
    })
}
```

Add a test: `baked_catalog_parses_and_carries_known_models` asserting `baked_catalog().entry("anthropic", "claude-haiku-4-5").is_some()` and that `snapshot_date()` is non-empty.

- [ ] **Step 5: Gate + commit**

Run: crate tests, fmt, workspace clippy.

```bash
jj commit -m "feat: baked models.dev snapshot and catalog-snapshot recipe"
```

---

### Task 3: Override files, env mapping, and CLI consumer wiring

**Files:**
- Modify: `crates/yach-cli/Cargo.toml` (add `yach-catalog = { path = "../yach-catalog", version = "0.1.0" }`)
- Modify: `crates/yach-cli/src/main.rs` — `rig_provider_adapter_config_from_env_with_model_override` (~line 770) and `provider_model_from_env` callers; a new `resolve_model_profile` helper.

**Interfaces:**
- Consumes: `yach_catalog::{resolve, baked_catalog, Overrides, EnvOverrides, effective_output_budget, ModelProfile, OutputTokensParam}`.
- Produces: `fn resolve_model_profile(provider_label: &str, model: &str) -> yach_catalog::ModelProfile` (Task 5 reuses it); `RigProviderAdapterConfig` now carries catalog-resolved numbers.

- [ ] **Step 1: The profile helper**

Add near the config parsing in main.rs:

```rust
/// Resolve the model's catalog profile: baked snapshot -> user
/// (~/.yach/models.toml) -> project (.yach/models.toml) -> env vars.
/// Missing or malformed override files degrade to absent with a stderr
/// warning — a bad correction file must never block a session.
fn resolve_model_profile(provider_label: &str, model: &str) -> yach_catalog::ModelProfile {
    let load = |path: std::path::PathBuf| -> Option<yach_catalog::Overrides> {
        let text = std::fs::read_to_string(&path).ok()?;
        match yach_catalog::Overrides::from_toml_str(&text) {
            Ok(overrides) => Some(overrides),
            Err(error) => {
                eprintln!("warning: ignoring malformed {}: {error}", path.display());
                None
            }
        }
    };
    let user = std::env::home_dir().map(|home| home.join(".yach/models.toml")).and_then(load);
    let project = load(std::path::PathBuf::from(".yach/models.toml"));
    let env = yach_catalog::EnvOverrides {
        context_window: optional_env("YACH_RIG_PROVIDER_CONTEXT_WINDOW").and_then(|value| value.parse().ok()),
        max_tokens: optional_env("YACH_RIG_PROVIDER_MAX_TOKENS").and_then(|value| value.parse().ok()),
        output_tokens_param: optional_env("YACH_RIG_PROVIDER_MAX_TOKENS_PARAM").as_deref().map(|value| match value {
            "max_completion_tokens" => yach_catalog::OutputTokensParam::MaxCompletionTokens,
            _ => yach_catalog::OutputTokensParam::MaxTokens,
        }),
    };
    yach_catalog::resolve(provider_label, model, yach_catalog::baked_catalog(), user.as_ref(), project.as_ref(), &env)
}
```

CAVEAT to verify while implementing: `std::env::home_dir()` is un-deprecated in recent Rust but check the workspace's toolchain accepts it without a warning; if it warns, use the same home-dir mechanism the codebase already uses (grep for how `~/.yach` or the user extension store is located — reuse that helper).

The existing bounded-env parsing (`optional_bounded_env`) validates ranges today; keep validation: apply the same bounds AFTER resolution (clamp env values through the existing `optional_bounded_env` calls rather than raw `parse` if simpler — the requirement is that env values keep their current bounds behavior; state in the report which mechanism you used).

- [ ] **Step 2: Wire `rig_provider_adapter_config_from_env_with_model_override`**

The function currently builds `RigProviderAdapterConfig` with `optional_bounded_env` defaults for `max_tokens` / `context_window` and env-only `max_tokens_param`. Rewire: after the provider match resolves the provider label and the model is known (the same values `provider_model_from_env` would produce — thread the resolved model in), call `resolve_model_profile`, then:

- `max_tokens: effective_output_budget(&profile, env_max_tokens).value`
- `context_window: profile.context_window.value`
- `max_tokens_param:` map `profile.output_tokens_param.value` — `OutputTokensParam::MaxTokens => MaxTokensParam::MaxTokens`, `MaxCompletionTokens => MaxTokensParam::MaxCompletionTokens`.

The stopgap comments at those three sites (32k cohort note, 200k note, spelling note) are REPLACED by one comment pointing at the catalog: the values now resolve through `yach-catalog` layers with env as override; the cohort-default and floor semantics live in the crate.

Keep the return type unchanged — backend structs are untouched in this slice.

- [ ] **Step 3: Tests**

In main.rs tests (no process-global env mutation — construct `EnvOverrides` directly where needed): a test that `resolve_model_profile`-style resolution feeding `RigProviderAdapterConfig` preserves the behavior floor for an unknown model (200k/32k/MaxTokens); a test that a baked model with a small ceiling (construct via a `Catalog` fixture through the pure functions, not the global) budgets `min(ceiling, 32k)`. Pure-function tests live in yach-catalog already; here test only the mapping glue you added (e.g. `OutputTokensParam -> MaxTokensParam`).

- [ ] **Step 4: Gate + commit**

Workspace tests + clippy + fmt.

```bash
jj commit -m "feat: catalog-resolved windows, budgets, and spellings in provider config"
```

---

### Task 4: Key-truthful-ready picker — catalog display names replace the curated list

**Files:**
- Modify: `crates/yach-backend/src/runner.rs` — `ANTHROPIC_MODEL_CHOICES` (~line 889) and `send_native_models` (~896); the backend config struct that carries provider info to the runner (locate: the struct holding `provider` consumed by `send_native_models` — likely `ProviderConfig`/backend session config; grep `provider_label() == "anthropic"`).
- Modify: `crates/yach-cli/src/main.rs` — where that backend config is built, populate the new field.

**Interfaces:**
- Consumes: `resolve_model_profile` (Task 3) and `baked_catalog()` listing.
- Produces: backend config field `catalog_models: Vec<ModelInfo>` (display list resolved by the CLI at startup).

- [ ] **Step 1: Backend accepts a supplied list**

Add `catalog_models: Vec<ModelInfo>` (default empty) to the config struct that reaches `send_native_models`. Rewrite `send_native_models`: if `catalog_models` is non-empty, the list is `active` + the supplied entries (minus the active id) — provider-agnostic; else EXACTLY today's behavior (curated anthropic list) as the fallback. Do not delete `ANTHROPIC_MODEL_CHOICES` yet — it becomes the fallback body and gets a comment: retired once discovery (slice 3) makes the supplied list universal.

- [ ] **Step 2: CLI supplies the list**

Where the CLI builds that config (TUI + headless paths — both feed the same struct; grep the construction sites), populate `catalog_models` for the configured provider from `baked_catalog()`: every model under the provider's catalog entry, `ModelInfo { id, name: display_name via resolve (so overrides apply), provider: label }`. For `openai-compatible`, supply only the configured model (aggregator namespaces are not enumerable from the catalog — slice 3's discovery owns that).

- [ ] **Step 3: Tests**

Backend: `send_native_models` with a supplied list emits it (active first, no duplicate); with empty list emits today's anthropic curated shape (existing tests keep passing — adjust only if they construct the config struct and now need the new field; `..Default::default()` or explicit empty vec).

- [ ] **Step 4: Gate + commit**

```bash
jj commit -m "feat: catalog-supplied /model list with curated fallback"
```

---

### Task 5: Cost and provenance in the outcome document

**Files:**
- Modify: `crates/yach-cli/src/headless.rs` — the outcome document builder (~line 620, `serde_json::json!` block) and its tests (~1080+).

**Interfaces:**
- Consumes: `resolve_model_profile` (Task 3), `sum_log_usage` (exists), profile cost rates.

- [ ] **Step 1: Extend the document**

The schema constant `OUTCOME_SCHEMA = "yach-run-outcome/1"` — read its versioning convention first (grep for consumers/docs; yacht's evidence_map reads declared fields). Fields here are ADDITIVE; if the convention allows additive fields within a version, keep `/1`; if the docs say any change bumps, bump to `/2` and say so in the report. Add beside `usage`:

```rust
        "config": {
            "context_window": { "value": profile.context_window.value, "source": catalog_source_label(&profile.context_window.source) },
            "output_budget": { "value": resolved_output_budget.value, "source": catalog_source_label(&resolved_output_budget.source) },
        },
        "cost": cost_block(&profile, input_tokens, output_tokens, usage_reported),
```

with:

```rust
/// Stable snake_case labels for evidence: "baked:<date>", "override:user",
/// "override:project", "env", "default".
fn catalog_source_label(source: &yach_catalog::CatalogSource) -> String {
    match source {
        yach_catalog::CatalogSource::Baked { snapshot_date } => format!("baked:{snapshot_date}"),
        yach_catalog::CatalogSource::Override { scope: yach_catalog::OverrideScope::User } => String::from("override:user"),
        yach_catalog::CatalogSource::Override { scope: yach_catalog::OverrideScope::Project } => String::from("override:project"),
        yach_catalog::CatalogSource::EnvOverride => String::from("env"),
        yach_catalog::CatalogSource::Default => String::from("default"),
    }
}

/// Cost honesty mirrors the meter: unreported usage -> no cost claim;
/// unknown rates -> explicit "unknown". Never a fabricated zero.
fn cost_block(profile: &yach_catalog::ModelProfile, input: u64, output: u64, reported: bool) -> serde_json::Value {
    match (&profile.cost, reported) {
        (Some(cost), true) => {
            let rates = &cost.value;
            let amount = (input as f64 * rates.input + output as f64 * rates.output) / 1_000_000.0;
            serde_json::json!({
                "status": "computed",
                "usd": (amount * 1_000_000.0).round() / 1_000_000.0,
                "rates_source": catalog_source_label(&cost.source),
            })
        }
        (None, true) => serde_json::json!({ "status": "unknown_rates" }),
        (_, false) => serde_json::json!({ "status": "unreported_usage" }),
    }
}
```

(Cache-read rates are omitted from the computation in this slice — `sum_log_usage` does not separate cache reads; note that in the code comment so the number is never over-claimed. Precision: round to 6 decimal places as shown.)

Thread BOTH the `ModelProfile` and the `Sourced<u64>` output budget computed at config time (Task 3's `effective_output_budget` result) into the builder — pass them down rather than re-resolving, so the document and the request-path behavior can never disagree. `resolved_output_budget` in the block above is that threaded value.

- [ ] **Step 2: Tests**

Extend the existing outcome-document tests: `document["config"]["context_window"]["source"]` equals `"default"` for the fixture provider; a test with a profile fixture carrying baked cost rates and reported usage asserts `cost.status == "computed"` and the exact usd for known token counts (e.g. 1_200 in / 340 out at 1.0/5.0 per million = 0.0029); unreported usage asserts `status == "unreported_usage"` with NO usd key.

- [ ] **Step 3: Gate + commit**

```bash
jj commit -m "feat: cost and catalog provenance in the outcome document"
```

---

### Task 6: Workspace audit and full verification (controller-led measurement at the end)

- [ ] **Step 1: Stopgap-comment audit**

Run: `grep -rn "model-catalog\|model catalog" crates --include="*.rs"` — every remaining marker must either be retired by this slice or name what still waits (truncated-call recovery, dialects, discovery). Fix stale ones.

- [ ] **Step 2: Full gate**

`just dev cargo clippy --workspace --all-targets && just dev cargo test --workspace` — green, zero warnings. `just dev cargo fmt --check` everywhere.

- [ ] **Step 3 (controller + owner): image, gate, sweep**

`just runtime-image` + freshness check; eval gate; the 125-cell sweep vs the current 125/125 reference (this slice changes budgets/windows in flight — sweep is mandatory). Spot-check an outcome document for the new `config`/`cost` blocks with sane provenance labels, and hand-verify one cost figure against provider-reported usage and baked rates.

- [ ] **Step 4: Record + board + commit (controller)**

Measurement record per the established style; board item to MEASURED for slice 1 (slices 2–3 stay queued).
