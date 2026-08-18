//! UX-time ChatGPT auth-file path policy.

use std::{
    fs::{self, OpenOptions},
    io,
    path::Path,
};

/// Outcome of preparing the logical ChatGPT auth file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFilePreparation {
    /// No final entry exists.
    Missing,
    /// Regular file is present and 0600.
    Ready,
    /// Regular file mode was normalized to 0600.
    Tightened,
}

/// Final-entry states that require user confirmation or manual repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFileProblem {
    /// The final path component is a symlink.
    Symlink,
    /// The final path component is a directory or other non-regular file.
    NonRegular,
}

pub fn prepare_chatgpt_auth_file(logical: &Path) -> Result<AuthFilePreparation, AuthFileProblem> {
    let parent = logical.parent().ok_or(AuthFileProblem::NonRegular)?;
    fs::create_dir_all(parent).map_err(|_| AuthFileProblem::NonRegular)?;
    tighten_dir_mode(parent)?;
    let canonical = parent
        .canonicalize()
        .map_err(|_| AuthFileProblem::NonRegular)?;
    let file_name = logical.file_name().ok_or(AuthFileProblem::NonRegular)?;
    inspect_final_entry(&canonical.join(file_name))
}

fn tighten_dir_mode(path: &Path) -> Result<(), AuthFileProblem> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path).map_err(|_| AuthFileProblem::NonRegular)?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode != 0o700 {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(path, permissions).map_err(|_| AuthFileProblem::NonRegular)?;
        }
    }
    let _ = path;
    Ok(())
}

fn inspect_final_entry(path: &Path) -> Result<AuthFilePreparation, AuthFileProblem> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AuthFilePreparation::Missing);
        }
        Err(_) => return Err(AuthFileProblem::NonRegular),
    };
    if metadata.file_type().is_symlink() {
        return Err(AuthFileProblem::Symlink);
    }
    if !metadata.file_type().is_file() {
        return Err(AuthFileProblem::NonRegular);
    }
    if tighten_file_mode(path)? {
        Ok(AuthFilePreparation::Tightened)
    } else {
        Ok(AuthFilePreparation::Ready)
    }
}

fn tighten_file_mode(path: &Path) -> Result<bool, AuthFileProblem> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| AuthFileProblem::NonRegular)?;
        let metadata = file.metadata().map_err(|_| AuthFileProblem::NonRegular)?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode == 0o600 {
            return Ok(false);
        }
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)
            .map_err(|_| AuthFileProblem::NonRegular)?;
        Ok(true)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthFilePreparation, AuthFileProblem, prepare_chatgpt_auth_file};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

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

    fn temp_auth_file() -> (PathBuf, PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("yach-chatgpt-auth-{}", uuid::Uuid::new_v4()));
        assert!(fs::create_dir_all(&directory).is_ok());
        let path = directory.join("chatgpt-subscription.json");
        (directory, path)
    }

    #[test]
    fn missing_file_passes() {
        let (directory, path) = temp_auth_file();
        assert_eq!(
            prepare_chatgpt_auth_file(&path).test_unwrap(),
            AuthFilePreparation::Missing
        );
        let mode = fs::metadata(&directory).test_unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn regular_0600_is_ready() {
        let (directory, path) = temp_auth_file();
        assert!(fs::write(&path, b"{}").is_ok());
        let mut permissions = fs::metadata(&path).test_unwrap().permissions();
        permissions.set_mode(0o600);
        assert!(fs::set_permissions(&path, permissions).is_ok());
        assert_eq!(
            prepare_chatgpt_auth_file(&path).test_unwrap(),
            AuthFilePreparation::Ready
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn loose_and_restrictive_modes_are_tightened() {
        let (directory, path) = temp_auth_file();
        assert!(fs::write(&path, b"{}").is_ok());
        for mode in [0o644, 0o400] {
            let mut permissions = fs::metadata(&path).test_unwrap().permissions();
            permissions.set_mode(mode);
            assert!(fs::set_permissions(&path, permissions).is_ok());
            assert_eq!(
                prepare_chatgpt_auth_file(&path).test_unwrap(),
                AuthFilePreparation::Tightened
            );
            let after = fs::metadata(&path).test_unwrap().permissions().mode() & 0o777;
            assert_eq!(after, 0o600);
        }
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn symlink_file_is_repair() {
        let (directory, path) = temp_auth_file();
        let target = directory.join("target.json");
        assert!(fs::write(&target, b"{}").is_ok());
        assert!(std::os::unix::fs::symlink(&target, &path).is_ok());
        let Err(error) = prepare_chatgpt_auth_file(&path) else {
            unreachable!("symlink auth file must be rejected");
        };
        assert_eq!(error, AuthFileProblem::Symlink);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn directory_entry_is_manual_repair() {
        let (directory, path) = temp_auth_file();
        assert!(fs::create_dir_all(&path).is_ok());
        let Err(error) = prepare_chatgpt_auth_file(&path) else {
            unreachable!("directory auth path must be rejected");
        };
        assert_eq!(error, AuthFileProblem::NonRegular);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn parent_symlink_is_accepted() {
        let root =
            std::env::temp_dir().join(format!("yach-chatgpt-parent-{}", uuid::Uuid::new_v4()));
        let real = root.join("real");
        let link = root.join("link");
        assert!(fs::create_dir_all(&real).is_ok());
        assert!(std::os::unix::fs::symlink(&real, &link).is_ok());
        let path = link.join("chatgpt-subscription.json");
        assert_eq!(
            prepare_chatgpt_auth_file(&path).test_unwrap(),
            AuthFilePreparation::Missing
        );
        let _ = fs::remove_dir_all(root);
    }
}
