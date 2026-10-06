//! Latest stable openai/codex release, cached for the ChatGPT Codex
//! `client_version`. Design:
//! docs/project/specs/2026-10-02-codex-release-client-version-design.md
//!
//! Network isolation boundary: `fetch_latest_release` is the only function
//! here that touches `reqwest`; every decision around it is pure or
//! path-parameterized.

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
        Some(cached) if yach_catalog::compare_dotted_versions(cached, baked).is_gt() => {
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
    Release {
        tag: String,
        version: String,
        etag: Option<String>,
    },
    NotModified,
    Failed,
}

fn fetch_latest_release(etag: Option<&str>) -> Fetched {
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .build()
    else {
        return Fetched::Failed;
    };
    let mut request = client
        .get(LATEST_RELEASE_URL)
        .header(
            reqwest::header::USER_AGENT,
            concat!("yach/", env!("CARGO_PKG_VERSION")),
        )
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
    match response
        .text()
        .ok()
        .as_deref()
        .and_then(parse_latest_release)
    {
        Some((tag, version)) => Fetched::Release { tag, version, etag },
        None => Fetched::Failed,
    }
}

/// The cache to persist after a fetch, and whether the fetch failed. A
/// failure keeps the last known version when there is one; with no usable
/// cache it still records the check under the baked release so the next
/// refresh is not due again for the whole interval (spec: failures wait
/// out the interval, never repeat back-to-back).
fn next_cache(
    existing: Option<&ReleaseCache>,
    fetched: Fetched,
    now_unix_ms: u64,
) -> (Option<ReleaseCache>, bool) {
    match fetched {
        Fetched::Release { tag, version, etag } => (
            Some(ReleaseCache {
                version,
                tag,
                etag,
                checked_at_unix_ms: now_unix_ms,
            }),
            false,
        ),
        Fetched::NotModified => (
            existing.map(|cache| cache_after_unchanged(cache, now_unix_ms)),
            false,
        ),
        Fetched::Failed => (
            Some(match existing {
                Some(existing) => cache_after_unchanged(existing, now_unix_ms),
                None => ReleaseCache {
                    version: yach_catalog::baked_codex_release_version().to_owned(),
                    tag: format!("rust-v{}", yach_catalog::baked_codex_release_version()),
                    etag: None,
                    checked_at_unix_ms: now_unix_ms,
                },
            }),
            true,
        ),
    }
}

/// Runs the check when due (or forced), persists the result, and updates
/// the effective version. Blocking: call off the async runtime.
///
/// Serialized process-wide so the detached startup check and the awaited
/// `/model` check cannot interleave their load/fetch/write (the shared
/// temp-file writer has exactly one writer per process this way). Due-ness
/// is re-evaluated after the lock, so a waiter released by a finished check
/// returns without refetching.
pub(crate) fn check_latest_release(mode: CheckMode) -> ReleaseCheck {
    let Ok(mut memory) = CHECK_LOCK.lock() else {
        let version = effective_client_version();
        return ReleaseCheck {
            after: version.clone(),
            before: version,
            failed: true,
        };
    };
    let path = release_cache_path();
    let check = run_release_check_at(path.as_deref(), mode, fetch_latest_release, &mut memory);
    if let Ok(mut effective) = EFFECTIVE.write() {
        effective.clone_from(&check.after);
    }
    check
}

static CHECK_LOCK: std::sync::Mutex<Option<ReleaseCache>> = std::sync::Mutex::new(None);

/// `check_latest_release` with the cache path and the fetch injected, so
/// tests exercise the due/persist sequence against a temp file without
/// `$HOME` mutation, a socket, or touching the effective-version global —
/// `run_release_check_at` never locks and never publishes; only the
/// `check_latest_release` wrapper holds `CHECK_LOCK` and writes `EFFECTIVE`,
/// so check + publication stay serialized. `memory` is that lock's process
/// cache: the last check this process completed, used when the disk write
/// did not land.
fn run_release_check_at(
    path: Option<&Path>,
    mode: CheckMode,
    fetch: fn(Option<&str>) -> Fetched,
    memory: &mut Option<ReleaseCache>,
) -> ReleaseCheck {
    let before = effective_client_version();
    let Some(path) = path else {
        return ReleaseCheck {
            after: before.clone(),
            before,
            failed: true,
        };
    };
    let on_disk = load_release_cache_from(path);
    // Disk is the cross-process cache. This process also remembers the last
    // check, so an unwritable cache still waits out the interval and a failed
    // write cannot roll the reported version back to an older disk entry.
    let existing = newer_release_cache(memory.as_ref(), on_disk.as_ref());
    let now = crate::catalog_refresh::catalog_date_now().1;
    if !release_check_due(existing.as_ref(), now, mode) {
        let after = effective_version(
            yach_catalog::baked_codex_release_version(),
            existing.as_ref().map(|cache| cache.version.as_str()),
        );
        return ReleaseCheck {
            before,
            after,
            failed: false,
        };
    }
    let fetched = fetch(existing.as_ref().and_then(|cache| cache.etag.as_deref()));
    let (next, failed) = next_cache(existing.as_ref(), fetched, now);
    if let Some(next) = &next {
        crate::catalog_refresh::write_json_to(path, next);
        *memory = Some(next.clone());
    }
    let after = effective_version(
        yach_catalog::baked_codex_release_version(),
        next.as_ref().map(|cache| cache.version.as_str()),
    );
    ReleaseCheck {
        before,
        after,
        failed,
    }
}

/// The cache entry a due-check should trust: the one checked more recently.
/// Equal timestamps keep the in-process entry, which is the one this process
/// just observed.
fn newer_release_cache(
    memory: Option<&ReleaseCache>,
    disk: Option<&ReleaseCache>,
) -> Option<ReleaseCache> {
    match (memory, disk) {
        (Some(memory), Some(disk)) if disk.checked_at_unix_ms > memory.checked_at_unix_ms => {
            Some(disk.clone())
        }
        (Some(memory), _) => Some(memory.clone()),
        (None, disk) => disk.cloned(),
    }
}

/// Startup trigger: never blocks the caller.
pub(crate) fn spawn_release_check_if_due() {
    std::thread::spawn(|| {
        let _ = check_latest_release(CheckMode::Due);
    });
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static UNWRITABLE_FETCH_CALLS: AtomicU64 = AtomicU64::new(0);

    trait TestUnwrap {
        type Output;
        fn test_unwrap(self) -> Self::Output;
    }

    impl<T, E> TestUnwrap for Result<T, E> {
        type Output = T;
        fn test_unwrap(self) -> Self::Output {
            assert!(self.is_ok());
            match self {
                Ok(value) => value,
                Err(_) => unreachable!(),
            }
        }
    }

    fn cache(version: &str, checked_at_unix_ms: u64) -> ReleaseCache {
        ReleaseCache {
            version: version.to_owned(),
            tag: format!("rust-v{version}"),
            etag: Some(String::from("\"e\"")),
            checked_at_unix_ms,
        }
    }

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("yach-codex-release-{}.json", uuid::Uuid::new_v4()))
    }

    #[test]
    fn parses_only_stable_latest_release_bodies() {
        assert_eq!(
            parse_latest_release(r#"{"tag_name":"rust-v0.162.0","prerelease":false}"#),
            Some((String::from("rust-v0.162.0"), String::from("0.162.0")))
        );
        assert_eq!(
            parse_latest_release(r#"{"tag_name":"rust-v0.162.0-alpha.7"}"#),
            None
        );
        assert_eq!(
            parse_latest_release(r#"{"tag_name":"rust-v0.162.0","prerelease":true}"#),
            None
        );
        assert_eq!(
            parse_latest_release(r#"{"tag_name":"rust-v0.162.0","draft":true}"#),
            None
        );
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
        assert!(!release_check_due(
            Some(&fresh),
            1_000 + interval - 1,
            CheckMode::Due
        ));
        assert!(release_check_due(
            Some(&fresh),
            1_000 + interval,
            CheckMode::Due
        ));
        assert!(
            release_check_due(Some(&fresh), 999, CheckMode::Due),
            "future timestamp"
        );
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
    fn fetch_results_decide_the_next_cache() {
        let existing = cache("0.162.0", 1_000);
        let release = Fetched::Release {
            tag: String::from("rust-v0.163.0"),
            version: String::from("0.163.0"),
            etag: Some(String::from("\"n\"")),
        };
        assert_eq!(
            next_cache(Some(&existing), release, 5_000),
            (
                Some(ReleaseCache {
                    version: String::from("0.163.0"),
                    tag: String::from("rust-v0.163.0"),
                    etag: Some(String::from("\"n\"")),
                    checked_at_unix_ms: 5_000,
                }),
                false
            )
        );
        assert_eq!(
            next_cache(Some(&existing), Fetched::NotModified, 5_000),
            (Some(cache_after_unchanged(&existing, 5_000)), false)
        );
        assert_eq!(
            next_cache(Some(&existing), Fetched::Failed, 5_000),
            (Some(cache_after_unchanged(&existing, 5_000)), true)
        );
        let (fallback, failed) = next_cache(None, Fetched::Failed, 5_000);
        assert!(failed);
        let Some(fallback) = fallback else {
            unreachable!("a first failed check still records the check under the baked release")
        };
        assert_eq!(
            fallback.version,
            yach_catalog::baked_codex_release_version()
        );
        assert_eq!(fallback.etag, None);
        assert_eq!(fallback.checked_at_unix_ms, 5_000);
        assert!(
            !release_check_due(Some(&fallback), 5_001, CheckMode::Due),
            "the fallback record keeps the next normal check from being due"
        );
    }

    #[test]
    fn release_cache_round_trips_through_an_explicit_path() {
        let path = temp_path();
        let written = cache("0.162.0", 7_000);
        crate::catalog_refresh::write_json_to(&path, &written);
        assert_eq!(load_release_cache_from(&path), Some(written));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unreadable_cache_is_absent() {
        let path = temp_path();
        assert!(std::fs::write(&path, b"{\"version\":").is_ok());
        assert!(load_release_cache_from(&path).is_none());
        assert!(
            std::fs::write(
                &path,
                br#"{"version":"0.162.0-beta","tag":"x","checked_at_unix_ms":1}"#
            )
            .is_ok()
        );
        assert!(
            load_release_cache_from(&path).is_none(),
            "non-stable version is ignored"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn check_runs_the_due_persist_update_sequence_against_an_explicit_path() {
        fn failed(_: Option<&str>) -> Fetched {
            Fetched::Failed
        }
        fn release(_: Option<&str>) -> Fetched {
            Fetched::Release {
                tag: String::from("rust-v999.999.0"),
                version: String::from("999.999.0"),
                etag: None,
            }
        }
        let path = temp_path();
        let mut memory = None;
        let outcome =
            run_release_check_at(Some(path.as_path()), CheckMode::Due, failed, &mut memory);
        assert!(outcome.failed);
        assert!(
            load_release_cache_from(&path).is_some_and(|cache| {
                cache.version == yach_catalog::baked_codex_release_version()
            }),
            "a failed first check still records it, under the baked release"
        );
        // The recorded failure makes the immediate re-check not due, so the
        // release fetch below is skipped.
        let outcome =
            run_release_check_at(Some(path.as_path()), CheckMode::Due, release, &mut memory);
        assert!(!outcome.failed);
        assert_eq!(
            load_release_cache_from(&path).map(|cache| cache.version),
            Some(yach_catalog::baked_codex_release_version().to_owned()),
            "the second check was not due and did not refetch"
        );
        let outcome = run_release_check_at(
            Some(path.as_path()),
            CheckMode::Forced,
            release,
            &mut memory,
        );
        assert!(!outcome.failed);
        assert_eq!(
            load_release_cache_from(&path).map(|cache| cache.version),
            Some(String::from("999.999.0")),
            "a forced check refetches and persists the release"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unwritable_cache_still_backs_off_and_keeps_the_version_just_learned() {
        fn release(_: Option<&str>) -> Fetched {
            Fetched::Release {
                tag: String::from("rust-v999.999.0"),
                version: String::from("999.999.0"),
                etag: Some(String::from("\"learned\"")),
            }
        }
        fn failed(_: Option<&str>) -> Fetched {
            UNWRITABLE_FETCH_CALLS.fetch_add(1, Ordering::SeqCst);
            Fetched::Failed
        }
        let dir = std::env::temp_dir().join(format!(
            "yach-codex-release-unwritable-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&dir).test_unwrap();
        let path = dir.join("codex-release.json");
        crate::catalog_refresh::write_json_to(&path, &cache("0.161.0", 1));
        // 0o500: the file stays readable, but a new temp file in this
        // directory cannot be created, so the atomic write fails.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).test_unwrap();

        let mut memory = None;
        let learned = run_release_check_at(
            Some(path.as_path()),
            CheckMode::Forced,
            release,
            &mut memory,
        );
        assert_eq!(
            learned.after, "999.999.0",
            "a failed write must not hide the version just fetched"
        );
        assert_eq!(
            load_release_cache_from(&path).map(|cache| cache.version),
            Some(String::from("0.161.0")),
            "disk stays at the older cache when the write cannot land"
        );

        UNWRITABLE_FETCH_CALLS.store(0, Ordering::SeqCst);
        let again = run_release_check_at(Some(path.as_path()), CheckMode::Due, failed, &mut memory);
        assert_eq!(
            UNWRITABLE_FETCH_CALLS.load(Ordering::SeqCst),
            0,
            "an unpersisted check must still wait out the failure interval"
        );
        assert!(!again.failed);
        assert_eq!(
            again.after, "999.999.0",
            "the follow-up reports the version learned in this process, not the older disk cache"
        );

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).test_unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
