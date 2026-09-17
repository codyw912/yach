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
    package_root: &Path,
    args: &[&str],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_yach"))
        .args(args)
        .env("HOME", home)
        .env("YACH_EXTENSION_USER_STORE", user_store)
        .env("YACH_EXTENSION_PROJECT_STORE", project_store)
        .env("YACH_EXTENSION_PACKAGE_ROOTS", package_root)
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
    let user_store = stores.root.join("user-extensions.json");
    let project_store = stores.root.join("project-extensions.json");
    fs::create_dir_all(&home).test_unwrap();
    copy_fixture(&package_root);

    let doctor = |home: &Path| {
        run_cli(
            home,
            &user_store,
            &project_store,
            &package_root,
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
        &package_root,
        &["extension", "trust", "example.capability-network"],
    );
    assert!(
        trust.status.success(),
        "trust should succeed: {}",
        String::from_utf8_lossy(&trust.stderr)
    );
    let trust_out = stdout(&trust);
    assert!(
        trust_out.contains("uses_network") && trust_out.contains("example.capability-network"),
        "trust must name the approved capability: {trust_out}"
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
        &package_root,
        &["extension", "revoke", "example.capability-network"],
    );
    assert!(
        revoke.status.success(),
        "revoke should succeed: {}",
        String::from_utf8_lossy(&revoke.stderr)
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
    assert!(
        doc["history"]
            .as_array()
            .is_some_and(|history| !history.is_empty()),
        "retained document must keep decision history: {doc}"
    );
}
