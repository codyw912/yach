use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::Duration;

use yach_trace::TraceRecord;

use crate::perf::registry::{Bin, Measured, Requirement, RunCtx, Workload};
use crate::perf::schema::{Class, Isolation};

macro_rules! startup_phase {
    ($label:literal) => {
        Workload {
            id: concat!("startup/phase/", $label),
            class: Class::Latency,
            isolation: Isolation::ChildProcess,
            requires: &[Requirement::Binary],
            bin: Some(Bin::Shipping),
            run: |ctx| run_phase(ctx, StartupProfileScenario::Baseline, $label),
        }
    };
}

macro_rules! scan_phase {
    ($label:literal) => {
        Workload {
            id: concat!("startup/phase/", $label),
            class: Class::Latency,
            isolation: Isolation::ChildProcess,
            requires: &[Requirement::Binary],
            bin: Some(Bin::Shipping),
            run: |ctx| run_phase(ctx, StartupProfileScenario::InactiveExtension, $label),
        }
    };
}

pub static STARTUP: [Workload; 26] = [
    Workload {
        id: "yach/tui_startup_first_output_pty",
        class: Class::Latency,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary],
        bin: Some(Bin::Shipping),
        run: |ctx| run_tui_first_output(ctx, "tui"),
    },
    Workload {
        id: "yach/tui_ready_startup_first_output_pty",
        class: Class::Latency,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary],
        bin: Some(Bin::Shipping),
        run: |ctx| run_tui_first_output(ctx, "tui-bench-ready"),
    },
    Workload {
        id: "yach/cli_startup_first_output",
        class: Class::Latency,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary],
        bin: Some(Bin::Shipping),
        run: run_cli_first_output,
    },
    Workload {
        id: "yach/tui_startup_profile/observed_process_to_first_render_pty",
        class: Class::Latency,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary],
        bin: Some(Bin::Shipping),
        run: |ctx| run_observed(ctx, StartupProfileScenario::Baseline),
    },
    Workload {
        id: "yach/tui_startup_profile_with_inactive_extension/observed_process_to_first_render_pty",
        class: Class::Latency,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary],
        bin: Some(Bin::Shipping),
        run: |ctx| run_observed(ctx, StartupProfileScenario::InactiveExtension),
    },
    Workload {
        id: "yach/tui_startup_profile_many_extensions/observed_process_to_first_render_pty",
        class: Class::Latency,
        isolation: Isolation::ChildProcess,
        requires: &[Requirement::Binary],
        bin: Some(Bin::Shipping),
        run: |ctx| run_observed(ctx, StartupProfileScenario::ManyExtensions),
    },
    startup_phase!("process_main_start"),
    startup_phase!("cli_args_parsed"),
    startup_phase!("command_run_start"),
    startup_phase!("tokio_runtime_created"),
    startup_phase!("backend_setup_start"),
    startup_phase!("backend_session_started"),
    startup_phase!("client_initialize_sent"),
    startup_phase!("backend_task_spawned"),
    startup_phase!("run_tui_start"),
    startup_phase!("tui_app_created"),
    startup_phase!("tui_raw_mode_enabled"),
    startup_phase!("tui_cursor_hidden"),
    startup_phase!("tui_terminal_created"),
    startup_phase!("tui_event_stream_created"),
    startup_phase!("tui_first_backend_event_received"),
    startup_phase!("tui_first_render_start"),
    startup_phase!("tui_first_render_end"),
    scan_phase!("extension_manifest_scan_scheduled"),
    scan_phase!("extension_manifest_scan_started"),
    scan_phase!("extension_manifest_scan_finished"),
];

#[must_use]
pub fn trace_labels_since_main(records: &[TraceRecord]) -> BTreeMap<String, Duration> {
    let Some(origin) = records
        .iter()
        .find(|record| record.scope == "startup" && record.label == "process_main_start")
        .map(|record| record.t_us)
    else {
        return BTreeMap::new();
    };

    let mut labels = BTreeMap::new();
    for record in records {
        if record.scope != "startup" {
            continue;
        }
        labels.insert(
            record.label.clone(),
            Duration::from_micros(record.t_us.saturating_sub(origin)),
        );
    }
    labels
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum StartupProfileScenario {
    Baseline,
    InactiveExtension,
    ManyExtensions,
}

impl StartupProfileScenario {
    const fn manifest_count(self) -> usize {
        match self {
            Self::Baseline => 0,
            Self::InactiveExtension => 1,
            Self::ManyExtensions => 50,
        }
    }

    const fn wait_label(self) -> &'static str {
        match self {
            Self::Baseline => "tui_first_render_end",
            Self::InactiveExtension | Self::ManyExtensions => "extension_manifest_scan_finished",
        }
    }
}

#[derive(Clone)]
struct StartupProfileSample {
    observed_process_to_first_render: Duration,
    records: Vec<TraceRecord>,
}

type ProfileCacheKey = (usize, StartupProfileScenario);
type ProfileCache = HashMap<ProfileCacheKey, Result<Vec<StartupProfileSample>, String>>;

static PROFILE_CACHE: LazyLock<Mutex<ProfileCache>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_profile_cache() -> std::sync::MutexGuard<'static, ProfileCache> {
    match PROFILE_CACHE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn cached_profiles(
    ctx: &RunCtx,
    scenario: StartupProfileScenario,
) -> Result<Vec<StartupProfileSample>, String> {
    let key = (ctx.samples, scenario);
    {
        let cache = lock_profile_cache();
        if let Some(existing) = cache.get(&key) {
            return existing.clone();
        }
    }
    let computed = collect_profile_samples(ctx, scenario);
    let mut cache = lock_profile_cache();
    cache.entry(key).or_insert_with(|| computed.clone());
    computed
}

fn collect_profile_samples(
    ctx: &RunCtx,
    scenario: StartupProfileScenario,
) -> Result<Vec<StartupProfileSample>, String> {
    let mut samples = Vec::with_capacity(ctx.samples);
    for sample_index in 0..ctx.samples {
        samples.push(
            sample_yach_tui_startup_profile(ctx, sample_index, scenario)
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(samples)
}

fn run_tui_first_output(ctx: &RunCtx, command: &str) -> Result<Measured, String> {
    let mut samples = Vec::with_capacity(ctx.samples);
    for _ in 0..ctx.samples {
        samples.push(sample_yach_tui_first_output(ctx, command).map_err(|error| error.to_string())?);
    }
    Ok(Measured::Latency {
        samples,
        alloc: None,
    })
}

fn run_cli_first_output(ctx: &RunCtx) -> Result<Measured, String> {
    let mut samples = Vec::with_capacity(ctx.samples);
    for _ in 0..ctx.samples {
        samples.push(sample_yach_cli_first_output(ctx).map_err(|error| error.to_string())?);
    }
    Ok(Measured::Latency {
        samples,
        alloc: None,
    })
}

fn run_observed(
    ctx: &RunCtx,
    scenario: StartupProfileScenario,
) -> Result<Measured, String> {
    let samples = cached_profiles(ctx, scenario)?;
    Ok(Measured::Latency {
        samples: samples
            .iter()
            .map(|sample| sample.observed_process_to_first_render)
            .collect(),
        alloc: None,
    })
}

fn run_phase(
    ctx: &RunCtx,
    scenario: StartupProfileScenario,
    label: &'static str,
) -> Result<Measured, String> {
    let samples = cached_profiles(ctx, scenario)?;
    let mut durations = Vec::new();
    for sample in &samples {
        if let Some(duration) = trace_labels_since_main(&sample.records).get(label).copied() {
            durations.push(duration);
        }
    }
    if durations.is_empty() {
        return Err(format!("startup label {label} missing from profile samples"));
    }
    Ok(Measured::Latency {
        samples: durations,
        alloc: None,
    })
}

fn sample_yach_tui_startup_profile(
    ctx: &RunCtx,
    sample_index: usize,
    scenario: StartupProfileScenario,
) -> io::Result<StartupProfileSample> {
    let bin = resolve_yach_cli_bin(ctx)?;
    let trace_path = std::env::temp_dir().join(format!(
        "yach-startup-trace-{}-{sample_index}.log",
        std::process::id()
    ));
    let _ = fs::remove_file(&trace_path);
    let manifest_dir = match scenario {
        StartupProfileScenario::Baseline => None,
        StartupProfileScenario::InactiveExtension | StartupProfileScenario::ManyExtensions => Some(
            ExtensionManifestPackageRoot::create(sample_index, scenario.manifest_count())?,
        ),
    };

    let start = std::time::Instant::now();
    let mut command = Command::new("script");
    command
        .args(["-q", "/dev/null", "--"])
        .arg(&bin)
        .arg("tui")
        .env("YACH_TRACE", &trace_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(manifest_dir) = manifest_dir.as_ref() {
        command.env("YACH_EXTENSION_PACKAGE_ROOTS", manifest_dir.path());
    }
    let mut child = command.spawn()?;
    let _stdin_guard = child.stdin.take();

    let first_render_records =
        wait_for_trace_label(&trace_path, "tui_first_render_end", Duration::from_secs(5))?;
    let observed_process_to_first_render = start.elapsed();
    let wait_label = scenario.wait_label();
    let records = wait_for_startup_profile_terminal_marks(
        &trace_path,
        wait_label,
        scenario == StartupProfileScenario::Baseline,
        Duration::from_secs(5),
    );
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&trace_path);

    let records = if wait_label == "tui_first_render_end" {
        Ok(first_render_records)
    } else {
        records
    };

    records.map(|records| StartupProfileSample {
        observed_process_to_first_render,
        records,
    })
}

struct ExtensionManifestPackageRoot {
    path: PathBuf,
}

impl ExtensionManifestPackageRoot {
    fn create(sample_index: usize, manifest_count: usize) -> io::Result<Self> {
        let manifest_dir = extension_manifest_package_root_path(sample_index);
        let _ = fs::remove_dir_all(&manifest_dir);
        let guard = Self { path: manifest_dir };
        fs::create_dir_all(guard.path())?;
        if manifest_count == 1 {
            fs::write(
                guard.path().join("yach.extension.json"),
                extension_manifest_json(0),
            )?;
        } else {
            fs::create_dir_all(guard.path().join("manifests"))?;
            let mut manifest_paths = Vec::with_capacity(manifest_count);
            for index in 0..manifest_count {
                let relative_path = format!("manifests/toy-{index}.extension.json");
                fs::write(
                    guard.path().join(&relative_path),
                    extension_manifest_json(index),
                )?;
                manifest_paths.push(relative_path);
            }
            fs::write(
                guard.path().join("package.json"),
                extension_package_json(&manifest_paths),
            )?;
        }
        Ok(guard)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ExtensionManifestPackageRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn extension_manifest_package_root_path(sample_index: usize) -> PathBuf {
    std::env::temp_dir().join(format!(
        "yach-inactive-extension-manifest-{}-{sample_index}",
        std::process::id()
    ))
}

fn extension_manifest_json(index: usize) -> String {
    format!(
        r#"{{
  "schema": "yach.extension.v1",
  "id": "example.inactive-toy-tools-{index}",
  "version": "0.1.0",
  "main": {{
    "command": "node",
    "args": ["./extension.js"]
  }},
  "activation": {{
    "events": ["onCommand:yach.extensions.activate.example.inactive-toy-tools-{index}"]
  }},
  "contributes": {{
    "tools": [{{
      "name": "inactive_toy_tool_{index}",
      "description": "Return static fixture metadata when activated.",
      "risk": "reads_local_metadata",
      "provider_visible": false
    }}]
  }}
}}"#
    )
}

fn extension_package_json(manifest_paths: &[String]) -> String {
    serde_json::json!({
        "name": "yach-bench-extension-package",
        "version": "0.1.0",
        "yach": {
            "manifests": manifest_paths,
        },
    })
    .to_string()
}

fn parse_live_startup_records(contents: &str) -> Result<Vec<TraceRecord>, String> {
    match yach_trace::parse_records(contents) {
        Ok(records) => Ok(records
            .into_iter()
            .filter(|record| record.scope == "startup")
            .collect()),
        Err(yach_trace::TraceParseError::TruncatedLine { .. }) => match contents.rfind('\n') {
            Some(index) => parse_live_startup_records(&contents[..=index]),
            None => Ok(Vec::new()),
        },
        Err(yach_trace::TraceParseError::Malformed { line_no, message }) => {
            Err(format!("trace line {line_no}: {message}"))
        }
    }
}

fn wait_for_trace_label(
    path: &PathBuf,
    label: &str,
    timeout: Duration,
) -> io::Result<Vec<TraceRecord>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(contents) = fs::read_to_string(path) {
            let records = parse_live_startup_records(&contents).map_err(io::Error::other)?;
            if records.iter().any(|record| record.label == label) {
                return Ok(records);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for startup trace label {label}; rebuild release yach-cli before profiling"
                ),
            ));
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_startup_profile_terminal_marks(
    path: &PathBuf,
    success_label: &str,
    ignore_scan_failure: bool,
    timeout: Duration,
) -> io::Result<Vec<TraceRecord>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(contents) = fs::read_to_string(path) {
            let records = parse_live_startup_records(&contents).map_err(io::Error::other)?;
            if records.iter().any(|record| record.label == success_label) {
                return Ok(records);
            }

            if !ignore_scan_failure
                && records
                    .iter()
                    .any(|record| record.label == "extension_manifest_scan_failed")
            {
                return Err(io::Error::other(format!(
                    "startup profile observed extension_manifest_scan_failed before {success_label}"
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for startup trace label {success_label}; rebuild release yach-cli before profiling"
                ),
            ));
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn sample_yach_tui_first_output(ctx: &RunCtx, command: &str) -> io::Result<Duration> {
    let bin = resolve_yach_cli_bin(ctx)?;
    let start = std::time::Instant::now();
    let mut child = Command::new("script")
        .args(["-q", "/dev/null", "--"])
        .arg(&bin)
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing script stdout"))?;
    let read_result = read_first_byte_with_timeout(stdout, Duration::from_secs(5));
    let elapsed = start.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    read_result.map(|()| elapsed)
}

fn sample_yach_cli_first_output(ctx: &RunCtx) -> io::Result<Duration> {
    let bin = resolve_yach_cli_bin(ctx)?;
    let start = std::time::Instant::now();
    let mut child = Command::new(&bin)
        .arg("--quiet")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing yach stdout"))?;
    let read_result = read_first_byte_with_timeout(stdout, Duration::from_secs(5));
    let elapsed = start.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    read_result.map(|()| elapsed)
}

fn resolve_yach_cli_bin(ctx: &RunCtx) -> io::Result<PathBuf> {
    if let Some(path) = ctx.yach_bin.as_ref() {
        return Ok(path.clone());
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_yach") {
        return Ok(PathBuf::from(path));
    }

    let current_exe = std::env::current_exe()?;
    let parent = current_exe.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "yach binary missing")
    })?;
    for name in ["yach", "yach-cli"] {
        let candidate = parent.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "yach binary missing",
    ))
}

fn read_first_byte_with_timeout(mut stdout: ChildStdout, timeout: Duration) -> io::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut first_byte = [0_u8; 1];
        let result = stdout.read_exact(&mut first_byte);
        let _ = tx.send(result);
    });

    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for first output byte",
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ExtensionManifestPackageRoot, parse_live_startup_records, trace_labels_since_main, STARTUP,
    };
    use crate::perf::registry::{Measured, RunCtx};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;
    use yach_backend::{ExtensionId, parse_extension_manifest};
    use yach_trace::TraceRecord;

    fn trace_record(t_us: u64, scope: &str, label: &str) -> TraceRecord {
        TraceRecord {
            t_us,
            scope: String::from(scope),
            turn_id: None,
            label: String::from(label),
            n: None,
        }
    }

    #[test]
    fn trace_labels_since_main_uses_startup_scope_relative_to_process_main_start() {
        let records = [
            trace_record(100, "startup", "process_main_start"),
            TraceRecord {
                t_us: 10,
                scope: String::from("turn"),
                turn_id: Some(String::from("t1")),
                label: String::from("prompt_received"),
                n: None,
            },
            trace_record(228, "startup", "cli_args_parsed"),
            trace_record(2600, "startup", "tui_first_render_end"),
        ];

        let labels = trace_labels_since_main(&records);
        assert_eq!(
            labels.get("process_main_start").copied(),
            Some(Duration::from_micros(0))
        );
        assert_eq!(
            labels.get("cli_args_parsed").copied(),
            Some(Duration::from_micros(128))
        );
        assert_eq!(
            labels.get("tui_first_render_end").copied(),
            Some(Duration::from_micros(2500))
        );
        assert!(!labels.contains_key("prompt_received"));
    }

    #[test]
    fn parse_live_startup_records_ignores_turn_scope_and_tolerates_truncated_trailing_line() {
        let records = parse_live_startup_records(
            "{\"t_us\":0,\"scope\":\"startup\",\"label\":\"process_main_start\"}\n{\"t_us\":10,\"scope\":\"turn\",\"turn_id\":\"t1\",\"label\":\"prompt_received\"}\n{\"t_us\":128,\"scope\":\"startup\",\"label\":\"cli_args_parsed\"}\n{\"t_us\":2500,\"scope\":\"startup\",\"label\":\"tui_first_render_end",
        );
        assert!(records.is_ok(), "parse failed: {records:?}");
        let Ok(records) = records else {
            return;
        };
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].label, "process_main_start");
        assert_eq!(records[1].label, "cli_args_parsed");
    }

    #[test]
    fn parse_live_startup_records_malformed_line_returns_err_with_line_number() {
        let result = parse_live_startup_records("not-json\n");
        assert!(result.is_err());
        if let Err(message) = result {
            assert!(message.contains("trace line 1"));
        }
    }

    #[test]
    fn inactive_extension_manifest_fixture_is_valid_and_inactive_by_command() -> Result<(), String>
    {
        let value = serde_json::from_str(&super::extension_manifest_json(0))
            .map_err(|error| format!("manifest JSON parse failed: {error}"))?;
        let manifest = parse_extension_manifest(value)
            .map_err(|error| format!("extension manifest parse failed: {error:?}"))?;

        let expected_id = ExtensionId(String::from("example.inactive-toy-tools-0"));
        if manifest.id != expected_id {
            return Err(format!("expected manifest id {expected_id:?}"));
        }
        if manifest.contributes.tools.len() != 1 {
            return Err(format!(
                "expected one contributed tool, found {}",
                manifest.contributes.tools.len()
            ));
        }
        if !manifest
            .contributes
            .tools
            .iter()
            .all(|tool| !tool.provider_visible)
        {
            return Err(String::from(
                "expected all contributed tools to be inactive",
            ));
        }
        Ok(())
    }

    #[test]
    fn inactive_extension_manifest_dir_contains_one_manifest() -> Result<(), String> {
        let manifest_dir = ExtensionManifestPackageRoot::create(usize::MAX, 1)
            .map_err(|error| format!("inactive manifest dir create failed: {error}"))?;

        let entries = fs::read_dir(manifest_dir.path())
            .map_err(|error| format!("inactive manifest dir read failed: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("inactive manifest dir entry read failed: {error}"))?;
        if entries.len() != 1 {
            return Err(format!(
                "expected one manifest entry, found {}",
                entries.len()
            ));
        }
        let Some(entry) = entries.first() else {
            return Err(String::from("expected manifest entry to exist"));
        };
        if entry.file_name().to_string_lossy() != "yach.extension.json" {
            return Err(format!(
                "expected package-root manifest name, found {}",
                entry.file_name().to_string_lossy()
            ));
        }
        Ok(())
    }

    #[test]
    fn inactive_extension_manifest_dir_is_removed_on_drop() -> Result<(), String> {
        let manifest_dir = ExtensionManifestPackageRoot::create(usize::MAX - 1, 1)
            .map_err(|error| format!("inactive manifest dir create failed: {error}"))?;
        let manifest_path = manifest_dir.path().to_path_buf();
        if !manifest_path.exists() {
            return Err(format!(
                "expected inactive manifest dir to exist: {}",
                manifest_path.display()
            ));
        }

        drop(manifest_dir);

        if manifest_path.exists() {
            return Err(format!(
                "expected inactive manifest dir to be removed: {}",
                manifest_path.display()
            ));
        }
        Ok(())
    }

    #[test]
    fn many_extension_manifest_package_contains_package_json_and_manifests() -> Result<(), String> {
        let manifest_dir = ExtensionManifestPackageRoot::create(usize::MAX - 2, 3)
            .map_err(|error| format!("many manifest package create failed: {error}"))?;

        if !manifest_dir.path().join("package.json").exists() {
            return Err(String::from("expected package.json manifest pointer"));
        }
        for index in 0..3 {
            let manifest_path = manifest_dir
                .path()
                .join(format!("manifests/toy-{index}.extension.json"));
            if !manifest_path.exists() {
                return Err(format!("expected manifest at {}", manifest_path.display()));
            }
        }

        Ok(())
    }

    fn debug_yach_bin() -> Option<PathBuf> {
        let output = Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        let target_directory = value.get("target_directory")?.as_str()?;
        let path = PathBuf::from(target_directory).join("debug").join("yach");
        path.is_file().then_some(path)
    }

    #[test]
    fn cli_startup_first_output_collects_requested_samples() -> Result<(), String> {
        let yach_bin = debug_yach_bin().ok_or_else(|| {
            String::from(
                "yach debug binary missing at cargo metadata target_directory/debug/yach; run `just dev cargo build -p yach`",
            )
        })?;
        let ctx = RunCtx {
            samples: 2,
            yach_bin: Some(yach_bin),
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            filter: None,
        };
        let workload = STARTUP
            .iter()
            .find(|workload| workload.id == "yach/cli_startup_first_output")
            .ok_or_else(|| String::from("registry is missing yach/cli_startup_first_output"))?;
        match (workload.run)(&ctx) {
            Ok(Measured::Latency { samples, .. }) => {
                if samples.len() != 2 {
                    return Err(format!("expected 2 samples, got {}", samples.len()));
                }
            }
            Ok(_) => return Err(String::from("expected Latency samples from cli startup")),
            Err(error) => return Err(format!("cli startup failed: {error}")),
        }
        Ok(())
    }
}
