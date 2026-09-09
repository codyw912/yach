use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use crate::perf::alloc::AllocCounts;
use crate::perf::schema::{Class, WorkloadRow};

pub use crate::perf::schema::Isolation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    Binary,
    Tty,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bin {
    Shipping,
    Bench,
}

#[derive(Clone)]
pub struct RunCtx {
    pub samples: usize,
    pub yach_bin: Option<PathBuf>,
    pub yach_bench_yach_bin: Option<PathBuf>,
    pub yach_bench_bin: Option<PathBuf>,
    pub filter: Option<glob::Pattern>,
}

pub enum Measured {
    Latency {
        samples: Vec<Duration>,
        alloc: Option<AllocCounts>,
    },
    Memory(Vec<u64>),
    Value(u64),
}

#[derive(Clone, Copy)]
pub struct Workload {
    pub id: &'static str,
    pub class: Class,
    pub isolation: Isolation,
    pub requires: &'static [Requirement],
    pub bin: Option<Bin>,
    pub run: fn(&RunCtx) -> Result<Measured, String>,
}

#[must_use]
pub fn derived_alloc_rows(base: &WorkloadRow, counts: AllocCounts) -> [WorkloadRow; 2] {
    [
        WorkloadRow::value(
            &format!("{}#alloc_count", base.id),
            Class::Count,
            base.isolation,
            counts.count,
        ),
        WorkloadRow::value(
            &format!("{}#alloc_bytes", base.id),
            Class::Count,
            base.isolation,
            counts.bytes,
        ),
    ]
}

static ALL: LazyLock<Vec<Workload>> = LazyLock::new(|| {
    let mut workloads = Vec::new();
    workloads.extend_from_slice(&crate::perf::workloads::tui::HEADLESS);
    workloads
});

#[must_use]
pub fn all() -> &'static [Workload] {
    ALL.as_slice()
}

#[cfg(test)]
mod tests {
    use super::{all, derived_alloc_rows, Isolation};
    use crate::perf::alloc::AllocCounts;
    use crate::perf::schema::{Class, Status, WorkloadRow};
    use std::collections::BTreeSet;

    #[test]
    fn ids_are_unique_and_preserve_legacy_labels() {
        let ids: Vec<&str> = all().iter().map(|w| w.id).collect();
        let set: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(ids.len(), set.len());
        assert_eq!(
            set,
            BTreeSet::from([
                "startup/backend_ready_to_first_interactive_headless",
                "keypress/idle_keypress_to_paint_headless",
                "keypress/active_stream_replay_headless/100",
                "replay/heavy_tool_output_tail_headless/102400",
                "paste/large_multiline_component/102400",
                "viewport/huge_transcript_scroll_headless/10000",
            ])
        );
    }

    #[test]
    fn only_serial_latency_workloads_get_alloc_rows() {
        let serial = all()
            .iter()
            .filter(|w| w.isolation == Isolation::InProcessSerial && w.class == Class::Latency)
            .count();
        assert!(serial > 0);
        let base = WorkloadRow::latency("request/assemble/10_turns", Isolation::InProcessSerial, &[]);
        let rows = derived_alloc_rows(&base, AllocCounts { count: 3, bytes: 300 });
        assert_eq!(rows[0].id, "request/assemble/10_turns#alloc_count");
        assert_eq!(rows[0].class, Class::Count);
        assert_eq!(rows[0].value, Some(3));
        assert_eq!(rows[1].id, "request/assemble/10_turns#alloc_bytes");
        assert_eq!(rows[1].value, Some(300));
        assert_eq!(rows[1].status, Status::Ok);
    }
}
