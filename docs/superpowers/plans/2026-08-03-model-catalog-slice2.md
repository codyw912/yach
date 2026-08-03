# Model Catalog Slice 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The fetched layer — runtime models.dev refresh with ETag caching under `~/.yach/catalog/`, background at session start, status-line surfacing, staleness never enforced, fully offline-safe.

**Architecture:** The models.dev→catalog transform (allowlist + policies) moves from the snapshot binary into the `yach-catalog` library so the build-time generator and the runtime refresher share one transform. A `Fetched` layer slots into `resolve()` between baked and the override layers. The CLI spawns a refresh thread at session start; the outcome rides a oneshot channel through `RunnerConfig` and surfaces once as a `StatusUpdated` event (both TUI and headless read that stream). The cache serves the NEXT resolution — startup or nothing; model-switch rehydration keeps using startup-resolved entries (documented limitation, slice 3 territory). Spec: `docs/superpowers/specs/2026-08-02-model-catalog-hydration-design.md` (fetched-layer contract); owner ruling 2026-08-03: status line + session start only, no /model-open trigger in this slice.

**Tech Stack:** Rust workspace; jj; `just dev <cmd>`; reqwest (blocking feature, new direct dep for yach-cli — already in-tree transitively via rig).

## Global Constraints

- `just dev cargo <...>` from /Users/cody/dev/yach. Strict clippy (`-D warnings`); house test idiom (`let Ok/Some(x) = … else { unreachable!("…") }`, never `.unwrap()`); no `panic!` in tests; `#[expect]` over `#[allow]`; fmt `--check` clean before every commit.
- NO network I/O in any test, and none in the yach-catalog library (the fetch lives in yach-cli; the library gains only pure transform/cache types).
- The offline invariant is absolute: a failed, slow, or absent fetch changes nothing about session behavior except one status line. Sessions never wait on the refresh.
- Trust boundary: fetched data reaches evidence only through provenance labels (`fetched:<date>`); the baked layer remains the release floor beneath it.
- Behavior floor unchanged: no cache file + fetch failing = byte-identical slice-1 behavior.
- `jj commit -m "..."` per task; no AI attribution; `jj st` only intended files.

---

### Task 1: Transform promotion, `Fetched` layer, and cache types (yach-catalog)

**Files:**
- Modify: `crates/yach-catalog/src/lib.rs`, `crates/yach-catalog/src/bin/snapshot.rs`

**Interfaces (produced; Task 2 consumes):**
```rust
pub fn transform_models_dev(raw: &serde_json::Value, snapshot_date: &str) -> serde_json::Value; // the allowlist+policy transform, moved from the bin
pub enum CatalogSource { Baked { snapshot_date: String }, Fetched { retrieved: String }, Override { scope: OverrideScope }, EnvOverride, Default }
pub struct CachedCatalog { pub etag: Option<String>, pub last_modified: Option<String>, pub retrieved: String, pub catalog: Catalog }
impl CachedCatalog { pub fn from_json_str(...) -> Result<...>; pub fn to_json_string(&self) -> Result<String, serde_json::Error>; }
// resolve() gains a fetched parameter between baked and the overrides —
// the catalog plus its RETRIEVED date (the cache's date, not the
// catalog's snapshot_date, which is meaningless for fetched data):
pub fn resolve(provider, model, baked, fetched: Option<(&Catalog, &str)>, user, project, env) -> ModelProfile;
```

- [ ] **Step 1: Failing tests first (RED)**

Add to the existing test module (house idiom throughout):

```rust
    #[test]
    fn fetched_layer_beats_baked_and_loses_to_overrides() {
        let baked = baked_with("anthropic", "m", CatalogEntry { context_window: Some(100_000), ..CatalogEntry::default() });
        let mut fetched = Catalog::empty("unused");
        fetched.insert("anthropic", "m", CatalogEntry { context_window: Some(150_000), output_ceiling: Some(40_000), ..CatalogEntry::default() });
        let Ok(project) = Overrides::from_toml_str("[anthropic.m]\ncontext_window = 160000\n") else { unreachable!("fixture toml must parse") };
        let profile = resolve("anthropic", "m", &baked, Some((&fetched, "2026-08-03")), None, Some(&project), &EnvOverrides::default());
        // project override wins where it speaks…
        assert_eq!(profile.context_window.value, 160_000);
        // …fetched beats baked where the override is silent.
        assert_eq!(profile.output_ceiling.value, 40_000);
        assert!(matches!(&profile.output_ceiling.source, CatalogSource::Fetched { retrieved } if retrieved == "2026-08-03"));
    }

    #[test]
    fn absent_fetched_layer_is_slice1_behavior() {
        let baked = baked_with("anthropic", "m", CatalogEntry { context_window: Some(100_000), ..CatalogEntry::default() });
        let profile = resolve("anthropic", "m", &baked, None, None, None, &EnvOverrides::default());
        assert_eq!(profile.context_window.value, 100_000);
        assert!(matches!(profile.context_window.source, CatalogSource::Baked { .. }));
    }

    #[test]
    fn cached_catalog_round_trips_with_validators() {
        let mut catalog = Catalog::empty("unused");
        catalog.insert("p", "m", CatalogEntry { context_window: Some(1), ..CatalogEntry::default() });
        let cached = CachedCatalog { etag: Some(String::from("\"abc\"")), last_modified: None, retrieved: String::from("2026-08-03"), catalog };
        let Ok(json) = cached.to_json_string() else { unreachable!("serialize must succeed") };
        let Ok(back) = CachedCatalog::from_json_str(&json) else { unreachable!("round-trip must parse") };
        assert_eq!(back.etag.as_deref(), Some("\"abc\""));
        assert_eq!(back.retrieved, "2026-08-03");
        let Some(entry) = back.catalog.entry("p", "m") else { unreachable!("entry survives round-trip") };
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
        let Ok(catalog) = Catalog::from_json_str(&value.to_string()) else { unreachable!("transform output must parse as a catalog") };
        let Some(claude) = catalog.entry("anthropic", "claude-x") else { unreachable!("allowlisted provider survives") };
        assert_eq!(claude.context_window, Some(1_000_000)); // no anthropic cap
        let Some(gpt) = catalog.entry("openai", "gpt-5.9") else { unreachable!("openai survives") };
        assert_eq!(gpt.context_window, Some(272_000)); // gpt-5.x pin applies
        assert!(gpt.cost.is_none()); // 0/0 rates filtered to null
        assert!(catalog.entry("not-allowlisted", "z").is_none());
    }
```

NOTE the `resolve` signature in these tests: the fetched parameter is `Option<(&Catalog, &str)>` — the catalog plus its retrieved date (the cache's `retrieved`, NOT the catalog's own snapshot_date, which is meaningless for fetched data). Adjust the interface block accordingly; this is deliberate: provenance must carry when the data was retrieved.

- [ ] **Step 2: Verify RED** (`just dev cargo test -p yach-catalog` — compile failures expected).

- [ ] **Step 3: Implement**

- Move `PROVIDER_ALLOWLIST`, `pinned_context_window`, `filtered_cost`, and the per-model entry construction from `bin/snapshot.rs` into lib.rs as `transform_models_dev(raw, snapshot_date) -> serde_json::Value` (same output shape the bin writes today, including `snapshot_date` and `source` fields). The bin becomes: parse args, read file, parse JSON, call the library, write output — its own `#[cfg(test)]` transform tests MOVE to lib.rs beside the function (keep every case; the bin keeps only an args-handling smoke test if it has one).
- Add `Fetched { retrieved: String }` to `CatalogSource`.
- Add `CachedCatalog` (derive Serialize + Deserialize; catalog field uses the existing `Catalog` Deserialize — add `Serialize` to `Catalog`/`ProviderEntry`/`CatalogEntry` derives, which is additive).
- Extend `resolve` with the fetched layer between baked and user/user-project layers, per-field, zero-filtered like the others: precedence env > project > user > fetched > baked > default. The per-field walker gains one rung; keep the existing closure/helper shape (split if clippy complexity trips).

- [ ] **Step 4: GREEN + workspace impact**

`just dev cargo test -p yach-catalog` green, then `just dev cargo check --workspace` — the resolve() call sites in yach-cli break (new parameter): fix them in this task by passing `None` (Task 2 wires the real cache), so the workspace compiles at every commit. Full workspace test + clippy + fmt.

- [ ] **Step 5: Commit**

```bash
jj commit -m "feat: fetched catalog layer, shared models.dev transform, cache types"
```

---

### Task 2: The refresher — fetch, cache, status surfacing, resolution wiring (yach-cli + runner seam)

**Files:**
- Modify: `crates/yach-cli/Cargo.toml` (add `reqwest = { version = "0.12", default-features = false, features = ["blocking", "rustls-tls"] }` — match the workspace's TLS posture; check what rig uses and mirror it; adjust version to what resolves cleanly with the tree)
- Create: `crates/yach-cli/src/catalog_refresh.rs`
- Modify: `crates/yach-cli/src/main.rs` (module decl, spawn at both session-start sites, cache into `resolve_model_profile`/`ModelOverrideLayers`)
- Modify: `crates/yach-backend/src/runner.rs` (`RunnerConfig` gains the outcome receiver; the loop emits one StatusUpdated)

**Interfaces:**
- Consumes: Task 1's `transform_models_dev`, `CachedCatalog`, `resolve` with fetched param.
- Produces: `catalog_refresh::spawn_refresh() -> std::sync::mpsc::Receiver<RefreshOutcome>`; `pub enum RefreshOutcome { Refreshed { models: usize, retrieved: String }, NotModified, Failed { fallback: String } }`; `RunnerConfig.catalog_refresh: Option<std::sync::mpsc::Receiver<RefreshOutcome>>`.

- [ ] **Step 1: The refresher module**

`crates/yach-cli/src/catalog_refresh.rs` — the network seam is one small function so everything else is pure and testable:

```rust
//! Runtime models.dev refresh: the fetched catalog layer. Background
//! only — sessions never wait on this; a failure costs one status line
//! and the baked/cached data serves (spec: offline invariant).

use std::path::PathBuf;
use yach_catalog::{CachedCatalog, transform_models_dev};

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

pub enum RefreshOutcome {
    Refreshed { models: usize, retrieved: String },
    NotModified,
    Failed { fallback: String },
}

pub fn cache_path() -> Option<PathBuf> { /* ~/.yach/catalog/models-dev.json via the same HOME mechanism as resolve_model_profile */ }

pub fn load_cache() -> Option<CachedCatalog> { /* read + parse; malformed -> None with a stderr warning, mirroring load_model_overrides */ }

/// One conditional fetch. Sends If-None-Match/If-Modified-Since from the
/// cache's validators; 304 refreshes checked-at only; 200 transforms and
/// rewrites the cache. Timeout hard-capped (10s connect+read) — this runs
/// on a background thread, but a hung fetch should still die promptly.
fn fetch_once(existing: Option<&CachedCatalog>) -> RefreshOutcome { /* reqwest::blocking with .timeout(std::time::Duration::from_secs(10)) */ }

/// Today's UTC date as YYYY-MM-DD, no date dependency: civil-from-days
/// (Howard Hinnant's algorithm) over SystemTime. Pure; unit-tested
/// against known epochs (0 -> 1970-01-01; 1_722_643_200 -> 2024-08-03).
fn utc_date_from_epoch_secs(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// Spawn the background refresh; the receiver rides RunnerConfig and the
/// runner emits one status line when the outcome arrives.
pub fn spawn_refresh() -> std::sync::mpsc::Receiver<RefreshOutcome> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let existing = load_cache();
        let _ = tx.send(fetch_once(existing.as_ref()));
    });
    rx
}
```

Structure the module so `fetch_once`'s HTTP call is isolated: parse/transform/cache-write logic lives in pure functions (`apply_success_response(body: &str, existing, now_date) -> (RefreshOutcome, Option<CachedCatalog-to-write>)` etc.) unit-tested WITHOUT any network; `fetch_once` itself is a thin untested shell (state this boundary in the module docs). The status strings: `catalog refreshed ({models} models, models.dev {retrieved})`, `catalog up to date`, `catalog fetch failed; serving {fallback}` where fallback names the best available layer (`cached 2026-08-03` or `baked 2026-08-02 snapshot`).

- [ ] **Step 2: Runner seam**

`RunnerConfig` gains `pub catalog_refresh: Option<std::sync::mpsc::Receiver<RefreshOutcome>>`… but RefreshOutcome is a CLI type and backend must not depend on the CLI. Invert: the field is `Option<std::sync::mpsc::Receiver<String>>` — the CLI formats the status string, the backend just emits it. In the runner loop, poll it non-blockingly once per loop iteration until it yields (try_recv; after the first message or a Disconnected, stop polling — a small `Option` you `.take()`). Emit `ServerEvent::StatusUpdated { message }`. Find the loop's existing periodic point (where other per-iteration work happens near the select) and add it there; if the loop is purely event-driven with no periodic tick, emit instead by converting the receiver into the loop's channel world at spawn time in the CLI (send the formatted string through a task that forwards into the existing event channel) — read the loop first, pick the shape that fits it, and DOCUMENT which you chose in your report. Both TUI and headless read the same event stream, so one seam covers both.

- [ ] **Step 3: Resolution wiring**

`resolve_model_profile` / `ModelOverrideLayers` (main.rs): load the cache once per invocation alongside the override files; pass `Some((&cached.catalog, &cached.retrieved))` into every `resolve(...)` call (the ones Task 1 stubbed with `None`). The refresh spawn happens at both session-start sites (TUI + headless) BEFORE config resolution, but resolution does NOT wait for it — it reads whatever cache already exists (the refresh feeds the next session). Order note in code comment.

- [ ] **Step 4: Tests**

- Pure: success-response application (fresh cache written, outcome counts models); 304 application (cache kept, `NotModified`); malformed body (Failed, cache untouched); the date helper.
- Resolution: with a cache fixture present via `ModelOverrideLayers` (construct directly — no filesystem), fetched beats baked and loses to project override (glue-level, mirrors the crate test).
- Offline: `load_cache()` on a missing path is `None` and resolution equals slice-1 behavior (reuse the floor test pattern).
- Runner: config with a pre-loaded receiver emits exactly one StatusUpdated with the given string (construct the channel, send, run the loop's poll path or forwarding task per the shape you chose).

- [ ] **Step 5: Full workspace gate; commit**

```bash
jj commit -m "feat: background models.dev refresh with ETag cache and status surfacing"
```

---

### Task 3: Verification (controller + owner)

- [ ] Workspace audit: `grep -rn "reqwest" crates/yach-cli/src/` — network calls exist ONLY in catalog_refresh.rs's fetch shell. `grep -rn "models.dev\|MODELS_DEV" crates/yach-catalog/src/` — the library carries no URL (transform is pure; the URL lives in the CLI).
- [ ] One live manual check (controller, network on): run the refresher once via a scratch invocation or `yach` session start; confirm `~/.yach/catalog/models-dev.json` appears with etag + retrieved; run again and confirm `catalog up to date` (304 path) in the status stream.
- [ ] Offline check: move the cache aside, disable network (or point MODELS_DEV_URL override if one exists — do NOT add one just for this), start a session: one `catalog fetch failed; serving baked …` status, session behaves identically.
- [ ] `just runtime-image` + gate + 125-cell sweep vs the 2026-08-03 reference (124/125 launched). The sweep runs in containers with network; cells will exercise the refresh path incidentally — cell stderr should show catalog status lines without affecting rewards.
- [ ] Record + board (slice 2 → MEASURED; the fetched provenance label appearing in any outcome document is worth quoting).
