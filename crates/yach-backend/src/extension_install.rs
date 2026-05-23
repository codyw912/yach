use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{ExtensionInstallScope, ExtensionPackageRoot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionInstallRefKind {
    LocalPath,
    Npm,
    Git,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionInstallRef {
    pub kind: ExtensionInstallRefKind,
    pub normalized: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionInstallError {
    EmptyRef,
    UnsupportedRef { source: String },
    AdapterUnavailable { kind: ExtensionInstallRefKind },
    MissingLocalPath { path: PathBuf },
    StoreIo,
    StoreMalformed,
    RecordNotFound { selector: String },
}

pub fn parse_extension_install_ref(
    source: &str,
) -> Result<ExtensionInstallRef, ExtensionInstallError> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err(ExtensionInstallError::EmptyRef);
    }
    let kind = if trimmed.starts_with("npm:") {
        ExtensionInstallRefKind::Npm
    } else if trimmed.starts_with("git:")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("ssh://")
        || trimmed.starts_with("git@")
    {
        ExtensionInstallRefKind::Git
    } else if trimmed.starts_with('/')
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed == "."
        || trimmed == ".."
    {
        ExtensionInstallRefKind::LocalPath
    } else {
        return Err(ExtensionInstallError::UnsupportedRef {
            source: trimmed.to_owned(),
        });
    };
    Ok(ExtensionInstallRef {
        kind,
        normalized: trimmed.to_owned(),
    })
}

const EXTENSION_INSTALL_STORE_SCHEMA: &str = "yach.extensions.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionInstallStore {
    pub schema: String,
    pub records: Vec<ExtensionInstallRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionInstallRecord {
    pub source: String,
    pub kind: ExtensionInstallRefKind,
    pub scope: ExtensionInstallScope,
    pub enabled: bool,
    pub package_root: PathBuf,
}

impl Default for ExtensionInstallStore {
    fn default() -> Self {
        Self {
            schema: EXTENSION_INSTALL_STORE_SCHEMA.to_owned(),
            records: Vec::new(),
        }
    }
}

impl ExtensionInstallStore {
    pub fn load_from_path(path: &Path) -> Result<Self, ExtensionInstallError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(path).map_err(|_| ExtensionInstallError::StoreIo)?;
        let store: Self =
            serde_json::from_str(&contents).map_err(|_| ExtensionInstallError::StoreMalformed)?;
        if store.schema != EXTENSION_INSTALL_STORE_SCHEMA {
            return Err(ExtensionInstallError::StoreMalformed);
        }
        Ok(store)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), ExtensionInstallError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| ExtensionInstallError::StoreIo)?;
        }
        let contents =
            serde_json::to_string_pretty(self).map_err(|_| ExtensionInstallError::StoreIo)?;
        fs::write(path, contents).map_err(|_| ExtensionInstallError::StoreIo)
    }

    pub fn install_ref(
        &mut self,
        source: &str,
        scope: ExtensionInstallScope,
        enabled: bool,
    ) -> Result<(), ExtensionInstallError> {
        let parsed = parse_extension_install_ref(source)?;
        match parsed.kind {
            ExtensionInstallRefKind::LocalPath => {
                let root = PathBuf::from(&parsed.normalized);
                self.install_local_path(&parsed.normalized, &root, scope, enabled)
            }
            ExtensionInstallRefKind::Npm | ExtensionInstallRefKind::Git => {
                Err(ExtensionInstallError::AdapterUnavailable { kind: parsed.kind })
            }
        }
    }

    pub fn install_local_path(
        &mut self,
        source: &str,
        package_root: &Path,
        scope: ExtensionInstallScope,
        enabled: bool,
    ) -> Result<(), ExtensionInstallError> {
        if !package_root.is_dir() {
            return Err(ExtensionInstallError::MissingLocalPath {
                path: package_root.to_path_buf(),
            });
        }
        let package_root =
            fs::canonicalize(package_root).map_err(|_| ExtensionInstallError::StoreIo)?;
        let record = ExtensionInstallRecord {
            source: source.to_owned(),
            kind: ExtensionInstallRefKind::LocalPath,
            scope,
            enabled,
            package_root: package_root.clone(),
        };
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|record| record.source == source || record.package_root == package_root)
        {
            *existing = record;
        } else {
            self.records.push(record);
        }
        self.records
            .sort_by(|left, right| left.source.cmp(&right.source));
        Ok(())
    }

    pub fn remove(&mut self, selector: &str) -> Result<(), ExtensionInstallError> {
        let before = self.records.len();
        let selector_path = PathBuf::from(selector);
        self.records
            .retain(|record| record.source != selector && record.package_root != selector_path);
        if self.records.len() == before {
            return Err(ExtensionInstallError::RecordNotFound {
                selector: selector.to_owned(),
            });
        }
        Ok(())
    }

    pub fn set_enabled(
        &mut self,
        selector: &str,
        enabled: bool,
    ) -> Result<(), ExtensionInstallError> {
        let selector_path = PathBuf::from(selector);
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.source == selector || record.package_root == selector_path)
        else {
            return Err(ExtensionInstallError::RecordNotFound {
                selector: selector.to_owned(),
            });
        };
        record.enabled = enabled;
        Ok(())
    }

    pub fn enabled_package_roots(&self) -> Vec<ExtensionPackageRoot> {
        self.records
            .iter()
            .filter(|record| record.enabled)
            .filter(|record| record.kind == ExtensionInstallRefKind::LocalPath)
            .map(|record| ExtensionPackageRoot {
                root: record.package_root.clone(),
                scope: record.scope,
                source_ref: Some(record.source.clone()),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_install_ref_parses_local_paths() -> Result<(), String> {
        let relative = expect_ok(parse_extension_install_ref("./extensions/fff"))?;
        expect_equal(&relative.kind, &ExtensionInstallRefKind::LocalPath)?;
        expect_equal(&relative.normalized, &String::from("./extensions/fff"))?;

        let absolute = expect_ok(parse_extension_install_ref("/tmp/yach-extension"))?;
        expect_equal(&absolute.kind, &ExtensionInstallRefKind::LocalPath)?;
        expect_equal(&absolute.normalized, &String::from("/tmp/yach-extension"))
    }

    #[test]
    fn extension_install_ref_parses_future_remote_refs() -> Result<(), String> {
        let npm = expect_ok(parse_extension_install_ref("npm:@scope/pkg@1.2.3"))?;
        expect_equal(&npm.kind, &ExtensionInstallRefKind::Npm)?;
        expect_equal(&npm.normalized, &String::from("npm:@scope/pkg@1.2.3"))?;

        let git = expect_ok(parse_extension_install_ref(
            "git:github.com/example/tools@v1",
        ))?;
        expect_equal(&git.kind, &ExtensionInstallRefKind::Git)?;
        expect_equal(
            &git.normalized,
            &String::from("git:github.com/example/tools@v1"),
        )?;

        let https = expect_ok(parse_extension_install_ref(
            "https://github.com/example/tools",
        ))?;
        expect_equal(&https.kind, &ExtensionInstallRefKind::Git)?;
        expect_equal(
            &https.normalized,
            &String::from("https://github.com/example/tools"),
        )?;

        let http = expect_ok(parse_extension_install_ref(
            "http://github.com/example/tools",
        ))?;
        expect_equal(&http.kind, &ExtensionInstallRefKind::Git)?;
        expect_equal(
            &http.normalized,
            &String::from("http://github.com/example/tools"),
        )
    }

    #[test]
    fn extension_install_ref_rejects_empty_ref() {
        assert_eq!(
            parse_extension_install_ref(""),
            Err(ExtensionInstallError::EmptyRef)
        );
    }

    #[test]
    fn extension_install_store_round_trips_records() -> Result<(), String> {
        let root = TempDir::new("store-round-trip")?;
        let package = root.path().join("packages/fff");
        expect_ok(fs::create_dir_all(&package))?;
        let expected_package = expect_ok(fs::canonicalize(&package))?;
        let store_path = root.path().join("extensions.json");

        let mut store = ExtensionInstallStore::default();
        expect_ok(store.install_local_path(
            "./packages/fff",
            &package,
            ExtensionInstallScope::User,
            true,
        ))?;
        expect_ok(store.save_to_path(&store_path))?;

        let loaded = expect_ok(ExtensionInstallStore::load_from_path(&store_path))?;
        expect_equal(&loaded.records.len(), &1)?;
        expect_equal(&loaded.records[0].source, &String::from("./packages/fff"))?;
        expect_equal(&loaded.records[0].kind, &ExtensionInstallRefKind::LocalPath)?;
        expect_equal(&loaded.records[0].scope, &ExtensionInstallScope::User)?;
        expect_true(loaded.records[0].enabled, "record should be enabled")?;
        expect_equal(&loaded.records[0].package_root, &expected_package)
    }

    #[test]
    fn extension_install_store_rejects_unavailable_remote_adapters() {
        let mut store = ExtensionInstallStore::default();
        let error = store.install_ref("npm:fff", ExtensionInstallScope::User, true);
        assert_eq!(
            error,
            Err(ExtensionInstallError::AdapterUnavailable {
                kind: ExtensionInstallRefKind::Npm
            })
        );
    }

    #[test]
    fn extension_install_store_remove_enable_disable_by_source() -> Result<(), String> {
        let root = TempDir::new("store-toggle")?;
        let package = root.path().join("fff");
        expect_ok(fs::create_dir_all(&package))?;

        let mut store = ExtensionInstallStore::default();
        expect_ok(store.install_local_path(
            "./fff",
            &package,
            ExtensionInstallScope::Project,
            true,
        ))?;

        expect_ok(store.set_enabled("./fff", false))?;
        expect_true(!store.records[0].enabled, "record should be disabled")?;

        expect_ok(store.set_enabled("./fff", true))?;
        expect_true(store.records[0].enabled, "record should be enabled")?;

        expect_ok(store.remove("./fff"))?;
        expect_true(store.records.is_empty(), "records should be empty")
    }

    #[test]
    fn extension_install_store_enabled_package_roots_excludes_disabled_records()
    -> Result<(), String> {
        let root = TempDir::new("package-roots")?;
        let enabled = root.path().join("enabled");
        let disabled = root.path().join("disabled");
        expect_ok(fs::create_dir_all(&enabled))?;
        expect_ok(fs::create_dir_all(&disabled))?;
        let expected_enabled = expect_ok(fs::canonicalize(&enabled))?;

        let mut store = ExtensionInstallStore::default();
        expect_ok(store.install_local_path(
            "./enabled",
            &enabled,
            ExtensionInstallScope::User,
            true,
        ))?;
        expect_ok(store.install_local_path(
            "./disabled",
            &disabled,
            ExtensionInstallScope::User,
            false,
        ))?;

        let roots = store.enabled_package_roots();
        expect_equal(&roots.len(), &1)?;
        expect_equal(&roots[0].root, &expected_enabled)?;
        expect_equal(&roots[0].scope, &ExtensionInstallScope::User)?;
        expect_equal(&roots[0].source_ref.as_deref(), &Some("./enabled"))
    }

    #[test]
    fn extension_install_store_enabled_package_roots_preserves_project_scope() -> Result<(), String>
    {
        let root = TempDir::new("project-roots")?;
        let package = root.path().join("project-package");
        expect_ok(fs::create_dir_all(&package))?;

        let mut store = ExtensionInstallStore::default();
        expect_ok(store.install_local_path(
            "./project-package",
            &package,
            ExtensionInstallScope::Project,
            true,
        ))?;

        let roots = store.enabled_package_roots();
        expect_equal(&roots[0].scope, &ExtensionInstallScope::Project)
    }

    #[test]
    fn extension_install_store_canonicalizes_relative_package_roots() -> Result<(), String> {
        let _guard = cwd_lock()?;
        let root = TempDir::new("relative-roots")?;
        let package = root.path().join("relative-package");
        expect_ok(fs::create_dir_all(&package))?;
        let expected = expect_ok(fs::canonicalize(&package))?;

        with_current_dir(root.path(), || {
            let mut store = ExtensionInstallStore::default();
            expect_ok(store.install_ref(
                "./relative-package",
                ExtensionInstallScope::Project,
                true,
            ))?;

            expect_equal(
                &store.records[0].source,
                &String::from("./relative-package"),
            )?;
            expect_equal(&store.records[0].package_root, &expected)
        })
    }

    fn expect_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
        result.map_err(|error| format!("{error:?}"))
    }

    fn expect_equal<T>(actual: &T, expected: &T) -> Result<(), String>
    where
        T: std::fmt::Debug + PartialEq,
    {
        if actual == expected {
            Ok(())
        } else {
            Err(format!("expected {expected:?}, got {actual:?}"))
        }
    }

    fn expect_true(actual: bool, message: &str) -> Result<(), String> {
        if actual {
            Ok(())
        } else {
            Err(message.to_owned())
        }
    }

    fn cwd_lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
        static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        expect_ok(CWD_LOCK.lock())
    }

    fn with_current_dir(path: &Path, f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let original = expect_ok(std::env::current_dir())?;
        expect_ok(std::env::set_current_dir(path))?;
        let result = f();
        let restore_result = expect_ok(std::env::set_current_dir(original));
        result.and(restore_result)
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Result<Self, String> {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "yach-extension-install-{name}-{}-{timestamp}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            expect_ok(fs::create_dir_all(&path))?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
