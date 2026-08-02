# Model-Catalog Hydration

**Date:** 2026-08-02
**Status:** In review
**Prior work:** the five stopgaps marked "model-catalog revisit" in
code and next.md (`max_tokens` 32k default, `context_window` 200k
default, `YACH_RIG_PROVIDER_MAX_TOKENS_PARAM`, the curated
`ANTHROPIC_MODEL_CHOICES` list, the flagged truncated-tool-call
recovery), plus cost reporting and the error-dialect home. Board:
"Model-catalog hydration design; unblocks five stopgaps."

## Problem

yach carries per-model constants as env-var stopgaps with global
defaults. Every one asks the operator to know an API detail the
harness should know: what a model's context window is, what its
output ceiling is, which parameter spelling its provider wants. The
context meter, compaction accounting, the `/model` picker, and yacht
evidence all consume these numbers; today they come from defaults
that are wrong for everything except the models they were tuned on.

## Cohort evidence (verified in clones, 2026-08-02)

The anchor cohort is the multi-provider trio — pi, omp, opencode.
Codex and Claude Code carry single-provider catalogs and are not
anchors here (owner direction).

- **Nobody is purely static, nobody is purely fetched.** pi bakes a
  `models.generated.ts` produced from models.dev at build time AND
  runs per-provider `refreshModels()` hooks with a persistent
  `ModelsStore` (ETag/If-None-Match, Last-Modified, checkedAt). omp
  breaks catalog into its own package with a documented four-layer
  precedence — static -> stencil.so -> cache -> dynamic — over a
  bundled 2.1MB `models.json`, with cost computation inside the
  catalog. opencode fetches models.dev at runtime.
- **The convergent shape:** a baked snapshot as the floor, layered
  runtime sources above it. The forks are only how many layers and
  who is trusted.
- **Provider `/models` APIs are thin** — IDs, mostly no windows or
  costs. That is why models.dev exists, and why the layers do
  different jobs rather than competing.

## Owner decisions (2026-08-02)

1. **Baked + override alone is insufficient** — models must update
   without cutting a release and without user intervention.
2. **Four layers, jobs split:** baked snapshot (release floor) ->
   models.dev refresh (metadata freshness) -> provider-API discovery
   (key-truthful existence) -> user override (corrections), with env
   vars as the final explicit override. Precedence is per-field, not
   per-model.
3. **In scope now:** per-field provenance and cost rates/reporting.
4. **Deferred:** the quirks-as-data overlay — it is data with no
   consumer, its taxonomy should be shaped by its first consumer,
   and it is the only layer with no external anchor (it couples to
   the rotation workflow). Deferral is free: the schema tolerates
   unknown fields and the resolved type is extensible, so the
   overlay is purely additive later. Trigger: it rides in with its
   first consumer.

## Design

### Home: a new `yach-catalog` crate

Mirrors omp's dedicated package. Owns the snapshot data, the layer
types, resolution, and provenance. The backend consumes one seam:

```rust
pub struct ModelProfile {
    pub context_window: Sourced<u64>,
    pub max_output_tokens: Sourced<u64>,
    pub max_tokens_param: Sourced<MaxTokensParam>,
    pub display_name: Sourced<String>,
    pub cost: Option<Sourced<CostRates>>, // per-million: input, output, cache_read
    // schema tolerates unknown fields; additive extension is free
}

pub struct Sourced<T> {
    pub value: T,
    pub source: CatalogSource, // Baked { snapshot_date } | Fetched { retrieved } | Discovered | Override | EnvOverride | Default
}

pub fn resolve(provider: &str, model: &str, layers: &Layers) -> ModelProfile;
```

`Default` is the source for the current global fallbacks when no
layer knows the model — the stopgap numbers survive as the
bottom-most floor so behavior never regresses, but their provenance
is now visible instead of silent.

### Layer 1: baked snapshot

Generated from models.dev's published JSON by a `just
catalog-snapshot` recipe: filtered to providers yach can drive,
transformed to the catalog schema, committed to the repo as data and
reviewed in PRs like any change. Trust boundary: a release ships
repo-reviewed data only. The generation script records the models.dev
snapshot date into the data.

### Layer 2: fetched refresh

Runtime fetch of the same models.dev document with pi's cache
mechanics: ETag/If-None-Match, Last-Modified, cached under
`~/.yach/catalog/`. Checked in the background at session start and on
`/model` open. Sessions NEVER block on the network; a failed or slow
refresh means the baked/cached data serves, and staleness is shown
(catalog age in `/model`), never enforced.

### Layer 3: provider discovery

Per-configured-provider `/models` calls feed existence only: the
`/model` picker lists what the operator's key can actually use,
intersected with catalog metadata where known. Discovery never
overwrites metadata fields. Retires `ANTHROPIC_MODEL_CHOICES`.

### Layer 4: user override + env

`~/.yach/models.toml` (user) and `.yach/models.toml` (project) for
corrections and unlisted models; the existing `YACH_RIG_PROVIDER_*`
env vars become explicit per-run overrides above the files. Env
semantics are unchanged for anyone setting them today — what retires
is their role as the only source.

### Provenance (yach-native)

Every resolved field carries `CatalogSource`. Evidence-bearing
consumers record it: the outcome document's config/usage block says
which layer supplied the context window and cost rates, so yacht
evidence and cost reports are auditable to their data source. No
cohort harness does this.

### Cost reporting (yach-native scope)

`CostRates` resolve through the same layers; per-session cost =
provider-reported usage x rates, landing in the outcome document
beside usage with the meter's honesty rules: unreported usage means
no cost claim (never a fabricated zero), and unknown rates mean
`cost: unknown`, never silence.

### Stopgap retirements

| stopgap | disposition |
|---|---|
| `YACH_RIG_PROVIDER_MAX_TOKENS` 32k default | catalog lookup; env survives as override |
| `YACH_RIG_PROVIDER_CONTEXT_WINDOW` 200k default | catalog lookup; env survives as override |
| `YACH_RIG_PROVIDER_MAX_TOKENS_PARAM` | catalog per-provider/model data; env survives as override |
| `ANTHROPIC_MODEL_CHOICES` curated list | discovery + catalog display names |
| truncated-tool-call recovery ceiling data | catalog supplies the ceilings; the recovery behavior remains its own item |

Error dialects: the catalog carves the per-provider `error_dialect`
home (schema field, data optional); the tiered classifier remains its
own slated design item.

## Validation

1. Unit tests on resolution precedence (every layer pairing) and
   provenance propagation; snapshot-generation round-trip test.
2. Workspace gates (strict clippy, full suite) per slice.
3. The 125-cell sweep after each slice that touches the request path
   (slice 1 changes output budgets/windows in flight): reference is
   the current 125/125. Cost numbers spot-checked against hand
   computation from provider-reported usage.
4. Slice 2 adds an offline test: refresh unavailable -> baked/cached
   data serves, no session impact.

## Risks

- **models.dev schema drift** breaks the generation recipe or the
  runtime fetch. Mitigated: the baked layer means drift can never
  break a session, only freshness; the recipe is a build-time tool
  whose failure is loud.
- **Wrong community data flowing into evidence.** Mitigated by
  provenance (the evidence says where the number came from) and the
  override layer; the baked layer is repo-reviewed.
- **Slice 1 size.** A new crate plus five consumer rewires is the
  largest slice since the tool-call work; the sweep gates it.

## Non-goals

- Quirks-as-data overlay (deferred; trigger = first consumer).
- The `/connect`-style provider/model product surface (separate
  slated item; this catalog is what it will read from).
- The tiered error-dialect classifier (home carved, design separate).
- Provider-native compaction (separate queued item).
- No new network dependency for correctness: every session outcome
  must be reachable with the network off.

## Future consideration: roles and subagents above the catalog

Owner-flagged (2026-08-02): omp's role/subagent mechanism —
`modelRoles` mapping role names to model references (with
user-definable extras via `modelTags`), and frontmatter agent
definitions whose `model` field references a role — has proven good
in practice, and yach may want extensions to contribute roles and
subagents the same way. This spec deliberately keeps that future
cheap without designing it: model identity stays plain
`(provider, model)` strings and `resolve()` stays a pure
data-in/data-out lookup, so a role layer is a thin mapping on top of
the catalog, never a change to it. The role/subagent design itself
belongs to the extension-seam and provider/model product-surface
items.

## Slices

1. **Crate + baked + override + consumers.** `yach-catalog`, the
   snapshot recipe and committed data, override files + env
   precedence, the five consumers rewired, cost + provenance in the
   outcome document. Sweep as regression gate.
2. **Fetched refresh.** ETag cache, background check, staleness
   display, offline test.
3. **Discovery + picker.** Per-provider `/models`, key-truthful
   `/model` list.

Each slice lands separately with its own measurement, per the
established pattern.
