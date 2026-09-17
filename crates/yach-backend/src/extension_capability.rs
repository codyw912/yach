//! Extension capability declarations and the user grants that admit them.
//!
//! A capability is a *declaration* the kernel records and gates activation
//! on. It is not enforced: the extension host is an ordinary subprocess with
//! the agent's privileges and can use the network whether or not it declared
//! doing so. See
//! `docs/project/specs/2026-09-16-extension-capability-contract-design.md`.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// What diagnostics can say about a recorded grant.
///
/// `Unknown` is for construction sites that do not have the manifest or
/// extension id. Collapsing that into `Absent` would print
/// `capability_grant=none` for an extension whose grant was never consulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionCapabilityGrantStatus {
    Unknown,
    Absent,
    Approved(BTreeSet<ExtensionCapability>),
}

impl ExtensionCapabilityGrantStatus {
    #[must_use]
    pub fn from_loaded(grant: Option<ExtensionCapabilityGrant>) -> Self {
        match grant {
            None => Self::Absent,
            Some(grant) => Self::Approved(grant.approved),
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
/// but must never grant it. The file name is only built from a validated
/// extension id, so `PathBuf::join` cannot be pointed at an arbitrary path.
#[must_use]
pub fn grant_path(extension_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    grant_path_in(
        &PathBuf::from(home).join(".yach").join("extensions"),
        extension_id,
    )
}

pub(crate) fn grant_path_in(grants_dir: &Path, extension_id: &str) -> Option<PathBuf> {
    if !crate::is_valid_extension_id(extension_id) {
        return None;
    }
    Some(grants_dir.join(format!("{extension_id}.json")))
}

/// Resolve the extension id that owns a grant file.
///
/// A discovered package's manifest id wins, including when the user passed a
/// package root. An undiscovered selector is accepted only when it is itself
/// a valid extension id, so uninstall-by-id still works and paths cannot be
/// used as grant filenames.
pub fn grant_id_from_selector(
    discovered_id: Option<&str>,
    selector: &str,
) -> Result<String, String> {
    if let Some(id) = discovered_id {
        return Ok(id.to_owned());
    }
    if crate::is_valid_extension_id(selector) {
        return Ok(selector.to_owned());
    }
    Err(format!(
        "extension not discovered: {selector}. Pass the extension id to revoke its grant."
    ))
}

/// Load a grant from user home. Missing, unreadable, or malformed records
/// are treated as no grant so they cannot be used as approval.
#[must_use]
pub fn load_grant(extension_id: &str) -> Option<ExtensionCapabilityGrant> {
    load_grant_at(&grant_path(extension_id)?)
}

pub(crate) fn load_grant_at(path: &Path) -> Option<ExtensionCapabilityGrant> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Persist a grant under `~/.yach/extensions/<id>.json`.
pub fn store_grant(extension_id: &str, grant: &ExtensionCapabilityGrant) -> io::Result<()> {
    let Some(path) = grant_path(extension_id) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "HOME and USERPROFILE are unset",
        ));
    };
    store_grant_at(&path, grant)
}

pub(crate) fn store_grant_at(path: &Path, grant: &ExtensionCapabilityGrant) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "grant path has no parent directory",
        ));
    };
    if !parent.as_os_str().is_empty() && !parent.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
    }
    let encoded = serde_json::to_vec_pretty(grant)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&encoded)?;
    file.flush()?;
    Ok(())
}

/// Remove a grant. A missing file is success.
pub fn remove_grant(extension_id: &str) -> io::Result<()> {
    let Some(path) = grant_path(extension_id) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "HOME and USERPROFILE are unset",
        ));
    };
    remove_grant_at(&path)
}

pub(crate) fn remove_grant_at(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Approve the capabilities requested by `tools`. Writes nothing when the
/// extension requests none, so an absent grant stays absent.
pub fn grant_requested(
    extension_id: &str,
    version: &str,
    tools: &[ExtensionToolContribution],
) -> io::Result<Option<ExtensionCapabilityGrant>> {
    let Some(path) = grant_path(extension_id) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "HOME and USERPROFILE are unset",
        ));
    };
    grant_requested_at(&path, version, tools)
}

pub(crate) fn grant_requested_at(
    path: &Path,
    version: &str,
    tools: &[ExtensionToolContribution],
) -> io::Result<Option<ExtensionCapabilityGrant>> {
    let approved = requested_capabilities(tools);
    if approved.is_empty() {
        return Ok(None);
    }
    let grant = ExtensionCapabilityGrant {
        approved,
        version_at_grant: version.to_owned(),
        granted_at: utc_timestamp_now(),
    };
    store_grant_at(path, &grant)?;
    Ok(Some(grant))
}

/// Remove a grant. Returns whether a file was present. Missing is success.
pub fn revoke_grant(extension_id: &str) -> io::Result<bool> {
    let Some(path) = grant_path(extension_id) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "HOME and USERPROFILE are unset",
        ));
    };
    revoke_grant_at(&path)
}

pub(crate) fn revoke_grant_at(path: &Path) -> io::Result<bool> {
    let had_grant = path.is_file();
    remove_grant_at(path)?;
    Ok(had_grant)
}

/// Human-readable approved set, naming the tools that requested each
/// capability. Empty when nothing is approved.
#[must_use]
pub fn capability_approval_label(
    tools: &[ExtensionToolContribution],
    approved: &BTreeSet<ExtensionCapability>,
) -> String {
    if approved.is_empty() {
        return String::from("none");
    }
    approved
        .iter()
        .map(|capability| {
            let names: Vec<&str> = tools
                .iter()
                .filter(|tool| capability_for_risk(tool.risk) == Some(*capability))
                .map(|tool| tool.name.as_str())
                .collect();
            if names.is_empty() {
                capability.as_str().to_owned()
            } else {
                format!("{} ({})", capability.as_str(), names.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[must_use]
pub fn grant_confirmation_message(
    extension_id: &str,
    tools: &[ExtensionToolContribution],
    approved: &BTreeSet<ExtensionCapability>,
) -> String {
    format!(
        "approved {} for {extension_id}. The extension may activate; Yach does not observe whether the host uses those capabilities.",
        capability_approval_label(tools, approved)
    )
}

#[must_use]
pub fn nothing_to_grant_message(extension_id: &str) -> String {
    format!("extension {extension_id} requests no capabilities that need a grant")
}

#[must_use]
pub fn revoke_confirmation_message(extension_id: &str, had_grant: bool) -> String {
    if had_grant {
        format!("revoked capability grant for {extension_id}")
    } else {
        format!("no capability grant recorded for {extension_id}")
    }
}

const fn capability_for_risk(risk: ExtensionToolRisk) -> Option<ExtensionCapability> {
    match risk {
        ExtensionToolRisk::UsesNetwork => Some(ExtensionCapability::UsesNetwork),
        ExtensionToolRisk::RunsProcess => Some(ExtensionCapability::RunsProcess),
        ExtensionToolRisk::ReadsLocalMetadata
        | ExtensionToolRisk::ReadsLocalContent
        | ExtensionToolRisk::MutatesLocalState => None,
    }
}

fn utc_timestamp_now() -> String {
    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return String::from("1970-01-01T00:00:00Z");
    };
    let total_secs = duration.as_secs();
    let days = total_secs / 86_400;
    let rem = total_secs % 86_400;
    let hour = rem / 3_600;
    let min = (rem % 3_600) / 60;
    let sec = rem % 60;
    let Some((year, month, day)) = civil_date_from_unix_days(days) else {
        return String::from("1970-01-01T00:00:00Z");
    };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Civil date from days since Unix epoch. Howard Hinnant's algorithm.
fn civil_date_from_unix_days(days: u64) -> Option<(i32, u32, u32)> {
    let z = i64::try_from(days).ok()?.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe.checked_add(era.checked_mul(400)?)?;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y.checked_add(i64::from(m <= 2))?;
    let year = i32::try_from(year).ok()?;
    let month = u32::try_from(m).ok()?;
    let day = u32::try_from(d).ok()?;
    Some((year, month, day))
}

#[cfg(test)]
mod tests {
    use super::{
        ExtensionCapability, ExtensionCapabilityGrant, capability_approval_label,
        grant_confirmation_message, grant_id_from_selector, grant_path_in, grant_requested_at,
        load_grant_at, missing_capabilities, requested_capabilities, revoke_grant_at,
        store_grant_at,
    };
    use crate::extension::{ExtensionToolContribution, ExtensionToolRisk};
    use std::collections::BTreeSet;
    use std::fs;

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

    fn temp_grant_path(name: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("yach-cap-grant-{name}-{stamp}.json"))
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
        let requested = BTreeSet::from([
            ExtensionCapability::UsesNetwork,
            ExtensionCapability::RunsProcess,
        ]);
        assert_eq!(
            missing_capabilities(&requested, Some(&approved)),
            BTreeSet::from([ExtensionCapability::RunsProcess])
        );
    }

    #[test]
    fn a_narrowed_capability_set_stays_covered() {
        let approved = grant(&[
            ExtensionCapability::UsesNetwork,
            ExtensionCapability::RunsProcess,
        ]);
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

        let path = temp_grant_path("roundtrip");
        let stored = store_grant_at(&path, &original);
        assert!(stored.is_ok(), "grant should store: {stored:?}");
        let loaded = load_grant_at(&path);
        let _ = fs::remove_file(&path);
        assert!(loaded.is_some(), "stored grant should load");
        let Some(loaded) = loaded else { return };
        assert_eq!(loaded.approved, original.approved);
    }

    #[test]
    fn a_corrupt_grant_file_loads_as_none() {
        let path = temp_grant_path("corrupt");
        let write = fs::write(&path, "{not-json");
        assert!(write.is_ok(), "corrupt grant should write: {write:?}");
        let exists = path.is_file();
        let loaded = load_grant_at(&path);
        let _ = fs::remove_file(&path);
        assert!(
            exists,
            "corrupt grant file must exist so None is not a missing-path miss"
        );
        assert!(
            loaded.is_none(),
            "corrupt grant must not be treated as approval"
        );
    }

    #[test]
    fn granting_requested_capabilities_covers_them_and_revoking_uncovers() {
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        let requested = requested_capabilities(&tools);
        assert_eq!(
            requested,
            BTreeSet::from([ExtensionCapability::UsesNetwork])
        );
        assert_eq!(missing_capabilities(&requested, None), requested);

        let path = temp_grant_path("grant-revoke");
        let stored = grant_requested_at(&path, "1.2.0", &tools);
        assert!(stored.is_ok(), "grant should store: {stored:?}");
        let Ok(stored) = stored else { return };
        assert!(stored.is_some(), "network tool must produce a grant");
        let Some(stored) = stored else { return };
        assert_eq!(stored.approved, requested);
        assert_eq!(stored.version_at_grant, "1.2.0");

        let loaded = load_grant_at(&path);
        assert!(
            missing_capabilities(&requested, loaded.as_ref()).is_empty(),
            "approved set must cover the request"
        );

        let revoked = revoke_grant_at(&path);
        let leftover = load_grant_at(&path);
        let _ = fs::remove_file(&path);
        assert!(revoked.is_ok(), "revoke should succeed: {revoked:?}");
        let Ok(had_grant) = revoked else { return };
        assert!(had_grant, "revoke must report that a grant was present");
        assert!(leftover.is_none(), "revoked grant must not load");
        assert_eq!(
            missing_capabilities(&requested, leftover.as_ref()),
            requested,
            "revoking must make the request missing again"
        );
    }

    #[test]
    fn granting_writes_nothing_when_no_capabilities_are_requested() {
        let tools = vec![tool("read", ExtensionToolRisk::ReadsLocalContent)];
        let path = temp_grant_path("empty-grant");
        let stored = grant_requested_at(&path, "1.0.0", &tools);
        let exists = path.is_file();
        let _ = fs::remove_file(&path);
        assert!(stored.is_ok(), "empty request should not error: {stored:?}");
        let Ok(stored) = stored else { return };
        assert!(
            stored.is_none(),
            "an extension that requests nothing must not get an approved grant"
        );
        assert!(
            !exists,
            "skip writing so diagnostics can keep absent distinct from an empty approved set"
        );
    }

    #[test]
    fn revoking_a_missing_grant_is_success() {
        let path = temp_grant_path("missing-revoke");
        let _ = fs::remove_file(&path);
        let revoked = revoke_grant_at(&path);
        assert!(
            revoked.is_ok(),
            "missing grant must not fail revoke: {revoked:?}"
        );
        let Ok(had_grant) = revoked else { return };
        assert!(
            !had_grant,
            "missing grant must report that none was present"
        );
    }

    #[test]
    fn grant_confirmation_names_capabilities_and_tools() {
        let tools = vec![
            tool("fetch_url", ExtensionToolRisk::UsesNetwork),
            tool("run_helper", ExtensionToolRisk::RunsProcess),
        ];
        let approved = BTreeSet::from([
            ExtensionCapability::UsesNetwork,
            ExtensionCapability::RunsProcess,
        ]);
        let label = capability_approval_label(&tools, &approved);
        assert!(
            label.contains("uses_network (fetch_url)"),
            "label must name the network tool: {label}"
        );
        assert!(
            label.contains("runs_process (run_helper)"),
            "label must name the process tool: {label}"
        );
        let message = grant_confirmation_message("example.net", &tools, &approved);
        assert!(
            message.contains("approved") && message.contains("example.net"),
            "confirmation must name the extension: {message}"
        );
        assert!(
            !message.contains("sandbox")
                && !message.contains("restricted")
                && !message.contains("limited to")
                && !message.contains("safe"),
            "confirmation must not overstate: {message}"
        );
    }

    #[test]
    fn revoking_an_undiscovered_absolute_path_does_not_delete_outside_the_grant_dir() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let grants_dir =
            std::env::temp_dir().join(format!("yach-cap-grants-{}-{}", std::process::id(), stamp));
        let selector_stem =
            std::env::temp_dir().join(format!("yach-cap-escape-{}-{}", std::process::id(), stamp));
        let Some(selector) = selector_stem.to_str() else {
            return;
        };
        let sentinel = std::path::PathBuf::from(format!("{selector}.json"));
        let write = fs::write(&sentinel, b"keep");
        assert!(write.is_ok(), "sentinel should write: {write:?}");

        let resolved = grant_id_from_selector(None, selector);
        if let Ok(id) = &resolved
            && let Some(path) = grant_path_in(&grants_dir, id)
        {
            let _ = revoke_grant_at(&path);
        }
        let still_there = sentinel.is_file();
        let _ = fs::remove_file(&sentinel);
        let _ = fs::remove_dir_all(&grants_dir);

        assert!(
            resolved.is_err(),
            "an undiscovered path must not be treated as a grant id: {resolved:?}"
        );
        let Err(message) = resolved else { return };
        assert!(
            message.contains("Pass the extension id"),
            "failure must tell the user to pass the extension id: {message}"
        );
        assert!(
            !message.contains("no capability grant recorded"),
            "must not look like a successful empty revoke: {message}"
        );
        assert!(still_there, "sentinel outside the grant dir must survive");
    }

    #[test]
    fn revoking_a_discovered_extension_by_package_root_uses_the_manifest_id() {
        let resolved = grant_id_from_selector(Some("example.toy-tools"), "/tmp/example-pkg");
        assert_eq!(
            resolved.as_deref(),
            Ok("example.toy-tools"),
            "package-root selectors resolve before the undiscovered fallback: {resolved:?}"
        );
    }

    #[test]
    fn an_undiscovered_valid_id_is_still_a_grant_target() {
        let resolved = grant_id_from_selector(None, "example.uninstalled");
        assert_eq!(
            resolved.as_deref(),
            Ok("example.uninstalled"),
            "uninstall-by-id must still reach the grant file: {resolved:?}"
        );
    }
}
