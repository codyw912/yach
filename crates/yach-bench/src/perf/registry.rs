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
    /// When true, the worker emits `#alloc_count`/`#alloc_bytes` from the
    /// workload's inner timed-operation window. Cached multi-phase families
    /// set this false rather than reporting fixture or cache allocations.
    pub emit_alloc: bool,
}

impl Workload {
    /// Whether this workload contributes derived `#alloc_count` and
    /// `#alloc_bytes` rows. The worker's measurement path, `--list`, and the
    /// unmatched-threshold check must agree, or `--list` advertises rows that
    /// are never produced and stale threshold rows escape the hard error.
    #[must_use]
    pub fn emits_alloc_rows(&self) -> bool {
        self.emit_alloc
            && self.isolation == Isolation::InProcessSerial
            && self.class == Class::Latency
    }

    /// The derived row ids this workload contributes, in emission order.
    #[must_use]
    pub fn alloc_row_ids(&self) -> Vec<String> {
        if self.emits_alloc_rows() {
            vec![
                format!("{}#alloc_count", self.id),
                format!("{}#alloc_bytes", self.id),
            ]
        } else {
            Vec::new()
        }
    }
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
    workloads.extend_from_slice(&crate::perf::workloads::tui::LIVE);
    workloads.extend_from_slice(&crate::perf::workloads::startup::STARTUP);
    workloads.extend_from_slice(&crate::perf::workloads::edit::EDIT);
    workloads.extend_from_slice(&crate::perf::workloads::extension::EXTENSION);
    workloads.extend_from_slice(&crate::perf::workloads::binary::BINARY);
    workloads.extend_from_slice(&crate::perf::workloads::core_loop::CORE_LOOP);

    workloads
});

#[must_use]
pub fn all() -> &'static [Workload] {
    ALL.as_slice()
}

#[cfg(test)]
mod tests {
    use super::{Isolation, all, derived_alloc_rows};
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
                "terminal/startup_ready_keypress_draw_flush_live",
                "terminal/idle_keypress_to_draw_flush_live",
                "terminal/active_stream_keypress_to_draw_flush_live",
                "terminal/stream_backlog_keypress_to_draw_flush_live",
                "terminal/async_backlog_keypress_to_draw_flush_live",
                "terminal/async_backlog_stress_keypress_to_draw_flush_live",
                "terminal/heavy_output_keypress_to_draw_flush_live",
                "terminal/large_transcript_scroll_to_draw_flush_live",
                "terminal/huge_transcript_scroll_to_draw_flush_live",
                "yach/tui_startup_first_output_pty",
                "yach/tui_ready_startup_first_output_pty",
                "yach/cli_startup_first_output",
                "yach/tui_startup_profile/observed_process_to_first_render_pty",
                "yach/tui_startup_profile_with_inactive_extension/observed_process_to_first_render_pty",
                "yach/tui_startup_profile_many_extensions/observed_process_to_first_render_pty",
                "startup/phase/process_main_start",
                "startup/phase/cli_args_parsed",
                "startup/phase/command_run_start",
                "startup/phase/tokio_runtime_created",
                "startup/phase/backend_setup_start",
                "startup/phase/backend_session_started",
                "startup/phase/client_initialize_sent",
                "startup/phase/backend_task_spawned",
                "startup/phase/run_tui_start",
                "startup/phase/tui_app_created",
                "startup/phase/tui_raw_mode_enabled",
                "startup/phase/tui_cursor_hidden",
                "startup/phase/tui_terminal_created",
                "startup/phase/tui_event_stream_created",
                "startup/phase/tui_first_backend_event_received",
                "startup/phase/tui_first_render_start",
                "startup/phase/tui_first_render_end",
                "startup/phase/extension_manifest_scan_scheduled",
                "startup/phase/extension_manifest_scan_started",
                "startup/phase/extension_manifest_scan_finished",
                "memory/peak_rss/tui_ready",
                "extension_runtime/metadata_host_activation",
                "extension_runtime/metadata_tool_invocation_round_trip",
                "native_edit/create_small_text_file/preview",
                "native_edit/create_small_text_file/prepared_evidence_summary",
                "native_edit/create_small_text_file/apply",
                "native_edit/create_small_text_file/finished_evidence_summary",
                "native_edit/create_small_text_file/session_append_events",
                "native_edit/create_small_text_file/end_to_end_harness_success",
                "native_edit/modify_single_hunk_small_file/preview",
                "native_edit/modify_single_hunk_small_file/prepared_evidence_summary",
                "native_edit/modify_single_hunk_small_file/apply",
                "native_edit/modify_single_hunk_small_file/finished_evidence_summary",
                "native_edit/modify_single_hunk_small_file/session_append_events",
                "native_edit/modify_single_hunk_small_file/end_to_end_harness_success",
                "native_edit/modify_multi_hunk_medium_file/preview",
                "native_edit/modify_multi_hunk_medium_file/prepared_evidence_summary",
                "native_edit/modify_multi_hunk_medium_file/apply",
                "native_edit/modify_multi_hunk_medium_file/finished_evidence_summary",
                "native_edit/modify_multi_hunk_medium_file/session_append_events",
                "native_edit/modify_multi_hunk_medium_file/end_to_end_harness_success",
                "native_edit/validation_failure_path_traversal/end_to_end_harness_validation_failure",
                "native_edit/apply_failure_hash_changed/apply",
                "native_edit/apply_failure_hash_changed/end_to_end_harness_apply_failure",
                "binary/size_bytes",
                "request/assemble/10_turns",
                "request/assemble/100_turns",
                "request/assemble/1000_turns",
                "request/roster_bytes/builtin",
                "request/roster_bytes/hashline_ext",
                "provider/encode/rig_messages/100_turns",
                "provider/encode/rig_tools/100_turns",
                "turn/scripted/text_only",
                "turn/scripted/tools_4/builtin",
                "turn/scripted/tools_4/builtin_child",
                "turn/scripted/tools_4/hashline_ext",
                "turn/scripted/tools_4/inactive_ext_8",
                "turn/phase/request_assembled",
                "turn/phase/provider_request_sent",
                "turn/phase/provider_first_event",
                "turn/phase/provider_stream_end",
                "turn/phase/tool_dispatched",
                "turn/phase/tool_result_appended",
                "turn/phase/session_persisted",
                "turn/phase/turn_completed",
                "memory/peak_rss/turn_scripted_tools_4",
                "turn/phase/hashline_ext/tool_dispatched",
                "turn/phase/hashline_ext/tool_result_appended",
                "extension/activation/hashline_ext/spawn",
                "extension/activation/hashline_ext/handshake",
                "extension/activation/hashline_ext/total",
                "extension/execute/hashline_ext/one_call",
                "extension/execute/hashline_ext/tools_4_total",
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
        let base =
            WorkloadRow::latency("request/assemble/10_turns", Isolation::InProcessSerial, &[]);
        let rows = derived_alloc_rows(
            &base,
            AllocCounts {
                count: 3,
                bytes: 300,
            },
        );
        assert_eq!(rows[0].id, "request/assemble/10_turns#alloc_count");
        assert_eq!(rows[0].class, Class::Count);
        assert_eq!(rows[0].value, Some(3));
        assert_eq!(rows[1].id, "request/assemble/10_turns#alloc_bytes");
        assert_eq!(rows[1].value, Some(300));
        assert_eq!(rows[1].status, Status::Ok);
    }

    #[test]
    fn emit_alloc_marks_only_timed_serial_operations() {
        let emitting: BTreeSet<&str> = all()
            .iter()
            .filter(|workload| workload.emit_alloc)
            .map(|workload| workload.id)
            .collect();
        assert_eq!(
            emitting,
            BTreeSet::from([
                "startup/backend_ready_to_first_interactive_headless",
                "keypress/idle_keypress_to_paint_headless",
                "keypress/active_stream_replay_headless/100",
                "replay/heavy_tool_output_tail_headless/102400",
                "paste/large_multiline_component/102400",
                "viewport/huge_transcript_scroll_headless/10000",
                "request/assemble/10_turns",
                "request/assemble/100_turns",
                "request/assemble/1000_turns",
                "provider/encode/rig_messages/100_turns",
                "provider/encode/rig_tools/100_turns",
                "turn/scripted/text_only",
                "turn/scripted/tools_4/builtin",
            ])
        );
    }
}
