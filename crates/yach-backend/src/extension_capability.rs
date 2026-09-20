//! Extension capability declarations and the user grants that admit them.
//!
//! A capability is a *declaration* the kernel records and gates activation
//! on. It is not enforced: the extension host is an ordinary subprocess with
//! the agent's privileges and can use the network whether or not it declared
//! doing so. See
//! `docs/project/specs/2026-09-16-extension-capability-contract-design.md`.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::extension::{ExtensionToolContribution, ExtensionToolRisk};

mod store;
pub use store::{ExtensionAuthorityError, ExtensionAuthorityStore, ExtensionDecisionSurface};

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
    ExtensionAuthorityStore::for_current_user()
        .ok()?
        .load_grant(extension_id)
        .ok()?
}

/// Approve the capabilities requested by `tools`. Writes nothing when the
/// extension requests none, so an absent grant stays absent.
pub fn grant_requested(
    extension_id: &str,
    version: &str,
    tools: &[ExtensionToolContribution],
    surface: ExtensionDecisionSurface,
) -> Result<Option<ExtensionCapabilityGrant>, ExtensionAuthorityError> {
    ExtensionAuthorityStore::for_current_user()?.grant_requested(
        extension_id,
        version,
        tools,
        surface,
    )
}

/// Revoke current authority. Returns whether an active grant existed.
pub fn revoke_grant(
    extension_id: &str,
    surface: ExtensionDecisionSurface,
) -> Result<bool, ExtensionAuthorityError> {
    ExtensionAuthorityStore::for_current_user()?.revoke_grant(extension_id, surface)
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

pub(super) fn utc_timestamp_now() -> String {
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
        ExtensionAuthorityStore, ExtensionCapability, ExtensionCapabilityGrant,
        ExtensionDecisionSurface, capability_approval_label, grant_confirmation_message,
        grant_id_from_selector, missing_capabilities, requested_capabilities,
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
        let home =
            std::env::temp_dir().join(format!("yach-cap-escape-home-{}", uuid::Uuid::new_v4()));
        assert!(fs::create_dir_all(&home).is_ok());
        let selector_stem = home.join("outside");
        let Some(selector) = selector_stem.to_str() else {
            let _ = fs::remove_dir_all(&home);
            return;
        };
        let sentinel = std::path::PathBuf::from(format!("{selector}.json"));
        let write = fs::write(&sentinel, b"keep");
        assert!(write.is_ok(), "sentinel should write: {write:?}");

        let resolved = grant_id_from_selector(None, selector);
        if let Ok(id) = &resolved {
            let store = ExtensionAuthorityStore::in_home(&home);
            let _ = store.revoke_grant(id, ExtensionDecisionSurface::Cli);
        }
        let still_there = sentinel.is_file();
        let _ = fs::remove_file(&sentinel);
        let _ = fs::remove_dir_all(&home);

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
