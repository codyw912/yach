# Provider continuation after resume (local protocol recovery)

**Entry point:** `scripts/drive-provider-resume`; real `yach tui`, then `yach tui --resume`.

**Scope:** deterministic, credless OpenAI Responses adapter/session protocol coverage on loopback, with **explicit model re-selection after transcript restore**. This is not automatic provider restoration or live OpenAI, Anthropic, gateway, or other provider-family coverage. The existing [`drive-resume`](resume.md) remains transcript/fixture coverage; its fixture echo cannot satisfy this recipe.

**Preconditions:** run `scripts/doctor` and `scripts/launch` first to record the bench-enabled binary. Use the declared development shell, which already supplies Rust and jq; no dependency installation or real key is needed. The drive compiles the std-only `lib/semantic-provider.rs` into its disposable state directory. Its private HOME contains a fresh empty current-schema models.dev cache to skip unrelated startup catalog networking. Provider model discovery is answered locally, separately from turn completion.

```bash
just dev-shell '.agents/skills/verify-yach/scripts/drive-provider-resume'
```

The shared `lib/semantic-provider.sh` is sourced after `common.sh`. Its small lifecycle API is `semantic_setup MODE [EVIDENCE_DIR]`, `semantic_start_tui [ARGS...]`, `semantic_session`, `semantic_wait_turn PROMPT REPLY`, and `semantic_finish BEFORE_JSONL`. Setup installs cleanup and exposes isolated `state`, `session`, `ev` and `base_url`; callers write `ev/protocol/config.json` before starting the TUI. The modal recipe reuses the same two-turn request validation without claiming it resumed.

`semantic_seed_catalog_cache [EVIDENCE_PATH]` seeds only the current isolated HOME's offline cache and saves it at the optional path (default `ev/catalog-cache.json`). The scripted edit-rejection case uses this before startup too; it does not start an HTTP fixture.

**Steps and required proof:**

1. Bind the shared HTTP fixture to `127.0.0.1:0`. Start the real TUI with `YACH_RIG_PROVIDER=openai`, `gpt-fixture`, the ephemeral `/v1` URL, and a dummy local key. The isolated environment does not inherit operator credentials or proxy configuration. Wait for the model and settled composer.
2. Submit the first prompt. The fixture must observe that actual user message before emitting `LOCAL-RESUME-FIRST-OK`. Require its successful turn-purpose provider attempt, assistant entry and completed turn in the JSONL. Save the session path and entire event prefix.
3. Quit, stop only this drive's private tmux server, then restart with `--resume` without resetting state. Capture the restored first user/assistant exchange **before** submitting another prompt. The initial integrated run restored transcript/model text but showed `backend: no provider configured`; a subsequent prompt produced fixture echo and no second wire request. That is an automatic-restoration failure, not a pass.
4. Explicitly open `/model` and type `gpt-fixture` **without Enter** to search the complete discovered list. A blank query shows curated models only and hides this unknown fixture model when no provider is active. Wait for the advertised `gpt-fixture [Environment]` row, then press Enter to reactivate it. The supported activation handoff opens **Thinking Level**; capture the selected `off` option and press Enter again to confirm it before waiting for the composer. Capture reactivation and require a newly persisted `session_model_changed` event with `source: "explicit"` and the exact local target. This recovery action must be reported. It may also persist a default in the isolated HOME; that state is cleaned up.
5. Submit a different question. The fixture emits `LOCAL-RESUME-HISTORY-OK` only if the new actual wire request contains, in order, exactly one original user message, original assistant response and new user message. Fixture echo, old visible text and message counts alone cannot pass.
6. Require the same one session file and session identity, unchanged saved prefix, exactly one new successful turn-purpose provider attempt correlated with the continuation, its completed turn and exactly one exact assistant reply parented to the new user entry. The continuation's turn ID and both entry IDs must be absent from the saved prefix; reused counters must fail rather than collapse two exchanges into one inspector turn. Both turn attempts must have model `gpt-fixture` and `provider_request_id` equal to the corresponding actual POST's `request_id`. Re-evaluate captured wire bodies, not just the server's `accepted` flags. Require exactly two accepted turn requests and no rejected/unexpected requests.

**Evidence:** six numbered current-pane captures (including pre-input restored transcript, local model selection, explicit Thinking Level/off confirmation and provider reactivation), `before-resume.jsonl`, `restored-before-input.jsonl`, `after-reselection.jsonl`, final `session.jsonl`, `session-inspector.json`, `assertions.json`, `scope.txt`, and process exit statuses. `protocol/requests.jsonl` records method/path, stage, request ID, actual body, validation verdict and explicit error; individual request bodies are retained alongside it. GET `/v1/models` is discovery, not a provider turn. `protocol/result.json` records fixture stage/rejection counts.

The EXIT/signal traps stop this drive's tmux server and local fixture and remove its isolated HOME, sessions, workspace and compiled helper on success or failure. Evidence remains outside the repository. Neither the default skill tmux server nor the operator's state is used.

**Negative controls (run after a successful drive, not instead of the drive):** compile the same helper into a disposable directory and point `ev` at the printed evidence directory. Removing only the continuation's successful provider attempt must fail even if its transcript remains intact. Dropping the restored exchange from the actual request must also fail.

```bash
s=.agents/skills/verify-yach/scripts
scratch=$(mktemp -d)
rustc --edition=2024 "$s/lib/semantic-provider.rs" -o "$scratch/provider"
# ev=/absolute/path/printed/by/drive-provider-resume
turn=$(jq -rs --slurpfile f "$ev/protocol/config.json" \
  '.[] | select(.type == "entry_appended" and .role == "user" and .text == $f[0].continuation_prompt) | .turn_id' "$ev/session.jsonl")
jq --arg turn "$turn" 'select(.type != "provider_attempt_finished" or .purpose != "turn" or .turn_id != $turn)' \
  "$ev/session.jsonl" > "$scratch/no-new-provider-attempt.jsonl"
if "$scratch/provider" check-evidence resume "$ev/protocol" "$ev/before-resume.jsonl" "$scratch/no-new-provider-attempt.jsonl"; then
  echo 'ERROR: fixture-only continuation accepted' >&2; exit 1
fi
jq -s '[.[] | select(.method == "POST")][-1].body | .input = [.input[-1]]' \
  "$ev/protocol/requests.jsonl" > "$scratch/no-restored-history.json"
if "$scratch/provider" check resume 1 "$ev/protocol/config.json" "$scratch/no-restored-history.json"; then
  echo 'ERROR: missing resumed history accepted' >&2; exit 1
fi
for kind in missing unrelated; do
  jq --arg turn "$turn" --arg kind "$kind" '
    if .type == "provider_attempt_finished" and .turn_id == $turn then
      if $kind == "missing" then del(.provider_request_id) else .provider_request_id = "unrelated-request" end
    else . end' "$ev/session.jsonl" > "$scratch/$kind-request-id.jsonl"
  if "$scratch/provider" check-evidence resume "$ev/protocol" "$ev/before-resume.jsonl" "$scratch/$kind-request-id.jsonl"; then
    echo "ERROR: $kind request identity accepted" >&2; exit 1
  fi
done
# Simulate a resumed process resetting turn/entry counters while preserving the
# old prefix and otherwise successful provider traffic.
jq -s --arg turn "$turn" --slurpfile before "$ev/before-resume.jsonl" '
  first($before[] | select(.type == "entry_appended" and .role == "user")) as $old_user |
  first($before[] | select(.type == "entry_appended" and .role == "assistant" and .turn_id == $old_user.turn_id)) as $old_reply |
  .[0:($before | length)] + (.[($before | length):] | map(
    if .turn_id? == $turn then
      .turn_id = $old_user.turn_id |
      if .type == "entry_appended" and .role == "user" then .entry_id = $old_user.entry_id
      elif .type == "entry_appended" and .role == "assistant" then .entry_id = $old_reply.entry_id | .parent_entry_id = $old_user.entry_id
      else . end
    else . end)) | .[]
' "$ev/session.jsonl" > "$scratch/reused-continuation-identities.jsonl"
if "$scratch/provider" check-evidence resume "$ev/protocol" "$ev/before-resume.jsonl" "$scratch/reused-continuation-identities.jsonl"; then
  echo 'ERROR: reused continuation turn/entry identities accepted' >&2; exit 1
fi
rm -rf "$scratch"
```

These controls reject omitted provider/request history, missing or unrelated request IDs, and reused continuation identities. They do not manufacture a live-family pass or prove that an external model understood the history.
