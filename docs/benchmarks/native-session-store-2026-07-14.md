# Native Session Store Benchmarks After Tool Payload Persistence

Date: 2026-07-14

Context: the session tool payload persistence design
(`docs/project/specs/2026-07-14-session-tool-payload-persistence-design.md`)
adds persisted tool argument and result content to session events. The
`native_session` Criterion bench previously appended text-only turns; its
fixture now includes one `ToolRequestRecorded` (bounded argument JSON) and one
`ToolExecutionFinished` with a ~1KiB persisted result body per turn, so load
and projection numbers reflect content-bearing logs.

Local run (`just dev cargo bench -p yach-bench --bench native_session`),
Criterion medians:

| Benchmark | Text-only fixture (pre-change) | Tool-payload fixture (current) |
| --- | --- | --- |
| `native_session_append_event` | 5.51 ms | 5.77 ms |
| `native_session_load_10_turns` | 28.1 µs | 62.7 µs |
| `native_session_load_100_turns` | 166 µs | 495 µs |
| `native_session_load_1000_turns` | 1.56 ms | 4.79 ms |
| `native_session_projection_10_turns` | 659 ns | 711 ns |
| `native_session_projection_100_turns` | 5.19 µs | 6.01 µs |
| `native_session_projection_1000_turns` | 54.2 µs | 62.6 µs |

Reading:

- Append remains fsync-dominated; payload size is immaterial per event.
- Load scales with file size as expected: a 1000-turn log carrying ~1KiB of
  tool content per turn parses in under 5 ms, far below any startup or
  resume budget that matters today.
- Projection cost is nearly unchanged.

Conclusion: no need for the side-car content store (design Option C) at
current bounds. Revisit if per-turn persisted content grows well beyond a
few KiB or session length grows by orders of magnitude.
