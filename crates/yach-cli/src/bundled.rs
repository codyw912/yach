//! Materialization of bundled extension packages under `~/.yach/bundled`.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use yach_backend::{
    Component, ComponentSet, ExtensionInstallStore, ExtensionPackageRoot, Preset, UserConfigStore,
};

pub(crate) struct BundledPackage {
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
        manifest_json: hashline_manifest_json,
    },
    BundledPackage {
        component: Component::JevReviewer,
        source: "yach.jev-reviewer",
        dir_name: "yach-jev-reviewer",
        host_arg: "jev",
        manifest_json: jev_manifest_json,
    },
];

#[cfg(feature = "bundled-hashline")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the not-compiled-in sibling returns None"
)]
fn hashline_manifest_json() -> Option<&'static str> {
    Some(yach_hashline_extension::MANIFEST_JSON)
}

#[cfg(not(feature = "bundled-hashline"))]
fn hashline_manifest_json() -> Option<&'static str> {
    None
}

#[cfg(feature = "bundled-jev")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the not-compiled-in sibling returns None"
)]
fn jev_manifest_json() -> Option<&'static str> {
    Some(yach_jev_reviewer::MANIFEST_JSON)
}

#[cfg(not(feature = "bundled-jev"))]
fn jev_manifest_json() -> Option<&'static str> {
    None
}

/// Whether a bundled extension source is compiled into this build.
pub(crate) fn is_compiled_in(source: &str) -> bool {
    BUNDLED
        .iter()
        .any(|package| package.source == source && (package.manifest_json)().is_some())
}

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
/// Re-materializes and repoints bundled records so the manifest's
/// `main.command` always names the running binary. Packages whose source is
/// in `removed` are left untouched. `materialize` skips an unchanged write
/// and `refresh_bundled` is a no-op when the root is unchanged, so the
/// steady state stays write-free. Returns whether the store changed.
///
/// `#[cfg(not(test))]` in `main.rs` hides its only production caller during
/// test builds; the tests exercise it directly.
#[cfg_attr(
    all(test, not(feature = "bundled-hashline")),
    expect(
        dead_code,
        reason = "only called from #[cfg(not(test))] code and bundled-hashline tests"
    )
)]
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

/// The outcome of applying a preset to user state: which bundled ids were
/// installed, re-enabled, disabled, preserved as-is, or unavailable in this
/// build.
pub(crate) struct PresetApplyReport {
    pub preset: Preset,
    pub installed: Vec<&'static str>,
    pub enabled: Vec<&'static str>,
    pub disabled: Vec<&'static str>,
    pub not_compiled_in: Vec<&'static str>,
    pub preserved: Vec<&'static str>,
}

/// Reconciles the bundled install store with `preset`, then persists the
/// preset marker. A record is never deleted here — exclusion only disables,
/// and `[bundled] removed` ids are skipped unless `reset` ignores them.
/// `preserve_existing` is true only for the implicit first run: an existing
/// record keeps its `enabled` value then; an explicit apply always enables
/// the preset's components. The marker is written last so a store failure
/// leaves `ensure_first_run` free to retry on the next start.
pub(crate) fn apply_preset(
    home: &Path,
    config: &UserConfigStore,
    store_path: &Path,
    preset: Preset,
    reset: bool,
    preserve_existing: bool,
) -> io::Result<PresetApplyReport> {
    // Validate the whole config up front: a malformed known field must fail
    // the apply before any bundled record changes. `reset` ignores the
    // removed set but never the validation.
    let snapshot = config.load().map_err(io::Error::other)?;
    let bundled_removed = if reset {
        BTreeSet::new()
    } else {
        snapshot.bundled_removed
    };
    let included = ComponentSet::from_preset(preset);
    let mut store = ExtensionInstallStore::load_from_path(store_path)
        .map_err(|error| crate::extension_install_io_error(&error))?;
    let mut report = PresetApplyReport {
        preset,
        installed: Vec::new(),
        enabled: Vec::new(),
        disabled: Vec::new(),
        not_compiled_in: Vec::new(),
        preserved: Vec::new(),
    };
    for package in &BUNDLED {
        let has_record = store
            .records
            .iter()
            .any(|record| record.source == package.source);
        if (package.manifest_json)().is_none() {
            // A package this build omits is never materialized and never
            // gains a record. An existing record still records the
            // preference so a later full build applies it.
            if included.contains(package.component) && !bundled_removed.contains(package.source) {
                if has_record {
                    if preserve_existing {
                        report.preserved.push(package.source);
                    } else {
                        store
                            .set_enabled(package.source, true)
                            .map_err(|error| crate::extension_install_io_error(&error))?;
                        report.enabled.push(package.source);
                    }
                }
                report.not_compiled_in.push(package.source);
            } else if !included.contains(package.component) && has_record {
                store
                    .set_enabled(package.source, false)
                    .map_err(|error| crate::extension_install_io_error(&error))?;
                report.disabled.push(package.source);
            }
            continue;
        }
        if included.contains(package.component) {
            if bundled_removed.contains(package.source) {
                continue;
            }
            if has_record {
                if preserve_existing {
                    report.preserved.push(package.source);
                } else {
                    store
                        .set_enabled(package.source, true)
                        .map_err(|error| crate::extension_install_io_error(&error))?;
                    report.enabled.push(package.source);
                }
            } else if install(home, &mut store, package)? {
                report.installed.push(package.source);
            } else {
                report.not_compiled_in.push(package.source);
            }
        } else if has_record {
            store
                .set_enabled(package.source, false)
                .map_err(|error| crate::extension_install_io_error(&error))?;
            report.disabled.push(package.source);
        }
    }
    store
        .save_to_path(store_path)
        .map_err(|error| crate::extension_install_io_error(&error))?;
    config
        .persist_preset(preset, reset, preserve_existing)
        .map_err(io::Error::other)?;
    Ok(report)
}

/// First run only: a config without `[preset] applied` gets `full` applied
/// once, preserving existing records' `enabled` values. Returns `None` on
/// any later run or when the config cannot load.
pub(crate) fn ensure_first_run(
    home: &Path,
    config: &UserConfigStore,
    store_path: &Path,
) -> io::Result<Option<PresetApplyReport>> {
    let snapshot = match config.load() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "warning: failed to load user config for preset apply: {error}"
            );
            return Ok(None);
        }
    };
    if snapshot.preset_applied.is_some() {
        return Ok(None);
    }
    apply_preset(home, config, store_path, Preset::Full, false, true).map(Some)
}

/// The component set for one session: the ephemeral preset when given, else
/// the persisted kernel components. Never writes user state.
pub(crate) fn session_components(
    config: &UserConfigStore,
    ephemeral: Option<Preset>,
) -> ComponentSet {
    if let Some(preset) = ephemeral {
        return ComponentSet::from_preset(preset);
    }
    match config.load() {
        Ok(snapshot) => {
            warn_unknown_components(&snapshot.unknown_components);
            snapshot.kernel_components()
        }
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "warning: failed to load user config for components: {error}"
            );
            ComponentSet::full()
        }
    }
}

/// Resolves an ephemeral `--preset` session's package roots in memory:
/// `persisted` non-bundled roots (read without refresh, without save) pass
/// through, and every bundled package the preset includes and this build
/// compiles is materialized under `~/.yach/bundled` — a cache, not user
/// state. Persisted bundled records and `[bundled] removed` are ignored.
pub(crate) fn ephemeral_package_roots(
    home: &Path,
    preset: Preset,
    persisted: Vec<ExtensionPackageRoot>,
) -> io::Result<Vec<ExtensionPackageRoot>> {
    let included = ComponentSet::from_preset(preset);
    let mut roots = persisted;
    for package in &BUNDLED {
        if !included.contains(package.component) {
            continue;
        }
        if let Some(root) = materialize(home, package)? {
            roots.push(ExtensionPackageRoot {
                root,
                scope: yach_backend::ExtensionInstallScope::User,
                source_ref: Some(String::from(package.source)),
            });
        }
    }
    Ok(roots)
}

/// One stderr warning per unknown component name in the user config.
pub(crate) fn warn_unknown_components(unknown: &[String]) {
    for name in unknown {
        let _ = writeln!(
            io::stderr(),
            "warning: unknown component '{name}' in ~/.yach/config.toml ignored"
        );
    }
}

#[cfg(test)]
mod tests {
    use yach_backend::{ExtensionInstallStore, Preset, UserConfigStore};

    use super::{apply_preset, ensure_first_run};
    // Used by tests that exist whenever hashline is absent (the
    // not-compiled-in cases) or whenever both packages materialize.
    #[cfg(any(not(feature = "bundled-hashline"), feature = "bundled-jev"))]
    use yach_backend::ExtensionInstallScope;

    #[cfg(all(feature = "bundled-hashline", feature = "bundled-jev"))]
    use super::ephemeral_package_roots;
    #[cfg(feature = "bundled-hashline")]
    use super::{BUNDLED, install, refresh_on_upgrade};
    #[cfg(feature = "bundled-hashline")]
    use std::collections::BTreeSet;

    /// The version segment of a materialized package root: its last path
    /// component.
    #[cfg(all(feature = "bundled-hashline", feature = "bundled-jev"))]
    fn materialized_version(package_root: &std::path::Path) -> Option<&str> {
        package_root.file_name()?.to_str()
    }

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

    #[cfg(all(feature = "bundled-hashline", feature = "bundled-jev"))]
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

    #[cfg(feature = "bundled-hashline")]
    #[test]
    fn upgrade_refresh_repaints_manifest_command_at_the_same_version() {
        let home = temp_home("bundled-same-version");
        let mut store = ExtensionInstallStore::default();
        let package = &BUNDLED[0]; // hashline
        assert!(matches!(install(&home, &mut store, package), Ok(true)));
        // Simulate a manifest installed by an older build at the same
        // version: its `main.command` points at a binary that is gone.
        let manifest_path = home
            .join(".yach/bundled/yach-hashline")
            .join(env!("CARGO_PKG_VERSION"))
            .join("yach.extension.json");
        let Ok(raw) = std::fs::read_to_string(&manifest_path) else {
            unreachable!("materialized manifest exists")
        };
        let Ok(mut manifest) = serde_json::from_str::<serde_json::Value>(&raw) else {
            unreachable!("materialized manifest is JSON")
        };
        manifest["main"]["command"] =
            serde_json::Value::String(String::from("/nonexistent/old-yach"));
        let Ok(bytes) = serde_json::to_vec(&manifest) else {
            unreachable!("manifest serializes")
        };
        assert!(std::fs::write(&manifest_path, bytes).is_ok());

        let changed = refresh_on_upgrade(&home, &mut store, &BTreeSet::new());
        assert!(matches!(changed, Ok(false)), "record root is unchanged");
        let Ok(written_raw) = std::fs::read_to_string(&manifest_path) else {
            unreachable!("manifest still readable")
        };
        let Ok(written) = serde_json::from_str::<serde_json::Value>(&written_raw) else {
            unreachable!("manifest still JSON")
        };
        let command = written["main"]["command"].as_str();
        assert_eq!(
            command,
            std::env::current_exe()
                .ok()
                .as_deref()
                .and_then(|p| p.to_str()),
            "same-version refresh repaints main.command onto the running binary"
        );
    }

    #[cfg(all(feature = "bundled-hashline", feature = "bundled-jev"))]
    #[test]
    fn first_apply_preserves_existing_disabled_record() {
        let home = temp_home("first-apply");
        let config = UserConfigStore::in_home(&home);
        let store_path = home.join(".yach/extensions.json");
        let mut store = ExtensionInstallStore::default();
        let package = &BUNDLED[0]; // hashline
        assert!(matches!(install(&home, &mut store, package), Ok(true)));
        assert!(store.set_enabled("yach.hashline", false).is_ok());
        assert!(store.save_to_path(&store_path).is_ok());

        let report = ensure_first_run(&home, &config, &store_path);
        assert!(
            matches!(&report, Ok(Some(r)) if r.preset == Preset::Full && r.preserved.contains(&"yach.hashline"))
        );
        let Ok(store) = ExtensionInstallStore::load_from_path(&store_path) else {
            unreachable!()
        };
        let hashline = store.records.iter().find(|r| r.source == "yach.hashline");
        assert!(
            hashline.is_some_and(|r| !r.enabled),
            "existing choice preserved"
        );
        assert!(
            store
                .records
                .iter()
                .any(|r| r.source == "yach.jev-reviewer" && r.enabled)
        );
        assert!(
            matches!(ensure_first_run(&home, &config, &store_path), Ok(None)),
            "runs once"
        );
    }

    #[cfg(feature = "bundled-hashline")]
    #[test]
    fn preset_use_never_readds_removed_until_reset() {
        let home = temp_home("preset-removed");
        let config = UserConfigStore::in_home(&home);
        let store_path = home.join(".yach/extensions.json");
        assert!(
            config
                .persist_bundled_removed("yach.hashline", true)
                .is_ok()
        );

        let report = apply_preset(&home, &config, &store_path, Preset::Full, false, false);
        assert!(matches!(&report, Ok(r) if !r.installed.contains(&"yach.hashline")));
        let Ok(store) = ExtensionInstallStore::load_from_path(&store_path) else {
            unreachable!()
        };
        assert!(store.records.iter().all(|r| r.source != "yach.hashline"));

        let reset = apply_preset(&home, &config, &store_path, Preset::Full, true, false);
        assert!(matches!(&reset, Ok(r) if r.installed.contains(&"yach.hashline")));
    }

    #[cfg(all(feature = "bundled-hashline", feature = "bundled-jev"))]
    #[test]
    fn ephemeral_roots_materialize_included_without_touching_the_store() {
        let home = temp_home("ephemeral-full");
        let store_path = home.join(".yach/extensions.json");
        let mut store = ExtensionInstallStore::default();
        let stale = home.join(".yach/bundled/yach-hashline/0.0.1");
        assert!(std::fs::create_dir_all(&stale).is_ok());
        let local = home.join("third-party");
        assert!(std::fs::create_dir_all(&local).is_ok());
        assert!(
            store
                .install_bundled("yach.hashline", &stale, ExtensionInstallScope::User)
                .is_ok()
        );
        assert!(
            store
                .install_local_path(
                    "/opt/example.third-party",
                    &local,
                    ExtensionInstallScope::User,
                    true,
                )
                .is_ok()
        );
        assert!(store.save_to_path(&store_path).is_ok());
        let before = std::fs::read(&store_path).unwrap_or_default();

        let persisted: Vec<yach_backend::ExtensionPackageRoot> = store
            .records
            .iter()
            .filter(|r| r.enabled)
            .filter(|r| r.kind == yach_backend::ExtensionInstallRefKind::LocalPath)
            .map(|r| yach_backend::ExtensionPackageRoot {
                root: r.package_root.clone(),
                scope: r.scope,
                source_ref: Some(r.source.clone()),
            })
            .collect();
        let roots = ephemeral_package_roots(&home, Preset::Full, persisted);
        let Ok(roots) = roots else { unreachable!() };
        let sources: Vec<_> = roots
            .iter()
            .filter_map(|r| r.source_ref.as_deref())
            .collect();
        assert!(
            sources.contains(&"/opt/example.third-party"),
            "non-bundled persisted roots pass through"
        );
        for id in ["yach.hashline", "yach.jev-reviewer"] {
            let root = roots.iter().find(|r| r.source_ref.as_deref() == Some(id));
            assert!(
                root.is_some_and(|r| {
                    materialized_version(&r.root) == Some(env!("CARGO_PKG_VERSION"))
                        && r.root.join("yach.extension.json").is_file()
                }),
                "{id} is materialized at the current version in memory"
            );
        }
        assert_eq!(
            std::fs::read(&store_path).unwrap_or_default(),
            before,
            "ephemeral resolution writes no user state"
        );

        let minimal = ephemeral_package_roots(&home, Preset::Minimal, Vec::new());
        let Ok(minimal) = minimal else { unreachable!() };
        assert!(
            minimal.iter().all(|r| {
                !matches!(
                    r.source_ref.as_deref(),
                    Some("yach.hashline" | "yach.jev-reviewer")
                )
            }),
            "minimal contributes no bundled roots"
        );
    }

    #[test]
    fn first_run_store_load_failure_leaves_preset_unapplied() {
        let home = temp_home("first-run-unwritable");
        let config = UserConfigStore::in_home(&home);
        // A directory at the store path fails the load (read on a dir).
        let store_path = home.join(".yach/extensions.json");
        assert!(std::fs::create_dir_all(&store_path).is_ok());

        let report = ensure_first_run(&home, &config, &store_path);
        assert!(report.is_err(), "store load failure propagates");
        let Ok(snapshot) = config.load() else {
            unreachable!()
        };
        assert_eq!(
            snapshot.preset_applied, None,
            "the preset marker is written only after the store save"
        );
    }

    #[test]
    fn first_run_save_failure_leaves_preset_unapplied() {
        let home = temp_home("first-run-readonly");
        let config = UserConfigStore::in_home(&home);
        // A regular file where the store's parent should be: the store load
        // sees no file and returns the default, materialization writes under
        // ~/.yach/bundled fine, and the save fails in create_dir_all — a
        // deterministic, privilege-independent save failure.
        assert!(std::fs::write(home.join("not-a-dir"), b"").is_ok());
        let store_path = home.join("not-a-dir/extensions.json");

        let report = ensure_first_run(&home, &config, &store_path);
        assert!(report.is_err(), "store save failure propagates");
        #[cfg(any(feature = "bundled-hashline", feature = "bundled-jev"))]
        assert!(
            home.join(".yach/bundled").is_dir(),
            "reconciliation ran; only the store save failed"
        );
        let Ok(snapshot) = config.load() else {
            unreachable!()
        };
        assert_eq!(
            snapshot.preset_applied, None,
            "a failed save leaves the preset unapplied for next start"
        );
    }

    #[test]
    fn apply_preset_validates_config_before_touching_the_store() {
        let home = temp_home("preset-malformed-config");
        let config = UserConfigStore::in_home(&home);
        let store_path = home.join(".yach/extensions.json");
        let config_path = config.path().to_path_buf();
        assert!(std::fs::create_dir_all(config_path.parent().unwrap_or(&home)).is_ok());
        // A removed bundled id plus a malformed known field: the load fails,
        // so neither the removal is ignored nor the store is written.
        assert!(
            std::fs::write(
                &config_path,
                "[bundled]\nremoved = [\"yach.hashline\"]\n\n[components]\nproject-tools = \"yes\"\n",
            )
            .is_ok()
        );

        let report = apply_preset(&home, &config, &store_path, Preset::Full, false, false);
        assert!(
            report.is_err(),
            "malformed config fails before the store save"
        );
        assert!(
            !store_path.exists(),
            "extensions.json is untouched on a config load failure"
        );
    }

    #[cfg(all(feature = "bundled-hashline", feature = "bundled-jev"))]
    #[test]
    fn explicit_preset_use_reenables_existing_records() {
        let home = temp_home("preset-round-trip");
        let config = UserConfigStore::in_home(&home);
        let store_path = home.join(".yach/extensions.json");

        let minimal = apply_preset(&home, &config, &store_path, Preset::Minimal, false, false);
        assert!(matches!(&minimal, Ok(r) if r.preset == Preset::Minimal));
        // Minimal installs nothing; now seed both as the full preset would.
        let full = apply_preset(&home, &config, &store_path, Preset::Full, false, false);
        assert!(matches!(&full, Ok(r) if r.installed.len() == 2));
        let minimal_again =
            apply_preset(&home, &config, &store_path, Preset::Minimal, false, false);
        assert!(matches!(&minimal_again, Ok(r) if r.disabled.len() == 2));
        let Ok(store) = ExtensionInstallStore::load_from_path(&store_path) else {
            unreachable!()
        };
        assert!(store.records.iter().all(|r| !r.enabled));

        // The spec round-trip: an explicit `preset use full` after `minimal`
        // re-enables the existing records instead of preserving their
        // disabled state.
        let full_again = apply_preset(&home, &config, &store_path, Preset::Full, false, false);
        assert!(matches!(&full_again, Ok(r) if r.enabled.len() == 2 && r.preserved.is_empty()));
        let Ok(store) = ExtensionInstallStore::load_from_path(&store_path) else {
            unreachable!()
        };
        assert_eq!(store.records.len(), 2);
        assert!(store.records.iter().all(|r| r.enabled));
    }

    #[cfg(not(feature = "bundled-hashline"))]
    #[test]
    fn full_preset_reports_hashline_not_compiled_in() {
        let home = temp_home("not-compiled");
        let config = UserConfigStore::in_home(&home);
        let store_path = home.join(".yach/extensions.json");

        let report = apply_preset(&home, &config, &store_path, Preset::Full, false, false);
        assert!(
            matches!(&report, Ok(r) if r.not_compiled_in.contains(&"yach.hashline")),
            "the omitted bundled extension is reported, not installed"
        );
        let Ok(store) = ExtensionInstallStore::load_from_path(&store_path) else {
            unreachable!()
        };
        assert!(
            store.records.iter().all(|r| r.source != "yach.hashline"),
            "a core build never creates a record for an omitted package"
        );
        let Ok(snapshot) = config.load() else {
            unreachable!()
        };
        assert_eq!(
            snapshot.preset_applied,
            Some(Preset::Full),
            "preset marker recorded"
        );
    }

    #[cfg(not(feature = "bundled-hashline"))]
    #[test]
    fn full_preset_enables_existing_hashline_record_when_not_compiled_in() {
        let home = temp_home("not-compiled-existing");
        let config = UserConfigStore::in_home(&home);
        let store_path = home.join(".yach/extensions.json");
        // A record left behind by a full build: enabled preference is false,
        // and the record survives even though this build omits the package.
        let package_root = home.join(".yach/bundled/yach-hashline/0.0.1");
        assert!(std::fs::create_dir_all(&package_root).is_ok());
        let mut store = ExtensionInstallStore::default();
        assert!(
            store
                .install_bundled("yach.hashline", &package_root, ExtensionInstallScope::User)
                .is_ok()
        );
        assert!(store.set_enabled("yach.hashline", false).is_ok());
        assert!(store.save_to_path(&store_path).is_ok());

        let report = apply_preset(&home, &config, &store_path, Preset::Full, false, false);
        assert!(
            matches!(&report, Ok(r) if r.not_compiled_in.contains(&"yach.hashline")),
            "the omitted bundled extension is reported, not installed"
        );
        let Ok(store) = ExtensionInstallStore::load_from_path(&store_path) else {
            unreachable!()
        };
        let hashline = store.records.iter().find(|r| r.source == "yach.hashline");
        assert!(
            hashline.is_some_and(|r| r.enabled),
            "an explicit apply records the enabled preference for a later full build"
        );
    }
}
