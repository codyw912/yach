use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use yach_backend::{
    DenyExtensionResources, ExtensionActivationErrorKind, ExtensionActivationState,
    ExtensionBackgroundActivationConfig, ExtensionInstallScope, ExtensionManifestIndex,
    ExtensionPackageRoot, ExtensionToolExecution, ExtensionToolResultStatus, PendingToolRequest,
    ToolPermissionState, ToolValidation, TurnId, activate_background_metadata_extensions,
    grant_requested, revoke_grant,
};

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

static HOME_LOCK: Mutex<()> = Mutex::new(());

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

struct HomeGuard {
    previous: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", path);
        }
        Self { previous }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
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

fn activation_config() -> ExtensionBackgroundActivationConfig {
    ExtensionBackgroundActivationConfig {
        registration_timeout: Duration::from_secs(2),
        invocation_timeout: Duration::from_secs(2),
        max_stdout_line_bytes: 64 * 1024,
        max_result_bytes: 64 * 1024,
    }
}

fn activate(package_root: PathBuf) -> yach_backend::ExtensionActivationSnapshot {
    let index = ExtensionManifestIndex::from_package_roots([ExtensionPackageRoot {
        root: package_root,
        scope: ExtensionInstallScope::User,
        source_ref: Some(String::from("capability-network-fixture")),
    }])
    .test_unwrap();
    activate_background_metadata_extensions(index.records(), activation_config(), None)
}

fn request(id: &str, name: &str, arguments: serde_json::Value) -> PendingToolRequest {
    PendingToolRequest {
        request_id: id.to_owned(),
        turn_id: TurnId(String::from("turn-capability")),
        tool_name: name.to_owned(),
        provider_call_id: Some(format!("provider-{id}")),
        arguments,
    }
}

fn allowed(request: &PendingToolRequest) -> ToolValidation {
    ToolValidation {
        request_id: request.request_id.clone(),
        tool_name: request.tool_name.clone(),
        permission: ToolPermissionState::Allowed,
    }
}

#[cfg(unix)]
#[test]
fn network_fixture_is_blocked_until_granted_then_callable_and_survives_restart() {
    // Removing `capability_block_reason` from
    // `activate_background_metadata_extensions` must fail this test: without
    // a grant the host would start and `host_start_count` would not stay 0.
    let _home_lock = HOME_LOCK.lock().test_unwrap();
    let stores = TempPackage::new().test_unwrap();
    let home = stores.root.join("home");
    let package_root = stores.root.join("package");
    fs::create_dir_all(&home).test_unwrap();
    copy_fixture(&package_root);
    let _home = HomeGuard::set(&home);

    let blocked = activate(package_root.clone());
    assert_eq!(blocked.host_start_count, 0);
    assert_eq!(blocked.diagnostics.len(), 1);
    let blocked_diagnostic = blocked.diagnostics.first().test_unwrap();
    assert_eq!(
        blocked_diagnostic.activation_state,
        ExtensionActivationState::Blocked
    );
    assert_eq!(
        blocked_diagnostic.last_error_kind,
        Some(ExtensionActivationErrorKind::PolicyBlocked)
    );
    let blocked_summary = blocked_diagnostic
        .last_error_summary
        .as_deref()
        .test_unwrap();
    assert!(
        blocked_summary.contains("uses_network"),
        "block diagnostic must name the missing capability: {blocked_summary}"
    );
    drop(blocked);

    let index = ExtensionManifestIndex::from_package_roots([ExtensionPackageRoot {
        root: package_root.clone(),
        scope: ExtensionInstallScope::User,
        source_ref: Some(String::from("capability-network-fixture")),
    }])
    .test_unwrap();
    let record = index.records().first().test_unwrap();
    let stored = grant_requested(
        &record.manifest.id.0,
        &record.manifest.version,
        &record.manifest.contributes.tools,
    )
    .test_unwrap();
    assert!(stored.is_some(), "uses_network tool must produce a grant");

    let granted = activate(package_root.clone());
    assert_eq!(granted.host_start_count, 1);
    assert_eq!(granted.active_tool_names(), vec!["fetch_url"]);
    assert!(
        granted
            .diagnostics
            .iter()
            .all(|diagnostic| { diagnostic.activation_state == ExtensionActivationState::Active })
    );

    let invoke = request(
        "fetch-1",
        "fetch_url",
        serde_json::json!({"url": "https://example.test"}),
    );
    let execution = granted
        .executor
        .execute_with_resources(
            &granted.registry,
            &invoke,
            &allowed(&invoke),
            &DenyExtensionResources,
        )
        .test_unwrap();
    let ExtensionToolExecution::Result {
        result,
        status,
        reason,
    } = execution
    else {
        unreachable!("network fixture did not return a tool result");
    };
    assert_eq!(status, ExtensionToolResultStatus::Completed);
    assert_eq!(reason, None);
    assert_eq!(result.summary, "{\"ok\":true}");
    drop(granted);

    let restarted = activate(package_root.clone());
    assert_eq!(restarted.host_start_count, 1);
    assert_eq!(restarted.active_tool_names(), vec!["fetch_url"]);
    assert!(
        restarted
            .diagnostics
            .iter()
            .all(|diagnostic| { diagnostic.activation_state == ExtensionActivationState::Active })
    );
    drop(restarted);

    let revoked = revoke_grant(&record.manifest.id.0).test_unwrap();
    assert!(revoked, "revoke must remove the grant written above");

    let after_revoke = activate(package_root);
    assert_eq!(after_revoke.host_start_count, 0);
    let revoked_diagnostic = after_revoke.diagnostics.first().test_unwrap();
    assert_eq!(
        revoked_diagnostic.activation_state,
        ExtensionActivationState::Blocked
    );
    assert_eq!(
        revoked_diagnostic.last_error_kind,
        Some(ExtensionActivationErrorKind::PolicyBlocked)
    );
}
