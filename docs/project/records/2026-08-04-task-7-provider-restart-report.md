# Task 7 provider restart acceptance hardening

## Review fixes

- Bounded the restart fixture to the exact required HTTP sequence: A validation `GET /v1/models`, B discovery `GET /v1/models`, then B `POST /v1/chat/completions`. The test records request kinds and count, and requires the fixture to complete all three responses.
- Replaced the header-only fixture reader with a 64 KiB, five-second-bounded HTTP reader. It finds complete headers, parses exactly one `Content-Length`, retains bytes already read, and waits through the declared body before routing a request.
- Captured both child streams with `Command::output`. The harness rejects the sentinel in stdout and stderr before evaluating exit status; failure diagnostics contain only phase, exit status, byte counts, and allow-listed non-secret causes.
- Kept the credential environment-only: only create and dedicated leak children receive `YACH_TASK_7_SECRET`; verification receives no secret environment variable.
- Corrected the restart prompt to use the runner's current `default` session. The prior test-only session ID prevented prompt startup after a valid model activation, so the provider was never contacted.
- The fixture uses nonblocking accepts and one caller-supplied absolute deadline across accept and every request read. The reader recomputes the remaining socket timeout immediately before each read, while preserving its 64 KiB request bound. If the parent's completion wait expires first, it sets a shutdown token, shuts down the tracked active socket to wake a blocked read, joins the worker, then reports the parent timeout. Restart acceptance gives the fixture ten seconds and waits eleven; the missing-request regression gives it 200 ms and waits one second.

## Focused RED-to-GREEN evidence

- RED then GREEN: `tests::restart_fixture_reader_waits_for_a_segmented_request_body` initially returned after headers; it now waits for the separately sent `Content-Length` body. Exact focused run: 1 passed, 108 filtered.
- RED then GREEN: `tests::provider_connection_restart_child_rejects_secret_output_without_echoing_it` initially accepted a dedicated child that emitted the sentinel. With captured streams, separate stdout and stderr leak children are rejected without writing the sentinel to the parent test output. Exact focused run: 1 passed, 108 filtered.
- RED then GREEN: `tests::provider_connections_survive_restart_and_complete_a_real_provider_turn` exposed the bounded fixture's missing third request and then the invalid test-only prompt session. The final acceptance requires exactly three requests in order, successful fresh discovery, prompt completion, authorization, and exact model use. Exact focused run: 1 passed, 108 filtered.
- RED then GREEN: `tests::restart_fixture_reports_missing_request_before_parent_timeout` previously could not observe the detached fixture worker and the first missing request blocked it forever. The fixture now returns an actionable timeout error for request 1 of 3. Exact focused run: 1 passed, 110 filtered.
- RED then GREEN: `tests::restart_fixture_reader_enforces_one_absolute_deadline_for_a_slow_drip` sent an incomplete request one byte every 50 ms. RED returned the inherited nonblocking socket's `Resource temporarily unavailable` error instead of an absolute-deadline result; GREEN first restores blocking mode after accept, then recomputes the remaining duration before every read. Exact focused run: 1 passed, 111 filtered.
- `tests::restart_fixture_parent_timeout_wakes_and_joins_the_active_reader` holds an incomplete connection beyond the parent's wait and proves the active socket is woken rather than left detached. Exact focused run: 1 passed, 111 filtered.

## Required verification

```text
cargo test -p yach tests::provider_connections_survive_restart_and_complete_a_real_provider_turn -- --exact --nocapture
cargo test -p yach tests::restart_fixture_reader_waits_for_a_segmented_request_body -- --exact --nocapture
cargo test -p yach tests::provider_connection_restart_child_rejects_secret_output_without_echoing_it -- --exact --nocapture
cargo test -p yach tests::restart_fixture_reader_enforces_one_absolute_deadline_for_a_slow_drip -- --exact --nocapture
cargo test -p yach tests::restart_fixture_reports_missing_request_before_parent_timeout -- --exact --nocapture
cargo test -p yach tests::restart_fixture_parent_timeout_wakes_and_joins_the_active_reader -- --exact --nocapture
```

Current focused validation ran the slow-drip deadline regression, parent wake-and-join lifecycle regression, missing-request regression, segmented-reader regression, and restart acceptance; each ran one test, passed, and filtered 111 tests. The secret-output check remains covered by the preceding recorded evidence. No sentinel appeared in the current command output.

## Concerns

- The fixture intentionally accepts only the three requests that prove restart durability. Its absolute deadline cannot be extended by partial reads, and a parent completion timeout actively wakes and joins the worker before returning. The restart acceptance retains its ten-second fixture deadline for CI subprocess startup.
- The Task 7 PTY smoke fixture and harness were not changed, so its prior PTY evidence was not re-run.
