use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::perf::registry::{self, Isolation};
use crate::perf::schema::{
    AbDoc, Class, ResultDoc, Status, VerdictRow, WorkloadRow, SCHEMA,
};
use crate::perf::thresholds::{Budget, Thresholds};
use crate::perf::verdict::{judge_latency, judge_value, Detail, RoundStat, Verdict};
use crate::perf::worker::{self, WorkerArgs};
use crate::perf::Outcome;

pub const EXTERNAL_IDS: [&str; 5] = worker::EXTERNAL_IDS;

static SLOT_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseMode {
    Auto,
    Worker,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStage {
    Shipping,
    BenchAndWorker,
}

#[derive(Debug, Clone)]
pub struct Side {
    pub checkout: PathBuf,
    pub target_dir: PathBuf,
    pub yach_bin: PathBuf,
    pub yach_bench_yach_bin: PathBuf,
    pub yach_bench_bin: PathBuf,
}

#[derive(Debug)]
pub struct AbOptions {
    pub rounds: usize,
    pub samples: usize,
    pub filter: Option<glob::Pattern>,
    pub deterministic: bool,
    pub base_mode: BaseMode,
    pub thresholds: Thresholds,
    pub out: PathBuf,
}

pub struct AbRunOptions {
    pub options: AbOptions,
    pub base: Option<String>,
    pub base_dir: Option<PathBuf>,
    pub no_build: bool,
    pub build_cmd: Vec<String>,
}

#[derive(Clone, Copy)]
enum ResolvedMode {
    Worker,
    External,
}

#[derive(Clone, Copy)]
enum Which {
    Base,
    Current,
}

struct SlotRun<'a> {
    mode: ResolvedMode,
    opts: &'a AbOptions,
    base: &'a Side,
    current: &'a Side,
    slots: &'a SlotDir,
    seq: Cell<u64>,
}

struct SlotSelect {
    filter: Option<String>,
    ids: Option<String>,
    classes: Option<String>,
}

struct SlotDir {
    path: PathBuf,
}

impl SlotDir {
    fn create() -> Result<Self, String> {
        let n = SLOT_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "yach-ab-{}-{millis}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)
            .map_err(|error| format!("create {}: {error}", path.display()))?;
        Ok(Self { path })
    }

    fn slot_out(&self, seq: u64) -> PathBuf {
        self.path.join(format!("slot-{seq}.json"))
    }
}

impl Drop for SlotDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}



pub(crate) fn cmd_ab(args: &[String]) -> Result<Outcome, String> {
    let opts = parse_ab_args(args)?;
    let (doc, exit_code) = run(&opts)?;
    let mut lines = vec![format!("base-mode: {}", doc.base_mode)];
    lines.extend(render_verdicts(&doc.verdicts));
    Ok(Outcome { lines, exit_code })
}

pub fn materialize_base(rev: &str) -> Result<PathBuf, String> {
    let root = worker::resolve_checkout(None)?;
    materialize_base_in(&root, rev)
}

fn materialize_base_in(root: &Path, rev: &str) -> Result<PathBuf, String> {
    let version = Command::new("jj")
        .arg("--version")
        .output()
        .map_err(|error| format!("jj missing: {error}"))?;
    if !version.status.success() {
        return Err(String::from("jj missing"));
    }
    let dest = root.join(".perf/base");
    std::fs::create_dir_all(root.join(".perf"))
        .map_err(|error| format!("create .perf: {error}"))?;
    if dest.exists() {
        run_jj(root, &["-R", ".perf/base", "workspace", "update-stale"])?;
        run_jj(root, &["-R", ".perf/base", "new", rev])?;
    } else {
        run_jj(
            root,
            &[
                "workspace",
                "add",
                "--name",
                "perf-base",
                ".perf/base",
                "-r",
                rev,
            ],
        )?;
    }
    Ok(dest)
}

fn run_jj(root: &Path, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("jj");
    cmd.args(args).current_dir(root);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    let status = cmd.status().map_err(|error| format!("jj: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("jj {args:?} failed: {status}"))
    }
}

pub fn build_side(
    checkout: &Path,
    target: &Path,
    stage: BuildStage,
    build_cmd: &[String],
) -> Result<Side, String> {
    let release_root = target.join("release-root");
    let release_root_s = release_root.to_string_lossy();
    worker::cargo_status(
        build_cmd,
        checkout,
        &[("CARGO_TARGET_DIR", release_root_s.as_ref())],
        &["build", "--release", "--locked", "-p", "yach"],
    )?;
    worker::cargo_status(
        build_cmd,
        checkout,
        &[("CARGO_TARGET_DIR", release_root_s.as_ref())],
        &["build", "--release", "--locked", "-p", "yach-bench"],
    )?;

    let yach_bin = release_root.join("release/yach");
    let yach_bench_bin = release_root.join("release/yach-bench");
    let bench_dir = target.join("bench");
    let yach_bench_yach_bin = bench_dir.join("release/yach");

    if matches!(stage, BuildStage::BenchAndWorker) {
        let bench_dir_s = bench_dir.to_string_lossy();
        worker::cargo_status(
            build_cmd,
            checkout,
            &[("CARGO_TARGET_DIR", bench_dir_s.as_ref())],
            &[
                "build",
                "--release",
                "--locked",
                "-p",
                "yach",
                "--features",
                "bench",
            ],
        )?;
        if !yach_bench_yach_bin.is_file() {
            return Err(format!("missing artifact {}", yach_bench_yach_bin.display()));
        }
    }

    for path in [&yach_bin, &yach_bench_bin] {
        if !path.is_file() {
            return Err(format!("missing artifact {}", path.display()));
        }
    }

    Ok(Side {
        checkout: checkout.to_path_buf(),
        target_dir: target.to_path_buf(),
        yach_bin,
        yach_bench_yach_bin,
        yach_bench_bin,
    })
}

fn side_from_paths(checkout: &Path, target: &Path) -> Result<Side, String> {
    let yach_bin = target.join("release-root/release/yach");
    let yach_bench_bin = target.join("release-root/release/yach-bench");
    let yach_bench_yach_bin = target.join("bench/release/yach");
    for path in [&yach_bin, &yach_bench_bin] {
        if !path.is_file() {
            return Err(format!("missing artifact {}", path.display()));
        }
    }
    Ok(Side {
        checkout: checkout.to_path_buf(),
        target_dir: target.to_path_buf(),
        yach_bin,
        yach_bench_yach_bin,
        yach_bench_bin,
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}


fn existing_absolute(path: &Path) -> Result<PathBuf, String> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cwd: {error}"))?
            .join(path)
    };
    joined.canonicalize().map_err(|error| {
        format!("canonicalize {}: {error}", joined.display())
    })
}

pub fn probe_worker(bench_bin: &Path) -> Option<u32> {
    let output = Command::new(bench_bin)
        .args(["perf", "worker", "--schema-probe"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let schema = value.get("schema")?.as_u64()?;
    u32::try_from(schema).ok()
}

fn is_cargo_executable(arg: &str) -> bool {
    Path::new(arg)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem == "cargo")
}

fn compiler_probe_argv(build_cmd: &[String]) -> Result<Vec<String>, String> {
    let last = build_cmd
        .last()
        .ok_or_else(|| String::from("empty --build-cmd"))?;
    let mut argv = if is_cargo_executable(last) {
        build_cmd[..build_cmd.len() - 1].to_vec()
    } else {
        build_cmd.to_vec()
    };
    argv.push(String::from("rustc"));
    argv.push(String::from("--version"));
    Ok(argv)
}

fn run_probe(checkout: &Path, argv: &[String]) -> Result<String, String> {
    let program = argv
        .first()
        .ok_or_else(|| String::from("empty compiler probe"))?;
    let output = Command::new(program)
        .args(&argv[1..])
        .current_dir(checkout)
        .output()
        .map_err(|error| format!("{}: {error}", argv.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "{} failed: {}",
            argv.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("{} stdout: {error}", argv.join(" ")))?;
    Ok(stdout.trim().to_owned())
}

fn rustc_version(checkout: &Path, build_cmd: &[String]) -> Result<String, String> {
    let argv = compiler_probe_argv(build_cmd)?;
    run_probe(checkout, &argv)
}

pub fn run(opts: &AbRunOptions) -> Result<(AbDoc, u8), String> {
    let current_checkout = existing_absolute(&worker::resolve_checkout(None)?)?;
    let resolved_target = worker::cargo_target_dir(&current_checkout, &opts.build_cmd)?;
    let current_target = match existing_absolute(&resolved_target) {
        Ok(path) => path,
        Err(_) => resolved_target,
    };

    let base_checkout = if let Some(dir) = &opts.base_dir {
        existing_absolute(dir)?
    } else {
        let Some(rev) = &opts.base else {
            return Err(String::from("missing --base or --base-dir"));
        };
        existing_absolute(&materialize_base_in(&current_checkout, rev)?)?
    };
    let base_target = if same_path(&base_checkout, &current_checkout) {
        current_target.clone()
    } else {
        base_checkout.join("target")
    };

    let mut base_side = if opts.no_build {
        side_from_paths(&base_checkout, &base_target)?
    } else {
        build_side(
            &base_checkout,
            &base_target,
            BuildStage::Shipping,
            &opts.build_cmd,
        )?
    };
    if !opts.no_build && probe_worker(&base_side.yach_bench_bin) == Some(SCHEMA) {
        base_side = build_side(
            &base_checkout,
            &base_target,
            BuildStage::BenchAndWorker,
            &opts.build_cmd,
        )?;
    }

    let current_side = if opts.no_build {
        side_from_paths(&current_checkout, &current_target)?
    } else {
        build_side(
            &current_checkout,
            &current_target,
            BuildStage::BenchAndWorker,
            &opts.build_cmd,
        )?
    };

    let base_rustc = rustc_version(&base_checkout, &opts.build_cmd)?;
    let current_rustc = rustc_version(&current_checkout, &opts.build_cmd)?;
    if base_rustc != current_rustc {
        return Err(format!(
            "rustc mismatch: base {base_rustc}, current {current_rustc}"
        ));
    }

    run_with_sides(&opts.options, &base_side, &current_side)
}

pub fn run_with_sides(
    opts: &AbOptions,
    base: &Side,
    current: &Side,
) -> Result<(AbDoc, u8), String> {
    let base_schema = probe_worker(&base.yach_bench_bin);
    let current_schema = probe_worker(&current.yach_bench_bin);
    if let Some(schema) = current_schema
        && schema != SCHEMA
    {
        return Err(format!(
            "schema mismatch: controller {SCHEMA}, worker {schema}"
        ));
    }

    let mode = match opts.base_mode {
        BaseMode::External => ResolvedMode::External,
        BaseMode::Worker | BaseMode::Auto => match base_schema {
            Some(SCHEMA) => ResolvedMode::Worker,
            Some(schema) => {
                return Err(format!(
                    "schema mismatch: controller {SCHEMA}, worker {schema}"
                ));
            }
            None => match opts.base_mode {
                BaseMode::Worker => {
                    return Err(String::from("base has no perf worker"));
                }
                BaseMode::Auto | BaseMode::External => ResolvedMode::External,
            },
        },
    };

    let mut base_docs = Vec::new();
    let mut current_docs = Vec::new();
    let filter = opts.filter.as_ref().map(ToString::to_string);
    let slots = SlotDir::create()?;
    let run = SlotRun {
        mode,
        opts,
        base,
        current,
        slots: &slots,
        seq: Cell::new(0),
    };
    let planned = opts.rounds.max(1);
    for round in 0..planned {
        let select = if round == 0 {
            SlotSelect {
                filter: filter.clone(),
                ids: None,
                classes: None,
            }
        } else {
            SlotSelect {
                filter: filter.clone(),
                ids: None,
                classes: Some(String::from("latency,memory")),
            }
        };
        run_abba(&run, &select, &mut base_docs, &mut current_docs)?;
    }

    check_unmatched(opts, &base_docs, &current_docs)?;

    let mut extra = 0;
    loop {
        let verdicts = judge(opts, mode, &base_docs, &current_docs);
        let inconclusive: Vec<String> = verdicts
            .iter()
            .filter(|row| {
                row.verdict == Verdict::Inconclusive
                    && matches!(row.class, Class::Latency | Class::Memory)
            })
            .map(|row| row.id.clone())
            .collect();
        if inconclusive.is_empty() || extra >= 2 {
            let base_mode = match mode {
                ResolvedMode::Worker => "worker",
                ResolvedMode::External => "external",
            };
            let doc = AbDoc {
                schema: SCHEMA,
                base_mode: String::from(base_mode),
                base: base_docs,
                current: current_docs,
                verdicts,
            };
            let payload = serde_json::to_vec_pretty(&doc)
                .map_err(|error| format!("serialize ab document: {error}"))?;
            std::fs::write(&opts.out, payload)
                .map_err(|error| format!("write {}: {error}", opts.out.display()))?;
            let exit_code = exit_code(&doc.verdicts);
            return Ok((doc, exit_code));
        }
        extra += 1;
        let select = SlotSelect {
            filter: None,
            ids: Some(inconclusive.join(",")),
            classes: Some(String::from("latency,memory")),
        };

        run_abba(&run, &select, &mut base_docs, &mut current_docs)?;
    }
}


fn run_abba(
    run: &SlotRun<'_>,
    select: &SlotSelect,
    base_docs: &mut Vec<ResultDoc>,
    current_docs: &mut Vec<ResultDoc>,
) -> Result<(), String> {
    let order = [
        Which::Base,
        Which::Current,
        Which::Current,
        Which::Base,
    ];
    for which in order {
        let doc = measure_slot(run, which, select)?;
        match which {
            Which::Base => base_docs.push(doc),
            Which::Current => current_docs.push(doc),
        }
    }
    Ok(())
}

fn measure_slot(
    run: &SlotRun<'_>,
    which: Which,
    select: &SlotSelect,
) -> Result<ResultDoc, String> {
    let n = run.seq.get();
    run.seq.set(n + 1);
    let out = run.slots.slot_out(n);
    match (run.mode, which) {
        (ResolvedMode::External, Which::Base) => {
            let mut args = WorkerArgs::external_sampler(
                select.filter.clone(),
                run.opts.samples,
                run.base.yach_bin.clone(),
                run.base.checkout.clone(),
                run.opts.deterministic,
                out,
            );
            args.ids.clone_from(&select.ids);
            args.classes.clone_from(&select.classes);

            worker::spawn(&run.current.yach_bench_bin, &args)
        }
        (_, Which::Base) => worker::spawn(
            &run.base.yach_bench_bin,
            &worker_args(run.base, run.opts, select, out),
        ),
        (_, Which::Current) => worker::spawn(
            &run.current.yach_bench_bin,
            &worker_args(run.current, run.opts, select, out),
        ),
    }
}

fn worker_args(
    side: &Side,
    opts: &AbOptions,
    select: &SlotSelect,
    out: PathBuf,
) -> WorkerArgs {
    WorkerArgs {
        schema: SCHEMA,
        filter: select.filter.clone(),
        ids: select.ids.clone(),
        classes: select.classes.clone(),
        samples: opts.samples,
        yach_bin: Some(side.yach_bin.clone()),
        yach_bench_yach_bin: side
            .yach_bench_yach_bin
            .is_file()
            .then(|| side.yach_bench_yach_bin.clone()),
        yach_bench_bin: Some(side.yach_bench_bin.clone()),
        checkout: Some(side.checkout.clone()),
        deterministic: opts.deterministic,
        raw: true,
        out,
        external: false,
    }
}


fn check_unmatched(
    opts: &AbOptions,
    base_docs: &[ResultDoc],
    current_docs: &[ResultDoc],
) -> Result<(), String> {
    let mut ids: Vec<String> = registry::all()
        .iter()
        .map(|workload| workload.id.to_owned())
        .collect();
    for workload in registry::all() {
        if workload.isolation == Isolation::InProcessSerial && workload.class == Class::Latency {
            ids.push(format!("{}#alloc_count", workload.id));
            ids.push(format!("{}#alloc_bytes", workload.id));
        }
    }
    for doc in base_docs.iter().chain(current_docs) {
        for row in &doc.workloads {
            if !ids.iter().any(|id| id == &row.id) {
                ids.push(row.id.clone());
            }
        }
    }
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let unmatched = opts.thresholds.unmatched_rows(&refs);
    if unmatched.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "unmatched threshold rows: {}",
            unmatched.join(", ")
        ))
    }
}

fn judge(
    opts: &AbOptions,
    mode: ResolvedMode,
    base_docs: &[ResultDoc],
    current_docs: &[ResultDoc],
) -> Vec<VerdictRow> {
    let base_rounds = round_maps(base_docs);
    let current_rounds = round_maps(current_docs);
    let ids = ordered_ids(current_docs, base_docs);
    ids.into_iter()
        .map(|id| judge_id(opts, mode, &id, &base_rounds, &current_rounds))
        .collect()
}

fn round_maps(docs: &[ResultDoc]) -> Vec<BTreeMap<String, WorkloadRow>> {
    docs.chunks(2).map(merge_docs).collect()
}

fn merge_docs(docs: &[ResultDoc]) -> BTreeMap<String, WorkloadRow> {
    let mut grouped: BTreeMap<String, Vec<&WorkloadRow>> = BTreeMap::new();
    for doc in docs {
        for row in &doc.workloads {
            grouped.entry(row.id.clone()).or_default().push(row);
        }
    }
    grouped
        .into_iter()
        .map(|(id, rows)| (id, merge_rows(&rows)))
        .collect()
}

fn merge_rows(rows: &[&WorkloadRow]) -> WorkloadRow {
    if let Some(row) = rows.iter().copied().find(|row| row.status == Status::Error) {
        return row.clone();
    }
    if let Some(row) = rows
        .iter()
        .copied()
        .find(|row| row.status == Status::Skipped)
    {
        return row.clone();
    }
    let ok: Vec<&WorkloadRow> = rows
        .iter()
        .copied()
        .filter(|row| row.status == Status::Ok)
        .collect();
    let Some(first) = ok.first().copied() else {
        return rows[0].clone();
    };
    match first.class {
        Class::Latency => merge_latency(&ok),
        Class::Memory => merge_memory(&ok),
        Class::Size | Class::Count => first.clone(),
    }
}

fn merge_latency(rows: &[&WorkloadRow]) -> WorkloadRow {
    let mut samples = Vec::new();
    let mut have_all = true;
    for row in rows {
        match &row.samples_ns {
            Some(values) => samples.extend_from_slice(values),
            None => have_all = false,
        }
    }
    if have_all && !samples.is_empty() {
        let durations: Vec<Duration> = samples
            .iter()
            .copied()
            .map(Duration::from_nanos)
            .collect();
        let mut merged = WorkloadRow::latency(&rows[0].id, rows[0].isolation, &durations);
        merged.samples_ns = Some(samples);
        merged
    } else {
        rows[0].clone()
    }
}

fn merge_memory(rows: &[&WorkloadRow]) -> WorkloadRow {
    let mut samples = Vec::new();
    let mut have_all = true;
    let mut peak = 0_u64;
    for row in rows {
        if let Some(value) = row.value {
            peak = peak.max(value);
        }
        match &row.samples_bytes {
            Some(values) => samples.extend_from_slice(values),
            None => have_all = false,
        }
    }
    if have_all && !samples.is_empty() {
        let mut merged = WorkloadRow::memory(&rows[0].id, &samples);
        merged.samples_bytes = Some(samples);
        merged
    } else {
        let mut row = rows[0].clone();
        row.value = Some(peak);
        row
    }
}

fn ordered_ids(current: &[ResultDoc], base: &[ResultDoc]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    for doc in current.iter().chain(base) {
        for row in &doc.workloads {
            if seen.insert(row.id.clone()) {
                ids.push(row.id.clone());
            }
        }
    }
    ids
}

fn judge_id(
    opts: &AbOptions,
    mode: ResolvedMode,
    id: &str,
    base_rounds: &[BTreeMap<String, WorkloadRow>],
    current_rounds: &[BTreeMap<String, WorkloadRow>],
) -> VerdictRow {
    let class = class_of(id, base_rounds, current_rounds).unwrap_or(Class::Latency);
    let budget = opts.thresholds.for_id(id);
    let external_id = EXTERNAL_IDS.contains(&id);
    if matches!(mode, ResolvedMode::External) && !external_id {
        return VerdictRow {
            id: id.to_owned(),
            class,
            verdict: Verdict::NoBaseWorker,
            detail: None,
            base_summary: None,
            current_summary: None,
            budget,
        };
    }

    let used_rounds = if matches!(class, Class::Size | Class::Count) {
        1_usize
    } else {
        base_rounds.len().max(current_rounds.len())
    };

    let mut base_ok = Vec::new();
    let mut current_ok = Vec::new();
    let mut any_error = false;
    let mut any_skipped = false;
    for index in 0..used_rounds {
        let base_row = base_rounds.get(index).and_then(|round| round.get(id));
        let current_row = current_rounds.get(index).and_then(|round| round.get(id));
        for row in [base_row, current_row].into_iter().flatten() {
            match row.status {
                Status::Error => any_error = true,
                Status::Skipped => any_skipped = true,
                Status::Ok => {}
            }
        }
        if let Some(row) = base_row.filter(|row| row.status == Status::Ok)
            && let Some(stat) = primary_stat(row)
        {
            base_ok.push(stat);
        }
        if let Some(row) = current_row.filter(|row| row.status == Status::Ok)
            && let Some(stat) = primary_stat(row)
        {
            current_ok.push(stat);
        }

    }

    let base_summary = median_f64(&base_ok);
    let current_summary = median_f64(&current_ok);

    if any_error {
        return row_with(id, class, Verdict::Error, None, base_summary, current_summary, budget);
    }
    if any_skipped {
        return row_with(
            id,
            class,
            Verdict::Skipped,
            None,
            base_summary,
            current_summary,
            budget,
        );
    }

    let has_base = !base_ok.is_empty();
    let has_current = !current_ok.is_empty();
    if has_current && !has_base {
        return row_with(id, class, Verdict::Added, None, None, current_summary, budget);
    }
    if has_base && !has_current {
        return row_with(id, class, Verdict::Removed, None, base_summary, None, budget);
    }
    if !has_base && !has_current {
        return row_with(id, class, Verdict::Error, None, None, None, budget);
    }

    let (verdict, detail) = match class {
        Class::Latency | Class::Memory => {
            let n = base_ok.len().min(current_ok.len());
            let rounds: Vec<RoundStat> = (0..n)
                .map(|index| RoundStat {
                    base: base_ok[index],
                    current: current_ok[index],
                })
                .collect();
            let threshold = if class == Class::Memory {
                budget.memory_pct
            } else {
                budget.latency_pct
            };
            judge_latency(&rounds, threshold)
        }
        Class::Size => {
            let base = first_u64(base_rounds, id).unwrap_or(0);
            let current = first_u64(current_rounds, id).unwrap_or(0);
            judge_value(base, current, Some(budget.size_pct), None)
        }
        Class::Count => {
            let base = first_u64(base_rounds, id).unwrap_or(0);
            let current = first_u64(current_rounds, id).unwrap_or(0);
            judge_value(base, current, None, Some(budget.count))
        }

    };
    row_with(
        id,
        class,
        verdict,
        Some(detail),
        base_summary,
        current_summary,
        budget,
    )
}

fn class_of(
    id: &str,
    base_rounds: &[BTreeMap<String, WorkloadRow>],
    current_rounds: &[BTreeMap<String, WorkloadRow>],
) -> Option<Class> {
    base_rounds
        .iter()
        .chain(current_rounds)
        .find_map(|round| round.get(id).map(|row| row.class))
}

fn first_u64(rounds: &[BTreeMap<String, WorkloadRow>], id: &str) -> Option<u64> {
    rounds.first().and_then(|round| round.get(id)).and_then(|row| row.value)
}


fn primary_stat(row: &WorkloadRow) -> Option<f64> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "round statistics are compared as percentages"
    )]
    let from_u64 = |value: u64| value as f64;
    match row.class {
        Class::Latency => row.p95_ns.map(from_u64),
        Class::Memory | Class::Size | Class::Count => row.value.map(from_u64),
    }
}

fn median_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(sorted[sorted.len() / 2])
}

fn row_with(
    id: &str,
    class: Class,
    verdict: Verdict,
    detail: Option<Detail>,
    base_summary: Option<f64>,
    current_summary: Option<f64>,
    budget: Budget,
) -> VerdictRow {
    VerdictRow {
        id: id.to_owned(),
        class,
        verdict,
        detail,
        base_summary,
        current_summary,
        budget,
    }
}

fn exit_code(verdicts: &[VerdictRow]) -> u8 {
    if verdicts
        .iter()
        .any(|row| matches!(row.verdict, Verdict::Error | Verdict::Regressed))
    {
        1
    } else if verdicts
        .iter()
        .any(|row| row.verdict == Verdict::Inconclusive)
    {
        2
    } else {
        0
    }
}

fn render_verdicts(rows: &[VerdictRow]) -> Vec<String> {
    let mut table: Vec<[String; 7]> = Vec::with_capacity(rows.len().saturating_add(1));
    table.push([
        String::from("id"),
        String::from("class"),
        String::from("verdict"),
        String::from("base"),
        String::from("current"),
        String::from("delta%"),
        String::from("budget"),
    ]);
    for row in rows {
        table.push([
            row.id.clone(),
            class_label(row.class).to_owned(),
            verdict_label(row.verdict).to_owned(),
            fmt_summary(row.class, row.base_summary),
            fmt_summary(row.class, row.current_summary),
            row.detail
                .map(|detail| format!("{:.2}", detail.median_delta_pct))
                .unwrap_or_default(),
            budget_cell(row),
        ]);
    }
    let mut widths = [0_usize; 7];
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

fn class_label(class: Class) -> &'static str {
    match class {
        Class::Latency => "latency",
        Class::Memory => "memory",
        Class::Size => "size",
        Class::Count => "count",
    }
}

fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Regressed => "regressed",
        Verdict::Improved => "improved",
        Verdict::Unchanged => "unchanged",
        Verdict::Inconclusive => "inconclusive",
        Verdict::Skipped => "skipped",
        Verdict::Error => "error",
        Verdict::Added => "added",
        Verdict::Removed => "removed",
        Verdict::NoBaseWorker => "no_base_worker",
    }
}

fn fmt_opt_f64(value: Option<f64>) -> String {
    value.map(|n| format!("{n:.0}")).unwrap_or_default()
}

fn fmt_summary(class: Class, value: Option<f64>) -> String {
    match (class, value) {
        (Class::Latency, Some(ns)) if ns.is_finite() && ns >= 0.0 => {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "latency summaries are non-negative nanoseconds"
            )]
            let rounded = ns.round() as u64;
            crate::perf::report::render_duration(rounded)
        }
        (_, Some(_)) => fmt_opt_f64(value),
        (_, None) => String::new(),
    }
}

fn budget_cell(row: &VerdictRow) -> String {
    match row.class {
        Class::Latency => format!("{}%", row.budget.latency_pct),
        Class::Memory => format!("{}%", row.budget.memory_pct),
        Class::Size => format!("{}%", row.budget.size_pct),
        Class::Count => row.budget.count.to_string(),
    }
}

fn parse_ab_args(args: &[String]) -> Result<AbRunOptions, String> {
    let mut base = None;
    let mut base_dir = None;
    let mut base_mode = BaseMode::Auto;
    let mut filter = None;
    let mut rounds = 5_usize;
    let mut samples = 100_usize;
    let mut deterministic = false;
    let mut thresholds = None;
    let mut out = None;
    let mut no_build = false;
    let mut build_cmd = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--deterministic" => deterministic = true,
            "--no-build" => no_build = true,
            "--base" | "--base-dir" | "--base-mode" | "--filter" | "--rounds" | "--samples"
            | "--thresholds" | "--out" | "--build-cmd" => {
                let value = iter.next().ok_or_else(|| {
                    format!("missing value for {arg}\n{}", crate::perf::USAGE)
                })?;
                match arg.as_str() {
                    "--base" => base = Some(value.clone()),
                    "--base-dir" => base_dir = Some(PathBuf::from(value)),
                    "--base-mode" => {
                        base_mode = parse_base_mode(value)?;
                    }
                    "--filter" => filter = Some(value.clone()),
                    "--rounds" => {
                        rounds = parse_count("--rounds", value)?;
                    }
                    "--samples" => {
                        samples = parse_count("--samples", value)?;
                    }
                    "--thresholds" => thresholds = Some(PathBuf::from(value)),
                    "--out" => out = Some(PathBuf::from(value)),
                    "--build-cmd" => build_cmd = Some(parse_build_cmd(value)?),
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
    if base.is_some() && base_dir.is_some() {
        return Err(String::from("use either --base or --base-dir, not both"));
    }
    if base.is_none() && base_dir.is_none() {
        return Err(String::from("missing --base or --base-dir"));
    }
    let out = out.ok_or_else(|| format!("missing --out\n{}", crate::perf::USAGE))?;
    let filter = match filter {
        Some(pattern) => Some(
            glob::Pattern::new(&pattern)
                .map_err(|error| format!("invalid --filter glob: {error}"))?,
        ),
        None => None,
    };
    let checkout = worker::resolve_checkout(None)?;
    let thresholds = match thresholds {
        Some(path) => Thresholds::load(&path)?,
        None => Thresholds::load(&checkout.join("crates/yach-bench/perf-thresholds.toml"))?,
    };
    Ok(AbRunOptions {
        options: AbOptions {
            rounds,
            samples,
            filter,
            deterministic,
            base_mode,
            thresholds,
            out,
        },
        base,
        base_dir,
        no_build,
        build_cmd: build_cmd.unwrap_or_else(worker::default_build_cmd),
    })
}

fn parse_build_cmd(raw: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = raw.split_whitespace().map(str::to_owned).collect();
    if parts.is_empty() {
        Err(String::from("empty --build-cmd"))
    } else {
        Ok(parts)
    }
}

fn parse_base_mode(value: &str) -> Result<BaseMode, String> {
    match value {
        "auto" => Ok(BaseMode::Auto),
        "worker" => Ok(BaseMode::Worker),
        "external" => Ok(BaseMode::External),
        _ => Err(format!(
            "invalid --base-mode: {value} (auto|worker|external)"
        )),
    }
}

fn parse_count(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("invalid {flag}: {value}"))
}

#[cfg(test)]
mod tests {
    use super::{AbOptions, BaseMode, Side, compiler_probe_argv, run_with_sides};
    use crate::perf::schema::SCHEMA;
    use crate::perf::verdict::Verdict;
    use std::path::PathBuf;


    fn stub_worker(
        dir: &std::path::Path,
        name: &str,
        schema: u32,
        p95_ns: u64,
        log: &std::path::Path,
    ) -> PathBuf {
        let path = dir.join(name);
        let body = format!(
            "#!/bin/sh\necho \"{name}\" >> {log}\nif [ \"$1\" = \"perf\" ] && [ \"$2\" = \"worker\" ] && [ \"$3\" = \"--schema-probe\" ]; then echo '{{\"schema\":{schema}}}'; exit 0; fi\n\
             out=; while [ $# -gt 0 ]; do if [ \"$1\" = \"--out\" ]; then out=$2; fi; shift; done\n\
             cat > \"$out\" <<EOF\n{{\"schema\":{schema},\"host\":{{\"fingerprint\":\"f\",\"cpu\":\"c\",\"cores\":1,\"os\":\"o\",\"kernel\":\"k\"}},\"build\":{{\"source_sha256\":\"s\",\"commit\":null,\"dirty\":true,\"profile\":\"release\",\"rustc\":\"r\",\"cargo_lock_sha256\":\"l\",\"yach_bin_sha256\":null}},\"started_at\":\"t\",\"workloads\":[{{\"id\":\"request/assemble/10_turns\",\"class\":\"latency\",\"isolation\":\"in_process_serial\",\"status\":\"ok\",\"reason\":null,\"count\":10,\"p50_ns\":{p95_ns},\"p95_ns\":{p95_ns},\"p99_ns\":{p95_ns},\"max_ns\":{p95_ns},\"value\":null,\"samples_ns\":null,\"samples_bytes\":null}}]}}\nEOF\n",
            log = log.display()
        );

        let _ = std::fs::write(&path, body);
        let _ = std::process::Command::new("chmod")
            .arg("+x")
            .arg(&path)
            .status();
        path
    }

    fn side(dir: &std::path::Path, bench: PathBuf) -> Side {
        Side {
            checkout: dir.to_path_buf(),
            target_dir: dir.join("target"),
            yach_bin: dir.join("yach"),
            yach_bench_yach_bin: dir.join("yach-bench-yach"),
            yach_bench_bin: bench,
        }
    }

    #[test]
    fn abba_order_and_regression_verdict() {
        let dir = std::env::temp_dir().join(format!("yach-ab-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("order.log");
        let base = stub_worker(&dir, "base-worker", SCHEMA, 100_000, &log);
        let current = stub_worker(&dir, "current-worker", SCHEMA, 120_000, &log);

        let opts = AbOptions {
            rounds: 2,
            samples: 5,
            filter: None,
            deterministic: false,
            base_mode: BaseMode::Auto,
            thresholds: crate::perf::thresholds::Thresholds::defaults(),
            out: dir.join("ab.json"),
        };
        let result = run_with_sides(&opts, &side(&dir, base), &side(&dir, current));
        let order = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_ok(), "{result:?}");
        let Ok((doc, code)) = result else { return };
        let calls: Vec<&str> = order.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(
            &calls[2..],
            &[
                "base-worker",
                "current-worker",
                "current-worker",
                "base-worker",
                "base-worker",
                "current-worker",
                "current-worker",
                "base-worker"
            ]
        );
        assert_eq!(doc.verdicts[0].verdict, Verdict::Regressed);
        assert_eq!(code, 1);
    }

    #[test]
    fn schema_mismatch_is_hard_error() {
        let dir = std::env::temp_dir().join(format!("yach-ab-schema-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("order.log");
        let base = stub_worker(&dir, "base-worker", 7, 1, &log);
        let current = stub_worker(&dir, "current-worker", SCHEMA, 1, &log);

        let opts = AbOptions {
            rounds: 1,
            samples: 1,
            filter: None,
            deterministic: false,
            base_mode: BaseMode::Worker,
            thresholds: crate::perf::thresholds::Thresholds::defaults(),
            out: dir.join("ab.json"),
        };
        let result = run_with_sides(&opts, &side(&dir, base), &side(&dir, current));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_err_and(|error| error.contains("schema")));
    }

    #[test]
    fn missing_worker_falls_back_to_external_with_no_base_worker_rows() {
        let dir = std::env::temp_dir().join(format!("yach-ab-ext-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("order.log");
        let base = dir.join("old-bench");
        let _ = std::fs::write(&base, "#!/bin/bash\nexit 2\n");
        let _ = std::process::Command::new("chmod")
            .arg("+x")
            .arg(&base)
            .status();
        let current = stub_worker(&dir, "current-worker", SCHEMA, 1, &log);

        let opts = AbOptions {
            rounds: 1,
            samples: 1,
            filter: glob::Pattern::new("request/*").ok(),
            deterministic: false,
            base_mode: BaseMode::Auto,
            thresholds: crate::perf::thresholds::Thresholds::defaults(),
            out: dir.join("ab.json"),
        };
        let result = run_with_sides(&opts, &side(&dir, base), &side(&dir, current));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_ok(), "{result:?}");
        let Ok((doc, code)) = result else { return };
        assert_eq!(doc.base_mode, "external");
        assert_eq!(doc.verdicts[0].verdict, Verdict::NoBaseWorker);
        assert_eq!(code, 0);
    }

    #[test]
    fn compiler_probe_argv_covers_just_dev_bare_and_absolute_cargo() {
        let just = compiler_probe_argv(&[
            String::from("just"),
            String::from("dev"),
            String::from("cargo"),
        ]);
        assert!(just.is_ok(), "{just:?}");
        let Ok(just) = just else { return };
        assert_eq!(
            just,
            vec![
                String::from("just"),
                String::from("dev"),
                String::from("rustc"),
                String::from("--version"),
            ]
        );

        let bare = compiler_probe_argv(&[String::from("cargo")]);
        assert!(bare.is_ok(), "{bare:?}");
        let Ok(bare) = bare else { return };
        assert_eq!(
            bare,
            vec![String::from("rustc"), String::from("--version")]
        );

        let abs = compiler_probe_argv(&[String::from("/usr/bin/cargo")]);
        assert!(abs.is_ok(), "{abs:?}");
        let Ok(abs) = abs else { return };
        assert_eq!(
            abs,
            vec![String::from("rustc"), String::from("--version")]
        );
    }
}
