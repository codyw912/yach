use std::fs;
use std::io;
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
}
