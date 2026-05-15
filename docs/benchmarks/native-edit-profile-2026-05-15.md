# Native Edit Profile - 2026-05-15

## Summary

This is the first native edit profiling baseline after native edit preview,
guarded apply, redacted evidence, and the backend-local harness. It uses
synthetic deterministic edit scenarios only. It is not a Pi comparison, and it
does not measure CLI, TUI, or provider edit UX.

## Environment

- Date: 2026-05-15
- Branch: `native-edit-profile-impl`
- Profiled revision: base commit `c74426c` (`Plan native edit profiling (#38)`) plus uncommitted native edit profiling implementation changes
- Build/profile mode: release `yach-bench`
- Machine: local macOS development machine

## Command

```bash
just dev cargo run -p yach-bench --release -- native-edit-profile-report --samples 5
```

## Results

```text
samples_requested=5
samples_collected=5
workload=native_edit/apply_failure_hash_changed/apply count=5 p50=42.792us p95=48.666us p99=48.666us max=48.666us
workload=native_edit/apply_failure_hash_changed/end_to_end_harness_apply_failure count=5 p50=71.584us p95=77.459us p99=77.459us max=77.459us
workload=native_edit/create_small_text_file/apply count=5 p50=6.100ms p95=9.810ms p99=9.810ms max=9.810ms
workload=native_edit/create_small_text_file/end_to_end_harness_success count=5 p50=5.260ms p95=5.677ms p99=5.677ms max=5.677ms
workload=native_edit/create_small_text_file/finished_evidence_summary count=5 p50=333ns p95=4.291us p99=4.291us max=4.291us
workload=native_edit/create_small_text_file/prepared_evidence_summary count=5 p50=208ns p95=250ns p99=250ns max=250ns
workload=native_edit/create_small_text_file/preview count=5 p50=22.209us p95=86.250us p99=86.250us max=86.250us
workload=native_edit/create_small_text_file/session_append_events count=5 p50=417ns p95=583ns p99=583ns max=583ns
workload=native_edit/modify_multi_hunk_medium_file/apply count=5 p50=5.338ms p95=5.785ms p99=5.785ms max=5.785ms
workload=native_edit/modify_multi_hunk_medium_file/end_to_end_harness_success count=5 p50=5.649ms p95=5.861ms p99=5.861ms max=5.861ms
workload=native_edit/modify_multi_hunk_medium_file/finished_evidence_summary count=5 p50=416ns p95=542ns p99=542ns max=542ns
workload=native_edit/modify_multi_hunk_medium_file/prepared_evidence_summary count=5 p50=250ns p95=291ns p99=291ns max=291ns
workload=native_edit/modify_multi_hunk_medium_file/preview count=5 p50=86.791us p95=111.333us p99=111.333us max=111.333us
workload=native_edit/modify_multi_hunk_medium_file/session_append_events count=5 p50=417ns p95=583ns p99=583ns max=583ns
workload=native_edit/modify_single_hunk_small_file/apply count=5 p50=5.144ms p95=5.239ms p99=5.239ms max=5.239ms
workload=native_edit/modify_single_hunk_small_file/end_to_end_harness_success count=5 p50=5.529ms p95=6.171ms p99=6.171ms max=6.171ms
workload=native_edit/modify_single_hunk_small_file/finished_evidence_summary count=5 p50=291ns p95=417ns p99=417ns max=417ns
workload=native_edit/modify_single_hunk_small_file/prepared_evidence_summary count=5 p50=208ns p95=333ns p99=333ns max=333ns
workload=native_edit/modify_single_hunk_small_file/preview count=5 p50=40.000us p95=50.666us p99=50.666us max=50.666us
workload=native_edit/modify_single_hunk_small_file/session_append_events count=5 p50=334ns p95=417ns p99=417ns max=417ns
workload=native_edit/validation_failure_path_traversal/end_to_end_harness_validation_failure count=5 p50=1.750us p95=2.375us p99=2.375us max=2.375us
```

## Interpretation

This profile separates native edit preview, apply, redacted evidence summary,
session append, and end-to-end harness paths. It is a local baseline only.

## Limitations

This run uses `--samples 5` as a smoke baseline. At this sample count, p95 and
p99 collapse near max and should not be treated as stable tail-latency claims.
Use larger sample runs before making optimization or product latency claims.
