use std::fs;
use std::path::{Path, PathBuf};
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
        let record = ExtensionInstallRecord {
            source: source.to_owned(),
            kind: ExtensionInstallRefKind::LocalPath,
            scope,
            enabled,
            package_root: package_root.to_path_buf(),
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
        self.records.retain(|record| {
            record.source != selector && record.package_root != PathBuf::from(selector)
        });
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
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.source == selector || record.package_root == PathBuf::from(selector))
        else {
            return Err(ExtensionInstallError::RecordNotFound {
                selector: selector.to_owned(),
            });
        };
        record.enabled = enabled;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_install_ref_parses_local_paths() {
        let relative = parse_extension_install_ref("./extensions/fff").unwrap();
        assert_eq!(relative.kind, ExtensionInstallRefKind::LocalPath);
        assert_eq!(relative.normalized, "./extensions/fff");

        let absolute = parse_extension_install_ref("/tmp/yach-extension").unwrap();
        assert_eq!(absolute.kind, ExtensionInstallRefKind::LocalPath);
        assert_eq!(absolute.normalized, "/tmp/yach-extension");
    }

    #[test]
    fn extension_install_ref_parses_future_remote_refs() {
        let npm = parse_extension_install_ref("npm:@scope/pkg@1.2.3").unwrap();
        assert_eq!(npm.kind, ExtensionInstallRefKind::Npm);
        assert_eq!(npm.normalized, "npm:@scope/pkg@1.2.3");

        let git = parse_extension_install_ref("git:github.com/example/tools@v1").unwrap();
        assert_eq!(git.kind, ExtensionInstallRefKind::Git);
        assert_eq!(git.normalized, "git:github.com/example/tools@v1");

        let https = parse_extension_install_ref("https://github.com/example/tools").unwrap();
        assert_eq!(https.kind, ExtensionInstallRefKind::Git);
        assert_eq!(https.normalized, "https://github.com/example/tools");
    }

    #[test]
    fn extension_install_ref_rejects_empty_ref() {
        assert_eq!(
            parse_extension_install_ref(""),
            Err(ExtensionInstallError::EmptyRef)
        );
    }

    #[test]
    fn extension_install_store_round_trips_records() {
        let root = TempDir::new("store-round-trip");
        let package = root.path().join("packages/fff");
        fs::create_dir_all(&package).unwrap();
        let store_path = root.path().join("extensions.json");

        let mut store = ExtensionInstallStore::default();
        store
            .install_local_path(
                "./packages/fff",
                &package,
                ExtensionInstallScope::User,
                true,
            )
            .unwrap();
        store.save_to_path(&store_path).unwrap();

        let loaded = ExtensionInstallStore::load_from_path(&store_path).unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0].source, "./packages/fff");
        assert_eq!(loaded.records[0].kind, ExtensionInstallRefKind::LocalPath);
        assert_eq!(loaded.records[0].scope, ExtensionInstallScope::User);
        assert!(loaded.records[0].enabled);
        assert_eq!(loaded.records[0].package_root, package);
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
    fn extension_install_store_remove_enable_disable_by_source() {
        let root = TempDir::new("store-toggle");
        let package = root.path().join("fff");
        fs::create_dir_all(&package).unwrap();

        let mut store = ExtensionInstallStore::default();
        store
            .install_local_path("./fff", &package, ExtensionInstallScope::Project, true)
            .unwrap();

        store.set_enabled("./fff", false).unwrap();
        assert!(!store.records[0].enabled);

        store.set_enabled("./fff", true).unwrap();
        assert!(store.records[0].enabled);

        store.remove("./fff").unwrap();
        assert!(store.records.is_empty());
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "yach-extension-install-{name}-{}-{timestamp}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
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
