use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use yach_backend::bench_loop::{run_scripted_turn, Script, ScriptedTurnConfig};
use yach_backend::{
    advertised_roster_bytes, build_provider_tool_advertising_extension, hashline_bundle_definitions,
    ActivatedToolReplacementBundle, ExtensionActivationSnapshot, ExtensionToolReplacementContract,
    ExtensionToolReplacementMember, ProviderModel, ProviderRequest, ToolPermissionPolicy,
    ToolRegistry, ToolReplacementSource, TurnId,
};
use yach_trace::TraceRecord;

use crate::perf::alloc::{AllocCounts, AllocWindow};
use crate::perf::registry::{Bin, Measured, Requirement, RunCtx, Workload};
use crate::perf::rss::{peak_rss_bytes, Spawn, StopBoundary};
use crate::perf::schema::{Class, Isolation};
use crate::perf::workloads::startup::ExtensionManifestPackageRoot;

macro_rules! provider_phase {
    ($label:literal) => {
        Workload {
            id: concat!("turn/phase/", $label),
            class: Class::Latency,
            isolation: Isolation::ChildProcess,
            requires: &[Requirement::Binary],
            bin: Some(Bin::Bench),
            run: |ctx| run_turn_phase(ctx, CachedChildKind::TextOnly, $label, None),
        }
    };
}

macro_rules! tools_phase {
    ($label:literal, $n:expr) => {
        Workload {
            id: concat!("turn/phase/", $label),
            class: Class::Latency,
            isolation: Isolation::ChildProcess,
            requires: &[Requirement::Binary],
            bin: Some(Bin::Bench),
            run: |ctx| run_turn_phase(ctx, CachedChildKind::Tools4Builtin, $label, Some($n)),
        }
    };
}

macro_rules! tools_phase_mark {
    ($label:literal) => {
        Workload {
            id: concat!("turn/phase/", $label),
            class: Class::Latency,
            isolation: Isolation::ChildProcess,
            requires: &[Requirement::Binary],
            bin: Some(Bin::Bench),
            run: |ctx| run_turn_phase(ctx, CachedChildKind::Tools4Builtin, $label, None),
        }
    };
}

pub static CORE_LOOP: [Workload; 20] = [
    Workload {
        id: "request/assemble/10_turns",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| Ok(assemble_workload(10, ctx)),
    },
    Workload {
        id: "request/assemble/100_turns",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| Ok(assemble_workload(100, ctx)),
    },
    Workload {
        id: "request/assemble/1000_turns",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| Ok(assemble_workload(1000, ctx)),
    },
    Workload {
        id: "request/roster_bytes/builtin",
        class: Class::Count,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: roster_bytes_builtin,
    },
    Workload {
        id: "request/roster_bytes/hashline_ext",
        class: Class::Count,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: roster_bytes_hashline,
    },
    Workload {
        id: "provider/encode/rig_messages/100_turns",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: encode_rig_messages,
    },
    Workload {
        id: "provider/encode/rig_tools/100_turns",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: encode_rig_tools,
    },
    Workload {
        id: "turn/scripted/text_only",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| scripted_in_process(ctx, ScriptKind::TextOnly),
    },
    Workload {
        id: "turn/scripted/tools_4/builtin",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| scripted_in_process(ctx, ScriptKind::Tools4),
    },
    Workload {
        id: "turn/scripted/tools_4/hashline_ext",
        class: Class::Latency,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary],
        bin: Some(Bin::Bench),
        run: hashline_ext_child,
    },
    Workload {
        id: "turn/scripted/tools_4/inactive_ext_8",
        class: Class::Latency,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary],
        bin: Some(Bin::Bench),
        run: inactive_ext_8_child,
    },
    provider_phase!("request_assembled"),
    provider_phase!("provider_request_sent"),
    provider_phase!("provider_first_event"),
    provider_phase!("provider_stream_end"),
    tools_phase!("tool_dispatched", 4),
    tools_phase!("tool_result_appended", 4),
    tools_phase_mark!("session_persisted"),
    tools_phase_mark!("turn_completed"),
    Workload {
        id: "memory/peak_rss/turn_scripted_tools_4",
        class: Class::Memory,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary, Requirement::Linux],
        bin: Some(Bin::Bench),
        run: peak_rss_turn_scripted_tools_4,
    },
];

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
enum ScriptKind {
    TextOnly,
    Tools4,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum CachedChildKind {
    TextOnly,
    Tools4Builtin,
}

#[derive(Clone)]
struct CachedChildSample {
    records: Vec<TraceRecord>,
}

type ChildCacheKey = (usize, CachedChildKind);
type ChildCache = HashMap<ChildCacheKey, Result<Vec<CachedChildSample>, String>>;

static CHILD_CACHE: LazyLock<Mutex<ChildCache>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_child_cache() -> std::sync::MutexGuard<'static, ChildCache> {
    match CHILD_CACHE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn assemble_workload(turns: usize, ctx: &RunCtx) -> Measured {
    let log = yach_backend::request_assembly::fixture_log(turns, 1);
    let current = TurnId(format!("turn-{}", turns + 1));
    let mut samples = Vec::with_capacity(ctx.samples);
    let window = AllocWindow::begin();
    for _ in 0..ctx.samples {
        let start = Instant::now();
        let messages = yach_backend::request_assembly::assemble(&log, &current, None);
        samples.push(start.elapsed());
        std::hint::black_box(messages);
    }
    let alloc = window.end();
    Measured::Latency {
        samples,
        alloc: Some(alloc),
    }
}

fn builtin_provider_definitions() -> Vec<yach_backend::ToolDefinition> {
    let registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
    let policy = ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
        ["project_path_info"],
        ["read_text_file", "search_project", "list_project_paths"],
        ["edit_text_file", "create_text_file"],
    );
    let catalog = registry.resolve_provider_turn_catalog(
        &policy,
        [
            "project_path_info",
            "read_text_file",
            "search_project",
            "list_project_paths",
            "edit_text_file",
            "create_text_file",
        ],
    );
    catalog.provider_definitions()
}

fn roster_bytes_builtin(_ctx: &RunCtx) -> Result<Measured, String> {
    let definitions = builtin_provider_definitions();
    let bytes = advertised_roster_bytes(&definitions).map_err(|error| format!("{error:?}"))?;
    Ok(Measured::Value(bytes as u64))
}

fn roster_bytes_hashline(_ctx: &RunCtx) -> Result<Measured, String> {
    let mut snapshot = ExtensionActivationSnapshot::default();
    for definition in hashline_bundle_definitions() {
        snapshot
            .registry
            .register_extension_tool(definition)
            .map_err(|error| format!("{error:?}"))?;
    }
    snapshot.replacement_bundles = vec![ActivatedToolReplacementBundle {
        extension_id: String::from("yach.hashline"),
        extension_version: String::from("0.1.0"),
        bundle_id: String::from("hashline"),
        source: ToolReplacementSource::User,
        members: vec![
            ExtensionToolReplacementMember {
                builtin: String::from("read_text_file"),
                tool: String::from("hashline_read"),
                contract: ExtensionToolReplacementContract::Preserve,
            },
            ExtensionToolReplacementMember {
                builtin: String::from("edit_text_file"),
                tool: String::from("hashline_edit"),
                contract: ExtensionToolReplacementContract::Replace,
            },
        ],
    }];
    let policy = ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
        ["project_path_info"],
        [
            "read_text_file",
            "search_project",
            "list_project_paths",
            "hashline_read",
        ],
        ["edit_text_file", "create_text_file", "hashline_edit"],
    );
    let (catalog, diagnostics) = snapshot.resolve_provider_turn_catalog(
        &policy,
        [
            "project_path_info",
            "read_text_file",
            "search_project",
            "list_project_paths",
            "edit_text_file",
            "create_text_file",
            "hashline_read",
            "hashline_edit",
        ],
    );
    if !diagnostics.is_empty() {
        return Err(format!(
            "hashline replacement diagnostics: {diagnostics:?}"
        ));
    }
    let bytes =
        advertised_roster_bytes(&catalog.provider_definitions()).map_err(|error| format!("{error:?}"))?;
    Ok(Measured::Value(bytes as u64))
}

fn assembled_request(turns: usize) -> ProviderRequest {
    let log = yach_backend::request_assembly::fixture_log(turns, 1);
    let current = TurnId(format!("turn-{}", turns + 1));
    let messages = yach_backend::request_assembly::assemble(&log, &current, None);
    ProviderRequest {
        turn_id: current,
        model: ProviderModel {
            provider: String::from("scripted"),
            model: String::from("scripted-model"),
        },
        messages,
        native_request: None,
        extensions: Vec::new(),
        approved_tool_advertising: None,
    }
}

fn encode_rig_messages(ctx: &RunCtx) -> Result<Measured, String> {
    let request = assembled_request(100);
    let mut samples = Vec::with_capacity(ctx.samples);
    let window = AllocWindow::begin();
    for _ in 0..ctx.samples {
        let start = Instant::now();
        let mapped = yach_backend::rig_adapter::bench_rig_messages_from_request(&request)
            .map_err(|error| format!("{error:?}"))?;
        samples.push(start.elapsed());
        std::hint::black_box(mapped);
    }
    let alloc = window.end();
    Ok(Measured::Latency {
        samples,
        alloc: Some(alloc),
    })
}

fn encode_rig_tools(ctx: &RunCtx) -> Result<Measured, String> {
    let definitions = builtin_provider_definitions();
    let advertising = build_provider_tool_advertising_extension(&definitions)
        .map_err(|error| format!("{error:?}"))?;
    let approved: Vec<String> = definitions.iter().map(|tool| tool.name.clone()).collect();
    let mut request = assembled_request(100);
    request.extensions = vec![advertising.clone()];
    request.approved_tool_advertising = Some(advertising);
    let mut samples = Vec::with_capacity(ctx.samples);
    let window = AllocWindow::begin();
    for _ in 0..ctx.samples {
        let start = Instant::now();
        let tools = yach_backend::rig_adapter::rig_tool_definitions_from_request_with_approved_tools(
            &request,
            approved.iter().map(String::as_str),
        )
        .map_err(|error| format!("{error:?}"))?;
        samples.push(start.elapsed());
        std::hint::black_box(tools);
    }
    let alloc = window.end();
    Ok(Measured::Latency {
        samples,
        alloc: Some(alloc),
    })
}

fn scripted_in_process(ctx: &RunCtx, kind: ScriptKind) -> Result<Measured, String> {
    let mut samples = Vec::with_capacity(ctx.samples);
    let mut alloc = AllocCounts {
        count: 0,
        bytes: 0,
    };
    for _ in 0..ctx.samples {
        let project = TempFs::project("in-process")?;
        let session_path = project.path().join("session.jsonl");
        let script = match kind {
            ScriptKind::TextOnly => Script::text_only("ok"),
            ScriptKind::Tools4 => Script::read_tool_calls(&["src/lib.rs"; 4], "done"),
        };
        let window = AllocWindow::begin();
        let profile = run_scripted_turn(ScriptedTurnConfig {
            project_root: project.path().to_path_buf(),
            session_path,
            script,
            prompt: String::from("hello"),
            trace: None,
        });
        let counts = window.end();
        let profile = profile?;
        samples.push(profile.wall);
        alloc.count = alloc.count.saturating_add(counts.count);
        alloc.bytes = alloc.bytes.saturating_add(counts.bytes);
    }
    Ok(Measured::Latency {
        samples,
        alloc: Some(alloc),
    })
}

fn hashline_ext_child(ctx: &RunCtx) -> Result<Measured, String> {
    let script = Script::read_tool_calls(&["src/lib.rs"; 4], "done");
    let mut samples = Vec::with_capacity(ctx.samples);
    for _ in 0..ctx.samples {
        let home = TempFs::dir("hashline-home")?;
        let extra = [("HOME", home.path().to_string_lossy().into_owned())];
        let trace = TempFs::file("hashline-trace", "jsonl");
        let run = scripted_child_run(ctx, &script, &extra, trace.path())?;
        confirm_hashline_host(&run)?;
        samples.push(run.wall);
    }
    Ok(Measured::Latency {
        samples,
        alloc: None,
    })
}

fn inactive_ext_8_child(ctx: &RunCtx) -> Result<Measured, String> {
    let script = Script::read_tool_calls(&["src/lib.rs"; 4], "done");
    let mut samples = Vec::with_capacity(ctx.samples);
    for sample_index in 0..ctx.samples {
        let mut roots = Vec::with_capacity(8);
        for package in 0..8 {
            let unique = 10_000 + sample_index * 8 + package;
            roots.push(
                ExtensionManifestPackageRoot::create(unique, 1).map_err(|error| error.to_string())?,
            );
        }
        let joined = std::env::join_paths(roots.iter().map(ExtensionManifestPackageRoot::path))
            .map_err(|error| error.to_string())?;
        let extra = [(
            "YACH_EXTENSION_PACKAGE_ROOTS",
            joined.to_string_lossy().into_owned(),
        )];
        let trace = TempFs::file("inactive-ext-trace", "jsonl");
        let run = scripted_child_run(ctx, &script, &extra, trace.path())?;
        confirm_inactive_scan(&run)?;
        drop(roots);
        samples.push(run.wall);
    }
    Ok(Measured::Latency {
        samples,
        alloc: None,
    })
}

fn peak_rss_turn_scripted_tools_4(ctx: &RunCtx) -> Result<Measured, String> {
    let bin = ctx
        .yach_bench_yach_bin
        .as_ref()
        .ok_or_else(|| String::from("bench yach binary missing"))?;
    let script = Script::read_tool_calls(&["src/lib.rs"; 4], "done");
    let mut samples = Vec::with_capacity(ctx.samples);
    for _ in 0..ctx.samples {
        let prepared = prepare_scripted_child(&script)?;
        let mut cmd = Command::new(bin);
        apply_scripted_child_command(&mut cmd, &prepared)?;
        cmd.env_remove("HOME");
        let rss = peak_rss_bytes(
            cmd,
            Spawn::Piped,
            StopBoundary::TraceLabel {
                path: prepared.trace.path().to_path_buf(),
                label: "turn_completed",
            },
            Duration::from_secs(30),
        );

        samples.push(rss?);
    }
    Ok(Measured::Memory(samples))
}

fn cached_child_samples(
    ctx: &RunCtx,
    kind: CachedChildKind,
) -> Result<Vec<CachedChildSample>, String> {
    let key = (ctx.samples, kind);
    {
        let cache = lock_child_cache();
        if let Some(existing) = cache.get(&key) {
            return existing.clone();
        }
    }
    let computed = collect_child_samples(ctx, kind);
    let mut cache = lock_child_cache();
    cache.entry(key).or_insert_with(|| computed.clone());
    computed
}

fn collect_child_samples(
    ctx: &RunCtx,
    kind: CachedChildKind,
) -> Result<Vec<CachedChildSample>, String> {
    let script = match kind {
        CachedChildKind::TextOnly => Script::text_only("ok"),
        CachedChildKind::Tools4Builtin => Script::read_tool_calls(&["src/lib.rs"; 4], "done"),
    };
    let mut samples = Vec::with_capacity(ctx.samples);
    for _ in 0..ctx.samples {
        let trace = TempFs::file("phase-trace", "jsonl");
        let run = scripted_child_run(ctx, &script, &[], trace.path())?;
        samples.push(CachedChildSample {
            records: run.records,
        });
    }
    Ok(samples)
}

fn run_turn_phase(
    ctx: &RunCtx,
    kind: CachedChildKind,
    label: &'static str,
    n: Option<u32>,
) -> Result<Measured, String> {
    let samples = cached_child_samples(ctx, kind)?;
    let mut durations = Vec::with_capacity(samples.len());
    for sample in &samples {
        durations.push(phase_offset(&sample.records, label, n)?);
    }
    Ok(Measured::Latency {
        samples: durations,
        alloc: None,
    })
}

fn phase_offset(
    records: &[TraceRecord],
    label: &str,
    n: Option<u32>,
) -> Result<Duration, String> {
    let origin = records
        .iter()
        .find(|record| record.label == "prompt_received")
        .ok_or_else(|| String::from("prompt_received missing from turn trace"))?;
    let mark = records.iter().find(|record| {
        record.label == label && n.is_none_or(|expected| record.n == Some(expected))
    });
    let Some(mark) = mark else {
        return Err(format!("turn label {label} missing from trace"));
    };
    Ok(Duration::from_micros(mark.t_us.saturating_sub(origin.t_us)))
}

struct ScriptedChildRun {
    wall: Duration,
    records: Vec<TraceRecord>,
    session: String,
}

fn scripted_child_run(
    ctx: &RunCtx,
    script: &Script,
    extra_env: &[(&str, String)],
    trace_path: &Path,
) -> Result<ScriptedChildRun, String> {
    let bin = ctx
        .yach_bench_yach_bin
        .as_ref()
        .ok_or_else(|| String::from("bench yach binary missing"))?;
    let prepared = prepare_scripted_child(script)?;
    let mut cmd = Command::new(bin);
    apply_scripted_child_command(&mut cmd, &prepared)?;
    cmd.env("YACH_TRACE", trace_path);
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("spawn bench yach: {error}"))?;
    let status = wait_child_timeout(&mut child, Duration::from_secs(30))?;
    let wall = start.elapsed();
    let session = fs::read_to_string(prepared.session_path()).unwrap_or_default();
    let trace = fs::read_to_string(trace_path).unwrap_or_default();
    if !status.success() {
        let stderr = fs::read_to_string(prepared.stderr.path()).unwrap_or_default();
        return Err(format!(
            "scripted child exited {}: {session}; stderr={stderr}",
            status.code().unwrap_or(-1)
        ));
    }
    let records = yach_trace::parse_records(&trace).map_err(|error| format!("{error:?}"))?;
    Ok(ScriptedChildRun {
        wall,
        records,
        session,
    })
}

struct PreparedChild {
    project: TempFs,
    script: TempFs,
    trace: TempFs,
    stderr: TempFs,
}

impl PreparedChild {
    fn session_path(&self) -> PathBuf {
        self.project.path().join("session.jsonl")
    }
}

fn prepare_scripted_child(script: &Script) -> Result<PreparedChild, String> {
    let project = TempFs::project("child")?;
    let script_file = TempFs::file("bench-script", "json");
    let json = serde_json::to_string(script).map_err(|error| error.to_string())?;
    fs::write(script_file.path(), json).map_err(|error| error.to_string())?;
    Ok(PreparedChild {
        project,
        script: script_file,
        trace: TempFs::file("child-trace", "jsonl"),
        stderr: TempFs::file("child-stderr", "log"),
    })
}

fn apply_scripted_child_command(
    cmd: &mut Command,
    prepared: &PreparedChild,
) -> Result<(), String> {
    let stderr = fs::File::create(prepared.stderr.path()).map_err(|error| error.to_string())?;
    cmd.arg("run")
        .arg("--prompt")
        .arg("hello")
        .arg("--project-root")
        .arg(prepared.project.path())
        .arg("--session-path")
        .arg(prepared.session_path())
        .env("YACH_RIG_PROVIDER", "scripted")
        .env("YACH_BENCH_SCRIPT", prepared.script.path())
        .env("YACH_TRACE", prepared.trace.path())
        .env_remove("HOME")
        .env_remove("YACH_EXTENSION_PACKAGE_ROOTS")
        .env_remove("YACH_EXTENSION_USER_STORE")
        .env_remove("YACH_EXTENSION_PROJECT_STORE")
        .current_dir(prepared.project.path())
        .stdout(Stdio::null())
        .stderr(stderr);
    Ok(())
}

fn wait_child_timeout(
    child: &mut Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(String::from("child timed out after 30s"));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn confirm_hashline_host(run: &ScriptedChildRun) -> Result<(), String> {
    let has_results = run
        .records
        .iter()
        .any(|record| record.label == "tool_result_appended" && record.n == Some(4));
    let names_host = run.session.contains("hashline_read")
        || run.session.contains("hashline_edit")
        || run.session.contains("yach.hashline");
    if has_results && names_host {
        return Ok(());
    }
    Err(String::from(
        "hashline extension does not activate under headless run: postFirstPaint activation is scheduled on FirstRenderCompleted, but the scripted prompt is submitted as soon as ModelActivationFinished arrives and does not wait for extension_background_activation_finished",
    ))
}

fn confirm_inactive_scan(run: &ScriptedChildRun) -> Result<(), String> {
    if run
        .records
        .iter()
        .any(|record| record.label == "extension_manifest_scan_failed")
    {
        return Err(String::from("inactive_ext_8 scan failed"));
    }
    let finished = run.records.iter().any(|record| {
        record.scope == "startup" && record.label == "extension_manifest_scan_finished"
    });
    if !finished {
        return Err(String::from(
            "inactive_ext_8 missing startup extension_manifest_scan_finished",
        ));
    }
    Ok(())
}

enum TempKind {
    Dir,
    File,
}

struct TempFs {
    path: PathBuf,
    kind: TempKind,
}

impl TempFs {
    fn dir(label: &str) -> Result<Self, String> {
        let path = unique_temp_path(label, None);
        let guard = Self {
            path,
            kind: TempKind::Dir,
        };
        fs::create_dir_all(guard.path()).map_err(|error| error.to_string())?;
        Ok(guard)
    }

    fn project(label: &str) -> Result<Self, String> {
        let dir = Self::dir(label)?;
        fs::create_dir_all(dir.path().join("src")).map_err(|error| error.to_string())?;
        fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}\n")
            .map_err(|error| error.to_string())?;
        Ok(dir)
    }

    fn file(label: &str, ext: &str) -> Self {
        Self {
            path: unique_temp_path(label, Some(ext)),
            kind: TempKind::File,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFs {
    fn drop(&mut self) {
        match self.kind {
            TempKind::Dir => {
                let _ = fs::remove_dir_all(&self.path);
            }
            TempKind::File => {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

fn unique_temp_path(label: &str, ext: Option<&str>) -> PathBuf {
    let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = match ext {
        Some(ext) => format!("yach-core-loop-{label}-{}-{n}.{ext}", std::process::id()),
        None => format!("yach-core-loop-{label}-{}-{n}", std::process::id()),
    };
    std::env::temp_dir().join(name)
}

#[cfg(test)]
mod tests {
    use crate::perf::registry::{RunCtx, all};
    use crate::perf::schema::Class;

    #[test]
    fn core_loop_ids_registered() {
        let ids: Vec<&str> = all().iter().map(|w| w.id).collect();
        for id in [
            "request/assemble/10_turns",
            "request/assemble/1000_turns",
            "request/roster_bytes/builtin",
            "turn/scripted/text_only",
            "turn/scripted/tools_4/builtin",
            "turn/phase/turn_completed",
            "memory/peak_rss/turn_scripted_tools_4",
        ] {
            assert!(ids.contains(&id), "missing {id}");
        }
    }

    #[test]
    fn roster_bytes_builtin_is_positive() {
        let w = all().iter().find(|w| w.id == "request/roster_bytes/builtin");
        let Some(w) = w else {
            return;
        };
        let ctx = RunCtx {
            samples: 1,
            yach_bin: None,
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            filter: None,
        };
        let measured = (w.run)(&ctx);
        assert!(matches!(measured, Ok(crate::perf::registry::Measured::Value(v)) if v > 1000));
        assert_eq!(w.class, Class::Count);
    }

    #[test]
    fn scripted_text_turn_measures_in_process() {
        let _guard = crate::perf::alloc::lock_window_for_test();
        let w = all().iter().find(|w| w.id == "turn/scripted/text_only");
        let Some(w) = w else {
            return;
        };
        let ctx = RunCtx {
            samples: 2,
            yach_bin: None,
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            filter: None,
        };
        let measured = (w.run)(&ctx);
        assert!(
            measured.is_ok(),
            "{}",
            measured.as_ref().err().map_or("", String::as_str)
        );
        match measured {
            Ok(crate::perf::registry::Measured::Latency { samples, alloc }) => {
                assert_eq!(samples.len(), 2);
                assert!(alloc.is_some_and(|a| a.count > 0));
            }
            Ok(crate::perf::registry::Measured::Memory(_)) => {
                assert_eq!("Latency", "Memory", "unexpected variant: Memory");
            }
            Ok(crate::perf::registry::Measured::Value(_)) => {
                assert_eq!("Latency", "Value", "unexpected variant: Value");
            }
            Err(error) => {
                assert_eq!(String::new(), error);
            }
        }
    }
}
