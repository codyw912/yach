use std::collections::BTreeMap;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::perf::Outcome;
use crate::perf::provenance::{capture_build, capture_host};
use crate::perf::registry::{self, Bin, Measured, Requirement, RunCtx, Workload};
use crate::perf::schema::{Class, Isolation, ResultDoc, SCHEMA, Status, WorkloadRow};

pub const EXTERNAL_IDS: [&str; 5] = [
    "binary/size_bytes",
    "yach/tui_startup_first_output_pty",
    "yach/tui_ready_startup_first_output_pty",
    "yach/cli_startup_first_output",
    "memory/peak_rss/tui_ready",
];

#[derive(Debug, Clone)]
pub struct Artifacts {
    pub yach_bin: PathBuf,
    pub yach_bench_yach_bin: PathBuf,
    pub yach_bench_bin: PathBuf,
}

#[derive(Debug, Clone)]
pub struct WorkerArgs {
    pub schema: u32,
    pub filter: Option<String>,
    pub ids: Option<String>,
    pub classes: Option<String>,
    pub samples: usize,
    pub yach_bin: Option<PathBuf>,
    pub yach_bench_yach_bin: Option<PathBuf>,
    pub yach_bench_bin: Option<PathBuf>,
    pub checkout: Option<PathBuf>,
    pub deterministic: bool,
    pub raw: bool,
    pub out: PathBuf,
    pub external: bool,
}

impl WorkerArgs {
    #[must_use]
    pub fn external_sampler(
        filter: Option<String>,
        samples: usize,
        yach_bin: PathBuf,
        checkout: PathBuf,
        deterministic: bool,
        out: PathBuf,
    ) -> Self {
        Self {
            schema: SCHEMA,
            filter,
            ids: None,
            classes: None,
            samples,
            yach_bin: Some(yach_bin),
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            checkout: Some(checkout),
            deterministic,
            raw: true,
            out,
            external: true,
        }
    }
}

struct Flags {
    schema: Option<u32>,
    schema_probe: bool,
    filter: Option<String>,
    ids: Option<String>,
    classes: Option<String>,
    samples: Option<usize>,
    yach_bin: Option<PathBuf>,
    yach_bench_yach_bin: Option<PathBuf>,
    yach_bench_bin: Option<PathBuf>,
    checkout: Option<PathBuf>,
    deterministic: bool,
    raw: bool,
    out: Option<PathBuf>,
}

pub fn measure(ctx: &RunCtx, deterministic: bool, raw: bool) -> Vec<WorkloadRow> {
    measure_restricted(ctx, deterministic, raw, None, None, None)
}

fn measure_restricted(
    ctx: &RunCtx,
    deterministic: bool,
    raw: bool,
    only: Option<&[&str]>,
    exact_ids: Option<&[String]>,
    classes: Option<&[Class]>,
) -> Vec<WorkloadRow> {
    let has_tty = io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut planned = Vec::new();
    for (index, workload) in registry::all().iter().enumerate() {
        if let Some(only) = only
            && !only.contains(&workload.id)
        {
            continue;
        }
        if let Some(ids) = exact_ids {
            if !ids.iter().any(|id| id == workload.id) {
                continue;
            }
        } else if let Some(filter) = &ctx.filter
            && !id_matches(filter, workload.id)
        {
            continue;
        }
        if let Some(classes) = classes
            && !classes.contains(&workload.class)
        {
            continue;
        }
        let wants_row = !deterministic || matches!(workload.class, Class::Size | Class::Count);
        let wants_alloc = workload.emit_alloc
            && workload.isolation == Isolation::InProcessSerial
            && workload.class == Class::Latency
            && classes.is_none_or(|allowed| allowed.contains(&Class::Count));
        if !wants_row && !wants_alloc {
            continue;
        }
        planned.push((index, workload, wants_row, wants_alloc));
    }
    planned.sort_by_key(|(index, workload, _, _)| {
        (workload.isolation != Isolation::ChildProcess, *index)
    });
    let mut grouped: BTreeMap<usize, Vec<WorkloadRow>> = BTreeMap::new();
    for (index, workload, wants_row, wants_alloc) in planned {
        let mut rows = Vec::new();
        if let Some(reason) = unmet(workload, ctx, has_tty) {
            if wants_row {
                rows.push(WorkloadRow::skipped(
                    workload.id,
                    workload.class,
                    workload.isolation,
                    reason,
                ));
            }
            if wants_alloc {
                for suffix in ["#alloc_count", "#alloc_bytes"] {
                    rows.push(WorkloadRow::skipped(
                        &format!("{}{suffix}", workload.id),
                        Class::Count,
                        workload.isolation,
                        reason,
                    ));
                }
            }
            grouped.entry(index).or_default().extend(rows);
            continue;
        }
        let samples = if deterministic && !wants_row {
            1
        } else {
            ctx.samples
        };

        let local = RunCtx {
            samples,
            ..ctx.clone()
        };
        let result = (workload.run)(&local);
        match result {
            Ok(measured) => {
                let inner = match &measured {
                    Measured::Latency {
                        alloc: Some(counts),
                        ..
                    } => Some(*counts),
                    _ => None,
                };
                let row = row_from(workload, measured, raw);
                if wants_row {
                    rows.push(row.clone());
                }
                if wants_alloc && let Some(counts) = inner {
                    rows.extend(registry::derived_alloc_rows(&row, counts));
                }
            }
            Err(message) => {
                if wants_row {
                    rows.push(WorkloadRow::error(
                        workload.id,
                        workload.class,
                        workload.isolation,
                        &message,
                    ));
                }
                if wants_alloc {
                    for suffix in ["#alloc_count", "#alloc_bytes"] {
                        rows.push(WorkloadRow::error(
                            &format!("{}{suffix}", workload.id),
                            Class::Count,
                            workload.isolation,
                            &message,
                        ));
                    }
                }
            }
        }
        grouped.entry(index).or_default().extend(rows);
    }
    grouped.into_values().flatten().collect()
}

fn unmet(workload: &Workload, ctx: &RunCtx, has_tty: bool) -> Option<&'static str> {
    for requirement in workload.requires {
        match requirement {
            Requirement::Tty if !has_tty => return Some("requires tty"),
            Requirement::Linux if !cfg!(target_os = "linux") => {
                return Some("unsupported_os");
            }
            Requirement::Binary => {
                let missing = match workload.bin {
                    Some(Bin::Bench) => ctx.yach_bench_yach_bin.is_none(),
                    Some(Bin::Shipping) | None => ctx.yach_bin.is_none(),
                };
                if missing {
                    return Some("yach binary missing");
                }
            }
            Requirement::Tty | Requirement::Linux => {}
        }
    }
    None
}

fn row_from(workload: &Workload, measured: Measured, raw: bool) -> WorkloadRow {
    match measured {
        Measured::Latency { samples, alloc: _ } => {
            let mut row = WorkloadRow::latency(workload.id, workload.isolation, &samples);
            if raw {
                row.samples_ns = Some(samples.iter().copied().map(duration_ns).collect());
            }
            row
        }
        Measured::Memory(samples_bytes) => {
            let mut row = WorkloadRow::memory(workload.id, &samples_bytes);
            if raw {
                row.samples_bytes = Some(samples_bytes);
            }
            row
        }
        Measured::Value(value) => {
            WorkloadRow::value(workload.id, workload.class, workload.isolation, value)
        }
    }
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn id_matches(pattern: &glob::Pattern, id: &str) -> bool {
    pattern.matches_with(
        id,
        glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        },
    )
}

pub fn spawn(bench_bin: &Path, args: &WorkerArgs) -> Result<ResultDoc, String> {
    let mut cmd = Command::new(bench_bin);
    cmd.arg("perf");
    if args.external {
        cmd.arg("external-sampler");
    } else {
        cmd.arg("worker");
        cmd.arg("--schema").arg(args.schema.to_string());
    }
    if let Some(filter) = &args.filter {
        cmd.arg("--filter").arg(filter);
    }
    if let Some(ids) = &args.ids {
        cmd.arg("--ids").arg(ids);
    }
    if let Some(classes) = &args.classes {
        cmd.arg("--classes").arg(classes);
    }

    cmd.arg("--samples").arg(args.samples.to_string());
    if let Some(path) = &args.yach_bin {
        cmd.arg("--yach-bin").arg(path);
    }
    if let Some(path) = &args.yach_bench_yach_bin {
        cmd.arg("--yach-bench-yach-bin").arg(path);
    }
    if let Some(path) = &args.yach_bench_bin {
        cmd.arg("--yach-bench-bin").arg(path);
    }
    if let Some(path) = &args.checkout {
        cmd.arg("--checkout").arg(path);
    }
    if args.deterministic {
        cmd.arg("--deterministic");
    }
    if args.raw {
        cmd.arg("--raw");
    }
    cmd.arg("--out").arg(&args.out);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    let status = cmd
        .status()
        .map_err(|error| format!("spawn {}: {error}", bench_bin.display()))?;
    if !status.success() {
        return Err(format!("worker exited {status}"));
    }
    let missing = || {
        format!(
            "worker exited 0 without a result document at {}",
            args.out.display()
        )
    };
    let bytes = std::fs::read(&args.out).map_err(|_| missing())?;
    let doc: ResultDoc = serde_json::from_slice(&bytes).map_err(|_| missing())?;
    if doc.schema != SCHEMA {
        return Err(format!(
            "schema mismatch: controller {SCHEMA}, worker {}",
            doc.schema
        ));
    }
    Ok(doc)
}

pub(crate) fn default_build_cmd() -> Vec<String> {
    vec![
        String::from("just"),
        String::from("dev"),
        String::from("cargo"),
    ]
}

#[must_use]
pub(crate) fn cargo_invocation(build_cmd: &[String], cargo_args: &[&str]) -> Vec<String> {
    let mut argv = Vec::with_capacity(build_cmd.len().saturating_add(cargo_args.len()));
    argv.extend(build_cmd.iter().cloned());
    argv.extend(cargo_args.iter().map(|arg| (*arg).to_owned()));
    argv
}

pub(crate) fn cargo_target_dir(checkout: &Path, build_cmd: &[String]) -> Result<PathBuf, String> {
    let meta = cargo_output(
        build_cmd,
        checkout,
        &[],
        &["metadata", "--format-version", "1", "--no-deps"],
    )?;
    if !meta.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&meta.stderr)
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&meta.stdout)
        .map_err(|error| format!("cargo metadata json: {error}"))?;
    let target_dir = json
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| String::from("cargo metadata missing target_directory"))?;
    Ok(PathBuf::from(target_dir))
}

pub fn build_current(checkout: &Path) -> Result<Artifacts, String> {
    let cmd = default_build_cmd();
    let target = cargo_target_dir(checkout, &cmd)?;
    let side = crate::perf::ab::build_side(
        checkout,
        &target,
        crate::perf::ab::BuildStage::BenchAndWorker,
        &cmd,
    )?;
    Ok(Artifacts {
        yach_bin: side.yach_bin,
        yach_bench_yach_bin: side.yach_bench_yach_bin,
        yach_bench_bin: side.yach_bench_bin,
    })
}

pub(crate) fn cmd_worker(args: &[String]) -> Result<Outcome, String> {
    run_in_process(args, false)
}

pub(crate) fn cmd_external_sampler(args: &[String]) -> Result<Outcome, String> {
    run_in_process(args, true)
}

pub(crate) fn cmd_run(args: &[String]) -> Result<Outcome, String> {
    if args.iter().any(|arg| arg == "--list") {
        return Ok(Outcome {
            lines: list_workloads(),
            exit_code: 0,
        });
    }
    let flags = parse_flags(args)?;
    let out = flags
        .out
        .ok_or_else(|| format!("missing --out\n{}", crate::perf::USAGE))?;
    let checkout = resolve_checkout(flags.checkout.as_deref())?;

    let artifacts = build_current(&checkout)?;
    let samples = flags.samples.unwrap_or(100);
    let bench_bin = artifacts.yach_bench_bin.clone();
    let worker_args = WorkerArgs {
        schema: SCHEMA,
        filter: flags.filter,
        ids: flags.ids,
        classes: flags.classes,
        samples,
        yach_bin: Some(artifacts.yach_bin),
        yach_bench_yach_bin: Some(artifacts.yach_bench_yach_bin),
        yach_bench_bin: Some(artifacts.yach_bench_bin),
        checkout: Some(checkout),
        deterministic: flags.deterministic,
        raw: flags.raw,
        out,
        external: false,
    };

    let doc = spawn(&bench_bin, &worker_args)?;
    let lines = render_table(&doc.workloads);
    let exit_code = u8::from(doc.workloads.iter().any(|row| row.status == Status::Error));
    Ok(Outcome { lines, exit_code })
}

fn run_in_process(args: &[String], external: bool) -> Result<Outcome, String> {
    let flags = parse_flags(args)?;
    if flags.schema_probe {
        return Ok(Outcome {
            lines: vec![format!("{{\"schema\":{SCHEMA}}}")],
            exit_code: 0,
        });
    }
    if let Some(want) = flags.schema
        && want != SCHEMA
    {
        return Ok(Outcome {
            lines: vec![format!(
                "{{\"error\":\"schema mismatch\",\"have\":{SCHEMA},\"want\":{want}}}"
            )],
            exit_code: 3,
        });
    }
    let out = flags
        .out
        .ok_or_else(|| format!("missing --out\n{}", crate::perf::USAGE))?;
    let samples = flags.samples.unwrap_or(100);
    let filter = match flags.filter {
        Some(pattern) => Some(
            glob::Pattern::new(&pattern)
                .map_err(|error| format!("invalid --filter glob: {error}"))?,
        ),
        None => None,
    };
    let exact_ids = parse_ids(flags.ids.as_deref())?;
    let classes = parse_classes(flags.classes.as_deref())?;
    let checkout = resolve_checkout(flags.checkout.as_deref())?;

    let ctx = RunCtx {
        samples,
        yach_bin: flags.yach_bin,
        yach_bench_yach_bin: flags.yach_bench_yach_bin,
        yach_bench_bin: flags.yach_bench_bin,
        filter,
    };
    let only = if external {
        Some(EXTERNAL_IDS.as_slice())
    } else {
        None
    };
    let started_at = started_at_now();
    let host = capture_host();
    let build = capture_build(&checkout, ctx.yach_bin.as_deref())?;
    let workloads = measure_restricted(
        &ctx,
        flags.deterministic,
        flags.raw,
        only,
        exact_ids.as_deref(),
        classes.as_deref(),
    );

    let doc = ResultDoc {
        schema: SCHEMA,
        host,
        build,
        started_at,
        workloads,
    };
    let payload =
        serde_json::to_vec_pretty(&doc).map_err(|error| format!("serialize results: {error}"))?;
    std::fs::write(&out, payload).map_err(|error| format!("write {}: {error}", out.display()))?;
    Ok(Outcome {
        lines: Vec::new(),
        exit_code: 0,
    })
}

fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut flags = Flags {
        schema: None,
        schema_probe: false,
        filter: None,
        ids: None,
        classes: None,
        samples: None,
        yach_bin: None,
        yach_bench_yach_bin: None,
        yach_bench_bin: None,
        checkout: None,
        deterministic: false,
        raw: false,
        out: None,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--schema-probe" => flags.schema_probe = true,
            "--deterministic" => flags.deterministic = true,
            "--raw" => flags.raw = true,
            "--schema"
            | "--filter"
            | "--ids"
            | "--classes"
            | "--samples"
            | "--yach-bin"
            | "--yach-bench-yach-bin"
            | "--yach-bench-bin"
            | "--checkout"
            | "--out" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("missing value for {arg}\n{}", crate::perf::USAGE))?;
                match arg.as_str() {
                    "--schema" => {
                        flags.schema = Some(value.parse().map_err(|_| {
                            format!("invalid --schema: {value}\n{}", crate::perf::USAGE)
                        })?);
                    }
                    "--filter" => flags.filter = Some(value.clone()),
                    "--ids" => flags.ids = Some(value.clone()),
                    "--classes" => flags.classes = Some(value.clone()),
                    "--samples" => {
                        flags.samples = Some(value.parse().map_err(|_| {
                            format!("invalid --samples: {value}\n{}", crate::perf::USAGE)
                        })?);
                    }
                    "--yach-bin" => flags.yach_bin = Some(PathBuf::from(value)),
                    "--yach-bench-yach-bin" => {
                        flags.yach_bench_yach_bin = Some(PathBuf::from(value));
                    }
                    "--yach-bench-bin" => flags.yach_bench_bin = Some(PathBuf::from(value)),
                    "--checkout" => flags.checkout = Some(PathBuf::from(value)),
                    "--out" => flags.out = Some(PathBuf::from(value)),
                    _ => {
                        return Err(format!("unknown flag: {arg}\n{}", crate::perf::USAGE));
                    }
                }
            }
            _ => {
                return Err(format!("unknown flag: {arg}\n{}", crate::perf::USAGE));
            }
        }
    }
    Ok(flags)
}

fn parse_ids(raw: Option<&str>) -> Result<Option<Vec<String>>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let ids: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if ids.is_empty() {
        return Err(String::from("empty --ids"));
    }
    Ok(Some(ids))
}

fn parse_classes(raw: Option<&str>) -> Result<Option<Vec<Class>>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut classes = Vec::new();
    for part in raw.split(',') {
        let class = match part.trim() {
            "latency" => Class::Latency,
            "memory" => Class::Memory,
            "size" => Class::Size,
            "count" => Class::Count,
            other => {
                return Err(format!("invalid --classes: {other}"));
            }
        };
        classes.push(class);
    }
    if classes.is_empty() {
        return Err(String::from("empty --classes"));
    }
    Ok(Some(classes))
}

pub(crate) fn resolve_checkout(flag: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = flag {
        return find_repo_root(path);
    }
    let cwd = std::env::current_dir().map_err(|error| format!("cwd: {error}"))?;
    find_repo_root(&cwd)
}

fn find_repo_root(start: &Path) -> Result<PathBuf, String> {
    let mut dir = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cwd: {error}"))?
            .join(start)
    };
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("evals").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(String::from(
                "not inside a yach checkout (need Cargo.toml and evals/)",
            ));
        }
    }
}

fn started_at_now() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let secs = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
            unix_secs_to_rfc3339_utc(secs)
        }
        Err(_) => unix_secs_to_rfc3339_utc(0),
    }
}

fn unix_secs_to_rfc3339_utc(secs: i64) -> String {
    const DAY: i64 = 86_400;
    let days = secs.div_euclid(DAY);
    let tod = secs.rem_euclid(DAY);
    let hour = tod / 3_600;
    let min = (tod % 3_600) / 60;
    let sec = tod % 60;

    // Howard Hinnant civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

pub(crate) fn cargo_output(
    build_cmd: &[String],
    checkout: &Path,
    extra_env: &[(&str, &str)],
    cargo_args: &[&str],
) -> Result<std::process::Output, String> {
    cargo_command(build_cmd, checkout, extra_env, cargo_args)?
        .output()
        .map_err(|error| format!("{}: {error}", build_cmd.join(" ")))
}

pub(crate) fn cargo_status(
    build_cmd: &[String],
    checkout: &Path,
    extra_env: &[(&str, &str)],
    cargo_args: &[&str],
) -> Result<(), String> {
    let mut cmd = cargo_command(build_cmd, checkout, extra_env, cargo_args)?;
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    let status = cmd
        .status()
        .map_err(|error| format!("{}: {error}", build_cmd.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {cargo_args:?} failed: {status}",
            build_cmd.join(" ")
        ))
    }
}

fn cargo_command(
    build_cmd: &[String],
    checkout: &Path,
    extra_env: &[(&str, &str)],
    cargo_args: &[&str],
) -> Result<Command, String> {
    let argv = cargo_invocation(build_cmd, cargo_args);
    let program = argv
        .first()
        .ok_or_else(|| String::from("empty --build-cmd"))?;
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..]).current_dir(checkout);
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    Ok(cmd)
}

fn list_workloads() -> Vec<String> {
    let mut lines = Vec::new();
    for workload in registry::all() {
        let requires = requires_cell(workload.requires);
        lines.push(format!(
            "{} | {} | {} | {requires}",
            workload.id,
            crate::perf::report::class_name(workload.class),
            crate::perf::report::isolation_name(workload.isolation),
        ));
        if workload.isolation == Isolation::InProcessSerial && workload.class == Class::Latency {
            let isolation = crate::perf::report::isolation_name(workload.isolation);
            lines.push(format!(
                "{}#alloc_count | count | {isolation} | derived",
                workload.id
            ));
            lines.push(format!(
                "{}#alloc_bytes | count | {isolation} | derived",
                workload.id
            ));
        }
    }
    lines
}

fn requires_cell(requires: &[Requirement]) -> String {
    requires
        .iter()
        .map(|requirement| match requirement {
            Requirement::Binary => "binary",
            Requirement::Tty => "tty",
            Requirement::Linux => "linux",
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn render_table(rows: &[WorkloadRow]) -> Vec<String> {
    let mut table: Vec<[String; 8]> = Vec::with_capacity(rows.len().saturating_add(1));
    table.push([
        String::from("id"),
        String::from("status"),
        String::from("p50"),
        String::from("p95"),
        String::from("p99"),
        String::from("max"),
        String::from("value"),
        String::from("alloc"),
    ]);
    for row in rows {
        table.push([
            row.id.clone(),
            String::from(status_label(row.status)),
            opt_duration(row.p50_ns),
            opt_duration(row.p95_ns),
            opt_duration(row.p99_ns),
            opt_duration(row.max_ns),
            opt_u64(row.value),
            alloc_cell(rows, row),
        ]);
    }
    let mut widths = [0_usize; 8];
    for line in &table {
        for (index, cell) in line.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.len());
            }
        }
    }
    table
        .iter()
        .map(|line| {
            line.iter()
                .enumerate()
                .map(|(index, cell)| {
                    let width = widths.get(index).copied().unwrap_or(0);
                    format!("{cell:<width$}")
                })
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect()
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Skipped => "skipped",
        Status::Error => "error",
    }
}

fn opt_u64(value: Option<u64>) -> String {
    value.map(|n| n.to_string()).unwrap_or_default()
}

fn opt_duration(value: Option<u64>) -> String {
    value
        .map(crate::perf::report::render_duration)
        .unwrap_or_default()
}

fn alloc_cell(rows: &[WorkloadRow], row: &WorkloadRow) -> String {
    if row.id.contains("#alloc_") {
        return String::new();
    }
    let count_id = format!("{}#alloc_count", row.id);
    let bytes_id = format!("{}#alloc_bytes", row.id);
    let count = rows
        .iter()
        .find(|candidate| candidate.id == count_id)
        .and_then(|candidate| candidate.value);
    let bytes = rows
        .iter()
        .find(|candidate| candidate.id == bytes_id)
        .and_then(|candidate| candidate.value);
    match (count, bytes) {
        (Some(count), Some(bytes)) => format!("{count}/{bytes}"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{cargo_invocation, measure, unix_secs_to_rfc3339_utc};
    use crate::perf::alloc::lock_window_for_test;
    use crate::perf::registry::RunCtx;
    use crate::perf::schema::{Class, Status};

    #[test]
    fn deterministic_measure_emits_only_size_and_count_rows_with_alloc_derivatives() {
        let _guard = lock_window_for_test();
        let ctx = RunCtx {
            samples: 1,
            yach_bin: None,
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            filter: glob::Pattern::new("request/assemble/10_turns").ok(),
        };
        let rows = measure(&ctx, true, false);
        assert!(
            rows.iter()
                .all(|row| matches!(row.class, Class::Size | Class::Count)),
            "expected only size/count rows, got {:?}",
            rows.iter()
                .map(|row| (&row.id, row.class))
                .collect::<Vec<_>>()
        );
        assert!(
            rows.iter().any(|row| {
                row.id == "request/assemble/10_turns#alloc_bytes" && row.status == Status::Ok
            }),
            "missing alloc_bytes row: {:?}",
            rows.iter().map(|row| &row.id).collect::<Vec<_>>()
        );
        assert!(
            !rows.iter().any(|row| row.id == "request/assemble/10_turns"),
            "deterministic run should omit the latency row"
        );
    }

    #[test]
    fn tty_workloads_skip_without_terminal() {
        let ctx = RunCtx {
            samples: 1,
            yach_bin: None,
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            filter: glob::Pattern::new("terminal/idle_*").ok(),
        };
        let rows = measure(&ctx, false, false);
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, ["terminal/idle_keypress_to_draw_flush_live"]);
        assert!(
            rows.iter().all(|row| row.status == Status::Skipped),
            "expected all skipped, got {:?}",
            rows.iter().map(|row| row.status).collect::<Vec<_>>()
        );
        assert!(
            rows.iter()
                .all(|row| row.reason.as_deref() == Some("requires tty")),
            "expected requires tty, got {:?}",
            rows.iter().map(|row| &row.reason).collect::<Vec<_>>()
        );
    }

    #[test]
    fn build_cmd_is_threaded_into_cargo_invocation() {
        assert_eq!(
            cargo_invocation(
                &[
                    String::from("just"),
                    String::from("dev"),
                    String::from("cargo"),
                ],
                &["build", "--release", "--locked", "-p", "yach"],
            ),
            [
                "just",
                "dev",
                "cargo",
                "build",
                "--release",
                "--locked",
                "-p",
                "yach",
            ]
        );
        assert_eq!(
            cargo_invocation(
                &[String::from("cargo")],
                &["metadata", "--format-version", "1", "--no-deps"],
            ),
            ["cargo", "metadata", "--format-version", "1", "--no-deps"]
        );
    }
    #[test]
    fn deterministic_tty_workloads_do_not_emit_alloc_rows() {
        let ctx = RunCtx {
            samples: 1,
            yach_bin: None,
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            filter: glob::Pattern::new("terminal/idle_*").ok(),
        };
        let rows = measure(&ctx, true, false);
        assert!(
            rows.is_empty(),
            "live tty workloads must not emit alloc rows, got {:?}",
            rows.iter().map(|row| &row.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn known_epoch_formats_as_rfc3339_utc() {
        assert_eq!(unix_secs_to_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            unix_secs_to_rfc3339_utc(1_000_000_000),
            "2001-09-09T01:46:40Z"
        );
    }
}
