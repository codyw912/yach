# Summary compaction (local protocol proof)

**Entry point:** `scripts/drive-compaction`; real `yach tui`, `/compact`, then a provider-backed continuation.

**Scope:** deterministic credless proof of portable **summary** compaction through the real OpenAI Responses adapter. It is not provider-native `/responses/compact`, a live provider-family result, model-quality evaluation, or human assessment. The server validates actual wire context; FIFO scripted output cannot satisfy this claim.

**Preconditions:** run `scripts/doctor` and `scripts/launch` first. Use the declared development shell's existing Rust and jq. The drive compiles the shared std-only `lib/semantic-provider.rs` under disposable state; no runtime/dependency installation is needed. An isolated fresh empty current-schema models.dev cache suppresses unrelated startup refresh. GET `/v1/models` is handled by the loopback fixture and does not advance its turn stages. No real credentials or operator state are read.

```bash
just dev-shell '.agents/skills/verify-yach/scripts/drive-compaction'
```

The drive sources `lib/semantic-provider.sh` after `common.sh` for the shared local launcher, fixture lifecycle, durable turn wait and session/request assertions. The HTTP server is the same helper used by provider-resume and modal focus verification, not a separate mocked requester.

**Deterministic setup:** the HTTP fixture binds `127.0.0.1:0`. Project configuration sets `compactor: "summary"`, `enabled: false` (disable automatic compaction only), `keep_recent_tokens: 1`, `reserve_tokens: 0`, `masking: false`. Explicit `/compact` still runs. The one-token kept budget forces folding while retaining the real mandatory last assistant entry. The history is deliberately short; this proves a real checkpoint/context cut, not filling a context window or guaranteed token reduction (the fixed-schema summary may be longer than this tiny prefix).

**Steps and required proof:**

1. On empty history invoke `/compact`. Capture `nothing to compact yet`, assert no checkpoint/provider attempt and no POST request. Keep this **no-op** evidence separately; it is not successful compaction.
2. Send a standing instruction: later launch-code questions must receive exactly `ORBIT-731`. Include a distinctive `FOLDED-ORIGINAL-COMPACTION-ONLY` marker in the same short original user entry. The fixture validates the actual request before acknowledging; it never echoes the code in the acknowledgement.
3. Complete a buffer turn with reply `LOCAL-KEPT-TAIL`. Require real successful provider attempts and completed turns. Save the entire session prefix with zero checkpoints.
4. Invoke the TUI's `/compact`. The fixture validates that the summary request has the summarization preamble, `<conversation>`, original instruction and folded marker, and buffer prompt, but not the future question. Only then return an eight-section summary preserving the standing instruction verbatim and adding `COMPACTED-CONSTRAINT-V1`, not the folded marker.
5. Require a **new** durable manual `compaction_checkpoint`, `compactor: "summary"`, a correlated successful `compaction_summary` provider attempt before it, and a valid `first_kept_entry_id` pointing to the buffer assistant entry. Poll the new durable records after the single action; use the stable `⟲1` footer/current composer for capture readiness, not the transient `context compacted (summary)` status that a session-count update may overwrite. Token estimates remain evidence, not a size-reduction assertion for this short history.
6. Ask `What is the launch code?` without the answer or standing instruction in that new prompt. Return `ORBIT-731` **only if** actual provider instructions contain the authoritative compacted-summary frame, summary marker and standing instruction, while effective input excludes the original instruction/folded marker and retains the kept assistant tail plus new question. Missing summary instructions or replayed folded history produces HTTP 400 and a failed recipe, not a canned successful answer.
7. Require the same one session/identity and unchanged saved prefix, successful continuation provider attempt and completed turn after the checkpoint, exact reply, exactly four accepted POST stages (initial, buffer, summary, continuation) and no fixture rejections. Every stage must join the corresponding successful persisted attempt by `provider_request_id == request_id` and model `gpt-fixture`. Re-evaluate captured wire bodies as well as session evidence.

**Evidence:** five numbered current-pane captures (including distinct empty-history no-op), `noop-session.jsonl`, `before-compact.jsonl`, `after-compact.jsonl`, final `session.jsonl`, `session-inspector.json`, `assertions.json`, `compaction-config.json`, `catalog-cache.json`, `scope.txt` and exit statuses. `protocol/requests.jsonl` preserves actual requests and explicit validation results; raw bodies and `protocol/result.json` remain beside it. An EXIT/signal trap stops this drive's private tmux and loopback server and removes isolated state/compiled helper. Evidence is kept outside the repo; no shared default TUI server is started.

**Negative controls:** after a real successful drive, these mutations must all be rejected. Set `ev` to its printed evidence directory. The helper's offline entry points use the same acceptance predicates as the live server and final drive assertions.

```bash
s=.agents/skills/verify-yach/scripts
scratch=$(mktemp -d)
rustc --edition=2024 "$s/lib/semantic-provider.rs" -o "$scratch/provider"
# ev=/absolute/path/printed/by/drive-compaction
jq 'select(.type != "compaction_checkpoint")' "$ev/session.jsonl" > "$scratch/no-checkpoint.jsonl"
if "$scratch/provider" check-evidence compaction "$ev/protocol" "$ev/before-compact.jsonl" "$scratch/no-checkpoint.jsonl"; then
  echo 'ERROR: no-op/no checkpoint accepted as compaction' >&2; exit 1
fi
jq -s '[.[] | select(.method == "POST")][-1].body' "$ev/protocol/requests.jsonl" > "$scratch/continuation.json"
jq '.instructions = ""' "$scratch/continuation.json" > "$scratch/no-standing-instruction.json"
if "$scratch/provider" check compaction 3 "$ev/protocol/config.json" "$scratch/no-standing-instruction.json"; then
  echo 'ERROR: missing compacted instruction accepted' >&2; exit 1
fi
jq --slurpfile f "$ev/protocol/config.json" \
  '.input += [{type:"message",role:"user",content:[{type:"input_text",text:$f[0].first_prompt}]}]' \
  "$scratch/continuation.json" > "$scratch/replayed-folded-original.json"
if "$scratch/provider" check compaction 3 "$ev/protocol/config.json" "$scratch/replayed-folded-original.json"; then
  echo 'ERROR: original folded entry replay accepted' >&2; exit 1
fi
for kind in missing unrelated; do
  jq --arg kind "$kind" '
    if .type == "provider_attempt_finished" and .purpose == "compaction_summary" then
      if $kind == "missing" then del(.provider_request_id) else .provider_request_id = "unrelated-request" end
    else . end' "$ev/session.jsonl" > "$scratch/$kind-summary-request-id.jsonl"
  if "$scratch/provider" check-evidence compaction "$ev/protocol" "$ev/before-compact.jsonl" "$scratch/$kind-summary-request-id.jsonl"; then
    echo "ERROR: $kind summary request identity accepted" >&2; exit 1
  fi
done
rm -rf "$scratch"
```

A passing local protocol proof demonstrates persisted checkpoint and effective-context reconstruction. It does not close a live-cohort compaction requirement or show that a real model followed the retained instruction.
