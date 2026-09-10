use crate::perf::Outcome;
use crate::perf::schema::{AbDoc, Class, Isolation, ResultDoc, Status, VerdictRow, WorkloadRow};
use crate::perf::verdict::Verdict;

#[must_use]
pub fn render_duration(ns: u64) -> String {
    if ns < 1_000 {
        format!("{ns} ns")
    } else if ns < 1_000_000 {
        format_scaled(ns, 1_000, "µs")
    } else if ns < 1_000_000_000 {
        format_scaled(ns, 1_000_000, "ms")
    } else {
        format_scaled(ns, 1_000_000_000, "s")
    }
}

fn format_scaled(ns: u64, unit: u64, suffix: &str) -> String {
    let whole = ns / unit;
    let frac2 = (ns % unit).saturating_mul(100) / unit;
    format!("{whole}.{frac2:02} {suffix}")
}

#[must_use]
pub fn render(doc: &ResultDoc, ab: Option<&AbDoc>) -> String {
    let mut out = String::new();
    push_header(&mut out, doc);
    push_section(&mut out, "Command or harness", "`yach-bench perf report`");
    push_section(
        &mut out,
        "Build/profile mode",
        &format!(
            "{}; rustc {}; source {}",
            doc.build.profile, doc.build.rustc, doc.build.source_sha256
        ),
    );
    push_workload_section(&mut out, doc);
    push_results(&mut out, doc);
    push_comparison(&mut out, ab);
    out.push_str("## Claim supported\n\n");
    out.push_str("## Confidence/limitations\n\n");
    out.push_str("## Follow-up\n");
    out
}

pub(crate) fn cmd_report(args: &[String]) -> Result<Outcome, String> {
    let paths: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| !arg.is_empty())
        .collect();
    let (doc, ab) = match paths.as_slice() {
        [results] => load_one(results)?,
        [results, ab] => {
            let doc = parse_result(results)?;
            let ab = parse_ab(ab)?;
            (doc, Some(ab))
        }
        _ => {
            return Err(String::from(
                "usage: yach-bench perf report <results.json> [<ab.json>]",
            ));
        }
    };
    let md = render(&doc, ab.as_ref());
    Ok(Outcome {
        lines: md.lines().map(str::to_owned).collect(),
        exit_code: 0,
    })
}

fn load_one(path: &str) -> Result<(ResultDoc, Option<AbDoc>), String> {
    let value = load_json(path)?;
    if is_ab(&value) {
        let ab: AbDoc = serde_json::from_value(value)
            .map_err(|error| format!("parse {path} as ab: {error}"))?;
        let doc = result_from_ab(&ab)?;
        Ok((doc, Some(ab)))
    } else {
        let doc: ResultDoc =
            serde_json::from_value(value).map_err(|error| format!("parse {path}: {error}"))?;
        Ok((doc, None))
    }
}

fn result_from_ab(ab: &AbDoc) -> Result<ResultDoc, String> {
    merge_result_docs(&ab.current)
        .or_else(|| merge_result_docs(&ab.base))
        .ok_or_else(|| String::from("ab document has no result documents"))
}

fn merge_result_docs(docs: &[ResultDoc]) -> Option<ResultDoc> {
    let first = docs.first()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut workloads = Vec::new();
    for doc in docs {
        for row in &doc.workloads {
            if seen.insert(row.id.clone()) {
                workloads.push(row.clone());
            }
        }
    }
    Some(ResultDoc {
        workloads,
        ..first.clone()
    })
}

fn parse_result(path: &str) -> Result<ResultDoc, String> {
    let value = load_json(path)?;
    serde_json::from_value(value).map_err(|error| format!("parse {path}: {error}"))
}

fn parse_ab(path: &str) -> Result<AbDoc, String> {
    let value = load_json(path)?;
    serde_json::from_value(value).map_err(|error| format!("parse {path}: {error}"))
}

fn load_json(path: &str) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {path}: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {path}: {error}"))
}

fn is_ab(value: &serde_json::Value) -> bool {
    value.get("base_mode").is_some() || value.get("verdicts").is_some()
}

fn push_header(out: &mut String, doc: &ResultDoc) {
    let date = date_from_started_at(&doc.started_at);
    let commit = doc.build.commit.as_deref().unwrap_or("unknown");
    out.push_str("# Performance Report\n\n");
    push_field(out, "Date", date);
    push_field(out, "Commit", commit);
    push_field(out, "Dirty", if doc.build.dirty { "true" } else { "false" });
    push_field(out, "Source digest", &doc.build.source_sha256);
    push_field(out, "Machine/environment", &doc.host.cpu);
    out.push_str("**Host:** ");
    out.push_str(&doc.host.fingerprint);
    out.push_str(" (");
    out.push_str(&doc.host.os);
    out.push(' ');
    out.push_str(&doc.host.kernel);
    out.push_str(", ");
    out.push_str(&doc.host.cores.to_string());
    out.push_str(" cores)\n");
    push_field(out, "rustc", &doc.build.rustc);
    push_field(out, "Profile", &doc.build.profile);
    out.push('\n');
}

fn push_field(out: &mut String, label: &str, value: &str) {
    out.push_str("**");
    out.push_str(label);
    out.push_str(":** ");
    out.push_str(value);
    out.push('\n');
}

fn push_section(out: &mut String, title: &str, body: &str) {
    out.push_str("## ");
    out.push_str(title);
    out.push_str("\n\n");
    out.push_str(body);
    out.push_str("\n\n");
}

fn date_from_started_at(started_at: &str) -> &str {
    started_at
        .split_once('T')
        .map_or(started_at, |(date, _)| date)
}

fn push_workload_section(out: &mut String, doc: &ResultDoc) {
    out.push_str("## Workload\n\n");
    if doc.workloads.is_empty() {
        out.push_str("(none)\n\n");
        return;
    }
    for row in &doc.workloads {
        out.push_str("- `");
        out.push_str(&row.id);
        out.push_str("` (");
        out.push_str(class_name(row.class));
        out.push_str(", ");
        out.push_str(isolation_name(row.isolation));
        out.push_str(", n=");
        out.push_str(&row.count.to_string());
        out.push_str(")\n");
    }
    out.push('\n');
}

fn push_results(out: &mut String, doc: &ResultDoc) {
    out.push_str("## Results\n\n");
    for class in [Class::Latency, Class::Memory, Class::Size, Class::Count] {
        let rows: Vec<&WorkloadRow> = doc
            .workloads
            .iter()
            .filter(|row| row.class == class)
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str("### ");
        out.push_str(class_name(class));
        out.push_str("\n\n");
        match class {
            Class::Latency => push_latency_table(out, &rows),
            Class::Memory | Class::Size | Class::Count => push_value_table(out, &rows),
        }
        out.push('\n');
    }
}

fn push_latency_table(out: &mut String, rows: &[&WorkloadRow]) {
    out.push_str("| id | status | count | p50 | p95 | p99 | max |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");
    for row in rows {
        out.push_str("| ");
        out.push_str(&row.id);
        out.push_str(" | ");
        out.push_str(status_name(row.status));
        out.push_str(" | ");
        out.push_str(&row.count.to_string());
        out.push_str(" | ");
        out.push_str(&opt_duration(row.p50_ns));
        out.push_str(" | ");
        out.push_str(&opt_duration(row.p95_ns));
        out.push_str(" | ");
        out.push_str(&opt_duration(row.p99_ns));
        out.push_str(" | ");
        out.push_str(&opt_duration(row.max_ns));
        out.push_str(" |\n");
    }
}

fn push_value_table(out: &mut String, rows: &[&WorkloadRow]) {
    out.push_str("| id | status | count | value |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for row in rows {
        out.push_str("| ");
        out.push_str(&row.id);
        out.push_str(" | ");
        out.push_str(status_name(row.status));
        out.push_str(" | ");
        out.push_str(&row.count.to_string());
        out.push_str(" | ");
        out.push_str(&opt_u64(row.value));
        out.push_str(" |\n");
    }
}

fn push_comparison(out: &mut String, ab: Option<&AbDoc>) {
    out.push_str("## Comparison target\n\n");
    let Some(ab) = ab else {
        out.push_str("(none)\n\n");
        return;
    };
    out.push_str("base-mode: ");
    out.push_str(&ab.base_mode);
    out.push_str("\n\n");
    out.push_str("| id | verdict | Δ% | sign agreement | base spread | budget |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for row in &ab.verdicts {
        out.push_str("| ");
        out.push_str(&row.id);
        out.push_str(" | ");
        out.push_str(verdict_name(row.verdict));
        out.push_str(" | ");
        out.push_str(&opt_delta(row));
        out.push_str(" | ");
        out.push_str(&opt_sign(row));
        out.push_str(" | ");
        out.push_str(&opt_spread(row));
        out.push_str(" | ");
        out.push_str(&budget_cell(row));
        out.push_str(" |\n");
    }
    out.push('\n');
}

fn opt_duration(value: Option<u64>) -> String {
    value.map(render_duration).unwrap_or_default()
}

fn opt_u64(value: Option<u64>) -> String {
    value.map(|n| n.to_string()).unwrap_or_default()
}

fn opt_delta(row: &VerdictRow) -> String {
    row.detail
        .map(|detail| format!("{:.2}", detail.median_delta_pct))
        .unwrap_or_default()
}

fn opt_sign(row: &VerdictRow) -> String {
    row.detail
        .map(|detail| format!("{:.2}", detail.sign_agreement))
        .unwrap_or_default()
}

fn opt_spread(row: &VerdictRow) -> String {
    row.detail
        .map(|detail| format!("{:.2}", detail.base_spread_pct))
        .unwrap_or_default()
}

fn budget_cell(row: &VerdictRow) -> String {
    match row.class {
        Class::Latency => format!("{}%", row.budget.latency_pct),
        Class::Memory => format!("{}%", row.budget.memory_pct),
        Class::Size => format!("{}%", row.budget.size_pct),
        Class::Count => row.budget.count.to_string(),
    }
}

pub(crate) fn class_name(class: Class) -> &'static str {
    match class {
        Class::Latency => "latency",
        Class::Memory => "memory",
        Class::Size => "size",
        Class::Count => "count",
    }
}

pub(crate) fn isolation_name(isolation: Isolation) -> &'static str {
    match isolation {
        Isolation::InProcessSerial => "in_process_serial",
        Isolation::InProcessThreaded => "in_process_threaded",
        Isolation::ChildProcess => "child_process",
    }
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Skipped => "skipped",
        Status::Error => "error",
    }
}

fn verdict_name(verdict: Verdict) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::{cmd_report, render};
    use crate::perf::schema::{
        AbDoc, BuildInfo, Class, HostInfo, Isolation, ResultDoc, SCHEMA, WorkloadRow,
    };
    use std::time::Duration;

    #[test]
    fn renders_required_report_sections() {
        let doc = ResultDoc {
            schema: SCHEMA,
            host: HostInfo {
                fingerprint: String::from("abcd"),
                cpu: String::from("cpu"),
                cores: 24,
                os: String::from("Linux"),
                kernel: String::from("6.18"),
            },
            build: BuildInfo {
                source_sha256: String::from("s"),
                commit: Some(String::from("0123456789abcdef")),
                dirty: false,
                profile: String::from("release"),
                rustc: String::from("rustc 1.90"),
                cargo_lock_sha256: String::from("l"),
                yach_bin_sha256: None,
            },
            started_at: String::from("2026-09-08T20:00:00Z"),
            workloads: vec![WorkloadRow::latency(
                "request/assemble/10_turns",
                Isolation::InProcessSerial,
                &[Duration::from_micros(10), Duration::from_micros(20)],
            )],
        };
        let md = render(&doc, None);
        for needle in [
            "# Performance Report",
            "**Date:** 2026-09-08",
            "**Commit:** 0123456789abcdef",
            "**Machine/environment:** cpu",
            "| request/assemble/10_turns |",
            "p95",
        ] {
            assert!(md.contains(needle), "missing {needle}\n{md}");
        }
    }

    fn sample_doc(workloads: Vec<WorkloadRow>) -> ResultDoc {
        ResultDoc {
            schema: SCHEMA,
            host: HostInfo {
                fingerprint: String::from("abcd"),
                cpu: String::from("cpu"),
                cores: 24,
                os: String::from("Linux"),
                kernel: String::from("6.18"),
            },
            build: BuildInfo {
                source_sha256: String::from("s"),
                commit: Some(String::from("0123456789abcdef")),
                dirty: false,
                profile: String::from("release"),
                rustc: String::from("rustc 1.90"),
                cargo_lock_sha256: String::from("l"),
                yach_bin_sha256: None,
            },
            started_at: String::from("2026-09-08T20:00:00Z"),
            workloads,
        }
    }

    #[test]
    fn ab_report_keeps_size_and_alloc_rows_from_earlier_rounds() {
        let round1 = sample_doc(vec![
            WorkloadRow::value(
                "binary/size_bytes",
                Class::Size,
                Isolation::ChildProcess,
                12_345,
            ),
            WorkloadRow::value(
                "request/assemble/10_turns#alloc_bytes",
                Class::Count,
                Isolation::InProcessSerial,
                50,
            ),
            WorkloadRow::latency(
                "request/assemble/10_turns",
                Isolation::InProcessSerial,
                &[Duration::from_micros(10)],
            ),
        ]);
        let round2 = sample_doc(vec![WorkloadRow::latency(
            "request/assemble/10_turns",
            Isolation::InProcessSerial,
            &[Duration::from_micros(20)],
        )]);
        let ab = AbDoc {
            schema: SCHEMA,
            base_mode: String::from("worker"),
            base: vec![round1.clone()],
            current: vec![round1, round2],
            verdicts: Vec::new(),
        };
        let path = std::env::temp_dir().join(format!(
            "yach-ab-report-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let payload = serde_json::to_vec(&ab);
        assert!(payload.is_ok(), "serialize ab: {payload:?}");
        let Ok(payload) = payload else { return };
        let written = std::fs::write(&path, payload);
        assert!(written.is_ok(), "write {}: {written:?}", path.display());
        let outcome = cmd_report(&[path.to_string_lossy().into_owned()]);
        let _ = std::fs::remove_file(&path);
        assert!(
            outcome.is_ok(),
            "cmd_report: {}",
            outcome.as_ref().err().cloned().unwrap_or_default()
        );
        let Ok(outcome) = outcome else { return };
        let md = outcome.lines.join("\n");
        for needle in [
            "| binary/size_bytes |",
            "| request/assemble/10_turns#alloc_bytes |",
        ] {
            assert!(md.contains(needle), "missing {needle}\n{md}");
        }
    }
}
