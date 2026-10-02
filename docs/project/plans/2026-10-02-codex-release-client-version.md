# Codex Release Client Version Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Yach advertises the latest stable Codex release as `client_version` to the ChatGPT Codex backend, keeps that current without code changes, and offers a forced refresh from `/model refresh`, `Ctrl+R` in the picker, and `yach models refresh`.

**Architecture:** The baked Codex snapshot is pinned to a release tag whose version is the floor. A new `codex_release` module in `yach-cli` caches the latest stable release from the GitHub API (4h, ETag) and exposes the effective version, max(baked, cached), to every Codex request. `refresh_models` gains a `RefreshMode`; `Forced` bypasses every interval and returns a report the backend turns into one status line.

**Tech Stack:** Rust workspace (`yach-catalog`, `yach-cli`, `yach-backend`, `yach-proto`, `yach-ui`), `reqwest::blocking`, tokio, ratatui, `just`, GitHub Actions.

**Spec:** `docs/project/specs/2026-10-02-codex-release-client-version-design.md`

**Source:** plane:YACH-17

## Global Constraints

- Run project commands through `just` (`just dev cargo …`, `just fmt`, `just lint`, `just test`); never bare `cargo`.
- Release tags must match `^rust-v\d+\.\d+\.\d+$`; prereleases (any suffix) are never accepted.
- The effective client version never goes below the baked release version.
- GitHub requests send `User-Agent: yach/<CARGO_PKG_VERSION>`, `Accept: application/vnd.github+json`, `If-None-Match` when an ETag is cached, and use a 5s timeout.
- Release check interval: `REMOTE_CATALOG_REFRESH_INTERVAL_MS` (4h); GitHub failures (network, timeout, 403, 429, bad body) are soft: keep the last version.
- Startup never waits on the network.
- models.dev refresh is unchanged and not part of the forced chain.
- No version cap setting.
- Keep #288 semantics: a Codex discovery entry or Codex catalog fetched under another `client_version` is stale; the catalog drops its old ETag on a version change.
- Tests follow existing module-local `#[cfg(test)] mod tests` patterns and `test_unwrap()` helpers.

---

### Task 1: Pin the baked Codex snapshot to a release

**Files:**
- Modify: `crates/yach-catalog/src/lib.rs:256-288` (version functions), tests near `:2088` and `protocol_version_selects_highest_listed_5_6_minimum`
- Modify: `crates/yach-catalog/data/codex-models.pin`, `crates/yach-catalog/data/codex-models.json`
- Modify: `justfile:214-244` (`catalog-codex-snapshot`)
- Modify callers: `crates/yach-cli/src/provider_connections.rs:255,285,1523`, `crates/yach-cli/src/main.rs:4090`, `crates/yach-cli/src/model_discovery_cache.rs:331-334`

**Interfaces:**
- Produces: `pub struct CodexPin { pub tag: String, pub version: String, pub commit: String }`, `pub fn parse_codex_pin(raw: &str) -> Option<CodexPin>`, `pub fn release_tag_version(tag: &str) -> Option<&str>`, `pub fn baked_codex_release_version() -> &'static str`, and `pub fn compare_dotted_versions(left: &str, right: &str) -> std::cmp::Ordering` (made public).
- Removes: `baked_codex_protocol_version`, `max_listed_codex_protocol_version`.

- [ ] **Step 1: Write the failing tests** in `crates/yach-catalog/src/lib.rs` tests module, replacing `protocol_version_selects_highest_listed_5_6_minimum` and the `baked_codex_protocol_version()` assertion:

```rust
#[test]
fn codex_pin_parses_a_release_tag_and_commit() {
    let pin = parse_codex_pin("rust-v0.160.0 a956835d020762cb2b570053af06f643a11c0ecc\n")
        .test_unwrap();
    assert_eq!(pin.tag, "rust-v0.160.0");
    assert_eq!(pin.version, "0.160.0");
    assert_eq!(pin.commit, "a956835d020762cb2b570053af06f643a11c0ecc");
}

#[test]
fn codex_pin_rejects_non_release_shapes() {
    for raw in [
        "e7ea5f4a8658ebe49e879be933effed2340fa276",
        "rust-v0.162.0-alpha.7 a956835d020762cb2b570053af06f643a11c0ecc",
        "v0.160.0 a956835d020762cb2b570053af06f643a11c0ecc",
        "rust-v0.160.0",
        "rust-v0.160.0 not-a-sha",
        "rust-v0.160.0 a956835d a956835d",
    ] {
        assert!(parse_codex_pin(raw).is_none(), "{raw} must be rejected");
    }
}

#[test]
fn release_tag_version_accepts_only_stable_tags() {
    assert_eq!(release_tag_version("rust-v0.160.0"), Some("0.160.0"));
    assert_eq!(release_tag_version("rust-v0.162.0-alpha.7"), None);
    assert_eq!(release_tag_version("rust-v0.160"), None);
    assert_eq!(release_tag_version("codex-v0.160.0"), None);
}

#[test]
fn committed_pin_is_a_release_and_drives_the_baked_version() {
    let pin = parse_codex_pin(include_str!("../data/codex-models.pin")).test_unwrap();
    assert_eq!(baked_codex_release_version(), pin.version);
    assert_eq!(baked_codex_release_version(), "0.160.0");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `just dev cargo test -q --locked -p yach-catalog -- codex_pin release_tag committed_pin`
Expected: compile errors (`parse_codex_pin`, `release_tag_version`, `baked_codex_release_version` not found).

- [ ] **Step 3: Implement** in `crates/yach-catalog/src/lib.rs`, replacing `baked_codex_protocol_version` and `max_listed_codex_protocol_version`:

```rust
/// One release of openai/codex the baked snapshot was taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexPin {
    pub tag: String,
    pub version: String,
    pub commit: String,
}

/// The `X.Y.Z` of a stable `rust-vX.Y.Z` release tag; `None` for
/// prereleases and any other shape.
#[must_use]
pub fn release_tag_version(tag: &str) -> Option<&str> {
    let version = tag.strip_prefix("rust-v")?;
    let mut parts = version.split('.');
    let valid = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    }) && parts.next().is_none();
    valid.then_some(version)
}

/// Parses `codex-models.pin`: `<rust-vX.Y.Z> <40-hex commit>`.
#[must_use]
pub fn parse_codex_pin(raw: &str) -> Option<CodexPin> {
    let mut fields = raw.split_whitespace();
    let (tag, commit) = (fields.next()?, fields.next()?);
    if fields.next().is_some()
        || commit.len() != 40
        || !commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(CodexPin {
        tag: tag.to_owned(),
        version: release_tag_version(tag)?.to_owned(),
        commit: commit.to_owned(),
    })
}

/// The Codex release the baked snapshot was taken from, used as the floor
/// for the `/models?client_version=` value. Parsed once per process.
#[must_use]
pub fn baked_codex_release_version() -> &'static str {
    static VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        parse_codex_pin(include_str!("../data/codex-models.pin"))
            .map_or_else(|| String::from("0.0.1"), |pin| pin.version)
    });
    VERSION.as_str()
}
```

Make `compare_dotted_versions` `pub` with a doc comment (`/// Numeric dotted-version order; non-numeric parts compare as 0.`). Delete the `CodexModelsDocument`-based version scan only if nothing else uses those types (check with `rg -n CodexModelsDocument crates/yach-catalog`).

- [ ] **Step 4: Update the snapshot recipe** (`justfile:214-244`) to:

```just
# Refresh the baked Codex subscription catalog from a stable openai/codex
# release. Default: the latest stable release. CODEX_MODELS_TAG=rust-vX.Y.Z
# pins a specific release. Local override: CODEX_MODELS_JSON (path) with
# CODEX_MODELS_TAG and CODEX_MODELS_PIN (40-hex commit).
catalog-codex-snapshot:
  #!/usr/bin/env bash
  set -euo pipefail
  pin_file=crates/yach-catalog/data/codex-models.pin
  dest=crates/yach-catalog/data/codex-models.json
  api=https://api.github.com/repos/openai/codex
  curl_gh() { curl -fsSL -H 'Accept: application/vnd.github+json' -H 'User-Agent: yach-catalog-snapshot' "$@"; }
  tag="${CODEX_MODELS_TAG:-}"
  if [[ -z "$tag" ]]; then
    tag="$(curl_gh "$api/releases/latest" | jq -er .tag_name)"
  fi
  if [[ ! "$tag" =~ ^rust-v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "catalog-codex-snapshot: not a stable release tag: $tag" >&2
    exit 1
  fi
  if [[ -n "${CODEX_MODELS_JSON:-}" ]]; then
    commit="${CODEX_MODELS_PIN:?catalog-codex-snapshot: CODEX_MODELS_JSON requires CODEX_MODELS_PIN}"
    [[ -f "$CODEX_MODELS_JSON" ]] || { echo "catalog-codex-snapshot: not a file: $CODEX_MODELS_JSON" >&2; exit 1; }
    src="$CODEX_MODELS_JSON"
  else
    commit="$(curl_gh "$api/commits/$tag" | jq -er .sha)"
    src="$(mktemp)"
    trap 'rm -f "$src"' EXIT
    curl -fsSL "https://raw.githubusercontent.com/openai/codex/${commit}/codex-rs/models-manager/models.json" -o "$src"
  fi
  if [[ ! "$commit" =~ ^[0-9a-f]{40}$ ]]; then
    echo "catalog-codex-snapshot: not a full commit SHA: $commit" >&2
    exit 1
  fi
  cp "$src" "$dest"
  printf '%s %s\n' "$tag" "$commit" > "$pin_file"
  echo "wrote $dest from openai/codex $tag ($commit)"
```

- [ ] **Step 5: Re-pin to `rust-v0.160.0`**

Run: `CODEX_MODELS_TAG=rust-v0.160.0 just catalog-codex-snapshot`
Expected: `codex-models.pin` is `rust-v0.160.0 a956835d020762cb2b570053af06f643a11c0ecc`. Check the listed set is unchanged:
`jq -r '[.models[]|select(.visibility=="list")|.slug]|sort|join(" ")' crates/yach-catalog/data/codex-models.json`
Expected: `gpt-5.5 gpt-5.6-luna gpt-5.6-sol gpt-5.6-terra gpt-6-astra gpt-6-luna gpt-6-sol gpt-6.1-sol`. Update any `yach-catalog` test that asserts on removed/changed snapshot entries only if the data change requires it, and say so in the commit description.

- [ ] **Step 6: Switch callers** from `baked_codex_protocol_version()` to `baked_codex_release_version()` at `provider_connections.rs:255,285,1523`, `main.rs:4090`, and `model_discovery_cache.rs:333`. (Task 2 replaces these with the effective version.)

- [ ] **Step 7: Run tests to verify they pass**

Run: `just dev cargo test -q --locked -p yach-catalog -p yach`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
jj commit -m "Pin the baked Codex catalog to a stable release tag

codex-models.pin now records rust-vX.Y.Z and its commit, and the advertised
client_version is that release's version instead of the highest
minimal_client_version (which under-reports: gpt-6.1-sol declares 0.153.0
but is listed only from 0.160.0). Re-pin to rust-v0.160.0.

Refs: yach#np83, plane:YACH-17"
```

### Task 2: Track the latest stable Codex release at runtime

**Files:**
- Create: `crates/yach-cli/src/codex_release.rs`
- Modify: `crates/yach-cli/src/main.rs` (module declaration; `:4090` caller; startup trigger after `CliProviderConnectionRuntime::system` at `:946` and `:4039`)
- Modify: `crates/yach-cli/src/rpc.rs` (startup trigger if it builds its own runtime: `rg -n 'CliProviderConnectionRuntime::system' crates/yach-cli/src`)
- Modify: `crates/yach-cli/src/catalog_refresh.rs` (make `write_cache_to` generic over `Serialize`, or add a sibling `write_json_to`)
- Modify: `crates/yach-cli/src/provider_connections.rs:255,285,1523` and `crates/yach-cli/src/model_discovery_cache.rs:327-334`

**Interfaces:**
- Consumes: `yach_catalog::{baked_codex_release_version, release_tag_version, compare_dotted_versions}`.
- Produces (in `crate::codex_release`):
  - `pub(crate) enum CheckMode { Due, Forced }`
  - `#[derive(Serialize, Deserialize)] pub(crate) struct ReleaseCache { pub version: String, pub tag: String, pub etag: Option<String>, pub checked_at_unix_ms: u64 }`
  - `pub(crate) fn parse_latest_release(body: &str) -> Option<(String, String)>` returning `(tag, version)`
  - `pub(crate) fn release_check_due(cache: Option<&ReleaseCache>, now_unix_ms: u64, mode: CheckMode) -> bool`
  - `pub(crate) fn effective_version(baked: &str, cached: Option<&str>) -> String`
  - `pub(crate) fn effective_client_version() -> String` (process state)
  - `pub(crate) struct ReleaseCheck { pub before: String, pub after: String, pub failed: bool }`
  - `pub(crate) fn check_latest_release(mode: CheckMode) -> ReleaseCheck` (blocking; call via `spawn_blocking` or a thread)
  - `pub(crate) fn spawn_release_check_if_due()` (startup, fire-and-forget thread)

- [ ] **Step 1: Write the failing tests** in `codex_release.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cache(version: &str, checked_at_unix_ms: u64) -> ReleaseCache {
        ReleaseCache {
            version: version.to_owned(),
            tag: format!("rust-v{version}"),
            etag: Some(String::from("\"e\"")),
            checked_at_unix_ms,
        }
    }

    #[test]
    fn parses_only_stable_latest_release_bodies() {
        assert_eq!(
            parse_latest_release(r#"{"tag_name":"rust-v0.162.0","prerelease":false}"#),
            Some((String::from("rust-v0.162.0"), String::from("0.162.0")))
        );
        assert_eq!(parse_latest_release(r#"{"tag_name":"rust-v0.162.0-alpha.7"}"#), None);
        assert_eq!(parse_latest_release(r#"{"tag_name":"rust-v0.162.0","prerelease":true}"#), None);
        assert_eq!(parse_latest_release(r#"{"name":"x"}"#), None);
        assert_eq!(parse_latest_release("not json"), None);
    }

    #[test]
    fn effective_version_never_drops_below_the_baked_release() {
        assert_eq!(effective_version("0.160.0", Some("0.162.0")), "0.162.0");
        assert_eq!(effective_version("0.160.0", Some("0.158.0")), "0.160.0");
        assert_eq!(effective_version("0.160.0", None), "0.160.0");
        assert_eq!(effective_version("0.160.0", Some("0.99.0")), "0.160.0");
    }

    #[test]
    fn release_check_is_due_by_interval_or_when_forced() {
        let interval = REMOTE_CATALOG_REFRESH_INTERVAL_MS;
        assert!(release_check_due(None, 1_000, CheckMode::Due));
        let fresh = cache("0.160.0", 1_000);
        assert!(!release_check_due(Some(&fresh), 1_000 + interval - 1, CheckMode::Due));
        assert!(release_check_due(Some(&fresh), 1_000 + interval, CheckMode::Due));
        assert!(release_check_due(Some(&fresh), 999, CheckMode::Due), "future timestamp");
        assert!(release_check_due(Some(&fresh), 1_000, CheckMode::Forced));
    }

    #[test]
    fn not_modified_and_failures_keep_the_version_and_advance_checked_at() {
        let existing = cache("0.162.0", 1_000);
        let kept = cache_after_unchanged(&existing, 5_000);
        assert_eq!(kept.version, "0.162.0");
        assert_eq!(kept.etag, existing.etag);
        assert_eq!(kept.checked_at_unix_ms, 5_000);
    }

    #[test]
    fn unreadable_cache_is_absent() {
        let path = std::env::temp_dir().join(format!("yach-codex-release-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"{\"version\":").test_unwrap();
        assert!(load_release_cache_from(&path).is_none());
        std::fs::write(&path, br#"{"version":"0.162.0-beta","tag":"x","checked_at_unix_ms":1}"#).test_unwrap();
        assert!(load_release_cache_from(&path).is_none(), "non-stable version is ignored");
        let _ = std::fs::remove_file(path);
    }
}
```

(Use the crate's existing temp-path helper if one exists instead of `uuid`; check `rg -n 'temp_dir\(\)' crates/yach-cli/src/model_discovery_cache.rs`.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `just dev cargo test -q --locked -p yach -- codex_release`
Expected: compile errors (module missing).

- [ ] **Step 3: Implement `codex_release.rs`**

```rust
//! Latest stable openai/codex release, cached for the ChatGPT Codex
//! `client_version`. Design:
//! docs/project/specs/2026-10-02-codex-release-client-version-design.md

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::catalog_refresh::REMOTE_CATALOG_REFRESH_INTERVAL_MS;

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckMode {
    Due,
    Forced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReleaseCache {
    pub version: String,
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    pub checked_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseCheck {
    pub before: String,
    pub after: String,
    pub failed: bool,
}

static EFFECTIVE: LazyLock<RwLock<String>> = LazyLock::new(|| {
    let cached = release_cache_path().and_then(|path| load_release_cache_from(&path));
    RwLock::new(effective_version(
        yach_catalog::baked_codex_release_version(),
        cached.as_ref().map(|cache| cache.version.as_str()),
    ))
});

/// The `client_version` every Codex request sends now.
pub(crate) fn effective_client_version() -> String {
    EFFECTIVE.read().map_or_else(
        |_| yach_catalog::baked_codex_release_version().to_owned(),
        |version| version.clone(),
    )
}

pub(crate) fn effective_version(baked: &str, cached: Option<&str>) -> String {
    match cached {
        Some(cached)
            if yach_catalog::compare_dotted_versions(cached, baked).is_gt() =>
        {
            cached.to_owned()
        }
        _ => baked.to_owned(),
    }
}

pub(crate) fn parse_latest_release(body: &str) -> Option<(String, String)> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        #[serde(default)]
        prerelease: bool,
        #[serde(default)]
        draft: bool,
    }
    let release: Release = serde_json::from_str(body).ok()?;
    if release.prerelease || release.draft {
        return None;
    }
    let version = yach_catalog::release_tag_version(&release.tag_name)?.to_owned();
    Some((release.tag_name, version))
}

pub(crate) fn release_check_due(
    cache: Option<&ReleaseCache>,
    now_unix_ms: u64,
    mode: CheckMode,
) -> bool {
    mode == CheckMode::Forced
        || cache.is_none_or(|cache| {
            now_unix_ms
                .checked_sub(cache.checked_at_unix_ms)
                .is_none_or(|elapsed| elapsed >= REMOTE_CATALOG_REFRESH_INTERVAL_MS)
        })
}

pub(crate) fn cache_after_unchanged(existing: &ReleaseCache, now_unix_ms: u64) -> ReleaseCache {
    ReleaseCache {
        checked_at_unix_ms: now_unix_ms,
        ..existing.clone()
    }
}

fn release_cache_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".yach/catalog/codex-release.json"))
}

pub(crate) fn load_release_cache_from(path: &Path) -> Option<ReleaseCache> {
    let cache: ReleaseCache = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let valid = yach_catalog::release_tag_version(&cache.tag) == Some(cache.version.as_str());
    valid.then_some(cache)
}

enum Fetched {
    Release { tag: String, version: String, etag: Option<String> },
    NotModified,
    Failed,
}

fn fetch_latest_release(etag: Option<&str>) -> Fetched {
    let Ok(client) = reqwest::blocking::Client::builder().timeout(CHECK_TIMEOUT).build() else {
        return Fetched::Failed;
    };
    let mut request = client
        .get(LATEST_RELEASE_URL)
        .header(reqwest::header::USER_AGENT, concat!("yach/", env!("CARGO_PKG_VERSION")))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    if let Some(etag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let Ok(response) = request.send() else {
        return Fetched::Failed;
    };
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Fetched::NotModified;
    }
    if !response.status().is_success() {
        return Fetched::Failed;
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(String::from);
    match response.text().ok().as_deref().and_then(parse_latest_release) {
        Some((tag, version)) => Fetched::Release { tag, version, etag },
        None => Fetched::Failed,
    }
}

/// Runs the check when due (or forced), persists the result, and updates
/// the effective version. Blocking: call off the async runtime.
pub(crate) fn check_latest_release(mode: CheckMode) -> ReleaseCheck {
    let before = effective_client_version();
    let Some(path) = release_cache_path() else {
        return ReleaseCheck { after: before.clone(), before, failed: true };
    };
    let existing = load_release_cache_from(&path);
    let now = crate::catalog_refresh::catalog_date_now().1;
    if !release_check_due(existing.as_ref(), now, mode) {
        return ReleaseCheck { after: before.clone(), before, failed: false };
    }
    let (next, failed) = match fetch_latest_release(existing.as_ref().and_then(|c| c.etag.as_deref())) {
        Fetched::Release { tag, version, etag } => (
            Some(ReleaseCache { version, tag, etag, checked_at_unix_ms: now }),
            false,
        ),
        Fetched::NotModified => (existing.as_ref().map(|c| cache_after_unchanged(c, now)), false),
        Fetched::Failed => (existing.as_ref().map(|c| cache_after_unchanged(c, now)), true),
    };
    if let Some(next) = &next {
        crate::catalog_refresh::write_json_to(&path, next);
    }
    let after = effective_version(
        yach_catalog::baked_codex_release_version(),
        next.as_ref().map(|cache| cache.version.as_str()),
    );
    if let Ok(mut effective) = EFFECTIVE.write() {
        effective.clone_from(&after);
    }
    ReleaseCheck { before, after, failed }
}

/// Startup trigger: never blocks the caller.
pub(crate) fn spawn_release_check_if_due() {
    std::thread::spawn(|| {
        let _ = check_latest_release(CheckMode::Due);
    });
}
```

Note: a failed first check with no cache writes nothing, so the next launch retries; this matches "keep the last version". Make `REMOTE_CATALOG_REFRESH_INTERVAL_MS` `pub(crate)` and add `pub(crate) fn write_json_to(path: &Path, value: &impl Serialize)` in `catalog_refresh.rs` by generalizing `write_cache_to` (keep its temp-file + rename behaviour; `write_cache_to` becomes a call to it).

- [ ] **Step 4: Switch Codex callers to the effective version**
  - `provider_connections.rs:255,285` and `main.rs:4090`: `let version = crate::codex_release::effective_client_version();` then `Some(version.as_str())` (the futures are `async move`; bind inside the block).
  - `provider_connections.rs:1523`: `let client_version = crate::codex_release::effective_client_version();` and pass `&client_version` / `client_version.as_str()` where Task 1 passed the `&'static str`; the `tokio::spawn` closure moves the `String`.
  - `model_discovery_cache.rs`: `fn listing_client_version(provider: ProviderKind) -> Option<String>` returning `Some(crate::codex_release::effective_client_version())` for `ChatGptSubscription`; update the comparison to `entry.client_version.as_deref() == listing_client_version(connection.provider).as_deref()` and the `update` path that stores it.

- [ ] **Step 5: Add the startup trigger.** After each non-test `CliProviderConnectionRuntime::system(...)` construction (`main.rs:946`, `main.rs:4039`, and any in `rpc.rs`), when the runtime is `Some` and it has a ChatGPT subscription connection, call `crate::codex_release::spawn_release_check_if_due()`. Add `pub(crate) fn has_chatgpt_subscription(&self) -> bool` on `CliProviderConnectionRuntime` reading the metadata store (no credential I/O), mirroring `registry_has_stored_connections` (`provider_connections.rs:75-79`):

```rust
pub(crate) fn has_chatgpt_subscription(&self) -> bool {
    self.state
        .store
        .list()
        .is_ok_and(|connections| {
            connections
                .iter()
                .any(|connection| connection.provider == ProviderKind::ChatGptSubscription)
        })
}
```

(Use whatever list/load method `ProviderConnectionStore` exposes; check with `rg -n 'pub fn (list|load)' crates/yach-connections/src/lib.rs`.)

- [ ] **Step 6: Gate `/model` refreshes on a due check.** At the top of the `refresh_models` future (`provider_connections.rs:440`), after `resolve_ready_connections`, if any resolved connection is `RigProviderConfig::ChatGptSubscription`, run `spawn_blocking(|| crate::codex_release::check_latest_release(CheckMode::Due)).await` before `spawn_codex_catalog_refresh` and discovery. It returns immediately when not due; when due, it is bounded by the 5s timeout.

- [ ] **Step 7: Run tests**

Run: `just dev cargo test -q --locked -p yach`
Expected: PASS, including the #288 tests in `model_discovery_cache` and `catalog_refresh`.

- [ ] **Step 8: Commit**

```bash
jj commit -m "Track the latest stable Codex release for client_version

Cache GitHub's latest openai/codex release (4h, ETag, 5s timeout) in
~/.yach/catalog/codex-release.json and send max(baked release, cached
latest) as the Codex client_version. Check at launch in the background and
before a due model refresh.

Refs: yach#np83, plane:YACH-17"
```

### Task 3: Forced refresh in the connection runtime

**Files:**
- Modify: `crates/yach-backend/src/provider_connections.rs:255-270` (trait), `crates/yach-backend/src/runner.rs:97-109` (`ModelDiscoveryOutcome`), `runner.rs:28522` (test double)
- Modify: `crates/yach-backend/src/lib.rs` (re-exports)
- Modify: `crates/yach-cli/src/provider_connections.rs:32,433-513,1224-1253,1512-1616`
- Modify: `crates/yach-cli/Cargo.toml` (tokio `sync` feature)

**Interfaces:**
- Consumes: `crate::codex_release::{check_latest_release, CheckMode, ReleaseCheck}` (Task 2).
- Produces (in `yach-backend`, re-exported from the crate root):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefreshMode {
    #[default]
    Normal,
    Forced,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForcedRefreshReport {
    pub entries: Vec<CatalogModelEntry>,
    pub warnings: Vec<String>,
    /// `(before, after)` when the Codex client version changed.
    pub codex_version_change: Option<(String, String)>,
    /// Bounded step names that failed, e.g. "release check", "Codex catalog".
    pub failed_steps: Vec<String>,
    /// The effective Codex version, for failure messages.
    pub codex_version: Option<String>,
}

// New ModelDiscoveryOutcome variant:
Forced(ForcedRefreshReport),

// Trait change:
fn refresh_models(&self, active: Option<ActiveModelTarget>, mode: RefreshMode) -> ModelDiscoveryFuture;

/// One status line for a forced refresh. `previous` is the catalog advertised
/// before the refresh.
pub fn forced_refresh_status(previous: &[CatalogModelEntry], report: &ForcedRefreshReport) -> String;
```

- [ ] **Step 1: Write the failing tests**

In `crates/yach-backend/src/runner.rs` tests (or next to `forced_refresh_status`):

```rust
#[test]
fn forced_refresh_status_reports_version_change_new_rows_and_failures() {
    // `catalog_entry(id, name, provider)` is the existing helper at runner.rs:25846.
    let row = |id: &str| catalog_entry(id, id, "openai-codex");
    let previous = vec![row("gpt-6-sol")];
    let report = ForcedRefreshReport {
        entries: vec![row("gpt-6-sol"), row("gpt-6.1-sol")],
        warnings: Vec::new(),
        codex_version_change: Some((String::from("0.158.0"), String::from("0.160.0"))),
        failed_steps: Vec::new(),
        codex_version: Some(String::from("0.160.0")),
    };
    assert_eq!(
        forced_refresh_status(&previous, &report),
        "models refreshed · Codex 0.158.0 → 0.160.0 · +1 models"
    );
    let unchanged = ForcedRefreshReport { entries: previous.clone(), codex_version_change: None, ..report.clone() };
    assert_eq!(forced_refresh_status(&previous, &unchanged), "models refreshed · no changes");
    let failed = ForcedRefreshReport { failed_steps: vec![String::from("release check")], ..unchanged };
    assert_eq!(
        forced_refresh_status(&previous, &failed),
        "models refreshed · no changes · release check failed (using 0.160.0)"
    );
}
```

(Rows compare by `(info.provider, info.id)`.)

In `crates/yach-cli/src/provider_connections.rs` tests:

```rust
#[test]
fn forced_refresh_ignores_a_fresh_discovery_cache() {
    let connection = ready_compatible("Forced", "http://forced.invalid/v1");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let runtime = CliProviderConnectionRuntime::with_stores_and_discoverer(
        Arc::new(FixedMetadata {
            records: vec![connection],
        }),
        Arc::new(ReadyCredentials),
        super::super::model_layers_fixture(),
        None,
        Arc::new(move |_| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(vec![yach_backend::model_discovery::DiscoveredProviderModel {
                    id: String::from("fixture"),
                    display_name: None,
                }])
            })
        }),
    );
    let test_runtime = tokio::runtime::Runtime::new().test_unwrap();

    test_runtime.block_on(runtime.refresh_models(None, RefreshMode::Normal));
    test_runtime.block_on(runtime.refresh_models(None, RefreshMode::Normal));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "fresh cache reused");

    let outcome = test_runtime.block_on(runtime.refresh_models(None, RefreshMode::Forced));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2, "forced bypasses the cache");
    let ModelDiscoveryOutcome::Forced(report) = outcome else {
        unreachable!("forced refresh must return a report");
    };
    assert_eq!(report.entries[0].info.id, "fixture");
    assert!(report.codex_version_change.is_none(), "no ChatGPT connection");
    assert!(report.failed_steps.is_empty());
}

#[tokio::test]
async fn forced_codex_refresh_waits_for_an_in_flight_refresh() {
    let held = acquire_codex_refresh(RefreshMode::Normal).await.test_unwrap();
    assert!(acquire_codex_refresh(RefreshMode::Normal).await.is_none(), "normal skips when busy");
    let forced = tokio::spawn(acquire_codex_refresh(RefreshMode::Forced));
    tokio::task::yield_now().await;
    assert!(!forced.is_finished(), "forced waits");
    drop(held);
    assert!(forced.await.test_unwrap().is_some());
}

#[test]
fn forced_codex_catalog_refresh_ignores_the_interval() {
    // `cached_fixture_with_checked_at` lives in catalog_refresh.rs tests
    // (:554); put this test in that module.
    let cache = cached_fixture_with_checked_at(Some(1_000));
    assert!(!super::catalog_refresh::codex_refresh_due(Some(&cache), "0.160.0", 1_001, RefreshMode::Normal));
    assert!(super::catalog_refresh::codex_refresh_due(Some(&cache), "0.160.0", 1_001, RefreshMode::Forced));
}
```

The `forced_codex_catalog_refresh_ignores_the_interval` test belongs in
`catalog_refresh.rs`'s test module; the other two in
`provider_connections.rs`.

(If the fixture runtime has no discovery-cache path and therefore never
reuses a fresh entry, construct it with a temp cache path the way
`serialized_persistence_writes_the_latest_live_discovery_cache` (`:2421`)
does, and keep the call-count assertions unchanged.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `just dev cargo test -q --locked -p yach-backend -p yach -- forced`
Expected: compile errors.

- [ ] **Step 3: Implement**
  1. Add `RefreshMode`, `ForcedRefreshReport`, `ModelDiscoveryOutcome::Forced`, and `forced_refresh_status` in `yach-backend` (status format exactly as in the test: base `models refreshed`; then ` · Codex A → B` if changed; then ` · +N models` if N > 0, where N counts report rows whose `(provider, id)` is not in `previous`; if neither, ` · no changes`; then for each failed step ` · <step> failed`, with ` (using <codex_version>)` appended to the release-check failure). Re-export from `crates/yach-backend/src/lib.rs` next to `ModelDiscoveryOutcome`.
  2. Change the trait method signature and update every implementation and call site (`rg -n 'refresh_models\(' crates`); existing callers pass `RefreshMode::Normal`.
  3. Add `"sync"` to the yach-cli tokio features. Replace the `CODEX_CATALOG_REFRESH_IN_FLIGHT` `AtomicBool` and `CodexCatalogRefreshGuard` (`provider_connections.rs:32,1532-1541,1610-1616`) with:

```rust
static CODEX_CATALOG_REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn acquire_codex_refresh(
    mode: RefreshMode,
) -> Option<tokio::sync::MutexGuard<'static, ()>> {
    match mode {
        RefreshMode::Normal => CODEX_CATALOG_REFRESH.try_lock().ok(),
        RefreshMode::Forced => Some(CODEX_CATALOG_REFRESH.lock().await),
    }
}
```

  4. Add `mode: RefreshMode` to `catalog_refresh::codex_refresh_due` (Forced → `true`). Turn `spawn_codex_catalog_refresh` into `async fn refresh_codex_catalog(connections: &[ResolvedConnection], mode: RefreshMode) -> Option<bool>` (`None`: no ChatGPT connection or not due; `Some(true)`: fetched or 304; `Some(false)`: failed). It acquires the guard, re-checks due-ness after acquiring, and moves the fetch body out of `tokio::spawn`. Normal mode in `refresh_models` keeps it non-blocking: `tokio::spawn(async move { refresh_codex_catalog(&connections, RefreshMode::Normal).await })` with a cloned connection list; Forced mode awaits it. Update `spawn_codex_catalog_refresh_is_idle_without_chatgpt_connections` (`:1722`) to `refresh_codex_catalog(&[], RefreshMode::Forced).await.is_none()`.
  5. Add `mode` to `discover_connection_models`; in Forced, skip the `Some(cached) if cached.fresh` early return (`:1239-1251`).
  6. In `refresh_models`, for `RefreshMode::Forced`: run `check_latest_release(CheckMode::Forced)` via `spawn_blocking` when a ChatGPT connection is resolved (record `release check` in `failed_steps` if `failed`, and `codex_version_change` if `before != after`); await `refresh_codex_catalog(.., Forced)` (record `Codex catalog` on `Some(false)`); run discovery; return `ModelDiscoveryOutcome::Forced(report)` with the entries and warnings the Normal path would return. If discovery itself fails, return `ModelDiscoveryOutcome::Failed` as today.

- [ ] **Step 4: Run tests**

Run: `just dev cargo test -q --locked -p yach-backend -p yach`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add a forced model refresh to the connection runtime

refresh_models takes a RefreshMode. Forced runs the Codex release check,
the Codex catalog refresh and discovery for every connection regardless of
their intervals, waits for an in-flight Codex catalog refresh instead of
being dropped, and returns a report for one status line.

Refs: yach#np83, plane:YACH-17"
```

### Task 4: Forced refresh from the protocol and the TUI

**Files:**
- Modify: `crates/yach-proto/src/lib.rs:846` (`ClientEvent`)
- Modify: `crates/yach-backend/src/runner.rs:513-566` (refresh request/start/publish), `:598-674` (`ConnectionFlowEffectContext`), `:1334-1335`, `:1530-1585`, `:1894-1924`
- Modify: `crates/yach-ui/src/slash_commands.rs:187-231,336-345`, `crates/yach-ui/src/app.rs:1113-1118,2283-2296,2506-2508,3442-3446`, `crates/yach-ui/src/model_selector.rs:43`
- Modify: any other exhaustive `ClientEvent` match the compiler reports (e.g. `crates/yach-cli/src/rpc.rs`, other backends)

**Interfaces:**
- Consumes: `RefreshMode`, `ModelDiscoveryOutcome::Forced`, `forced_refresh_status` (Task 3).
- Produces: `ClientEvent::AvailableModelsRefreshRequested`.

- [ ] **Step 1: Write the failing tests**

`crates/yach-ui/src/slash_commands.rs`:

```rust
#[test]
fn parser_passes_model_arguments_to_the_app() {
    assert_eq!(
        parse_slash_command("/model refresh"),
        SlashParseResult::CommandWithArgs {
            action: SlashAction::Model,
            args: String::from("refresh"),
        }
    );
}
```

and change `parser_rejects_arguments_for_alpha_commands` to drop the `/model gpt-5` assertion (the app rejects it now; see next test).

`crates/yach-ui/src/app.rs` tests (follow `alt_m_opens_model_selector`, `:6074`):

```rust
#[test]
fn model_refresh_command_opens_the_picker_and_requests_a_forced_refresh() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx);
    app.set_prompt_text("/model refresh");
    app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(app.mode, AppMode::ModelSelect { .. }));
    assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRefreshRequested));
}

#[test]
fn model_command_rejects_other_arguments_without_opening_the_picker() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx);
    app.set_prompt_text("/model gpt-5");
    app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(app.status_message, "usage: /model [refresh]");
    assert!(rx.try_recv().is_err());
}

#[test]
fn ctrl_r_in_the_model_picker_requests_a_forced_refresh_without_editing_the_query() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx);
    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
    assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
    app.handle_key(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRefreshRequested));
    assert!(matches!(&app.mode, AppMode::ModelSelect { query, .. } if query.is_empty()));
}
```

`crates/yach-backend/src/runner.rs` tests (follow the `AvailableModelsRequested` tests near `:26271`): a recording runtime double whose `refresh_models` records the `RefreshMode`; send `AvailableModelsRefreshRequested` and assert it saw `RefreshMode::Forced`; send `AvailableModelsRefreshRequested` while a Normal refresh is in flight and assert the pending restart uses `Forced`; feed back a `ModelDiscoveryOutcome::Forced` and assert the final `StatusUpdated` is the `forced_refresh_status` line, not `provider models refreshed`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `just dev cargo test -q --locked -p yach-ui -p yach-backend -- model_refresh forced ctrl_r parser_passes_model`
Expected: compile errors / FAIL.

- [ ] **Step 3: Implement**
  1. `yach-proto`: add `AvailableModelsRefreshRequested` after `AvailableModelsRequested`, with a doc comment: "Like `AvailableModelsRequested`, but every provider cache interval is ignored."
  2. Runner: change `pending: &mut bool` / `connection_model_refresh_pending` / `model_refresh_pending` to `Option<RefreshMode>` (Forced wins when merging); thread `mode` through `request_connection_model_refresh` and `start_connection_model_refresh` into `runtime.refresh_models(.., mode)`. Handle `ClientEvent::AvailableModelsRefreshRequested` like `AvailableModelsRequested` with `RefreshMode::Forced`; without a connection runtime, behave like `AvailableModelsRequested` and send `StatusUpdated { message: "forced refresh needs a stored connection" }`.
  3. Runner: on `ModelDiscoveryOutcome::Forced(report)`, compute `forced_refresh_status(&advertised_catalog, &report)` before publishing, then publish the entries and warnings with that line as the final status. Give `publish_connection_catalog` a `final_message: String` parameter; existing callers pass `String::from("provider models refreshed")`. In the legacy discovery match (`:1383`), treat `Forced` like `AvailableWithWarnings`.
  4. Slash parser: add `SlashAction::Model` to the `has_args` allowlist (`slash_commands.rs:212-221`).
  5. App: `SlashParseResult::CommandWithArgs { action: SlashAction::Model, args }`: if `args == "refresh"`, clear input, open the picker, and request a forced refresh; otherwise clear input and set `status_message = "usage: /model [refresh]"`. Add:

```rust
fn request_forced_model_refresh(&mut self) -> bool {
    let requested = self.send_client_event(ClientEvent::AvailableModelsRefreshRequested);
    if requested {
        self.model_availability_refresh = ModelAvailabilityRefresh::Pending;
        self.status_message = String::from("refreshing models");
    }
    requested
}
```

  `open_model_selector` takes `forced: bool` and calls `request_forced_model_refresh` instead of `request_available_models` when true. In `handle_model_select_key`, add before the plain-character arm: `(KeyCode::Char('r'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => { self.request_forced_model_refresh(); }` (respect `backend_busy()` like `open_model_selector`).
  6. Picker title (`model_selector.rs:43`): `"Select Model · Ctrl+R refresh"`. Update any snapshot/render test that asserts the old title.
  7. Fix every other exhaustive `ClientEvent` match the compiler reports; non-native backends treat the new event like `AvailableModelsRequested`.

- [ ] **Step 4: Run tests**

Run: `just dev cargo test -q --locked -p yach-proto -p yach-ui -p yach-backend -p yach`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add /model refresh and Ctrl+R to force a model refresh

A new AvailableModelsRefreshRequested event runs the forced connection
refresh and reports one status line. /model accepts only the refresh
argument; Ctrl+R in the picker leaves the filter query untouched.

Refs: yach#np83, plane:YACH-17"
```

### Task 5: `yach models refresh`

**Files:**
- Modify: `crates/yach-cli/src/main.rs:141-190` (parser), `:197-266` (`Command`), `:469-503` (dispatch), `:526+` (`CommandResult` output), `:984-995` (usage lines), tests near `:6147`
- Modify: `README.md` (commands section near `:117`)

**Interfaces:**
- Consumes: `CliProviderConnectionRuntime::system`, `ProviderConnectionRuntime::{cached_models, refresh_models}`, `RefreshMode::Forced`, `forced_refresh_status` (Task 3).
- Produces: `Command::ModelsRefresh`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn models_refresh_command_parses() {
    assert_eq!(parse_command(&["models", "refresh"]), Command::ModelsRefresh);
    assert_eq!(
        parse_command(&["models", "list"]),
        Command::Unknown { name: String::from("models list") }
    );
    assert_eq!(
        parse_command(&["models"]),
        Command::Unknown { name: String::from("models ") }
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `just dev cargo test -q --locked -p yach -- models_refresh_command_parses`
Expected: compile error (`Command::ModelsRefresh` missing).

- [ ] **Step 3: Implement**
  - Parser: `Some("models") => match positional.get(1).map(String::as_str) { Some("refresh") => Command::ModelsRefresh, other => Command::Unknown { name: format!("models {}", other.unwrap_or_default()) } }`.
  - `run_models_refresh_command() -> CommandResult`: load `ModelOverrideLayers::load_for_project(None)`, build `CliProviderConnectionRuntime::system(layers, None, provider_connection_timeout(), None)`; if `None` or it has no stored connection, return a failed result with `error=no provider connection is configured`. Otherwise build a current-thread tokio runtime, take `previous = runtime.cached_models()`, await `refresh_models(None, RefreshMode::Forced)`, and print:
    - `Forced(report)` → `forced_refresh_status(&previous, &report)`; exit code 1 only when `failed_steps` covers every attempted step and discovery returned no entries.
    - `Failed { message }` → `error=<message>`, exit code 1.
    - anything else → `models refreshed`.
    Reuse `CommandResult::Preset { lines, failed }` if its output/exit handling fits (it prints lines and maps `failed` to the exit code); otherwise add `CommandResult::ModelsRefresh { line, failed }` next to it.
  - Usage: add `"       yach models refresh"` and `models` to the `commands:` line.
  - README (near `:117`): add to the commands paragraph: "`/model refresh` (or `Ctrl+R` in the `/model` picker) re-checks every provider's model list now, ignoring cache windows; `yach models refresh` does the same from the shell. For ChatGPT subscriptions, Yach identifies as the latest stable Codex release, checked at most every four hours."

- [ ] **Step 4: Run tests**

Run: `just dev cargo test -q --locked -p yach -- models_refresh_command_parses usage`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add yach models refresh

Runs the forced model refresh without a TUI and prints the same status
line.

Refs: yach#np83, plane:YACH-17"
```

### Task 6: Weekly re-pin workflow

**Files:**
- Create: `.github/workflows/codex-catalog-repin.yml`

**Interfaces:**
- Consumes: `just catalog-codex-snapshot` (Task 1).

- [ ] **Step 1: Write the workflow**

```yaml
name: Codex catalog re-pin

# Proposes a PR when a newer stable openai/codex release changes the baked
# Codex catalog. PRs opened with GITHUB_TOKEN do not trigger pull_request
# workflows; close and reopen the PR to run CI. Requires the repository
# setting "Allow GitHub Actions to create and approve pull requests".
on:
  schedule:
    - cron: "17 6 * * 1"
  workflow_dispatch:

permissions:
  contents: write
  pull-requests: write

jobs:
  repin:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
      - uses: extractions/setup-just@v3
      - name: Snapshot latest stable release
        run: just catalog-codex-snapshot
      - name: Open PR when the pin changed
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          set -euo pipefail
          if git diff --quiet -- crates/yach-catalog/data; then
            echo "pin unchanged"; exit 0
          fi
          tag="$(cut -d' ' -f1 crates/yach-catalog/data/codex-models.pin)"
          branch="codex-catalog/${tag}"
          if gh pr list --head "$branch" --state open --json number --jq 'length' | grep -qv '^0$'; then
            echo "PR for $branch already open"; exit 0
          fi
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
          git switch -c "$branch"
          git add crates/yach-catalog/data
          git commit -m "Re-pin baked Codex catalog to ${tag}"
          git push --force-with-lease origin "$branch"
          gh pr create --base main --head "$branch" \
            --title "Re-pin baked Codex catalog to ${tag}" \
            --body "Automated by .github/workflows/codex-catalog-repin.yml. Review the codex-models.json diff. Close and reopen this PR to run CI."
```

Check the `checkout` and `setup-just` major versions against `.github/workflows/ci.yml` and use the same ones. The recipe needs `jq` and `curl`, which `ubuntu-latest` provides.

- [ ] **Step 2: Validate**

Run: `uvx --from check-jsonschema check-jsonschema --builtin-schema vendor.github-workflows .github/workflows/codex-catalog-repin.yml`
Expected: `ok -- validation done`.
Run: `just catalog-codex-snapshot && jj diff --stat crates/yach-catalog/data`
Expected: a diff only if a release newer than `rust-v0.160.0` exists; restore with `jj restore crates/yach-catalog/data` afterward.

- [ ] **Step 3: Commit**

```bash
jj commit -m "Propose a weekly re-pin of the baked Codex catalog

Refs: plane:YACH-17"
```

### Task 7: Full verification and live acceptance

**Files:** none (evidence goes in the PR body).

- [ ] **Step 1: Full checks**

Run: `just fmt && just lint && just test`
Expected: all pass.

- [ ] **Step 2: Live acceptance** (needs a real ChatGPT subscription connection in `~/.yach`):
  1. `CODEX_MODELS_TAG=rust-v0.158.0 just catalog-codex-snapshot`, then build: `just dev cargo build -q -p yach`.
  2. Copy `~/.yach` to a temp HOME. There, write `catalog/codex-release.json` as `{"version":"0.158.0","tag":"rust-v0.158.0","checked_at_unix_ms":<now ms>}` so the launch check is not due, and set `[model.default]` in `config.toml` to a model other than `gpt-6.1-sol` (e.g. `openai-codex` / `gpt-6-sol`): `discover_connection_models` always lists the active model (`provider_connections.rs:1263-1272`), which would mask the result. Launch without `--resume`.
  3. Launch the built binary with that HOME in a pty, open `/model`, and type `gpt-6.1`: expect no rows.
  4. Press `Ctrl+R`: expect the status `models refreshed · Codex 0.158.0 → 0.160.0 · +1 models` (or higher if a newer stable release exists) and a `gpt-6.1-sol` row; `model-discovery.json` lists it under `client_version` `0.160.0`.
  5. `HOME=<temp> ./target/.../yach models refresh` → `models refreshed · no changes`.
  6. `jj restore crates/yach-catalog/data` to drop the temporary `rust-v0.158.0` pin; rebuild.

- [ ] **Step 3: Record** the outputs for the PR description; no commit.
