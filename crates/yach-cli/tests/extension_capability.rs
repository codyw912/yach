use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

impl<T> TestUnwrap for Option<T> {
    type Output = T;

    fn test_unwrap(self) -> Self::Output {
        assert!(self.is_some());
        match self {
            Some(value) => value,
            None => unreachable!(),
        }
    }
}

struct TempPackage {
    root: PathBuf,
}

impl TempPackage {
    fn new() -> Result<Self, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "yach-capability-extension-integration-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        Ok(Self { root })
    }
}

impl Drop for TempPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn copy_fixture(dest: &Path) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/capability-network");
    fs::create_dir_all(dest).test_unwrap();
    fs::copy(
        src.join("yach.extension.json"),
        dest.join("yach.extension.json"),
    )
    .test_unwrap();
    fs::copy(src.join("host.sh"), dest.join("host.sh")).test_unwrap();
}

fn run_cli(
    home: &Path,
    user_store: &Path,
    project_store: &Path,
    cwd: &Path,
    args: &[&str],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_yach"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("YACH_EXTENSION_USER_STORE", user_store)
        .env("YACH_EXTENSION_PROJECT_STORE", project_store)
        .env_remove("YACH_EXTENSION_PACKAGE_ROOTS")
        .output()
        .test_unwrap()
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).test_unwrap()
}

#[cfg(unix)]
#[test]
fn cli_trust_revoke_persists_across_isolated_child_restarts() {
    let stores = TempPackage::new().test_unwrap();
    let home = stores.root.join("home");
    let package_root = stores.root.join("package");
    let cwd = stores.root.join("cwd");
    let user_store = stores.root.join("user-extensions.json");
    let project_store = stores.root.join("project-extensions.json");
    fs::create_dir_all(&home).test_unwrap();
    fs::create_dir_all(&cwd).test_unwrap();
    copy_fixture(&package_root);

    let package_arg = package_root.to_string_lossy();
    let installed = run_cli(
        &home,
        &user_store,
        &project_store,
        &cwd,
        &["extension", "install", &package_arg],
    );
    assert!(installed.status.success());
    let installed_out = stdout(&installed);
    assert!(
        installed_out.contains("extension_action=install")
            && installed_out.contains("extension_outcome=Completed"),
        "fixture install must complete: {installed_out}"
    );

    let doctor = |home: &Path| {
        run_cli(
            home,
            &user_store,
            &project_store,
            &cwd,
            &["extension", "doctor", "example.capability-network"],
        )
    };

    let blocked = doctor(&home);
    assert!(
        blocked.status.success(),
        "doctor should succeed before trust: {}",
        String::from_utf8_lossy(&blocked.stderr)
    );
    let blocked_out = stdout(&blocked);
    assert!(
        blocked_out.contains("capability_grant=none"),
        "ungranted doctor must show no grant: {blocked_out}"
    );

    let trust = run_cli(
        &home,
        &user_store,
        &project_store,
        &cwd,
        &["extension", "trust", "example.capability-network"],
    );
    assert!(
        trust.status.success(),
        "trust should succeed: {}",
        String::from_utf8_lossy(&trust.stderr)
    );
    let trust_out = stdout(&trust);
    assert!(
        trust_out.contains("extension_action=trust")
            && trust_out.contains("extension_outcome=Completed")
            && trust_out.contains("uses_network")
            && trust_out.contains("example.capability-network"),
        "trust output must correlate completed approval: {trust_out}"
    );

    let granted = doctor(&home);
    assert!(granted.status.success());
    let granted_out = stdout(&granted);
    assert!(
        granted_out.contains("capability_grant=uses_network"),
        "doctor after trust must show the grant: {granted_out}"
    );

    let restarted = doctor(&home);
    assert!(restarted.status.success());
    let restarted_out = stdout(&restarted);
    assert!(
        restarted_out.contains("capability_grant=uses_network"),
        "a new CLI process must still see the grant: {restarted_out}"
    );

    let revoke = run_cli(
        &home,
        &user_store,
        &project_store,
        &cwd,
        &["extension", "revoke", "example.capability-network"],
    );
    assert!(
        revoke.status.success(),
        "revoke should succeed: {}",
        String::from_utf8_lossy(&revoke.stderr)
    );
    let revoke_out = stdout(&revoke);
    assert!(
        revoke_out.contains("extension_action=revoke")
            && revoke_out.contains("extension_outcome=Completed"),
        "revoke output must correlate completed removal: {revoke_out}"
    );

    let after_revoke = doctor(&home);
    assert!(after_revoke.status.success());
    let after_out = stdout(&after_revoke);
    assert!(
        after_out.contains("capability_grant=none"),
        "doctor after revoke must deny current authority: {after_out}"
    );

    let document = home
        .join(".yach")
        .join("extensions")
        .join("example.capability-network.json");
    assert!(
        document.is_file(),
        "revoke must retain the authority document"
    );
    let bytes = fs::read(&document).test_unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).test_unwrap();
    assert!(doc["current"].is_null());
    let history = doc["history"].as_array().test_unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0]["action"], "grant");
    assert_eq!(history[0]["surface"], "cli");
    assert_eq!(history[1]["action"], "revoke");
    assert_eq!(history[1]["surface"], "cli");
    assert_eq!(history[1]["before"], history[0]["after"]);
    assert!(history[1]["after"].is_null());
}

#[cfg(unix)]
#[test]
fn cli_mutations_preserve_corrupt_and_unknown_authority_bytes() {
    use std::os::unix::fs::PermissionsExt as _;

    let stores = TempPackage::new().test_unwrap();
    let home = stores.root.join("home");
    let package_root = stores.root.join("package");
    let cwd = stores.root.join("cwd");
    let user_store = stores.root.join("user-extensions.json");
    let project_store = stores.root.join("project-extensions.json");
    fs::create_dir_all(&home).test_unwrap();
    fs::create_dir_all(&cwd).test_unwrap();
    copy_fixture(&package_root);
    let package_arg = package_root.to_string_lossy();
    let installed = run_cli(
        &home,
        &user_store,
        &project_store,
        &cwd,
        &["extension", "install", &package_arg],
    );
    assert!(stdout(&installed).contains("extension_outcome=Completed"));

    let extensions = home.join(".yach/extensions");
    fs::create_dir_all(&extensions).test_unwrap();
    fs::set_permissions(&home.join(".yach"), fs::Permissions::from_mode(0o700)).test_unwrap();
    fs::set_permissions(&extensions, fs::Permissions::from_mode(0o700)).test_unwrap();
    let authority = extensions.join("example.capability-network.json");

    for (bytes, action) in [
        (b"{not json".as_slice(), "trust"),
        (
            br#"{"schema":"yach.extension-authority.v0","current":null}"#.as_slice(),
            "revoke",
        ),
    ] {
        fs::write(&authority, bytes).test_unwrap();
        fs::set_permissions(&authority, fs::Permissions::from_mode(0o600)).test_unwrap();
        let output = run_cli(
            &home,
            &user_store,
            &project_store,
            &cwd,
            &["extension", action, "example.capability-network"],
        );
        let out = stdout(&output);
        assert!(
            out.contains("extension_outcome=Failed"),
            "{action} must report failed for planted state: {out}"
        );
        assert_eq!(fs::read(&authority).test_unwrap(), bytes);
    }
}
