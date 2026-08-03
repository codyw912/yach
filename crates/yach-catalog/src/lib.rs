//! Layered model-metadata catalog: baked snapshot -> user/project
//! overrides -> env overrides, resolved per FIELD with provenance.
//! Design: docs/superpowers/specs/2026-08-02-model-catalog-hydration-design.md
//! The schema tolerates unknown fields so future layers (fetched,
//! discovered, quirks) are additive.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
pub const DEFAULT_OUTPUT_BUDGET: u64 = 32_000;

#[derive(Debug, Clone, PartialEq)]
pub enum CatalogSource {
    Baked {
        snapshot_date: String,
    },
    /// Data pulled live from models.dev (or another remote source) and
    /// cached locally. `retrieved` is the CACHE's retrieval date, never
    /// the source catalog's own `snapshot_date` — a fetched catalog's
    /// `snapshot_date` is meaningless (it's whatever the remote payload
    /// happened to carry, if anything), so provenance must be labeled
    /// from when yach actually pulled it.
    Fetched {
        retrieved: String,
    },
    Override {
        scope: OverrideScope,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputTokensParam {
    MaxTokens,
    MaxCompletionTokens,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CostRates {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct CatalogEntry {
    pub context_window: Option<u64>,
    pub output_ceiling: Option<u64>,
    pub output_tokens_param: Option<OutputTokensParam>,
    pub display_name: Option<String>,
    pub cost: Option<CostRates>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ProviderEntry {
    #[serde(default)]
    models: BTreeMap<String, CatalogEntry>,
    /// Per-provider error dialect id (the spec-carved home for the
    /// tiered-classifier item). Data optional; no consumer in this slice.
    #[serde(default)]
    error_dialect: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// A models.dev catalog fetched at runtime and persisted to disk, plus
/// the HTTP validators needed for a conditional re-fetch and the date it
/// was retrieved (the provenance label for every field it supplies — see
/// `CatalogSource::Fetched`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedCatalog {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub retrieved: String,
    pub catalog: Catalog,
}

impl CachedCatalog {
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
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
    fetched: Option<(&Catalog, &str)>,
    user: Option<&Overrides>,
    project: Option<&Overrides>,
    env: &EnvOverrides,
) -> ModelProfile {
    let baked_entry = baked
        .entry(provider, model)
        .or_else(|| baked.entry_by_model_id(model));
    // Paired with its retrieved date up front, so a `Fetched` source can
    // never be built without a real date (no empty-string sentinel to
    // accidentally observe).
    let fetched_pair: Option<(&CatalogEntry, &str)> = fetched.and_then(|(catalog, retrieved)| {
        catalog
            .entry(provider, model)
            .or_else(|| catalog.entry_by_model_id(model))
            .map(|entry| (entry, retrieved))
    });
    let user_entry = user.and_then(|o| o.entry(provider, model));
    let project_entry = project.and_then(|o| o.entry(provider, model));
    let snapshot = baked.snapshot_date().to_owned();

    // Per-field: env > project > user > fetched > baked > default.
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
        if let Some((value, retrieved)) = fetched_pair
            .and_then(|(entry, retrieved)| pick(entry).map(|value| (value, retrieved)))
            .filter(|&(value, _)| value != 0)
        {
            return Sourced {
                value,
                source: CatalogSource::Fetched {
                    retrieved: String::from(retrieved),
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
    } else if let Some((value, retrieved)) = fetched_pair
        .and_then(|(entry, retrieved)| entry.output_tokens_param.map(|value| (value, retrieved)))
    {
        Sourced {
            value,
            source: CatalogSource::Fetched {
                retrieved: String::from(retrieved),
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

    // Vec, not a fixed array: the fetched rung only exists (and only
    // needs a source built) when a fetched layer was actually supplied.
    let layers: Vec<(Option<&CatalogEntry>, CatalogSource)> = vec![
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
            fetched_pair.map(|(entry, _)| entry),
            match fetched_pair {
                Some((_, retrieved)) => CatalogSource::Fetched {
                    retrieved: String::from(retrieved),
                },
                None => CatalogSource::Default,
            },
        ),
        (
            baked_entry,
            CatalogSource::Baked {
                snapshot_date: snapshot,
            },
        ),
    ];

    let display_name = layers
        .iter()
        .find_map(|(entry, source)| {
            entry
                .and_then(|e| e.display_name.clone())
                .map(|value| Sourced {
                    value,
                    source: source.clone(),
                })
        })
        .unwrap_or(Sourced {
            value: String::from(model),
            source: CatalogSource::Default,
        });

    let cost = layers.iter().find_map(|(entry, source)| {
        entry.and_then(|e| e.cost.clone()).map(|value| Sourced {
            value,
            source: source.clone(),
        })
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

/// Providers yach can drive; anything else models.dev serves is dropped
/// during the transform below.
const PROVIDER_ALLOWLIST: &[&str] = &[
    "anthropic",
    "openai",
    "alibaba",
    "deepseek",
    "nvidia",
    "fireworks-ai",
];

// OpenAI gpt-5.x: the published figure is the extended window, which the
// API grants only with an explicit opt-in yach does not send, and input
// past 272k doubles session input pricing. Pin the standard window, as
// Codex and omp do. Retire when yach adds a deliberate extended-context
// option. Sources: developers.openai.com/api/docs/models/gpt-5.4;
// omp packages/catalog scripts/generated-policies.ts.
const OPENAI_STANDARD_CONTEXT_WINDOW: u64 = 272_000;

/// Applies the openai gpt-5.x context-window pin (see
/// `OPENAI_STANDARD_CONTEXT_WINDOW` above) and passes everything else
/// through unchanged, including a missing/non-numeric raw value.
fn pinned_context_window(
    provider: &str,
    model_id: &str,
    context: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let value = context?;
    if provider == "openai"
        && model_id.starts_with("gpt-5")
        && let Some(number) = value.as_u64()
    {
        return Some(serde_json::json!(
            number.min(OPENAI_STANDARD_CONTEXT_WINDOW)
        ));
    }
    Some(value.clone())
}

/// Suppresses cost data when both input and output rates are zero or
/// absent. Upstream (models.dev) represents "we don't have pricing for
/// this model" as `0`/missing rather than omitting the field (seen across
/// 94 of nvidia's 98 baked models) — baking that forward as `cost: {input:
/// 0, output: 0, ...}` would let the runtime present a fabricated $0
/// instead of an honest "unknown". Emitting `cost: null` here lets
/// `resolve` treat it exactly like a genuinely absent field.
fn filtered_cost(cost: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let cost = cost?;
    let input = cost
        .get("input")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let output = cost
        .get("output")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    if input == 0.0 && output == 0.0 {
        return None;
    }
    Some(serde_json::json!({
        "input": clamped_cost_rate(cost.get("input")),
        "output": clamped_cost_rate(cost.get("output")),
        "cache_read": clamped_cost_rate(cost.get("cache_read")),
        "cache_write": clamped_cost_rate(cost.get("cache_write")),
    }))
}

// Plausibility bounds, revised per owner ruling 2026-08-03 (a second
// ruling on the same day — the first draft clamped both ends of
// `context_window` and `output_ceiling`; this crate's own audit of the
// committed catalog against that draft found real `output_ceiling`s up to
// 512,000 and real small `context_window`s down to 77 that it would have
// distorted, which is what prompted the revision below).
//
// Fetched data is community-published and unreviewed — models.dev is a
// public wiki-style catalog, not a vendor API — but only ONE field here
// has genuinely unbounded blast radius from a wrong or hostile entry:
// `context_window`, which feeds compaction accounting directly (the
// budget math deciding when and how much to compact) with nothing else
// standing between it and the runtime. That field alone gets an upper
// cap; nothing floors it, because a real small window (a 77- or
// 8192-token embedding/image model) is honest data, and clamping a real
// value up to a floor is distortion, not defense.
//
// `output_ceiling` gets no clamp at all: `effective_output_budget` already
// takes `min(ceiling, DEFAULT_OUTPUT_BUDGET)`, so a runaway ceiling's
// blast radius is bounded downstream regardless of what this transform
// lets through, and real ceilings up to 512,000 exist in the current
// data — capping them here would understate real model capability rather
// than defend against anything.
//
// Cost rates keep a cap: a fabricated dollar figure feeds straight into
// budget and evidence output with no such downstream `min()` to catch it.
// Display names keep sanitization: a hostile or malformed name can carry
// terminal escape codes with no downstream filter either.
//
// Shared by both runtime paths that call this transform — the build-time
// `snapshot` bin baking a fresh catalog.json, and the runtime fetch
// layer's cache — so a baked regen gets the same treatment as a live
// fetch; a capped value shows up as an ordinary line in the snapshot's
// git diff, where it's reviewable like any other data change, rather than
// a note some other layer had to enforce silently at read time.
const CONTEXT_WINDOW_MAX: u64 = 2_000_000;
const COST_RATE_MIN: f64 = 0.0;
const COST_RATE_MAX: f64 = 1_000.0;
const DISPLAY_NAME_MAX_CHARS: usize = 64;

/// Caps a numeric field at `max` — one-sided, never floors: a non-numeric
/// or absent value passes through untouched (mirrors
/// `pinned_context_window`'s own None/non-numeric passthrough — this
/// transform never invents a field upstream didn't supply), and any real
/// value at or below `max`, including an explicit `0` (upstream's own
/// absence sentinel — see `filtered_cost`'s zero-rate comment), passes
/// through exactly as `min()` would leave it.
fn capped_numeric(value: Option<serde_json::Value>, max: u64) -> Option<serde_json::Value> {
    let value = value?;
    let Some(number) = value.as_u64() else {
        return Some(value);
    };
    Some(serde_json::json!(number.min(max)))
}

/// Clamps one cost-rate field (dollars per million tokens) to
/// `[COST_RATE_MIN, COST_RATE_MAX]`. Unlike `capped_numeric`, `0.0` needs
/// no sentinel exception here: it already sits inside the range (the
/// floor *is* zero), and `filtered_cost` has already suppressed the
/// all-zero case before this ever runs on a real entry.
fn clamped_cost_rate(value: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let value = value?;
    let Some(number) = value.as_f64() else {
        return Some(value.clone());
    };
    Some(serde_json::json!(
        number.clamp(COST_RATE_MIN, COST_RATE_MAX)
    ))
}

/// Strips ASCII control characters (a hostile or malformed upstream name
/// could carry terminal escape codes) and caps the result at
/// `DISPLAY_NAME_MAX_CHARS` characters — `.take` on a `Chars` iterator, so
/// the cut is inherently on a char boundary, never a byte offset into a
/// multi-byte character. A non-string or absent value passes through
/// unchanged, same passthrough contract as `capped_numeric`.
fn sanitized_display_name(value: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let value = value?;
    let Some(text) = value.as_str() else {
        return Some(value.clone());
    };
    let cleaned: String = text
        .chars()
        .filter(|ch| !ch.is_ascii_control())
        .take(DISPLAY_NAME_MAX_CHARS)
        .collect();
    Some(serde_json::json!(cleaned))
}

/// Transforms a models.dev `api.json` payload into this crate's catalog
/// JSON shape: allowlists providers to what yach can drive, applies the
/// gpt-5.x context-window pin, suppresses fabricated zero-rate pricing,
/// caps `context_window` and cost rates to plausibility bounds, and
/// sanitizes `display_name` (see the comment above `CONTEXT_WINDOW_MAX`
/// for why `output_ceiling` is deliberately left uncapped). The schema is
/// the catalog's own, not models.dev's, so upstream drift breaks this
/// transform loudly instead of the runtime quietly. Shared by the
/// build-time `snapshot` bin (baking a committed catalog) and the runtime
/// fetch layer (Task 2) — same shape either way, so `resolve`'s per-field
/// precedence sees the same JSON whether the data was baked or fetched.
#[must_use]
pub fn transform_models_dev(raw: &serde_json::Value, snapshot_date: &str) -> serde_json::Value {
    let mut providers = BTreeMap::new();
    for name in PROVIDER_ALLOWLIST {
        let Some(models_in) = raw
            .get(name)
            .and_then(|p| p.get("models"))
            .and_then(|m| m.as_object())
        else {
            continue;
        };
        let mut models = BTreeMap::new();
        for (id, model) in models_in {
            let context_window = capped_numeric(
                pinned_context_window(name, id, model.pointer("/limit/context")),
                CONTEXT_WINDOW_MAX,
            );
            let entry = serde_json::json!({
                "context_window": context_window,
                "output_ceiling": model.pointer("/limit/output"),
                "display_name": sanitized_display_name(model.get("name")),
                "cost": filtered_cost(model.get("cost")),
            });
            models.insert(id.clone(), entry);
        }
        providers.insert((*name).to_owned(), serde_json::json!({ "models": models }));
    }
    serde_json::json!({
        "snapshot_date": snapshot_date,
        "source": "models.dev api.json",
        "providers": providers,
    })
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
        let profile = resolve("anthropic", "m", &catalog, None, None, Some(&project), &env);
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
            None,
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
    fn fetched_layer_beats_baked_and_loses_to_overrides() {
        let baked = baked_with(
            "anthropic",
            "m",
            CatalogEntry {
                context_window: Some(100_000),
                ..CatalogEntry::default()
            },
        );
        let mut fetched = Catalog::empty("unused");
        fetched.insert(
            "anthropic",
            "m",
            CatalogEntry {
                context_window: Some(150_000),
                output_ceiling: Some(40_000),
                ..CatalogEntry::default()
            },
        );
        let Ok(project) = Overrides::from_toml_str("[anthropic.m]\ncontext_window = 160000\n")
        else {
            unreachable!("fixture toml must parse");
        };
        let profile = resolve(
            "anthropic",
            "m",
            &baked,
            Some((&fetched, "2026-08-03")),
            None,
            Some(&project),
            &EnvOverrides::default(),
        );
        // project override wins where it speaks…
        assert_eq!(profile.context_window.value, 160_000);
        // …fetched beats baked where the override is silent.
        assert_eq!(profile.output_ceiling.value, 40_000);
        assert!(
            matches!(&profile.output_ceiling.source, CatalogSource::Fetched { retrieved } if retrieved == "2026-08-03")
        );
    }

    #[test]
    fn fetched_layer_supplies_display_name_and_cost_when_silent_elsewhere() {
        // The brief's fetched-layer test only exercises the numeric
        // fields (context_window, output_ceiling); display_name and cost
        // walk a separate code path (a Vec of layers, not the `field`
        // closure), so they need their own coverage of the "fetched beats
        // baked" rung.
        let baked = Catalog::empty("2026-08-02");
        let mut fetched = Catalog::empty("unused");
        fetched.insert(
            "anthropic",
            "m",
            CatalogEntry {
                display_name: Some(String::from("Fetched Model")),
                cost: Some(CostRates {
                    input: 1.0,
                    output: 2.0,
                    cache_read: None,
                    cache_write: None,
                }),
                ..CatalogEntry::default()
            },
        );
        let profile = resolve(
            "anthropic",
            "m",
            &baked,
            Some((&fetched, "2026-08-03")),
            None,
            None,
            &EnvOverrides::default(),
        );
        assert_eq!(profile.display_name.value, "Fetched Model");
        assert!(
            matches!(&profile.display_name.source, CatalogSource::Fetched { retrieved } if retrieved == "2026-08-03")
        );
        let Some(cost) = profile.cost else {
            unreachable!("fetched cost must resolve");
        };
        assert_eq!(
            cost.value,
            CostRates {
                input: 1.0,
                output: 2.0,
                cache_read: None,
                cache_write: None,
            }
        );
        assert!(
            matches!(&cost.source, CatalogSource::Fetched { retrieved } if retrieved == "2026-08-03")
        );
    }

    #[test]
    fn absent_fetched_layer_never_labels_a_field_as_fetched() {
        // Guards against the empty-string "Fetched" sentinel that a naive
        // eager-source-construction would build even when no fetched
        // layer was supplied (see `fetched_pair` in `resolve`).
        let baked = baked_with(
            "anthropic",
            "m",
            CatalogEntry {
                display_name: Some(String::from("Baked Model")),
                cost: Some(CostRates {
                    input: 1.0,
                    output: 2.0,
                    cache_read: None,
                    cache_write: None,
                }),
                ..CatalogEntry::default()
            },
        );
        let profile = resolve(
            "anthropic",
            "m",
            &baked,
            None,
            None,
            None,
            &EnvOverrides::default(),
        );
        assert!(!matches!(
            profile.display_name.source,
            CatalogSource::Fetched { .. }
        ));
        let Some(cost) = profile.cost else {
            unreachable!("baked cost must resolve");
        };
        assert!(!matches!(cost.source, CatalogSource::Fetched { .. }));
    }

    #[test]
    fn absent_fetched_layer_is_slice1_behavior() {
        let baked = baked_with(
            "anthropic",
            "m",
            CatalogEntry {
                context_window: Some(100_000),
                ..CatalogEntry::default()
            },
        );
        let profile = resolve(
            "anthropic",
            "m",
            &baked,
            None,
            None,
            None,
            &EnvOverrides::default(),
        );
        assert_eq!(profile.context_window.value, 100_000);
        assert!(matches!(
            profile.context_window.source,
            CatalogSource::Baked { .. }
        ));
    }

    #[test]
    fn cached_catalog_round_trips_with_validators() {
        let mut catalog = Catalog::empty("unused");
        catalog.insert(
            "p",
            "m",
            CatalogEntry {
                context_window: Some(1),
                ..CatalogEntry::default()
            },
        );
        let cached = CachedCatalog {
            etag: Some(String::from("\"abc\"")),
            last_modified: None,
            retrieved: String::from("2026-08-03"),
            catalog,
        };
        let Ok(json) = cached.to_json_string() else {
            unreachable!("serialize must succeed");
        };
        let Ok(back) = CachedCatalog::from_json_str(&json) else {
            unreachable!("round-trip must parse");
        };
        assert_eq!(back.etag.as_deref(), Some("\"abc\""));
        assert_eq!(back.retrieved, "2026-08-03");
        let Some(entry) = back.catalog.entry("p", "m") else {
            unreachable!("entry survives round-trip");
        };
        assert_eq!(entry.context_window, Some(1));
    }

    #[test]
    fn transform_models_dev_applies_allowlist_and_policies() {
        let raw = serde_json::json!({
            "anthropic": { "models": { "claude-x": { "limit": { "context": 1_000_000, "output": 64_000 }, "name": "Claude X", "cost": { "input": 1.0, "output": 5.0 } } } },
            "openai": { "models": { "gpt-5.9": { "limit": { "context": 1_050_000, "output": 128_000 }, "name": "GPT-5.9", "cost": { "input": 0.0, "output": 0.0 } } } },
            "not-allowlisted": { "models": { "z": { "limit": { "context": 5 } } } }
        });
        let value = transform_models_dev(&raw, "2026-08-03");
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else {
            unreachable!("transform output must parse as a catalog");
        };
        let Some(claude) = catalog.entry("anthropic", "claude-x") else {
            unreachable!("allowlisted provider survives");
        };
        assert_eq!(claude.context_window, Some(1_000_000)); // no anthropic cap
        let Some(gpt) = catalog.entry("openai", "gpt-5.9") else {
            unreachable!("openai survives");
        };
        assert_eq!(gpt.context_window, Some(272_000)); // gpt-5.x pin applies
        assert!(gpt.cost.is_none()); // 0/0 rates filtered to null
        assert!(catalog.entry("not-allowlisted", "z").is_none());
    }

    #[test]
    fn transform_clamps_a_context_window_above_the_ceiling() {
        let raw = serde_json::json!({
            "anthropic": { "models": { "m": { "limit": { "context": 5_000_000 }, "name": "M" } } }
        });
        let value = transform_models_dev(&raw, "2026-08-03");
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else {
            unreachable!("transform output must parse as a catalog");
        };
        let Some(entry) = catalog.entry("anthropic", "m") else {
            unreachable!("entry must survive the transform");
        };
        assert_eq!(entry.context_window, Some(2_000_000));
    }

    #[test]
    fn transform_leaves_a_small_true_context_window_unclamped() {
        // Real data: nvidia's flux_1-schnell publishes exactly this
        // figure. A small real window (an embedding/image model's) is
        // honest data, not a defect — flooring it up would be distortion,
        // not defense, which is why the revised bounds (owner ruling
        // 2026-08-03, second pass) dropped the lower floor entirely.
        let raw = serde_json::json!({
            "anthropic": { "models": { "m": { "limit": { "context": 77 }, "name": "M" } } }
        });
        let value = transform_models_dev(&raw, "2026-08-03");
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else {
            unreachable!("transform output must parse as a catalog");
        };
        let Some(entry) = catalog.entry("anthropic", "m") else {
            unreachable!("entry must survive the transform");
        };
        assert_eq!(entry.context_window, Some(77));
    }

    #[test]
    fn transform_leaves_a_zero_context_window_as_the_absence_sentinel() {
        // 0 means "upstream has no data for this field" (see
        // `filtered_cost`'s zero-rate comment) — `capped_numeric`'s `min`
        // leaves it untouched, same as any other real value at or below
        // the cap.
        let raw = serde_json::json!({
            "anthropic": { "models": { "m": { "limit": { "context": 0 }, "name": "M" } } }
        });
        let value = transform_models_dev(&raw, "2026-08-03");
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else {
            unreachable!("transform output must parse as a catalog");
        };
        let Some(entry) = catalog.entry("anthropic", "m") else {
            unreachable!("entry must survive the transform");
        };
        assert_eq!(entry.context_window, Some(0));
    }

    #[test]
    fn transform_leaves_a_large_output_ceiling_unclamped() {
        // `effective_output_budget`'s `min(ceiling, DEFAULT_OUTPUT_BUDGET)`
        // already bounds a runaway ceiling's blast radius downstream, so
        // this transform must not understate a real model's capability by
        // capping it here (owner ruling 2026-08-03, second pass).
        let raw = serde_json::json!({
            "anthropic": { "models": { "m": { "limit": { "output": 999_999 }, "name": "M" } } }
        });
        let value = transform_models_dev(&raw, "2026-08-03");
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else {
            unreachable!("transform output must parse as a catalog");
        };
        let Some(entry) = catalog.entry("anthropic", "m") else {
            unreachable!("entry must survive the transform");
        };
        assert_eq!(entry.output_ceiling, Some(999_999));
    }

    #[test]
    fn transform_leaves_a_real_512k_output_ceiling_unclamped() {
        // Real data: fireworks-ai's minimax-m3 publishes exactly this
        // ceiling — the concrete case that prompted dropping the ceiling
        // cap entirely rather than raising it.
        let raw = serde_json::json!({
            "anthropic": { "models": { "m": { "limit": { "output": 512_000 }, "name": "M" } } }
        });
        let value = transform_models_dev(&raw, "2026-08-03");
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else {
            unreachable!("transform output must parse as a catalog");
        };
        let Some(entry) = catalog.entry("anthropic", "m") else {
            unreachable!("entry must survive the transform");
        };
        assert_eq!(entry.output_ceiling, Some(512_000));
    }

    #[test]
    fn transform_clamps_a_huge_cost_rate() {
        let raw = serde_json::json!({
            "anthropic": { "models": { "m": {
                "name": "M",
                "cost": { "input": 999_999.0, "output": 1.0 }
            } } }
        });
        let value = transform_models_dev(&raw, "2026-08-03");
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else {
            unreachable!("transform output must parse as a catalog");
        };
        let Some(entry) = catalog.entry("anthropic", "m") else {
            unreachable!("entry must survive the transform");
        };
        let Some(cost) = &entry.cost else {
            unreachable!("nonzero rates must survive filtered_cost");
        };
        assert!((cost.input - 1_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn transform_sanitizes_and_truncates_a_hostile_display_name() {
        let hostile = format!("\x1b[31mRed{}", "x".repeat(200));
        let raw = serde_json::json!({
            "anthropic": { "models": { "m": { "name": hostile } } }
        });
        let value = transform_models_dev(&raw, "2026-08-03");
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else {
            unreachable!("transform output must parse as a catalog");
        };
        let Some(entry) = catalog.entry("anthropic", "m") else {
            unreachable!("entry must survive the transform");
        };
        let Some(name) = &entry.display_name else {
            unreachable!("display_name must survive as a sanitized string");
        };
        assert_eq!(name.chars().count(), 64);
        assert!(!name.contains('\x1b'));
        assert!(name.starts_with("[31mRed"));
    }

    #[test]
    fn openai_gpt5_context_window_is_pinned_to_the_standard_window() {
        let raw = serde_json::json!(400_000);
        let pinned = pinned_context_window("openai", "gpt-5.4", Some(&raw));
        assert_eq!(pinned, Some(serde_json::json!(272_000)));
    }

    #[test]
    fn non_openai_context_window_passes_through_unchanged() {
        let raw = serde_json::json!(400_000);
        let passed = pinned_context_window("anthropic", "gpt-5.4", Some(&raw));
        assert_eq!(passed, Some(serde_json::json!(400_000)));
    }

    #[test]
    fn non_gpt5_openai_context_window_passes_through_unchanged() {
        let raw = serde_json::json!(400_000);
        let passed = pinned_context_window("openai", "gpt-4o", Some(&raw));
        assert_eq!(passed, Some(serde_json::json!(400_000)));
    }

    #[test]
    fn gpt5_context_window_below_the_pin_is_preserved_by_min() {
        // A natively-lower published window (e.g. a smaller gpt-5.x
        // variant) must not be raised up to the pin — min() only caps,
        // never inflates.
        let raw = serde_json::json!(128_000);
        let pinned = pinned_context_window("openai", "gpt-5-mini", Some(&raw));
        assert_eq!(pinned, Some(serde_json::json!(128_000)));
    }

    #[test]
    fn missing_context_window_passes_through_as_none() {
        assert_eq!(pinned_context_window("openai", "gpt-5.4", None), None);
    }

    #[test]
    fn cost_with_nonzero_rates_passes_through() {
        let cost = serde_json::json!({
            "input": 1.0,
            "output": 5.0,
            "cache_read": 0.1,
            "cache_write": 1.25,
        });
        let filtered = filtered_cost(Some(&cost));
        assert_eq!(
            filtered,
            Some(serde_json::json!({
                "input": 1.0,
                "output": 5.0,
                "cache_read": 0.1,
                "cache_write": 1.25,
            }))
        );
    }

    #[test]
    fn cost_with_zero_input_and_output_is_suppressed_to_null() {
        // Upstream's absence-as-zero (seen across most baked nvidia
        // entries) must not be baked forward as a fabricated $0 rate.
        let cost = serde_json::json!({ "input": 0, "output": 0 });
        assert_eq!(filtered_cost(Some(&cost)), None);
    }

    #[test]
    fn missing_cost_stays_null() {
        assert_eq!(filtered_cost(None), None);
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
