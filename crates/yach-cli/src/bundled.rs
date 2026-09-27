//! Materialization of bundled extension packages under `~/.yach/bundled`.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use yach_backend::{Component, ExtensionInstallStore};

pub(crate) struct BundledPackage {
    #[expect(dead_code, reason = "consumed by Task 7/8 component routing")]
    pub component: Component,
    pub source: &'static str,
    pub dir_name: &'static str,
    pub host_arg: &'static str,
    pub manifest_json: fn() -> Option<&'static str>,
}

pub(crate) const BUNDLED: [BundledPackage; 2] = [
    BundledPackage {
        component: Component::Hashline,
        source: "yach.hashline",
        dir_name: "yach-hashline",
        host_arg: "hashline",
        manifest_json: || Some(yach_hashline_extension::MANIFEST_JSON),
    },
    BundledPackage {
        component: Component::JevReviewer,
        source: "yach.jev-reviewer",
        dir_name: "yach-jev-reviewer",
        host_arg: "jev",
        manifest_json: || Some(yach_jev_reviewer::MANIFEST_JSON),
    },
];

/// Materializes the bundled package under `home/.yach/bundled/<dir>/<version>`,
/// writing the extension manifest atomically. Returns `None` when the package
/// is not compiled in.
pub(crate) fn materialize(home: &Path, package: &BundledPackage) -> io::Result<Option<PathBuf>> {
    let Some(manifest_json) = (package.manifest_json)() else {
        return Ok(None);
    };
    let root = home
        .join(".yach/bundled")
        .join(package.dir_name)
        .join(env!("CARGO_PKG_VERSION"));
    std::fs::create_dir_all(&root)?;
    #[cfg(unix)]
    std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;

    let executable = std::env::current_exe()?;
    let executable = executable.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "yach executable path is not UTF-8",
        )
    })?;
    let mut manifest =
        serde_json::from_str::<serde_json::Value>(manifest_json).map_err(io::Error::other)?;
    manifest["main"]["command"] = serde_json::Value::String(executable.to_owned());
    manifest["main"]["args"] = serde_json::json!(["__extension-host", package.host_arg]);
    let mut bytes = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    bytes.push(b'\n');

    let manifest_path = root.join("yach.extension.json");
    if std::fs::read(&manifest_path).ok().as_deref() != Some(bytes.as_slice()) {
        let temp_path = root.join(format!(".yach.extension.json.{}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp_path, &manifest_path)?;
    }
    #[cfg(unix)]
    std::fs::set_permissions(
        &manifest_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;

    Ok(Some(root))
}

/// The version segment of a materialized package root: its last path component.
pub(crate) fn materialized_version(package_root: &Path) -> Option<&str> {
    package_root.file_name()?.to_str()
}

/// Re-materializes and repoints bundled records whose stored version differs
/// from the running binary's version. Packages whose source is in `removed` are
/// left untouched. Returns whether the store changed.
pub(crate) fn refresh_on_upgrade(
    home: &Path,
    store: &mut ExtensionInstallStore,
    removed: &BTreeSet<String>,
) -> io::Result<bool> {
    let mut changed = false;
    for package in &BUNDLED {
        if removed.contains(package.source) {
            continue;
        }
        let has_record = store
            .records
            .iter()
            .any(|record| record.source == package.source);
        if !has_record {
            continue;
        }
        let stale = store
            .records
            .iter()
            .find(|record| record.source == package.source)
            .and_then(|record| materialized_version(&record.package_root))
            != Some(env!("CARGO_PKG_VERSION"));
        if !stale {
            continue;
        }
        let Some(package_root) = materialize(home, package)? else {
            continue;
        };
        if store
            .refresh_bundled(package.source, &package_root)
            .map_err(|error| crate::extension_install_io_error(&error))?
        {
            changed = true;
        }
    }
    Ok(changed)
}

/// Materializes a bundled package and installs its record into `store`.
/// Returns whether a record was installed.
#[expect(dead_code, reason = "consumed by Task 7 preset apply")]
pub(crate) fn install(
    home: &Path,
    store: &mut ExtensionInstallStore,
    package: &BundledPackage,
) -> io::Result<bool> {
    let Some(package_root) = materialize(home, package)? else {
        return Ok(false);
    };
    store
        .install_bundled(
            package.source,
            &package_root,
            yach_backend::ExtensionInstallScope::User,
        )
        .map_err(|error| crate::extension_install_io_error(&error))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use yach_backend::{ExtensionInstallScope, ExtensionInstallStore};

    use super::{materialized_version, refresh_on_upgrade};

    fn temp_home(name: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "yach-bundled-{name}-{}-{stamp}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        assert!(std::fs::create_dir_all(&path).is_ok());
        path
    }

    #[test]
    fn upgrade_refresh_repoints_existing_and_skips_removed() {
        let home = temp_home("bundled-upgrade");
        let stale = home.join(".yach/bundled/yach-hashline/0.0.1");
        let stale_jev = home.join(".yach/bundled/yach-jev-reviewer/0.0.1");
        for dir in [&stale, &stale_jev] {
            assert!(std::fs::create_dir_all(dir).is_ok());
        }
        let mut store = ExtensionInstallStore::default();
        assert!(
            store
                .install_bundled("yach.hashline", &stale, ExtensionInstallScope::User)
                .is_ok()
        );
        assert!(
            store
                .install_bundled("yach.jev-reviewer", &stale_jev, ExtensionInstallScope::User)
                .is_ok()
        );
        let removed = BTreeSet::from([String::from("yach.jev-reviewer")]);

        let changed = refresh_on_upgrade(&home, &mut store, &removed);
        assert!(matches!(changed, Ok(true)));
        let hashline = store.records.iter().find(|r| r.source == "yach.hashline");
        assert!(hashline.is_some_and(
            |r| materialized_version(&r.package_root) == Some(env!("CARGO_PKG_VERSION"))
        ));
        assert!(hashline.is_some_and(|r| r.package_root.join("yach.extension.json").is_file()));
        let jev = store
            .records
            .iter()
            .find(|r| r.source == "yach.jev-reviewer");
        assert!(
            jev.is_some_and(|r| r.package_root.ends_with("0.0.1")),
            "removed ids are untouched"
        );

        let unchanged = refresh_on_upgrade(&home, &mut store, &removed);
        assert!(matches!(unchanged, Ok(false)), "second pass is a no-op");
    }
}
