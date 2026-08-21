use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use yach_backend::{
    ExtensionActivationState, ExtensionBackgroundActivationConfig, ExtensionInstallScope,
    ExtensionManifestIndex, ExtensionPackageRoot, ExtensionResourceBroker,
    ExtensionResourceRequest, ExtensionResourceResult, ExtensionToolExecution, PendingToolRequest,
    ToolPermissionPolicy, ToolPermissionState, ToolValidation, TurnId,
    activate_background_metadata_extensions,
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
            "yach-hashline-extension-integration-{}-{stamp}",
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

struct FixtureResources {
    files: Mutex<BTreeMap<String, String>>,
}

impl FixtureResources {
    fn new(path: &str, text: &str) -> Self {
        Self {
            files: Mutex::new(BTreeMap::from([(path.to_owned(), text.to_owned())])),
        }
    }

    fn replace(&self, path: &str, text: &str) {
        if let Ok(mut files) = self.files.lock() {
            files.insert(path.to_owned(), text.to_owned());
        }
    }
}

impl ExtensionResourceBroker for FixtureResources {
    fn execute(&self, request: &ExtensionResourceRequest) -> ExtensionResourceResult {
        let ExtensionResourceRequest::ReadTextFile { path, .. } = request;
        let Ok(files) = self.files.lock() else {
            return ExtensionResourceResult::Failed {
                reason: String::from("fixture_lock"),
                message: String::from("fixture lock failed"),
            };
        };
        let Some(text) = files.get(path) else {
            return ExtensionResourceResult::Failed {
                reason: String::from("fixture_missing"),
                message: String::from("fixture file missing"),
            };
        };
        ExtensionResourceResult::Completed {
            path: path.clone(),
            text: text.clone(),
            sha256: String::from(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
        }
    }
}

fn request(id: &str, name: &str, arguments: serde_json::Value) -> PendingToolRequest {
    PendingToolRequest {
        request_id: id.to_owned(),
        turn_id: TurnId(String::from("turn-hashline")),
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

#[test]
fn first_party_hashline_package_activates_advertises_and_proposes_reviewed_edits() {
    let package = TempPackage::new().test_unwrap();
    let source_manifest = yach_hashline_extension::MANIFEST_JSON;
    let mut manifest = serde_json::from_str::<serde_json::Value>(source_manifest).test_unwrap();
    manifest["main"]["command"] =
        serde_json::Value::String(String::from(env!("CARGO_BIN_EXE_yach")));
    manifest["main"]["args"] = serde_json::json!(["__extension-host", "hashline"]);
    fs::write(
        package.root.join("yach.extension.json"),
        serde_json::to_vec_pretty(&manifest).test_unwrap(),
    )
    .test_unwrap();

    let index = ExtensionManifestIndex::from_package_roots([ExtensionPackageRoot {
        root: package.root.clone(),
        scope: ExtensionInstallScope::User,
        source_ref: Some(String::from("first-party-integration")),
    }])
    .test_unwrap();
    let snapshot = activate_background_metadata_extensions(
        index.records(),
        ExtensionBackgroundActivationConfig {
            registration_timeout: Duration::from_secs(2),
            invocation_timeout: Duration::from_secs(2),
            max_stdout_line_bytes: 64 * 1024,
            max_result_bytes: 64 * 1024,
        },
    );
    assert_eq!(snapshot.host_start_count, 1);
    assert_eq!(
        snapshot.active_tool_names(),
        vec!["hashline_read", "hashline_edit"]
    );
    assert!(
        snapshot
            .diagnostics
            .iter()
            .all(|diagnostic| { diagnostic.activation_state == ExtensionActivationState::Active })
    );

    let permission_policy =
        ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
            ["project_path_info"],
            [
                "read_text_file",
                "search_project",
                "list_project_paths",
                "hashline_read",
            ],
            ["edit_text_file", "create_text_file", "hashline_edit"],
        );
    let (catalog, diagnostics) =
        snapshot.resolve_provider_turn_catalog(&permission_policy, snapshot.active_tool_names());
    assert!(diagnostics.is_empty());
    let read_tool = catalog.resolved_tool("read_text_file").test_unwrap();
    assert_eq!(read_tool.implementation_name, "hashline_read");
    let edit_tool = catalog.resolved_tool("edit_text_file").test_unwrap();
    assert_eq!(edit_tool.implementation_name, "hashline_edit");

    let resources = FixtureResources::new("src/lib.rs", "alpha\nbeta\n");
    let read_request = request(
        "read-1",
        "hashline_read",
        serde_json::json!({"path":"src/lib.rs"}),
    );
    let read_execution = snapshot
        .executor
        .execute_with_resources(
            &snapshot.registry,
            &read_request,
            &allowed(&read_request),
            &resources,
        )
        .test_unwrap();
    let ExtensionToolExecution::Result(read_result) = read_execution else {
        unreachable!("hashline read did not return text");
    };
    let header = read_result.summary.lines().next().test_unwrap();
    assert!(header.starts_with("[src/lib.rs#"));
    assert_eq!(read_result.summary.lines().nth(1), Some("1:alpha"));
    assert_eq!(read_result.summary.lines().nth(2), Some("2:beta"));

    let edit_request = request(
        "edit-1",
        "hashline_edit",
        serde_json::json!({
            "input": format!("{header}\nPUT 2.=2:\n+gamma")
        }),
    );
    let edit_execution = snapshot
        .executor
        .execute_with_resources(
            &snapshot.registry,
            &edit_request,
            &allowed(&edit_request),
            &resources,
        )
        .test_unwrap();
    let ExtensionToolExecution::EditProposal(proposal) = edit_execution else {
        unreachable!("hashline edit did not return a proposal");
    };
    assert_eq!(proposal.operations.len(), 1);
    assert_eq!(
        serde_json::to_value(&proposal.operations[0]).test_unwrap(),
        serde_json::json!({
            "kind": "modify_text_file",
            "path": "src/lib.rs",
            "expected_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "after_text": "alpha\ngamma\n"
        })
    );

    resources.replace("src/lib.rs", "alpha\nchanged\n");
    let stale_request = request(
        "edit-2",
        "hashline_edit",
        serde_json::json!({
            "input": format!("{header}\nPUT 2.=2:\n+gamma")
        }),
    );
    let stale_execution = snapshot
        .executor
        .execute_with_resources(
            &snapshot.registry,
            &stale_request,
            &allowed(&stale_request),
            &resources,
        )
        .test_unwrap();
    let ExtensionToolExecution::Result(stale_result) = stale_execution else {
        unreachable!("stale edit unexpectedly produced a proposal");
    };
    assert_eq!(stale_result.summary, "[hashline error: snapshot is stale]");
}

#[test]
fn first_party_manifest_is_loadable_from_its_package_root() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../yach-hashline-extension");

    let index = ExtensionManifestIndex::from_package_roots([ExtensionPackageRoot {
        root,
        scope: ExtensionInstallScope::User,
        source_ref: Some(String::from("workspace")),
    }])
    .test_unwrap();
    let record = index.records().first().test_unwrap();
    assert_eq!(record.manifest.id.0, "yach.hashline");
    assert_eq!(
        record.manifest.contributes.tool_replacement_bundles.len(),
        1
    );
}
#[test]
fn bundled_hashline_package_lists_disables_and_reenables_through_cli() {
    let stores = TempPackage::new().test_unwrap();
    let user_store = stores.root.join("user-extensions.json");
    let project_store = stores.root.join("project-extensions.json");
    let home = stores.root.join("home");

    let list = Command::new(env!("CARGO_BIN_EXE_yach"))
        .args(["extension", "list"])
        .env("HOME", &home)
        .env("YACH_EXTENSION_USER_STORE", &user_store)
        .env("YACH_EXTENSION_PROJECT_STORE", &project_store)
        .output()
        .test_unwrap();
    assert!(list.status.success());
    let list_stdout = String::from_utf8(list.stdout).test_unwrap();
    assert!(list_stdout.contains("extension_count=1"));
    assert!(list_stdout.contains("extension id=yach.hashline"));
    assert!(list_stdout.contains("activation_state=discovered"));

    let disable = Command::new(env!("CARGO_BIN_EXE_yach"))
        .args(["extension", "disable", "yach.hashline"])
        .env("HOME", &home)
        .env("YACH_EXTENSION_USER_STORE", &user_store)
        .env("YACH_EXTENSION_PROJECT_STORE", &project_store)
        .output()
        .test_unwrap();
    assert!(disable.status.success());
    let disable_stdout = String::from_utf8(disable.stdout).test_unwrap();
    assert!(disable_stdout.contains("extension_outcome=Completed"));

    let disabled = Command::new(env!("CARGO_BIN_EXE_yach"))
        .args(["extension", "doctor", "yach.hashline"])
        .env("HOME", &home)
        .env("YACH_EXTENSION_USER_STORE", &user_store)
        .env("YACH_EXTENSION_PROJECT_STORE", &project_store)
        .output()
        .test_unwrap();
    assert!(disabled.status.success());
    let disabled_stdout = String::from_utf8(disabled.stdout).test_unwrap();
    assert!(disabled_stdout.contains("install_source=yach.hashline"));
    assert!(disabled_stdout.contains("extension id=yach.hashline"));
    assert!(disabled_stdout.contains("activation_state=blocked"));
    assert!(disabled_stdout.contains("last_error_kind=disabled"));

    let enable = Command::new(env!("CARGO_BIN_EXE_yach"))
        .args(["extension", "enable", "yach.hashline"])
        .env("HOME", &home)
        .env("YACH_EXTENSION_USER_STORE", &user_store)
        .env("YACH_EXTENSION_PROJECT_STORE", &project_store)
        .output()
        .test_unwrap();
    assert!(enable.status.success());

    let reenabled = Command::new(env!("CARGO_BIN_EXE_yach"))
        .args(["extension", "doctor", "yach.hashline"])
        .env("HOME", &home)
        .env("YACH_EXTENSION_USER_STORE", &user_store)
        .env("YACH_EXTENSION_PROJECT_STORE", &project_store)
        .output()
        .test_unwrap();
    assert!(reenabled.status.success());
    let reenabled_stdout = String::from_utf8(reenabled.stdout).test_unwrap();
    assert!(reenabled_stdout.contains("extension id=yach.hashline"));
    assert!(reenabled_stdout.contains("activation_state=discovered"));
}
