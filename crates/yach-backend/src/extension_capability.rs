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
/// but must never grant it.
#[must_use]
pub fn grant_path(extension_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".yach")
            .join("extensions")
            .join(format!("{extension_id}.json")),
    )
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

#[cfg(test)]
mod tests {
    use super::{
        ExtensionCapability, ExtensionCapabilityGrant, load_grant_at, missing_capabilities,
        requested_capabilities, store_grant_at,
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
}
