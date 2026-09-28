//! Acceptance scenarios for distribution presets over the RPC boundary and
//! the one-shot CLI commands. Each scenario maps to a numbered item in the
//! spec's Acceptance list
//! (docs/project/specs/2026-09-27-distribution-presets-design.md). Item 5 is
//! covered by Task 3's unit tests and item 9 by Task 5's; 12b below extends
//! item 12 to a core build starting on state a full build left behind.
//!
//! Helpers are copied from `rpc_review.rs` — integration test files do not
//! share modules in this crate — with `RpcChild::spawn` extended by
//! `extra_args` for `--preset` and `cli`/`rpc_command` factoring the shared
//! hermetic child environment.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

use yach_proto::{ClientEvent, DialogResponse, ServerEvent, SubmittedSecret, default_ui_handshake};

const PROVIDER_SECRET: &str = "presets-test-secret";
const MODEL_ID: &str = "presets-model";
const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

/// The six members of the `project-tools` component, advertised under these
/// names whether the built-ins or an activated hashline bundle serve them.
const PROJECT_TOOLS: [&str; 6] = [
    "project_path_info",
    "read_text_file",
    "search_project",
    "list_project_paths",
    "edit_text_file",
    "create_text_file",
];

/// Acceptance 1: with no config a first run applies `full` and the provider
/// request matches today's build — bash, the six project tool names, and the
/// baseline guidance system text.
#[test]
fn full_first_run_matches_todays_request() {
    let project = TempDir::new("preset-full-project");
    let home = TempDir::new("preset-full-home");
    let provider = CapturingOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    connect_and_prompt(&mut child, &provider, "Say hello.");

    let body = provider.post_body(0);
    let tools = advertised_tools(&body);
    for name in std::iter::once("bash").chain(PROJECT_TOOLS.iter().copied()) {
        assert!(tools.iter().any(|tool| tool == name), "missing tool {name}");
    }
    assert!(
        system_texts(&body)
            .iter()
            .any(|text| text.contains("coding agent running in the yach harness")),
        "baseline guidance system text missing"
    );
    let config = fs::read_to_string(home.path().join(".yach/config.toml")).test_unwrap();
    assert!(
        config.contains("applied = \"full\""),
        "config.toml must record the first-run apply:\n{config}"
    );
    child.shutdown();
    provider.join();
}

/// Acceptance 2: `preset use minimal` leaves the next session advertising
/// only `bash`, with no baseline guidance system text.
#[test]
fn minimal_preset_advertises_bash_and_no_guidance() {
    let project = TempDir::new("preset-minimal-project");
    let home = TempDir::new("preset-minimal-home");
    cli(home.path(), project.path(), &["preset", "use", "minimal"]);

    let provider = CapturingOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    connect_and_prompt(&mut child, &provider, "Say hello.");

    let body = provider.post_body(0);
    assert_eq!(advertised_tools(&body), ["bash"]);
    assert!(
        system_texts(&body)
            .iter()
            .all(|text| !text.contains("yach harness")),
        "minimal must send no baseline guidance"
    );
    child.shutdown();
    provider.join();
}

/// Acceptance 3: disabling `project-tools` removes all six members
/// atomically; enabling restores them in the next session.
#[test]
fn project_tools_disable_is_atomic() {
    let project = TempDir::new("preset-component-project");
    let home = TempDir::new("preset-component-home");
    let provider = CapturingOpenAiProvider::start();

    cli(
        home.path(),
        project.path(),
        &["component", "disable", "project-tools"],
    );
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    connect_and_prompt(&mut child, &provider, "Say hello.");
    let disabled = advertised_tools(&provider.post_body(0));
    assert_eq!(disabled, ["bash"]);
    child.shutdown();

    cli(
        home.path(),
        project.path(),
        &["component", "enable", "project-tools"],
    );
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    connect_and_prompt(&mut child, &provider, "Say hello again.");
    let enabled = advertised_tools(&provider.post_body(1));
    for name in PROJECT_TOOLS {
        assert!(
            enabled.iter().any(|tool| tool == name),
            "re-enabled component is missing {name}"
        );
    }
    child.shutdown();
    provider.join();
}

/// Acceptance 4: `minimal` plus an enabled hashline bundle never activates —
/// neither the native targets nor the member names reach the catalog — and
/// the diagnostic names the extension in a `tool_replacement_bundle_inactive`
/// status emitted when background activation resolves the bundle.
#[cfg(feature = "bundled-hashline")]
#[test]
fn minimal_plus_hashline_bundle_is_inactive_with_diagnostic() {
    let project = TempDir::new("preset-hashline-project");
    let home = TempDir::new("preset-hashline-home");
    cli(home.path(), project.path(), &["preset", "use", "minimal"]);
    cli(
        home.path(),
        project.path(),
        &["extension", "install", "--bundled", "yach.hashline"],
    );

    let provider = CapturingOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    child.send(&ClientEvent::Initialize(default_ui_handshake()));
    child.wait_for(|event| match event {
        ServerEvent::Ready { handshake }
            if handshake.protocol_version == yach_proto::PROTOCOL_VERSION =>
        {
            Some(())
        }
        _ => None,
    });
    let session_id = child.wait_for(|event| match event {
        ServerEvent::StateUpdated(state) => state.session_id,
        _ => None,
    });
    child.send(&ClientEvent::FirstRenderCompleted);
    // The runner emits this status when post-first-paint background
    // activation resolves the bundle; the trace mark that follows is not on
    // the wire.
    child.wait_for(|event| match event {
        ServerEvent::StatusUpdated { message }
            if message.starts_with("tool_replacement_bundle_inactive extension=yach.hashline") =>
        {
            Some(message)
        }
        _ => None,
    });

    connect_provider_and_activate(&mut child, &provider);
    child.send(&ClientEvent::PromptSubmitted {
        session_id,
        prompt: String::from("Say hello."),
    });
    wait_for_prompt_finished(&mut child);

    let tools = advertised_tools(&provider.post_body(0));
    assert_eq!(tools, ["bash"]);
    for name in [
        "read_text_file",
        "edit_text_file",
        "hashline_read",
        "hashline_edit",
    ] {
        assert!(
            !tools.iter().any(|tool| tool == name),
            "inactive bundle member {name} leaked into the catalog"
        );
    }
    child.shutdown();
    provider.join();
}

/// Acceptance 6: a removed bundled extension stays removed — two session
/// starts and a later `preset use full` must not re-add it — and its id is
/// remembered under `[bundled] removed`.
#[cfg(feature = "bundled-hashline")]
#[test]
fn removed_bundled_extension_stays_removed() {
    let project = TempDir::new("preset-removed-project");
    let home = TempDir::new("preset-removed-home");
    cli(home.path(), project.path(), &["preset", "use", "full"]);
    let list = cli(home.path(), project.path(), &["extension", "list"]);
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("yach.hashline"),
        "preset use full must install yach.hashline"
    );

    cli(
        home.path(),
        project.path(),
        &["extension", "remove", "yach.hashline"],
    );
    let assert_absent = |phase: &str| {
        let list = cli(home.path(), project.path(), &["extension", "list"]);
        assert!(
            !String::from_utf8_lossy(&list.stdout).contains("yach.hashline"),
            "yach.hashline reappeared in extension list {phase}"
        );
    };
    assert_absent("right after removal");

    // Two session starts: neither startup refresh may re-add the record. One
    // provider for both — the stored connection points at its address.
    let provider = CapturingOpenAiProvider::start();
    for _ in 0..2 {
        let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
        connect_and_prompt(&mut child, &provider, "Say hello.");
        child.shutdown();
        assert_absent("after an rpc session");
    }
    provider.join();

    cli(home.path(), project.path(), &["preset", "use", "full"]);
    assert_absent("after a later preset use full");

    let config = fs::read_to_string(home.path().join(".yach/config.toml")).test_unwrap();
    assert!(
        config.contains("removed") && config.contains("yach.hashline"),
        "config.toml must remember the removal under [bundled] removed:\n{config}"
    );
}

/// Acceptance 7: `yach rpc --preset minimal` is ephemeral — the session sees
/// the minimal catalog and persisted preset/component/bundled state is
/// unchanged afterwards. config.toml may legitimately gain a `[model.default]`
/// table: on a first-ever activation with no configured default the runner
/// saves one regardless of intent (runner.rs `save_default`), identical on
/// trunk with or without `--preset` — so the comparison is the parsed
/// `[preset]`, `[components]`, and `[bundled]` tables, not raw bytes.
#[test]
fn ephemeral_preset_writes_nothing() {
    let project = TempDir::new("preset-ephemeral-project");
    let home = TempDir::new("preset-ephemeral-home");
    cli(home.path(), project.path(), &["preset", "use", "full"]);

    let config_path = home.path().join(".yach/config.toml");
    let extensions_path = home.path().join(".yach/extensions.json");
    let config_before = preset_tables(&config_path);
    let extensions_before = fs::read(&extensions_path).test_unwrap();

    let provider = CapturingOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path(), &["--preset", "minimal"]);
    connect_and_prompt(&mut child, &provider, "Say hello.");
    assert_eq!(advertised_tools(&provider.post_body(0)), ["bash"]);
    child.shutdown();
    provider.join();

    assert_eq!(
        preset_tables(&config_path),
        config_before,
        "--preset session must not rewrite preset/component/bundled state"
    );
    assert_eq!(
        fs::read(&extensions_path).test_unwrap(),
        extensions_before,
        "--preset session must not write extensions.json"
    );
}

/// The `[preset]`, `[components]`, and `[bundled]` sections of a config.toml
/// as raw text — the user state a preset session must never rewrite. Keys
/// under any other table (like `[model.default]`) are out of scope.
fn preset_tables(path: &Path) -> String {
    let text = fs::read_to_string(path).test_unwrap();
    let mut sections = String::new();
    let mut keep = false;
    for line in text.lines() {
        if line.starts_with('[') {
            keep = matches!(line.trim(), "[preset]" | "[components]" | "[bundled]");
        }
        // Blank lines are formatting; a later `[model.default]` insert pads
        // one onto the end of the preceding section.
        if keep && !line.trim().is_empty() {
            sections.push_str(line);
            sections.push('\n');
        }
    }
    sections
}

/// Acceptance 8: applying a preset changes no user authority state — not the
/// persisted approval mode, not the shell allowlist, not capability grants.
#[test]
fn preset_changes_no_authority() {
    let project = TempDir::new("preset-authority-project");
    let home = TempDir::new("preset-authority-home");

    // A user shell allowlist predating the preset change.
    fs::create_dir_all(home.path().join(".yach")).test_unwrap();
    let user_config_json = home.path().join(".yach/config.json");
    fs::write(&user_config_json, r#"{"shell":{"allow":["just test"]}}"#).test_unwrap();

    // Persist an approval mode through a real RPC session.
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    child.send(&ClientEvent::Initialize(default_ui_handshake()));
    child.wait_for(|event| match event {
        ServerEvent::Ready { handshake }
            if handshake.protocol_version == yach_proto::PROTOCOL_VERSION =>
        {
            Some(())
        }
        _ => None,
    });
    child.send(&ClientEvent::ApprovalModeSelected {
        request_id: 41,
        mode: yach_proto::ApprovalMode::AcceptEdits,
    });
    child.wait_for(|event| match event {
        ServerEvent::ApprovalModeChanged {
            request_id: 41,
            mode: yach_proto::ApprovalMode::AcceptEdits,
        } => Some(()),
        _ => None,
    });
    child.shutdown();

    let permissions_dir = home.path().join(".yach/permissions");
    let permission_files = fs::read_dir(&permissions_dir)
        .test_unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(permission_files.len(), 1);
    let permission_path = permission_files.into_iter().next().test_unwrap();
    let permissions_before = fs::read(&permission_path).test_unwrap();
    let config_json_before = fs::read(&user_config_json).test_unwrap();

    // A real capability grant must also be untouched — seed one so the
    // grants directory is non-empty and the byte comparison is meaningful.
    let authority = yach_backend::ExtensionAuthorityStore::in_home(home.path());
    let tools = [yach_backend::ExtensionToolContribution {
        name: String::from("fetch"),
        description: String::from("fixture"),
        risk: yach_backend::ExtensionToolRisk::UsesNetwork,
        provider_visible: true,
    }];
    let grant = authority.grant_requested(
        "example.network-tools",
        "1.0.0",
        &tools,
        None,
        yach_backend::ExtensionDecisionSurface::Cli,
    );
    assert!(
        matches!(grant, Ok(Some(_))),
        "grant must be written: {grant:?}"
    );
    let grants_before = snapshot_dir(&home.path().join(".yach/extensions"));
    assert!(
        !grants_before.is_empty(),
        "seeded grant must produce a non-empty grants directory"
    );

    cli(home.path(), project.path(), &["preset", "use", "minimal"]);

    // Restart so session startup itself runs against the changed preset.
    let provider = CapturingOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    connect_and_prompt(&mut child, &provider, "Say hello.");
    child.shutdown();
    provider.join();

    let permissions_after = fs::read(&permission_path).test_unwrap();
    assert!(
        String::from_utf8_lossy(&permissions_after).contains("\"mode\":\"accept-edits\""),
        "persisted approval mode must survive preset use"
    );
    assert_eq!(
        permissions_after, permissions_before,
        "preset use must not rewrite the permissions file"
    );
    assert_eq!(
        fs::read(&user_config_json).test_unwrap(),
        config_json_before,
        "preset use must not touch the shell allowlist"
    );
    assert_eq!(
        snapshot_dir(&home.path().join(".yach/extensions")),
        grants_before,
        "preset use must not touch capability grants"
    );
}

/// Acceptance 10: first apply on a pre-feature install (hashline present and
/// disabled, no `[preset]` record) records `full` and keeps hashline
/// disabled.
#[cfg(feature = "bundled-hashline")]
#[test]
fn first_apply_preserves_disabled_hashline() {
    let project = TempDir::new("preset-first-apply-project");
    let home = TempDir::new("preset-first-apply-home");
    cli(home.path(), project.path(), &["preset", "use", "full"]);
    cli(
        home.path(),
        project.path(),
        &["extension", "disable", "yach.hashline"],
    );

    // Simulate a pre-feature install: drop the `[preset]` table so the next
    // session performs the first-run apply again.
    let config_path = home.path().join(".yach/config.toml");
    let config = fs::read_to_string(&config_path).test_unwrap();
    let mut filtered = String::new();
    let mut in_preset = false;
    for line in config.lines() {
        if line.starts_with('[') {
            in_preset = line.trim() == "[preset]";
        }
        if !in_preset {
            filtered.push_str(line);
            filtered.push('\n');
        }
    }
    fs::write(&config_path, filtered).test_unwrap();

    let provider = CapturingOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    connect_and_prompt(&mut child, &provider, "Say hello.");
    child.shutdown();
    provider.join();

    let config = fs::read_to_string(&config_path).test_unwrap();
    assert!(
        config.contains("applied = \"full\""),
        "first run must record the applied preset:\n{config}"
    );
    let doctor = cli(
        home.path(),
        project.path(),
        &["extension", "doctor", "yach.hashline"],
    );
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        stdout.contains("yach.hashline") && stdout.contains("last_error_kind=disabled"),
        "first apply must keep the disabled record:\n{stdout}"
    );
}

/// Acceptance 11: a bundled record materialized for an older version is
/// repointed to the current version's manifest during session extension
/// discovery, keeping `enabled`. `yach extension list` reads records without
/// refreshing, so the sync point is an RPC start.
#[cfg(feature = "bundled-hashline")]
#[test]
fn upgrade_refresh_repoints_bundled_record() {
    let project = TempDir::new("preset-upgrade-project");
    let home = TempDir::new("preset-upgrade-home");
    cli(home.path(), project.path(), &["preset", "use", "full"]);

    let store_path = home.path().join(".yach/extensions.json");
    let stale_dir = home.path().join(".yach/bundled/yach-hashline/0.0.1");
    fs::create_dir_all(&stale_dir).test_unwrap();
    let store_text = fs::read_to_string(&store_path).test_unwrap();
    let mut store_json: serde_json::Value = serde_json::from_str(&store_text).test_unwrap();
    let mut enabled_before = None;
    for record in store_json["records"].as_array_mut().test_unwrap() {
        if record["source"].as_str() == Some("yach.hashline") {
            enabled_before = record["enabled"].as_bool();
            let canonical = fs::canonicalize(&stale_dir).test_unwrap();
            record["package_root"] =
                serde_json::Value::String(canonical.to_string_lossy().into_owned());
        }
    }
    assert!(
        enabled_before.is_some(),
        "preset use full must install a yach.hashline record:\n{store_text}"
    );
    fs::write(
        &store_path,
        serde_json::to_string_pretty(&store_json).test_unwrap(),
    )
    .test_unwrap();

    // Start a session: extension discovery runs the upgrade refresh.
    let provider = CapturingOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    child.send(&ClientEvent::Initialize(default_ui_handshake()));
    child.wait_for(|event| match event {
        ServerEvent::Ready { handshake }
            if handshake.protocol_version == yach_proto::PROTOCOL_VERSION =>
        {
            Some(())
        }
        _ => None,
    });
    let session_id = child.wait_for(|event| match event {
        ServerEvent::StateUpdated(state) => state.session_id,
        _ => None,
    });
    child.send(&ClientEvent::FirstRenderCompleted);
    connect_provider_and_activate(&mut child, &provider);
    child.send(&ClientEvent::PromptSubmitted {
        session_id,
        prompt: String::from("Say hello."),
    });
    wait_for_prompt_finished(&mut child);
    child.shutdown();
    provider.join();

    let deadline = Instant::now() + EVENT_TIMEOUT;
    let record = loop {
        let store_text = fs::read_to_string(&store_path).test_unwrap();
        let store_json: serde_json::Value = serde_json::from_str(&store_text).test_unwrap();
        let record = store_json["records"].as_array().and_then(|records| {
            records
                .iter()
                .find(|record| record["source"].as_str() == Some("yach.hashline"))
                .cloned()
        });
        if let Some(record) = record.filter(|record| {
            record["package_root"]
                .as_str()
                .is_some_and(|root| root.ends_with(env!("CARGO_PKG_VERSION")))
        }) {
            break record;
        }
        assert!(
            Instant::now() < deadline,
            "bundled record was not repointed to {}: {store_text}",
            env!("CARGO_PKG_VERSION")
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        record["enabled"].as_bool(),
        enabled_before,
        "upgrade refresh must keep enabled"
    );
}

/// Acceptance 12 extended: a core build starting on state a full build left
/// behind — an enabled bundled hashline record with a real manifest — keeps
/// the foreign record byte-identical, reports `not_compiled_in` in doctor,
/// and never lets it reach the session's extension catalog.
#[cfg(not(feature = "bundled-hashline"))]
#[test]
fn core_build_session_starts_with_stale_bundled_record() {
    let project = TempDir::new("preset-core-project");
    let home = TempDir::new("preset-core-home");

    // State as a full build leaves it: a materialized package dir with a
    // discoverable manifest (read from the workspace crate — a path read,
    // no crate dependency), an enabled bundled record, and a recorded
    // `[preset]` so no first run runs.
    let package_root = home.path().join(".yach/bundled/yach-hashline/0.0.1");
    fs::create_dir_all(&package_root).test_unwrap();
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../yach-hashline-extension/yach.extension.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).test_unwrap()).test_unwrap();
    manifest["main"]["command"] =
        serde_json::Value::String(String::from(env!("CARGO_BIN_EXE_yach")));
    manifest["main"]["args"] = serde_json::json!(["__extension-host", "hashline"]);
    fs::write(
        package_root.join("yach.extension.json"),
        serde_json::to_string_pretty(&manifest).test_unwrap(),
    )
    .test_unwrap();

    let store_path = home.path().join(".yach/extensions.json");
    let mut store = yach_backend::ExtensionInstallStore::default();
    store
        .install_bundled(
            "yach.hashline",
            &package_root,
            yach_backend::ExtensionInstallScope::User,
        )
        .test_unwrap();
    store.save_to_path(&store_path).test_unwrap();
    fs::write(
        home.path().join(".yach/config.toml"),
        "[preset]\napplied = \"full\"\n",
    )
    .test_unwrap();
    let store_before = fs::read(&store_path).test_unwrap();

    let provider = CapturingOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path(), &[]);
    connect_and_prompt(&mut child, &provider, "Say hello.");
    // Barrier: the snapshot is meaningful only after background activation
    // finished publishing its diagnostics; the status line marks that point.
    child.wait_for(|event| match event {
        ServerEvent::StatusUpdated { message }
            if message.starts_with("extension_background_activation_finished") =>
        {
            Some(())
        }
        _ => None,
    });
    // The session catalog is the assertion the `is_compiled_in` filter
    child.send(&ClientEvent::ExtensionDiagnosticSnapshotRequested {
        request_id: String::from("core-snapshot"),
        selector: None,
    });
    let records = child.wait_for(|event| match event {
        ServerEvent::ExtensionDiagnosticSnapshotUpdated {
            request_id,
            records,
            ..
        } if request_id == "core-snapshot" => Some(records),
        _ => None,
    });
    assert!(
        !records
            .iter()
            .any(|record| record.id.as_deref() == Some("yach.hashline")),
        "core build must filter the not-compiled-in record from the catalog: {records:?}"
    );
    child.shutdown();
    provider.join();

    // The CLI diagnostic path reads install records directly and must report
    // the record as blocked: not compiled into this build, never discovered.
    let doctor = cli(
        home.path(),
        project.path(),
        &["extension", "doctor", "yach.hashline"],
    );
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        stdout.contains("yach.hashline")
            && stdout.contains("last_error_kind=not_compiled_in")
            && stdout.contains("discovered=false"),
        "doctor must report the stale record as not compiled in:\n{stdout}"
    );

    assert_eq!(
        fs::read(&store_path).test_unwrap(),
        store_before,
        "a core build must not rewrite or delete the full-build record"
    );
}

/// Tool names advertised on a chat-completions request body, sorted for
/// stable comparison.
fn advertised_tools(body: &serde_json::Value) -> Vec<String> {
    let mut names = body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Concatenated text of every `role == "system"` message on a request body.
/// The wire shape encodes `content` as a list of parts
/// (`[{"type":"text","text":…}]`), with a bare string tolerated.
fn system_texts(body: &serde_json::Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter(|message| message["role"].as_str() == Some("system"))
                .map(|message| match message["content"].as_str() {
                    Some(text) => String::from(text),
                    None => message["content"]
                        .as_array()
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|part| part["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A byte snapshot of every file under `dir`, sorted by relative path, so
/// changes to the capability-grant store are comparable across a preset use.
/// A missing directory snapshots as empty.
fn snapshot_dir(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut snapshot = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(dir).test_unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file() {
                let relative = path.strip_prefix(dir).test_unwrap().to_path_buf();
                snapshot.push((relative, fs::read(&path).test_unwrap()));
            }
        }
        snapshot.sort();
    }
    snapshot
}

/// Every preset/component/extension child gets the same hermetic environment
/// as `RpcChild::spawn`, plus `current_dir(project)` so the project-scope
/// store resolves under the fixture rather than the inherited cwd.
fn cli(home: &Path, project: &Path, args: &[&str]) -> Output {
    let mut command = rpc_command(home, project);
    command.args(args);
    let output = command.output().test_unwrap();
    assert!(
        output.status.success(),
        "yach {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn rpc_command(home: &Path, project: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yach"));
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("YACH_")
            || matches!(
                key.as_ref(),
                "OPENAI_API_KEY"
                    | "OPENAI_BASE_URL"
                    | "ANTHROPIC_API_KEY"
                    | "ANTHROPIC_BASE_URL"
                    | "CODEX_HOME"
            )
        {
            command.env_remove(&*key);
        }
    }
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .current_dir(project);
    command
}

/// Full first-render → connection → activation → prompt → finished sequence,
/// shared by every scenario that drives a provider turn.
fn connect_and_prompt(child: &mut RpcChild, provider: &CapturingOpenAiProvider, prompt: &str) {
    child.send(&ClientEvent::Initialize(default_ui_handshake()));
    child.wait_for(|event| match event {
        ServerEvent::Ready { handshake }
            if handshake.protocol_version == yach_proto::PROTOCOL_VERSION =>
        {
            Some(())
        }
        _ => None,
    });
    let session_id = child.wait_for(|event| match event {
        ServerEvent::StateUpdated(state) => state.session_id,
        _ => None,
    });
    child.send(&ClientEvent::FirstRenderCompleted);
    connect_provider_and_activate(child, provider);
    child.send(&ClientEvent::PromptSubmitted {
        session_id,
        prompt: String::from(prompt),
    });
    wait_for_prompt_finished(child);
}

/// The connection flow plus a `SessionOnly` model activation: the default
/// provider-connections backend with a local OpenAI-compatible fixture, so
/// nothing needs credentials or the network and no user default is written.
/// On a home that already has a connection, `FirstRenderCompleted` triggers
/// auto-activation of the saved default (request_id 0); wait for it rather
/// than the model list, which may not re-publish a stored connection's
/// models deterministically.
fn connect_provider_and_activate(child: &mut RpcChild, provider: &CapturingOpenAiProvider) {
    if child.home_has_connection() {
        child.wait_for(|event| match event {
            ServerEvent::ModelActivationFinished(result)
                if result.session_activated && result.target.model_id == MODEL_ID =>
            {
                Some(())
            }
            _ => None,
        });
        return;
    }
    child.send(&ClientEvent::ConnectionsRequested);
    child.resolve(
        "provider-connection:root",
        DialogResponse::Selection {
            value: String::from("add"),
        },
    );
    child.resolve(
        "provider-connection:provider",
        DialogResponse::Selection {
            value: String::from("openai-compatible"),
        },
    );
    child.resolve(
        "provider-connection:label",
        DialogResponse::Text {
            value: String::from("presets fixture"),
        },
    );
    child.resolve(
        "provider-connection:base-url",
        DialogResponse::Text {
            value: provider.base_url(),
        },
    );
    child.resolve(
        "provider-connection:secret:create",
        DialogResponse::Secret {
            value: SubmittedSecret::new(PROVIDER_SECRET),
        },
    );
    let model = child.wait_for(|event| match event {
        // Connection-backed models arrive on the discovery snapshot.
        ServerEvent::DiscoveredModelsUpdated { models }
            if models.iter().any(|model| model.id == MODEL_ID) =>
        {
            models.into_iter().find(|model| model.id == MODEL_ID)
        }
        _ => None,
    });
    let connection_id = model.connection_id.test_unwrap();
    child.send(&ClientEvent::ModelActivationRequested {
        target: yach_proto::ModelTarget {
            provider: model.provider,
            model_id: model.id,
            connection_id,
            connection_key: None,
        },
        intent: yach_proto::ModelActivationIntent::SessionOnly,
        request_id: 1,
    });
    child.wait_for(|event| match event {
        ServerEvent::ModelActivationFinished(result)
            if result.request_id == 1
                && result.session_activated
                && result.target.model_id == MODEL_ID =>
        {
            Some(())
        }
        _ => None,
    });
}

fn wait_for_prompt_finished(child: &mut RpcChild) {
    child.wait_for(|event| match event {
        ServerEvent::PromptFinished {
            outcome: yach_proto::PromptOutcome::Completed,
            ..
        } => Some(()),
        _ => None,
    });
}

/// An OpenAI-compatible fixture that answers `GET /models` like
/// `MockOpenAiProvider` and every `POST /chat/completions` with a single text
/// completion, recording each request body for tool/guidance assertions.
struct CapturingOpenAiProvider {
    base_url: String,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl CapturingOpenAiProvider {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").test_unwrap();
        let address = listener.local_addr().test_unwrap();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_bodies = Arc::clone(&bodies);
        let observed_shutdown = Arc::clone(&shutdown);
        let worker = thread::spawn(move || {
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                if observed_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let Ok(request) = read_http_request(&mut stream) else {
                    return;
                };
                let request_line = request.lines().next().unwrap_or_default();
                if request_line.starts_with("GET ") && request_line.contains("/models") {
                    write_http_response(
                        &mut stream,
                        "application/json",
                        r#"{"object":"list","data":[{"id":"presets-model","object":"model","created":0,"owned_by":"presets-fixture"}]}"#,
                    );
                    continue;
                }
                if request_line.starts_with("POST ") && request_line.contains("/chat/completions") {
                    if let Some((_, body)) = request.split_once("\r\n\r\n")
                        && let Ok(body) = serde_json::from_str::<serde_json::Value>(body)
                        && let Ok(mut bodies) = observed_bodies.lock()
                    {
                        bodies.push(body);
                    }
                    write_http_response(&mut stream, "text/event-stream", &text_completion_sse());
                    continue;
                }
                return;
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            bodies,
            shutdown,
            worker: Some(worker),
        }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// The nth recorded chat-completions request body.
    fn post_body(&self, index: usize) -> serde_json::Value {
        let bodies = self
            .bodies
            .lock()
            .unwrap_or_else(|error| unreachable!("captured request bodies lock poisoned: {error}"));
        bodies
            .get(index)
            .unwrap_or_else(|| {
                unreachable!("expected {} request bodies, got {index}", bodies.len())
            })
            .clone()
    }

    /// Signal the accept loop to stop and wake it with a final connection so
    /// `join` cannot hang waiting for a request that never comes.
    fn join(mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = TcpStream::connect(
            self.base_url
                .trim_start_matches("http://")
                .trim_end_matches("/v1"),
        );
        if let Some(worker) = self.worker.take() {
            worker.join().test_unwrap();
        }
    }
}

fn text_completion_sse() -> String {
    let content = serde_json::json!({
        "id": "chatcmpl-presets",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": MODEL_ID,
        "choices": [{
            "index": 0,
            "delta": {"role":"assistant","content":"Done."},
            "finish_reason": null
        }]
    });
    let finished = serde_json::json!({
        "id": "chatcmpl-presets",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": MODEL_ID,
        "choices": [{"index":0,"delta":{},"finish_reason":"stop"}]
    });
    format!("data: {content}\n\ndata: {finished}\n\ndata: [DONE]\n\n")
}

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    stream.set_read_timeout(Some(EVENT_TIMEOUT))?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end;
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "request ended",
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = end + 4;
            break;
        }
    }
    let header = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = header
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_http_response(stream: &mut TcpStream, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).test_unwrap();
    stream.flush().test_unwrap();
}

struct RpcChild {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<String>,
    pending: VecDeque<ServerEvent>,
    transcript: Vec<String>,
    home: PathBuf,
}

impl RpcChild {
    /// `extra_args` are appended to `yach rpc --project-root <dir>
    /// --no-catalog-refresh` — scenarios use them for `--preset <name>`.
    fn spawn(project_root: &Path, home: &Path, extra_args: &[&str]) -> Self {
        let mut command = rpc_command(home, project_root);
        command
            .args(["rpc", "--project-root"])
            .arg(project_root)
            // Deterministic test child: no background models.dev fetch.
            .arg("--no-catalog-refresh")
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = command.spawn().test_unwrap();
        let stdin = child.stdin.take().test_unwrap();
        let stdout = child.stdout.take().test_unwrap();
        let (tx, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            events,
            pending: VecDeque::new(),
            transcript: Vec::new(),
            home: home.to_path_buf(),
        }
    }

    /// Whether `~/.yach/connections.json` exists — set by an earlier session's
    /// connection-create flow, it means this session skips the create dialogs.
    fn home_has_connection(&self) -> bool {
        self.home.join(".yach/connections.json").is_file()
    }

    fn send(&mut self, event: &ClientEvent) {
        let Some(stdin) = self.stdin.as_mut() else {
            unreachable!("send client event after rpc stdin closed: {event:?}");
        };
        let line = event.to_jsonl().test_unwrap();
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .unwrap_or_else(|error| unreachable!("write rpc client event {event:?}: {error}"));
    }

    /// The reducer only accepts responses for the dialog it has issued, so a
    /// resolve must first observe the matching `DialogRequested` frame.
    fn resolve(&mut self, dialog_id: &str, response: DialogResponse) {
        let owned_id = String::from(dialog_id);
        self.wait_for(move |event| match event {
            ServerEvent::DialogRequested(request) if request.id.as_deref() == Some(&owned_id) => {
                Some(())
            }
            _ => None,
        });
        self.send(&ClientEvent::DialogResolved {
            dialog_id: String::from(dialog_id),
            response,
        });
    }

    fn wait_for<T>(&mut self, mut match_event: impl FnMut(ServerEvent) -> Option<T>) -> T {
        // Scan frames earlier waits read past first: a wait removes only the
        // frame it matched, so out-of-order expectations still succeed.
        let mut index = 0;
        while index < self.pending.len() {
            if let Some(value) = match_event(self.pending[index].clone()) {
                self.pending.remove(index);
                return value;
            }
            index += 1;
        }
        let deadline = Instant::now() + EVENT_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "RPC server event predicate timed out\nrpc transcript:\n{}",
                self.transcript.join("\n")
            );
            let line = match self.events.recv_timeout(remaining) {
                Ok(line) => line,
                Err(error) => unreachable!(
                    "RPC server event predicate timeout: {error}\nrpc transcript:\n{}",
                    self.transcript.join("\n")
                ),
            };
            self.transcript.push(line.clone());
            let event = ServerEvent::from_jsonl(&line).unwrap_or_else(|error| {
                unreachable!("RPC stdout was not a ServerEvent JSONL frame: {error}: {line}")
            });
            if let Some(value) = match_event(event.clone()) {
                return value;
            }
            self.pending.push_back(event);
        }
    }

    /// Closing stdin is the protocol's graceful EOF; give the child room to
    /// flush writes (session log, persisted state) before kill as cleanup.
    fn shutdown(&mut self) {
        self.stdin.take();
        for _ in 0..100 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RpcChild {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .test_unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("yach-{label}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).test_unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
