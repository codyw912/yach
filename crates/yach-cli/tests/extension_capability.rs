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

fn run_cli_with_native_stores(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_yach"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("YACH_EXTENSION_USER_STORE")
        .env_remove("YACH_EXTENSION_PROJECT_STORE")
        .env_remove("YACH_EXTENSION_PACKAGE_ROOTS")
        .output()
        .test_unwrap()
}

fn write_install_store(path: &Path, records: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).test_unwrap();
    }
    fs::write(
        path,
        serde_json::json!({
            "schema": "yach.extensions.v1",
            "records": records,
        })
        .to_string(),
    )
    .test_unwrap();
}

fn install_record(package_root: &Path, scope: &str) -> serde_json::Value {
    serde_json::json!({
        "source": package_root.to_string_lossy(),
        "kind": "local_path",
        "scope": scope,
        "enabled": true,
        "package_root": package_root,
    })
}

fn write_fixture_manifest(dest: &Path, id: &str, tool: &str) {
    fs::create_dir_all(dest).test_unwrap();
    fs::write(
        dest.join("yach.extension.json"),
        serde_json::json!({
            "schema": "yach.extension.v1",
            "id": id,
            "version": "0.1.0",
            "main": { "command": "sh", "args": ["host.sh"] },
            "activation": { "events": [] },
            "contributes": {
                "tools": [{
                    "name": tool,
                    "description": "Test tool.",
                    "risk": "reads_local_metadata",
                    "provider_visible": false
                }]
            }
        })
        .to_string(),
    )
    .test_unwrap();
}

#[test]
fn native_store_shared_by_home_and_cwd_is_loaded_once() {
    let temp = TempPackage::new().test_unwrap();
    let home = temp.root.join("home");
    let package = temp.root.join("package");
    fs::create_dir_all(&home).test_unwrap();
    copy_fixture(&package);

    let installed = run_cli_with_native_stores(
        &home,
        &home,
        &["extension", "install", &package.to_string_lossy()],
    );
    assert!(installed.status.success(), "{}", stdout(&installed));

    let doctor = run_cli_with_native_stores(
        &home,
        &home,
        &["extension", "doctor", "example.capability-network"],
    );
    assert!(doctor.status.success(), "{}", stdout(&doctor));
    assert!(stdout(&doctor).contains("extension_outcome=Completed"));

    let trust = run_cli_with_native_stores(
        &home,
        &home,
        &["extension", "trust", "example.capability-network"],
    );
    assert!(trust.status.success(), "{}", stdout(&trust));
    assert!(stdout(&trust).contains("extension_outcome=Completed"));
}

#[cfg(unix)]
#[test]
fn native_store_shared_through_symlinked_cwd_is_loaded_once() {
    let temp = TempPackage::new().test_unwrap();
    let home = temp.root.join("home");
    let cwd_alias = temp.root.join("home-alias");
    let package = temp.root.join("package");
    fs::create_dir_all(&home).test_unwrap();
    std::os::unix::fs::symlink(&home, &cwd_alias).test_unwrap();
    copy_fixture(&package);

    let installed = run_cli_with_native_stores(
        &home,
        &cwd_alias,
        &["extension", "install", &package.to_string_lossy()],
    );
    assert!(installed.status.success(), "{}", stdout(&installed));

    for action in ["doctor", "trust"] {
        let output = run_cli_with_native_stores(
            &home,
            &cwd_alias,
            &["extension", action, "example.capability-network"],
        );
        assert!(output.status.success(), "{action}: {}", stdout(&output));
        assert!(stdout(&output).contains("extension_outcome=Completed"));
    }
}

#[cfg(unix)]
#[test]
fn hard_linked_install_stores_are_loaded_once() {
    let temp = TempPackage::new().test_unwrap();
    let home = temp.root.join("home");
    let cwd = temp.root.join("cwd");
    let package = temp.root.join("package");
    let user_store = temp.root.join("user.json");
    let project_store = temp.root.join("project.json");
    fs::create_dir_all(&home).test_unwrap();
    fs::create_dir_all(&cwd).test_unwrap();
    copy_fixture(&package);
    write_install_store(
        &user_store,
        &serde_json::json!([install_record(&package, "user")]),
    );
    fs::hard_link(&user_store, &project_store).test_unwrap();

    let doctor = run_cli(
        &home,
        &user_store,
        &project_store,
        &cwd,
        &["extension", "doctor", "example.capability-network"],
    );
    assert!(doctor.status.success(), "{}", stdout(&doctor));
    assert!(stdout(&doctor).contains("extension_outcome=Completed"));
}

#[test]
fn shared_store_preserves_recorded_project_scope() {
    let temp = TempPackage::new().test_unwrap();
    let home = temp.root.join("home");
    let cwd = temp.root.join("cwd");
    let package = temp.root.join("package");
    let store = temp.root.join("extensions.json");
    fs::create_dir_all(&home).test_unwrap();
    fs::create_dir_all(&cwd).test_unwrap();
    copy_fixture(&package);
    write_install_store(
        &store,
        &serde_json::json!([install_record(&package, "project")]),
    );

    let list = run_cli(&home, &store, &store, &cwd, &["extension", "list"]);
    assert!(list.status.success(), "{}", stdout(&list));
    let out = stdout(&list);
    assert!(out.contains("id=example.capability-network"), "{out}");
    assert!(out.contains("scope=project"), "{out}");
}

#[test]
fn unrelated_invalid_project_store_is_not_hidden() {
    let temp = TempPackage::new().test_unwrap();
    let home = temp.root.join("home");
    let cwd = temp.root.join("cwd");
    let user_store = temp.root.join("user.json");
    let project_store = temp.root.join("project.json");
    fs::create_dir_all(&home).test_unwrap();
    fs::create_dir_all(&cwd).test_unwrap();
    write_install_store(&user_store, &serde_json::json!([]));

    for (setup, expected_error) in [("malformed", "store_malformed"), ("unreadable", "store_io")] {
        if setup == "malformed" {
            fs::write(&project_store, "{not json").test_unwrap();
        } else {
            let _ = fs::remove_file(&project_store);
            fs::create_dir(&project_store).test_unwrap();
        }

        let list = run_cli(
            &home,
            &user_store,
            &project_store,
            &cwd,
            &["extension", "list"],
        );
        assert_eq!(list.status.code(), Some(1));
        let out = stdout(&list);
        assert!(out.contains("extension_outcome=Failed"), "{out}");
        assert!(out.contains(expected_error), "{out}");

        if setup == "unreadable" {
            fs::remove_dir(&project_store).test_unwrap();
        }
    }
}

#[test]
fn genuine_extension_id_and_tool_conflicts_fail_with_status_one() {
    let temp = TempPackage::new().test_unwrap();
    let home = temp.root.join("home");
    let cwd = temp.root.join("cwd");
    let first = temp.root.join("first");
    let second = temp.root.join("second");
    let user_store = temp.root.join("user.json");
    let project_store = temp.root.join("project.json");
    fs::create_dir_all(&home).test_unwrap();
    fs::create_dir_all(&cwd).test_unwrap();

    for second_id in ["example.conflict", "example.other-conflict"] {
        write_fixture_manifest(&first, "example.conflict", "conflict_tool");
        write_fixture_manifest(&second, second_id, "conflict_tool");
        write_install_store(
            &user_store,
            &serde_json::json!([install_record(&first, "user")]),
        );
        write_install_store(
            &project_store,
            &serde_json::json!([install_record(&second, "project")]),
        );

        let doctor = run_cli(
            &home,
            &user_store,
            &project_store,
            &cwd,
            &["extension", "doctor"],
        );
        assert_eq!(doctor.status.code(), Some(1));
        let out = stdout(&doctor);
        assert!(out.contains("extension_outcome=Failed"), "{out}");
    }
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
    fs::set_permissions(home.join(".yach"), fs::Permissions::from_mode(0o700)).test_unwrap();
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
        assert_eq!(output.status.code(), Some(1));
        let out = stdout(&output);
        assert!(
            out.contains("extension_outcome=Failed"),
            "{action} must report failed for planted state: {out}"
        );
        assert_eq!(fs::read(&authority).test_unwrap(), bytes);
    }
}
