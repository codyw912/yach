use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::latency::LatencySummary;
use crate::perf::thresholds::Budget;
use crate::perf::verdict::{Detail, Verdict};

pub const SCHEMA: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Latency,
    Memory,
    Size,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    InProcessSerial,
    InProcessThreaded,
    ChildProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Skipped,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    pub fingerprint: String,
    pub cpu: String,
    pub cores: usize,
    pub os: String,
    pub kernel: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildInfo {
    pub source_sha256: String,
    pub commit: Option<String>,
    pub dirty: bool,
    pub profile: String,
    pub rustc: String,
    pub cargo_lock_sha256: String,
    pub yach_bin_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultDoc {
    pub schema: u32,
    pub host: HostInfo,
    pub build: BuildInfo,
    pub started_at: String,
    pub workloads: Vec<WorkloadRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkloadRow {
    pub id: String,
    pub class: Class,
    pub isolation: Isolation,
    pub status: Status,
    pub reason: Option<String>,
    pub count: usize,
    pub p50_ns: Option<u64>,
    pub p95_ns: Option<u64>,
    pub p99_ns: Option<u64>,
    pub max_ns: Option<u64>,
    pub value: Option<u64>,
    pub samples_ns: Option<Vec<u64>>,
    pub samples_bytes: Option<Vec<u64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbDoc {
    pub schema: u32,
    pub base_mode: String,
    pub base: Vec<ResultDoc>,
    pub current: Vec<ResultDoc>,
    pub verdicts: Vec<VerdictRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerdictRow {
    pub id: String,
    pub class: Class,
    pub verdict: Verdict,
    pub detail: Option<Detail>,
    pub base_summary: Option<f64>,
    pub current_summary: Option<f64>,
    pub budget: Budget,
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

impl WorkloadRow {
    #[must_use]
    pub fn latency(id: &str, isolation: Isolation, samples: &[Duration]) -> Self {
        let summary = LatencySummary::from_samples(None, samples);
        if summary.count == 0 {
            return Self::error(id, Class::Latency, isolation, "no samples");
        }
        Self {
            id: id.to_owned(),
            class: Class::Latency,
            isolation,
            status: Status::Ok,
            reason: None,
            count: summary.count,
            p50_ns: summary.p50.map(duration_ns),
            p95_ns: summary.p95.map(duration_ns),
            p99_ns: summary.p99.map(duration_ns),
            max_ns: summary.max.map(duration_ns),
            value: None,
            samples_ns: None,
            samples_bytes: None,
        }
    }

    #[must_use]
    pub fn memory(id: &str, samples_bytes: &[u64]) -> Self {
        let Some(value) = samples_bytes.iter().copied().max() else {
            return Self::error(id, Class::Memory, Isolation::ChildProcess, "no samples");
        };
        Self {
            id: id.to_owned(),
            class: Class::Memory,
            isolation: Isolation::ChildProcess,
            status: Status::Ok,
            reason: None,
            count: samples_bytes.len(),
            p50_ns: None,
            p95_ns: None,
            p99_ns: None,
            max_ns: None,
            value: Some(value),
            samples_ns: None,
            samples_bytes: None,
        }
    }

    #[must_use]
    pub fn value(id: &str, class: Class, isolation: Isolation, value: u64) -> Self {
        Self {
            id: id.to_owned(),
            class,
            isolation,
            status: Status::Ok,
            reason: None,
            count: 1,
            p50_ns: None,
            p95_ns: None,
            p99_ns: None,
            max_ns: None,
            value: Some(value),
            samples_ns: None,
            samples_bytes: None,
        }
    }

    #[must_use]
    pub fn skipped(id: &str, class: Class, isolation: Isolation, reason: &str) -> Self {
        Self::blank(id, class, isolation, Status::Skipped, reason)
    }

    #[must_use]
    pub fn error(id: &str, class: Class, isolation: Isolation, reason: &str) -> Self {
        Self::blank(id, class, isolation, Status::Error, reason)
    }

    fn blank(id: &str, class: Class, isolation: Isolation, status: Status, reason: &str) -> Self {
        Self {
            id: id.to_owned(),
            class,
            isolation,
            status,
            reason: Some(reason.to_owned()),
            count: 0,
            p50_ns: None,
            p95_ns: None,
            p99_ns: None,
            max_ns: None,
            value: None,
            samples_ns: None,
            samples_bytes: None,
        }
    }
}
