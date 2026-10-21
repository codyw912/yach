# Shared lifecycle for real-TUI loopback protocol proofs. Source after common.sh.
# semantic_setup MODE [EVIDENCE_DIR] allocates private state and cleanup.
# Caller writes protocol/config.json before semantic_start_tui; see feature docs.
# common.sh supplies tty, here, state and session; callers source it first.
# shellcheck disable=SC2154

semantic_cleanup() {
  local rc=$?
  trap - EXIT
  if (( rc != 0 )); then capture "$ev/failure-pane.txt" 2>/dev/null || true; fi
  "$tty" stop "$session"
  if [[ -n "$fixture_pid" ]]; then
    if (( rc != 0 )); then kill "$fixture_pid" 2>/dev/null || true; fi
    touch "$ev/protocol/stop"
    if ! wait "$fixture_pid"; then (( rc != 0 )) || rc=1; fi
  fi
  rm -rf "$state"
  printf '%s\n' "$rc" > "$ev/exit-status"
  if (( rc != 0 )); then printf 'verify-yach semantic %s: failed; evidence: %s\n' "$semantic_mode" "$ev" >&2; fi
  exit "$rc"
}

# Also usable before a scripted-only case; no fixture process is started.
semantic_seed_catalog_cache() {
  mkdir -p "$state/run/home/.yach/catalog"
  # Skip unrelated startup models.dev refresh using an isolated, freshly checked
  # empty cache, with both capability schemas required by the actual loader.
  jq -n '{etag:null,last_modified:null,checked_at_unix_ms:(now*1000|floor),retrieved:"local-fixture",catalog:{snapshot_date:"local-fixture",tool_call_capability_schema_version:1,responses_compact_capability_schema_version:1,providers:{}}}' \
    > "$state/run/home/.yach/catalog/models-dev.json"
  cp "$state/run/home/.yach/catalog/models-dev.json" "${1:-$ev/catalog-cache.json}"
}

semantic_setup() {
  semantic_mode=$1
  [[ $semantic_mode == resume || $semantic_mode == compaction ]] || { echo 'semantic_setup: expected resume or compaction' >&2; return 1; }
  state="$state/$semantic_mode-$$"
  session="yach-$semantic_mode-$$"
  ev=${2:-$("$tty" evidence-dir yach)}
  fixture_pid=
  trap semantic_cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  reset_run
  mkdir -p "$ev/protocol"
  semantic_seed_catalog_cache
  rustc --edition=2024 "$here/lib/semantic-provider.rs" -o "$state/semantic-provider" > "$ev/fixture-build.log" 2>&1
  "$state/semantic-provider" serve "$semantic_mode" "$ev/protocol" > "$ev/fixture-stdout.log" 2> "$ev/fixture-stderr.log" &
  fixture_pid=$!
  local deadline=$((SECONDS + 10))
  until [[ -s "$ev/protocol/ready" ]]; do
    kill -0 "$fixture_pid" 2>/dev/null || { echo 'verify-yach: fixture exited before readiness' >&2; return 1; }
    (( SECONDS < deadline )) || { echo 'verify-yach: fixture readiness timed out' >&2; return 1; }
    sleep 0.1
  done
  base_url=$(cat "$ev/protocol/ready")
}

semantic_start_tui() {
  "$tty" start "$session" "$state/run/workspace" env -i \
    HOME="$state/run/home" PATH="$PATH" TERM=xterm-256color \
    YACH_SESSION_DIR="$state/run/sessions" \
    YACH_RIG_PROVIDER=openai YACH_RIG_OPENAI_API_KEY=local-fixture-key \
    YACH_RIG_OPENAI_BASE_URL="$base_url" YACH_RIG_OPENAI_MODEL=gpt-fixture \
    YACH_RIG_PROVIDER_CONTEXT_WINDOW=200000 YACH_RIG_PROVIDER_MAX_TOKENS=1024 \
    YACH_RIG_PROVIDER_TIMEOUT_SECS=5 "$(yach_bin)" tui "$@"
  ui_wait 'gpt-fixture' 30
  ui_wait 'enter send · ctrl\+j newline' 30
}

semantic_session() {
  local files
  shopt -s nullglob globstar
  files=("$state/run/sessions"/**/*.jsonl)
  [[ ${#files[@]} -eq 1 ]] || { printf 'verify-yach: expected one semantic session, found %s\n' "${#files[@]}" >&2; return 1; }
  printf '%s\n' "${files[0]}"
}

semantic_wait_turn() {
  local prompt=$1 reply=$2 log deadline=$((SECONDS + 15))
  while (( SECONDS < deadline )); do
    if log=$(semantic_session 2>/dev/null) && jq -es --arg prompt "$prompt" --arg reply "$reply" '
      . as $events | first(.[] | select(.type == "entry_appended" and .role == "user" and .text == $prompt)) as $user |
      $user != null and any($events[]; .type == "provider_attempt_finished" and .purpose == "turn" and .turn_id == $user.turn_id and .outcome == "succeeded") and
      any($events[]; .type == "turn_finished" and .turn_id == $user.turn_id and .outcome == "completed") and
      any($events[]; .type == "entry_appended" and .role == "assistant" and .turn_id == $user.turn_id and .text == $reply)
    ' "$log" >/dev/null; then return 0; fi
    sleep 0.1
  done
  printf 'verify-yach: no durable successful provider turn for %s\n' "$prompt" >&2
  return 1
}

# Call after quitting the TUI. BEFORE is the complete first-turn prefix (resume)
# or pre-/compact prefix (compaction), never a manufactured session fixture.
semantic_finish() {
  local before=$1 log server_rc=0
  log=$(semantic_session)
  cp "$log" "$ev/session.jsonl"
  (cd "$state/run/workspace" && env -i HOME="$state/run/home" PATH="$PATH" \
    YACH_SESSION_DIR="$state/run/sessions" "$(yach_bin)" sessions show latest --json) > "$ev/session-inspector.json"
  "$state/semantic-provider" check-evidence "$semantic_mode" "$ev/protocol" "$before" "$ev/session.jsonl" > "$ev/assertions.json"
  touch "$ev/protocol/stop"
  wait "$fixture_pid" || server_rc=$?
  fixture_pid=
  printf '%s\n' "$server_rc" > "$ev/fixture-exit-status"
  [[ $server_rc -eq 0 ]] || { echo 'verify-yach: local protocol proof failed' >&2; return 1; }
}
