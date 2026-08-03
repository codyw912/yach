//! Layered model-metadata catalog: baked snapshot -> user/project
//! overrides -> env overrides, resolved per FIELD with provenance.
//! Design: docs/superpowers/specs/2026-08-02-model-catalog-hydration-design.md
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
pub enum OverrideScope {
    User,
    Project,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sourced<T> {
    pub value: T,
    pub source: CatalogSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputTokensParam {
    MaxTokens,
    MaxCompletionTokens,
}

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
        Self {
            snapshot_date: String::from(snapshot_date),
            providers: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, provider: &str, model: &str, entry: CatalogEntry) {
        self.providers
            .entry(String::from(provider))
            .or_insert_with(|| ProviderEntry {
                models: BTreeMap::new(),
                error_dialect: None,
            })
            .models
            .insert(String::from(model), entry);
    }

    #[must_use]
    pub fn entry(&self, provider: &str, model: &str) -> Option<&CatalogEntry> {
        self.providers
            .get(provider)
            .and_then(|p| p.models.get(model))
    }

    /// Fallback for aggregator shapes: find the model id under any baked
    /// provider. First match in provider name order; provenance still
    /// records the snapshot. Used when the configured provider has no
    /// entry (openai-compatible aggregators serve other vendors' models).
    #[must_use]
    pub fn entry_by_model_id(&self, model: &str) -> Option<&CatalogEntry> {
        self.providers.values().find_map(|p| p.models.get(model))
    }

    /// Model ids baked for a provider, in the catalog's `BTreeMap`
    /// (alphabetical) order. Empty when the provider has no baked
    /// entries — e.g. `openai-compatible`, whose aggregator namespaces
    /// are not enumerable from the catalog (slice 3's discovery owns
    /// that).
    #[must_use]
    pub fn model_ids(&self, provider: &str) -> Vec<&str> {
        self.providers
            .get(provider)
            .map(|entry| entry.models.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[must_use]
    pub fn snapshot_date(&self) -> &str {
        &self.snapshot_date
    }

    /// The provider's error-dialect id, when the data carries one.
    /// Spec-carved home; the classifier design is a separate item.
    #[must_use]
    pub fn provider_error_dialect(&self, provider: &str) -> Option<&str> {
        self.providers
            .get(provider)
            .and_then(|p| p.error_dialect.as_deref())
    }
}

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
    let baked_entry = baked
        .entry(provider, model)
        .or_else(|| baked.entry_by_model_id(model));
    let user_entry = user.and_then(|o| o.entry(provider, model));
    let project_entry = project.and_then(|o| o.entry(provider, model));
    let snapshot = baked.snapshot_date().to_owned();

    // Per-field: env > project > user > baked > default.
    let field = |env_value: Option<u64>,
                 pick: fn(&CatalogEntry) -> Option<u64>,
                 default: u64|
     -> Sourced<u64> {
        if let Some(value) = env_value {
            return Sourced {
                value,
                source: CatalogSource::EnvOverride,
            };
        }
        // Zero-valued metadata is absence in disguise: upstream (e.g.
        // models.dev's gpt-image-1 entry) represents "no data for this
        // field" as a literal 0 rather than omitting it. A resolved
        // window/ceiling of 0 would break every downstream consumer that
        // budgets or divides against it, so a baked or overridden 0 is
        // treated exactly like a missing field: fall through to the next
        // layer instead of returning it as a real value.
        if let Some(value) = project_entry.and_then(pick).filter(|&value| value != 0) {
            return Sourced {
                value,
                source: CatalogSource::Override {
                    scope: OverrideScope::Project,
                },
            };
        }
        if let Some(value) = user_entry.and_then(pick).filter(|&value| value != 0) {
            return Sourced {
                value,
                source: CatalogSource::Override {
                    scope: OverrideScope::User,
                },
            };
        }
        if let Some(value) = baked_entry.and_then(pick).filter(|&value| value != 0) {
            return Sourced {
                value,
                source: CatalogSource::Baked {
                    snapshot_date: snapshot.clone(),
                },
            };
        }
        Sourced {
            value: default,
            source: CatalogSource::Default,
        }
    };

    let context_window = field(
        env.context_window,
        |e| e.context_window,
        DEFAULT_CONTEXT_WINDOW,
    );
    let output_ceiling = field(None, |e| e.output_ceiling, DEFAULT_OUTPUT_BUDGET);
    // (env max_tokens applies at effective_output_budget, not to the ceiling)

    let output_tokens_param = if let Some(value) = env.output_tokens_param {
        Sourced {
            value,
            source: CatalogSource::EnvOverride,
        }
    } else if let Some(value) = project_entry.and_then(|e| e.output_tokens_param) {
        Sourced {
            value,
            source: CatalogSource::Override {
                scope: OverrideScope::Project,
            },
        }
    } else if let Some(value) = user_entry.and_then(|e| e.output_tokens_param) {
        Sourced {
            value,
            source: CatalogSource::Override {
                scope: OverrideScope::User,
            },
        }
    } else if let Some(value) = baked_entry.and_then(|e| e.output_tokens_param) {
        Sourced {
            value,
            source: CatalogSource::Baked {
                snapshot_date: snapshot.clone(),
            },
        }
    } else {
        Sourced {
            value: OutputTokensParam::MaxTokens,
            source: CatalogSource::Default,
        }
    };

    let display_name = [
        (
            project_entry,
            CatalogSource::Override {
                scope: OverrideScope::Project,
            },
        ),
        (
            user_entry,
            CatalogSource::Override {
                scope: OverrideScope::User,
            },
        ),
        (
            baked_entry,
            CatalogSource::Baked {
                snapshot_date: snapshot.clone(),
            },
        ),
    ]
    .into_iter()
    .find_map(|(entry, source)| {
        entry
            .and_then(|e| e.display_name.clone())
            .map(|value| Sourced { value, source })
    })
    .unwrap_or(Sourced {
        value: String::from(model),
        source: CatalogSource::Default,
    });

    let cost = [
        (
            project_entry,
            CatalogSource::Override {
                scope: OverrideScope::Project,
            },
        ),
        (
            user_entry,
            CatalogSource::Override {
                scope: OverrideScope::User,
            },
        ),
        (
            baked_entry,
            CatalogSource::Baked {
                snapshot_date: snapshot,
            },
        ),
    ]
    .into_iter()
    .find_map(|(entry, source)| {
        entry
            .and_then(|e| e.cost.clone())
            .map(|value| Sourced { value, source })
    });

    ModelProfile {
        context_window,
        output_ceiling,
        output_tokens_param,
        display_name,
        cost,
    }
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
/// When the ceiling is what caps the result (ceiling < the cohort
/// default), the ceiling's own source is honest provenance. But when the
/// cohort default is what caps it (ceiling >= the default), the
/// resulting value is the default, not whatever the ceiling's layer
/// supplied — so the source must say `Default`, or the outcome document
/// would attribute a number to a layer that never claimed it.
#[must_use]
pub fn effective_output_budget(
    profile: &ModelProfile,
    env_max_tokens: Option<u64>,
) -> Sourced<u64> {
    if let Some(value) = env_max_tokens {
        return Sourced {
            value,
            source: CatalogSource::EnvOverride,
        };
    }
    if profile.output_ceiling.value >= DEFAULT_OUTPUT_BUDGET {
        return Sourced {
            value: DEFAULT_OUTPUT_BUDGET,
            source: CatalogSource::Default,
        };
    }
    Sourced {
        value: profile.output_ceiling.value,
        source: profile.output_ceiling.source.clone(),
    }
}

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
        let profile = resolve(
            "openai-compatible",
            "mystery-model",
            &catalog,
            None,
            None,
            &EnvOverrides::default(),
        );
        assert_eq!(profile.context_window.value, 200_000);
        assert!(matches!(
            profile.context_window.source,
            CatalogSource::Default
        ));
        assert_eq!(profile.output_ceiling.value, 32_000);
        assert!(matches!(
            profile.output_ceiling.source,
            CatalogSource::Default
        ));
        assert!(matches!(
            profile.output_tokens_param.value,
            OutputTokensParam::MaxTokens
        ));
        assert_eq!(profile.display_name.value, "mystery-model");
        assert!(profile.cost.is_none());
    }

    #[test]
    fn baked_data_overrides_the_floor_and_carries_snapshot_provenance() {
        let catalog = baked_with(
            "anthropic",
            "claude-haiku-4-5",
            CatalogEntry {
                context_window: Some(200_000),
                output_ceiling: Some(64_000),
                display_name: Some(String::from("Claude Haiku 4.5")),
                cost: Some(CostRates {
                    input: 1.0,
                    output: 5.0,
                    cache_read: Some(0.1),
                    cache_write: Some(1.25),
                }),
                output_tokens_param: None,
            },
        );
        let profile = resolve(
            "anthropic",
            "claude-haiku-4-5",
            &catalog,
            None,
            None,
            &EnvOverrides::default(),
        );
        assert_eq!(profile.output_ceiling.value, 64_000);
        assert!(
            matches!(&profile.output_ceiling.source, CatalogSource::Baked { snapshot_date } if snapshot_date == "2026-08-02")
        );
        assert!(profile.cost.is_some());
    }

    #[test]
    fn baked_zero_window_falls_through_to_the_default_floor() {
        // models.dev represents "no data for this field" as a literal 0
        // for some models (e.g. gpt-image-1) rather than omitting it. A
        // baked 0 must not be treated as a real window/ceiling.
        let catalog = baked_with(
            "openai",
            "gpt-image-1",
            CatalogEntry {
                context_window: Some(0),
                output_ceiling: Some(0),
                ..CatalogEntry::default()
            },
        );
        let profile = resolve(
            "openai",
            "gpt-image-1",
            &catalog,
            None,
            None,
            &EnvOverrides::default(),
        );
        assert_eq!(profile.context_window.value, DEFAULT_CONTEXT_WINDOW);
        assert!(matches!(
            profile.context_window.source,
            CatalogSource::Default
        ));
        assert_eq!(profile.output_ceiling.value, DEFAULT_OUTPUT_BUDGET);
        assert!(matches!(
            profile.output_ceiling.source,
            CatalogSource::Default
        ));
    }

    #[test]
    fn model_ids_lists_a_providers_models_in_btreemap_order() {
        let mut catalog = Catalog::empty("2026-08-02");
        catalog.insert("anthropic", "claude-opus-4-8", CatalogEntry::default());
        catalog.insert("anthropic", "claude-haiku-4-5", CatalogEntry::default());

        assert_eq!(
            catalog.model_ids("anthropic"),
            vec!["claude-haiku-4-5", "claude-opus-4-8"]
        );
    }

    #[test]
    fn model_ids_is_empty_for_an_unbaked_provider() {
        let catalog = Catalog::empty("2026-08-02");

        assert!(catalog.model_ids("openai-compatible").is_empty());
    }

    #[test]
    fn env_beats_override_beats_baked_per_field() {
        let catalog = baked_with(
            "anthropic",
            "m",
            CatalogEntry {
                context_window: Some(100_000),
                output_ceiling: Some(50_000),
                ..CatalogEntry::default()
            },
        );
        let Ok(project) = Overrides::from_toml_str("[anthropic.m]\ncontext_window = 150000\n")
        else {
            unreachable!("fixture toml must parse");
        };
        let env = EnvOverrides {
            context_window: Some(180_000),
            ..EnvOverrides::default()
        };
        let profile = resolve("anthropic", "m", &catalog, None, Some(&project), &env);
        // env wins for the field it sets…
        assert_eq!(profile.context_window.value, 180_000);
        assert!(matches!(
            profile.context_window.source,
            CatalogSource::EnvOverride
        ));
        // …and does NOT shadow other fields: baked still supplies the ceiling.
        assert_eq!(profile.output_ceiling.value, 50_000);
        assert!(matches!(
            profile.output_ceiling.source,
            CatalogSource::Baked { .. }
        ));
    }

    #[test]
    fn project_override_beats_user_override() {
        let catalog = Catalog::empty("2026-08-02");
        let Ok(user) = Overrides::from_toml_str("[p.m]\ncontext_window = 111\n") else {
            unreachable!("fixture toml must parse");
        };
        let Ok(project) = Overrides::from_toml_str("[p.m]\ncontext_window = 222\n") else {
            unreachable!("fixture toml must parse");
        };
        let profile = resolve(
            "p",
            "m",
            &catalog,
            Some(&user),
            Some(&project),
            &EnvOverrides::default(),
        );
        assert_eq!(profile.context_window.value, 222);
        assert!(matches!(
            profile.context_window.source,
            CatalogSource::Override {
                scope: OverrideScope::Project
            }
        ));
    }

    #[test]
    fn effective_budget_is_min_of_ceiling_and_cohort_default_unless_env_set() {
        let mut profile_small = resolve(
            "p",
            "m",
            &Catalog::empty("d"),
            None,
            None,
            &EnvOverrides::default(),
        );
        profile_small.output_ceiling = Sourced {
            value: 8_192,
            source: CatalogSource::Baked {
                snapshot_date: String::from("d"),
            },
        };
        let small = effective_output_budget(&profile_small, None);
        assert_eq!(small.value, 8_192);
        // The ceiling is what caps the result here, so its own source is
        // the honest label.
        assert!(matches!(small.source, CatalogSource::Baked { .. }));

        let mut profile_capped = resolve(
            "p",
            "m",
            &Catalog::empty("d"),
            None,
            None,
            &EnvOverrides::default(),
        );
        profile_capped.output_ceiling = Sourced {
            value: 64_000,
            source: CatalogSource::Baked {
                snapshot_date: String::from("d"),
            },
        };
        let capped = effective_output_budget(&profile_capped, None);
        assert_eq!(capped.value, 32_000);
        // The cohort default is what caps the result here — the baked
        // ceiling never claimed 32_000, so its source must not be
        // attributed to the baked layer.
        assert!(matches!(capped.source, CatalogSource::Default));

        // Boundary: ceiling exactly equal to the cohort default. Pins the
        // `>=` (not `>`) comparison in `effective_output_budget` — under
        // `>` this would wrongly stay `Baked`, since a baked ceiling of
        // exactly 32_000 would fail a strict `>` check and fall through to
        // "the ceiling caps it, keep its source".
        let mut profile_at_boundary = resolve(
            "p",
            "m",
            &Catalog::empty("d"),
            None,
            None,
            &EnvOverrides::default(),
        );
        profile_at_boundary.output_ceiling = Sourced {
            value: 32_000,
            source: CatalogSource::Baked {
                snapshot_date: String::from("d"),
            },
        };
        let at_boundary = effective_output_budget(&profile_at_boundary, None);
        assert_eq!(at_boundary.value, 32_000);
        assert!(matches!(at_boundary.source, CatalogSource::Default));

        let profile_big = resolve(
            "p",
            "m",
            &Catalog::empty("d"),
            None,
            None,
            &EnvOverrides::default(),
        );
        let big = effective_output_budget(&profile_big, None);
        assert_eq!(big.value, 32_000);
        assert!(matches!(big.source, CatalogSource::Default));
        assert_eq!(
            effective_output_budget(&profile_big, Some(64_000)).value,
            64_000
        );
        assert!(matches!(
            effective_output_budget(&profile_big, Some(64_000)).source,
            CatalogSource::EnvOverride
        ));
    }

    #[test]
    fn catalog_json_tolerates_unknown_fields() {
        let json = r#"{ "snapshot_date": "2026-08-02", "providers": { "anthropic": { "models": { "m": { "context_window": 10, "future_field": {"x": 1} } } } } }"#;
        let Ok(catalog) = Catalog::from_json_str(json) else {
            unreachable!("fixture json must parse");
        };
        let Some(entry) = catalog.entry("anthropic", "m") else {
            unreachable!("entry inserted above");
        };
        assert_eq!(entry.context_window, Some(10));
    }

    #[test]
    fn baked_catalog_parses_and_carries_known_models() {
        let catalog = baked_catalog();
        assert!(catalog.entry("anthropic", "claude-haiku-4-5").is_some());
        assert!(!catalog.snapshot_date().is_empty());
    }

    #[test]
    fn entry_by_model_id_falls_back_to_a_non_configured_provider() {
        // "deepseek-chat" is only baked under the "deepseek" provider, not
        // under "openai-compatible" — the fallback must still find it via
        // the model id alone, which is how Zen-style aggregator cells
        // resolve metadata for vendors yach doesn't drive directly.
        let catalog = baked_catalog();
        assert!(
            catalog
                .entry("openai-compatible", "deepseek-chat")
                .is_none()
        );
        let Some(entry) = catalog.entry_by_model_id("deepseek-chat") else {
            unreachable!("deepseek-chat is baked under the deepseek provider");
        };
        assert!(entry.context_window.is_some());
    }
}
