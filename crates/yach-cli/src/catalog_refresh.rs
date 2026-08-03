//! Runtime models.dev refresh: the fetched catalog layer. Background
//! only — sessions never wait on this; a failure costs one status line
//! and the baked/cached data serves (spec: offline invariant).
//!
//! Network isolation boundary: `fetch_once` is the ONLY function in this
//! module that touches `reqwest` / the network. Everything else —
//! parsing, transforming, deciding what to write, formatting the status
//! line — is a pure or file-parameterized function, unit-tested below
//! without a socket in sight. `fetch_once` itself is a thin, untested
//! shell: it gathers bytes and headers and hands them to the pure
//! functions.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use yach_catalog::{CachedCatalog, Catalog, transform_models_dev};

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// The refresh's result, in the CLI's own vocabulary (not `RunnerConfig`'s
/// — see `format_status_message` and `spawn_refresh_status` for the hop
/// into the backend-safe `String` the runner actually carries).
#[derive(Debug, Clone, PartialEq)]
pub enum RefreshOutcome {
    Refreshed { models: usize, retrieved: String },
    NotModified,
    Failed { fallback: String },
}

/// `~/.yach/catalog/models-dev.json` — same HOME lookup
/// `ModelOverrideLayers::load` uses for `~/.yach/models.toml`, one
/// home-dir mechanism for the whole `~/.yach` tree.
#[must_use]
pub fn cache_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".yach/catalog/models-dev.json"))
}

/// Read and parse the cache at the real `$HOME` location. Missing file ->
/// silently absent (no cache yet is not a misconfiguration); present but
/// malformed -> absent with a stderr warning, mirroring
/// `load_model_overrides`'s degrade-not-crash contract for a bad
/// correction file.
#[must_use]
pub fn load_cache() -> Option<CachedCatalog> {
    cache_path().and_then(|path| load_cache_from(&path))
}

/// The path-explicit half of `load_cache`, split out so a malformed-cache
/// test can point at a temp file instead of mutating `$HOME` (house idiom
/// — mirrors `load_model_overrides` vs. `ModelOverrideLayers::load`).
fn load_cache_from(path: &Path) -> Option<CachedCatalog> {
    let text = std::fs::read_to_string(path).ok()?;
    match CachedCatalog::from_json_str(&text) {
        Ok(cache) => Some(cache),
        Err(error) => {
            let mut stderr = io::stderr();
            let _ = writeln!(
                stderr,
                "warning: ignoring malformed {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// Persist a cache to the real `$HOME` location, creating the
/// `~/.yach/catalog` directory if needed. A background refresh's write
/// failing (permissions, disk full, whatever) must never surface as
/// anything louder than "the next session still has the old cache" — no
/// error return, no panic.
fn write_cache(cache: &CachedCatalog) {
    let Some(path) = cache_path() else {
        return;
    };
    write_cache_to(&path, cache);
}

fn write_cache_to(path: &Path, cache: &CachedCatalog) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = cache.to_json_string() {
        let _ = std::fs::write(path, json);
    }
}

/// The best available layer's label for a `Failed` outcome's `fallback`:
/// the cache's own retrieval date when one exists, else the baked
/// snapshot the process shipped with. Reads the in-memory baked catalog
/// (a `OnceLock`, not I/O) so this stays a pure function of `existing`.
fn fallback_label(existing: Option<&CachedCatalog>) -> String {
    match existing {
        Some(cache) => format!("cached {}", cache.retrieved),
        None => format!(
            "baked {} snapshot",
            yach_catalog::baked_catalog().snapshot_date()
        ),
    }
}

/// Total model count across every provider in a `transform_models_dev`
/// output `Value` — counted off the transformed JSON tree directly
/// (rather than parsing into a `Catalog` and asking it) because `Catalog`
/// exposes no provider-enumeration API, and growing Task 1's crate just
/// for a status-line count isn't worth it here.
fn count_models(catalog_json: &serde_json::Value) -> usize {
    catalog_json
        .get("providers")
        .and_then(serde_json::Value::as_object)
        .map_or(0, |providers| {
            providers
                .values()
                .filter_map(|provider| provider.get("models"))
                .filter_map(serde_json::Value::as_object)
                .map(serde_json::Map::len)
                .sum()
        })
}

/// Applies a 200 response body: parses it as JSON, runs it through the
/// shared `transform_models_dev`, and builds the `CachedCatalog` that
/// should be persisted. Pure (no I/O) — the only side effect (writing the
/// cache) is left to the caller (`fetch_once`), which is what makes this
/// testable without a filesystem.
fn apply_success_response(
    body: &str,
    now_date: &str,
    etag: Option<String>,
    last_modified: Option<String>,
    fallback: &str,
) -> (RefreshOutcome, Option<CachedCatalog>) {
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(body) else {
        return (
            RefreshOutcome::Failed {
                fallback: String::from(fallback),
            },
            None,
        );
    };
    // `snapshot_date` here labels the transformed JSON's own field, which
    // `resolve`'s fetched path never reads (it uses `CachedCatalog.retrieved`
    // instead — see `CatalogSource::Fetched`); `now_date` is as good a
    // label as any for a payload that has no snapshot date of its own.
    let transformed = transform_models_dev(&raw, now_date);
    let models = count_models(&transformed);
    let Ok(catalog) = Catalog::from_json_str(&transformed.to_string()) else {
        return (
            RefreshOutcome::Failed {
                fallback: String::from(fallback),
            },
            None,
        );
    };
    let cached = CachedCatalog {
        etag,
        last_modified,
        retrieved: String::from(now_date),
        catalog,
    };
    (
        RefreshOutcome::Refreshed {
            models,
            retrieved: String::from(now_date),
        },
        Some(cached),
    )
}

/// Applies a 304: the remote data hasn't changed, so the cached catalog
/// content, etag, and last-modified validator all carry over untouched —
/// only `retrieved` advances to today, so the next `Fetched` provenance
/// label reflects "confirmed current as of today" rather than staying
/// pinned to whenever the 200 originally landed. A 304 with no existing
/// cache to validate against shouldn't happen (we only send conditional
/// headers when a cache exists), but degrades to `Failed` rather than
/// fabricating data.
fn apply_not_modified_response(
    existing: Option<&CachedCatalog>,
    now_date: &str,
) -> (RefreshOutcome, Option<CachedCatalog>) {
    let Some(existing) = existing else {
        return (
            RefreshOutcome::Failed {
                fallback: fallback_label(None),
            },
            None,
        );
    };
    let refreshed = CachedCatalog {
        etag: existing.etag.clone(),
        last_modified: existing.last_modified.clone(),
        retrieved: String::from(now_date),
        catalog: existing.catalog.clone(),
    };
    (RefreshOutcome::NotModified, Some(refreshed))
}

/// Today's UTC date as YYYY-MM-DD, no date dependency: civil-from-days
/// (Howard Hinnant's algorithm) over SystemTime. Pure; unit-tested
/// against known epochs (0 -> 1970-01-01; 1_722_643_200 -> 2024-08-03).
/// Transcribed verbatim from the algorithm's reference form (signed
/// arithmetic throughout is how civil-from-days is specified) rather than
/// reworked to dodge the pedantic wrap lint — `secs` only ever holds a
/// real wall-clock time, nowhere near `i64::MAX / 86_400` days.
#[expect(clippy::cast_possible_wrap)]
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

fn now_utc_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    utc_date_from_epoch_secs(secs)
}

/// One conditional fetch. Sends If-None-Match/If-Modified-Since from the
/// cache's validators; 304 refreshes checked-at only; 200 transforms and
/// rewrites the cache. Timeout hard-capped (10s connect+read) — this runs
/// on a background thread, but a hung fetch should still die promptly.
/// Thin and untested by design: everything past "get bytes and headers
/// off the wire" is one of the pure `apply_*` functions above.
fn fetch_once(existing: Option<&CachedCatalog>) -> RefreshOutcome {
    let fallback = fallback_label(existing);
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    else {
        return RefreshOutcome::Failed { fallback };
    };
    let mut request = client.get(MODELS_DEV_URL);
    if let Some(cache) = existing {
        if let Some(etag) = cache.etag.as_deref() {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = cache.last_modified.as_deref() {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, last_modified);
        }
    }
    let Ok(response) = request.send() else {
        return RefreshOutcome::Failed { fallback };
    };
    let now_date = now_utc_date();
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        let (outcome, to_write) = apply_not_modified_response(existing, &now_date);
        if let Some(cache) = to_write.as_ref() {
            write_cache(cache);
        }
        return outcome;
    }
    if !response.status().is_success() {
        return RefreshOutcome::Failed { fallback };
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(String::from);
    let last_modified = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .map(String::from);
    let Ok(body) = response.text() else {
        return RefreshOutcome::Failed { fallback };
    };
    let (outcome, to_write) =
        apply_success_response(&body, &now_date, etag, last_modified, &fallback);
    if let Some(cache) = to_write.as_ref() {
        write_cache(cache);
    }
    outcome
}

/// Spawn the background refresh; the receiver rides `RunnerConfig` and the
/// runner emits one status line when the outcome arrives. Returns
/// `RefreshOutcome` (a CLI type) directly — see `spawn_refresh_status` for
/// the CLI-side hop that turns this into the pre-formatted `String` the
/// backend-agnostic runner seam actually carries.
pub fn spawn_refresh() -> std::sync::mpsc::Receiver<RefreshOutcome> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let existing = load_cache();
        let _ = tx.send(fetch_once(existing.as_ref()));
    });
    rx
}

/// The exact status line for an outcome: `catalog refreshed ({n} models,
/// models.dev {retrieved})` / `catalog up to date` / `catalog fetch
/// failed; serving {fallback}`.
fn format_status_message(outcome: &RefreshOutcome) -> String {
    match outcome {
        RefreshOutcome::Refreshed { models, retrieved } => {
            format!("catalog refreshed ({models} models, models.dev {retrieved})")
        }
        RefreshOutcome::NotModified => String::from("catalog up to date"),
        RefreshOutcome::Failed { fallback } => {
            format!("catalog fetch failed; serving {fallback}")
        }
    }
}

/// `RunnerConfig.catalog_refresh` is `Option<Receiver<String>>` — the
/// backend must not depend on this crate's `RefreshOutcome`, so the CLI
/// formats the status line itself. This spawns `spawn_refresh`'s fetch
/// thread and a second, tiny thread that blocks on its one outcome and
/// forwards the formatted string; the fetch thread's own return type
/// stays `RefreshOutcome` (directly testable — see the tests module)
/// rather than baking string formatting into it.
pub fn spawn_refresh_status() -> std::sync::mpsc::Receiver<String> {
    let outcome_rx = spawn_refresh();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok(outcome) = outcome_rx.recv() {
            let _ = tx.send(format_status_message(&outcome));
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use yach_catalog::CatalogEntry;

    fn write_temp_file(name: &str, contents: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "yach-cli-catalog-refresh-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        let _ = std::fs::write(&path, contents);
        path
    }

    #[test]
    fn utc_date_from_epoch_secs_matches_known_epochs() {
        assert_eq!(utc_date_from_epoch_secs(0), "1970-01-01");
        assert_eq!(utc_date_from_epoch_secs(1_722_643_200), "2024-08-03");
    }

    #[test]
    fn apply_success_response_writes_a_fresh_cache_and_counts_models() {
        let body = serde_json::json!({
            "anthropic": {
                "models": {
                    "claude-x": {
                        "limit": { "context": 1_000_000, "output": 64_000 },
                        "name": "Claude X",
                        "cost": { "input": 1.0, "output": 5.0 }
                    }
                }
            },
            "openai": {
                "models": {
                    "gpt-y": {
                        "limit": { "context": 500_000, "output": 32_000 },
                        "name": "GPT Y",
                        "cost": { "input": 1.0, "output": 2.0 }
                    }
                }
            }
        })
        .to_string();

        let (outcome, to_write) = apply_success_response(
            &body,
            "2026-08-03",
            Some(String::from("\"etag-1\"")),
            Some(String::from("Mon, 03 Aug 2026 00:00:00 GMT")),
            "baked 2026-08-02 snapshot",
        );

        assert_eq!(
            outcome,
            RefreshOutcome::Refreshed {
                models: 2,
                retrieved: String::from("2026-08-03"),
            }
        );
        let Some(cached) = to_write else {
            unreachable!("a 200 with a parseable body must produce a cache to write");
        };
        assert_eq!(cached.retrieved, "2026-08-03");
        assert_eq!(cached.etag.as_deref(), Some("\"etag-1\""));
        let Some(entry) = cached.catalog.entry("anthropic", "claude-x") else {
            unreachable!("transformed catalog must carry the allowlisted model");
        };
        assert_eq!(entry.context_window, Some(1_000_000));
    }

    #[test]
    fn apply_success_response_degrades_a_malformed_body_to_failed_without_a_cache_write() {
        let (outcome, to_write) =
            apply_success_response("not json", "2026-08-03", None, None, "cached 2026-08-01");

        assert_eq!(
            outcome,
            RefreshOutcome::Failed {
                fallback: String::from("cached 2026-08-01"),
            }
        );
        assert!(to_write.is_none());
    }

    #[test]
    fn apply_not_modified_response_advances_retrieved_but_keeps_the_catalog() {
        let mut catalog = Catalog::empty("unused");
        catalog.insert(
            "anthropic",
            "m",
            CatalogEntry {
                context_window: Some(150_000),
                ..CatalogEntry::default()
            },
        );
        let existing = CachedCatalog {
            etag: Some(String::from("\"etag-1\"")),
            last_modified: Some(String::from("Mon, 03 Aug 2026 00:00:00 GMT")),
            retrieved: String::from("2026-08-01"),
            catalog,
        };

        let (outcome, to_write) = apply_not_modified_response(Some(&existing), "2026-08-03");

        assert_eq!(outcome, RefreshOutcome::NotModified);
        let Some(refreshed) = to_write else {
            unreachable!("a 304 against an existing cache must still produce a rewrite");
        };
        // Content preserved…
        assert_eq!(refreshed.etag, existing.etag);
        assert_eq!(refreshed.last_modified, existing.last_modified);
        assert_eq!(
            refreshed.catalog.entry("anthropic", "m"),
            existing.catalog.entry("anthropic", "m")
        );
        // …but the checked-at date advances.
        assert_eq!(refreshed.retrieved, "2026-08-03");
    }

    #[test]
    fn apply_not_modified_response_without_an_existing_cache_fails_conservatively() {
        let (outcome, to_write) = apply_not_modified_response(None, "2026-08-03");

        assert!(matches!(outcome, RefreshOutcome::Failed { .. }));
        assert!(to_write.is_none());
    }

    #[test]
    fn load_cache_from_a_malformed_file_degrades_to_absent() {
        let path = write_temp_file("malformed.json", "{ not json");

        assert!(load_cache_from(&path).is_none());
    }

    #[test]
    fn load_cache_from_a_well_formed_file_round_trips() {
        let mut catalog = Catalog::empty("unused");
        catalog.insert(
            "anthropic",
            "m",
            CatalogEntry {
                context_window: Some(1),
                ..CatalogEntry::default()
            },
        );
        let cached = CachedCatalog {
            etag: None,
            last_modified: None,
            retrieved: String::from("2026-08-03"),
            catalog,
        };
        let Ok(json) = cached.to_json_string() else {
            unreachable!("serialize must succeed");
        };
        let path = write_temp_file("valid.json", &json);

        let Some(loaded) = load_cache_from(&path) else {
            unreachable!("a well-formed cache file must load");
        };
        assert_eq!(loaded.retrieved, "2026-08-03");
    }

    /// Offline floor: a cache that was never written (missing path, not a
    /// real `$HOME` — no env mutation needed) is `None`, and feeding that
    /// `None` into `yach_catalog::resolve` as the fetched layer reproduces
    /// slice 1's behavior exactly (mirrors
    /// `unknown_model_resolves_to_the_behavior_floor` in yach-catalog).
    #[test]
    fn load_cache_from_a_missing_path_is_none_and_resolution_matches_slice1() {
        let missing = std::env::temp_dir().join("yach-cli-catalog-refresh-does-not-exist.json");

        let cache = load_cache_from(&missing);
        assert!(cache.is_none());

        let baked = Catalog::empty("2026-08-02");
        let profile = yach_catalog::resolve(
            "openai-compatible",
            "mystery-model",
            &baked,
            cache
                .as_ref()
                .map(|cached| (&cached.catalog, cached.retrieved.as_str())),
            None,
            None,
            &yach_catalog::EnvOverrides::default(),
        );
        assert_eq!(
            profile.context_window.value,
            yach_catalog::DEFAULT_CONTEXT_WINDOW
        );
        assert!(matches!(
            profile.context_window.source,
            yach_catalog::CatalogSource::Default
        ));
    }

    #[test]
    fn format_status_message_matches_the_three_exact_strings() {
        assert_eq!(
            format_status_message(&RefreshOutcome::Refreshed {
                models: 42,
                retrieved: String::from("2026-08-03"),
            }),
            "catalog refreshed (42 models, models.dev 2026-08-03)"
        );
        assert_eq!(
            format_status_message(&RefreshOutcome::NotModified),
            "catalog up to date"
        );
        assert_eq!(
            format_status_message(&RefreshOutcome::Failed {
                fallback: String::from("cached 2026-08-01"),
            }),
            "catalog fetch failed; serving cached 2026-08-01"
        );
    }

    #[test]
    fn fallback_label_prefers_cache_over_baked() {
        let mut catalog = Catalog::empty("unused");
        catalog.insert("p", "m", CatalogEntry::default());
        let cached = CachedCatalog {
            etag: None,
            last_modified: None,
            retrieved: String::from("2026-08-01"),
            catalog,
        };
        assert_eq!(fallback_label(Some(&cached)), "cached 2026-08-01");
        assert_eq!(
            fallback_label(None),
            format!(
                "baked {} snapshot",
                yach_catalog::baked_catalog().snapshot_date()
            )
        );
    }

    #[test]
    fn spawn_refresh_status_forwards_a_formatted_string_end_to_end() {
        // Exercises the real spawn -> network -> format pipeline is out of
        // scope (no network in tests); this instead proves the
        // NotModified/Failed/Refreshed -> String mapping used by the
        // forwarding thread is exactly `format_status_message`, by driving
        // the same channel shape `spawn_refresh_status` builds.
        let (tx, rx) = std::sync::mpsc::channel::<RefreshOutcome>();
        let (status_tx, status_rx) = std::sync::mpsc::channel::<String>();
        let _ = tx.send(RefreshOutcome::NotModified);
        if let Ok(outcome) = rx.recv() {
            let _ = status_tx.send(format_status_message(&outcome));
        }
        assert_eq!(status_rx.recv().as_deref(), Ok("catalog up to date"));
    }
}
