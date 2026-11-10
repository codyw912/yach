# Modal transitions (edit rejection and composer focus)

**Entry point:** `yach tui` in the default `review` approval mode; `scripts/drive-modal-transitions`.

**Preconditions:** the bench-enabled binary, private HOME/session/workspace state, and settled current-pane helpers in `scripts/lib/common.sh`. Edit rejection uses the scripted provider; the selector case uses the shared loopback Responses adapter and model-list fixture through `scripts/lib/semantic-provider.sh`. No real credentials or external provider requests are needed. The cases use independent run state.

Both startups in this new recipe seed an isolated fresh current-schema models.dev cache to avoid that unrelated external catalog refresh. The older scripted recipes' canned turns do not by themselves guarantee network-free startup; this change does not alter those drives or claim otherwise.

## Scenarios

### Edit review: one Down rejects

1. Seed `notes.txt` and retain its original bytes. Script a `read_text_file` round, followed by an `edit_text_file` hashline proposal using that file's snapshot tag, followed by a distinct assistant reply.
2. Submit the edit prompt in default review mode. Require a pending edit review for `notes.txt`, a real diff, initially selected `› Approve`, and `Reject`. There must be no command-only `Approve once` or `Approve for this session` option.
3. Send **Down once**, wait for the changed settled frame with `› Reject`, and capture it before Enter. Enter must resolve the review as rejected and allow the scripted turn to finish.
4. Compare the workspace file byte-for-byte with its seed. Inspect the completed session and retain its JSONL: the completed read must precede the matching edit review; that edit's review decision must be `reject`, its permission decision must be denied with `user_rejected`, and its tool result must retain `user_rejected` plus `[rejected by review]`. A scripted acknowledgement alone is not proof.

The inspector's extension-edit tool name can be null: correlate its tool item with the rejection review by `tool_request_id`. The durable review supplies `edit_text_file`, the target path, and the permission-decision ID; the preceding read proves the snapshot, and the pending capture proves the proposed diff. Do not require a separate exact edit-argument record that this extension-proposal path does not emit.

The existing `drive-tool-review` rejection case requests `bash rm notes.txt`. Command review has three options and uses **Down, Down**. It neither exercises an edit proposal nor establishes the edit review's two-option, one-Down invariant. Likewise, the full-auto multi-file recipe never enters the review controls. This scenario fills those baseline gaps; it does not replace either recipe.

### Model selector: Escape restores the composer

1. Start a fresh local-adapter TUI and complete a warmup prompt. Retain the warmup capture, session inspection, and pre-selector JSONL. The shared fixture serves both `/v1/responses` and `/v1/models` on `127.0.0.1:0`; a fresh current-schema catalog cache prevents the unrelated models.dev refresh.
2. Submit `/model` and wait for the `Select Model` selector, its search/navigation/Escape controls, and the active `gpt-fixture` row. Capture this visible selector state.
3. Send Escape once. Require a changed settled pane with the empty composer's send/newline hint and no model-selector title, search field, or activation action. Capture the dismissed state.
4. Submit a run-unique prompt whose reply has not appeared in either earlier capture. Require a new changed settled frame with its distinct conditional local-provider completion. The persisted second turn must match that exact prompt, complete successfully, record a new succeeded turn-purpose provider attempt, and contain the matching assistant entry with `gpt-fixture` metadata in the same turn. Actual loopback request validation must show the warmup exchange before this new prompt. The warmup reply, retained model label, empty composer hint, or a fixture echo cannot satisfy this proof.

This recipe does not activate a model or request a forced refresh. Opening `/model` itself can request discovery, so using the scripted requester here would still allow external Anthropic model-list traffic. The selector case uses the shared fixture's two-turn `resume` protocol mode solely for request-history validation; it does not restart or claim resume coverage. Dismissal is the behavior under test; pressing Enter inside the selector would add an unnecessary activation path and could change provider setup. This is local protocol coverage, not live-provider family coverage.

## Evidence and cleanup

The script prints one evidence directory last. Numbered captures show pending edit controls, the single-Down Reject selection, the rejected result, the model-selector warmup, the open selector, the dismissed composer, and the fresh completion. Numbered session inspections and copied JSONL logs correlate screen evidence with durable effects; seed/result files and a byte-comparison record prove the rejected edit did not mutate the file. The `protocol/` subdirectory contains actual local requests and fixture results, with a correlated semantic assertion report.

EXIT traps stop the private tmux server on success or failure. The shared local-provider helper also stops/waits for its fixture, records its exit status, and removes its isolated state; edit evidence remains in the same retained directory. The standard cleanup command removes the scripted case's run directory:

```bash
s=.agents/skills/verify-yach/scripts
$s/drive-modal-transitions; rc=$?
$s/cleanup; crc=$?; [ "$rc" -ne 0 ] || rc=$crc
printf 'exit status: %s\n' "$rc"; [ "$rc" -eq 0 ]
```

## Negative controls

Run these only in an isolated verification change; restore each mutation before the next run. The parent integration pass owns runtime checks.

- Introduce the command-only session grant into edit review: the pending-controls check or the one-Down `› Reject` wait must fail.
- Select Approve instead of Reject, or remove the Enter rejection: the changed-frame wait, unchanged-byte comparison, or correlated persisted rejection assertion must fail.
- Remove the recorded rejection/denial while leaving the screen acknowledgement: the session/JSONL assertions must fail.
- Prevent Escape from dismissing the model selector, or prevent the composer from accepting input: the closed-modal check or distinct second-turn completion assertion must fail.
- Replay only the warmup/old completion without completing the unique prompt: the exact second-turn prompt, assistant entry, and new succeeded attempt checks must fail.
- **Two Downs are not a reliable negative runtime input:** edit review navigation deliberately saturates at Reject. The drive therefore sends exactly one Down and captures its result before Enter; a two-Down run does not establish that acceptance criterion.

Human layout/focus assessment remains separate from these automated checks; use the README's affected-view TUI design-review table and leave human results unverified until someone performs them.
