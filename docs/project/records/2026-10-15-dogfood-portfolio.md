# M1 Dogfood Portfolio

**Outcome:** linear:CEW-147
**Status:** draft for owner review, design approved 2026-10-15; refreshed 2026-10-20 for the landed anthropic auth prerequisite
**Portfolio version:** 2

Date: 2026-10-15

This is the portfolio gate for linear:CEW-147, not a cohort run. It versions
the repositories, task classes, provider families, failure injections,
automation/manual split, evidence format, failure classes, blocker severity,
and re-run gate. It does not record a live session, and it does not add
harness code. A green eval gate is adjacent evidence, not this re-run gate.

## Why this record exists

`docs/project/roadmap.md:43-46` states the M1 outcome: representative real
coding sessions produce trustworthy, reproducible evidence about
effectiveness, resilience, interaction quality, autonomy, and coverage.
Priorities come from observed blockers, not feature comparison. The proof
rule at `docs/project/roadmap.md:67-69` is the default: automated tests,
protocol matrices, deterministic evals, and scripted provider scenarios prove
what they can observe; manual evidence is reserved for claims that require a
person.

linear:CEW-147 splits done-when into two gates:

1. **Portfolio gate (this record).** An approved, versioned record of the
   items above. Coverage must include fresh start, multi-tool work that
   changes more than one file and runs verification commands, review and
   autonomous modes, context pressure, provider failure, cancellation,
   restart and resume, and at least two provider families.
2. **Cohort gate (after this record is approved, and after the missing
   scripted drives exist).** Record the scheduled scripted and live runs,
   classify every observed failure, and resolve and re-run every S1 and S2
   blocker in the first recorded severity cohort against the same portfolio.
   Scripted cancellation supplies required cancellation coverage; live
   cancellation is recorded only if it occurs for cause, not scheduled.
   A blocked prerequisite with no provider attempt has no failure class;
   missing evidence that prevents classifying an observed failure is S2 and
   keeps the gate open.

The 2026-07-16 native MVP declaration does not satisfy M1.
`docs/project/roadmap.md:24-25` says so. The checkpoint itself is a
single-repo prompt sequence, not a portfolio
(`docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md:151-164`).
It separates no-secret checks from live runs (`:11-26`), records
pass/blocked/failed with evidence (`:32-38`, `:56-62`), and ends with a
next-blocker rule (`:166-170`). It covers one repository, one live provider
family, review-mode create/edit, and resume. It does not define task
classes, an automation/manual split, failure classes, severity, or a re-run
gate. Its search-budget blocker (`:61`) is the kind of finding this portfolio
is meant to produce systematically.

The eval portfolio is a different instrument.
`docs/project/specs/2026-07-28-eval-portfolio-design.md:9-14` and `:32-43`
define a harness regression gate (Harbor tasks, artifact assertions, pinned
model, `just eval-gate`). Its founding rule — assert on artifacts, not
utterances — is borrowed here. A green eval gate does not observe interaction
quality, autonomy, or a live provider failure.

## Decisions

Owner, 2026-10-15. These override the brief's recommended definitions where
they differ.

1. **Approach A.** Layer S (scripted, credless verify-yach drives, every
   re-run) plus Layer L (live cohort). Live sessions are not spent on
   contracts the scripted provider already implements.
2. **Repository.** Cohort 1 is `ylog` only
   (`/srv/agent-dev/projects/ylog`). Dogfood runs use a dedicated jj
   workspace of ylog, never its main checkout. `sjujperpowers` and a
   dedicated `yach` workspace are later cohorts, not cohort 1.
3. **Provider families.** Both through the host subscription gateway
   `http://omp-subscriptions.home.lan:4000`, launched with
   `secretspec run --provider omp-subscriptions --scope <scope> --reason ... -- <cmd>`.
   Family is the adapter plus the vendor line, not the model id:
   - OpenAI Codex models (for example `openai-codex/gpt-6-sol`) via yach's
     `openai-compatible` provider. In this tree that adapter speaks chat
     completions, not Responses. See Provider families.
   - Claude models (for example `claude-sonnet-4-5`) via yach's `anthropic`
     provider, with `YACH_RIG_ANTHROPIC_BASE_URL` set to the gateway and
     `YACH_RIG_ANTHROPIC_AUTH_TOKEN` supplied by the `anthropic-gateway`
     secretspec scope (`README.md:180-182`, `secretspec.toml:16-17`).
   Prerequisite for the Anthropic half only, landed 2026-10-20: Kata
   `yach#q6rj` (#331, `4b95c442`). The gateway returns 403 for `x-api-key`
   and 200 for Bearer (verified 2026-10-15); the anthropic provider now
   sends `Authorization: Bearer` when the auth token is set. Before that
   landed, the anthropic family could not start; that was a start blocker
   (S1) for that family, not a reason to drop the family. The native
   `openai-codex` ChatGPT login is not exercised by cohort 1.
4. **Autonomous.** Session-only `full-access`, one live multi-tool scenario.
   `auto-review` is a different product question and is not in cohort 1.
   `accept-edits` is not the autonomous tier.
5. **Context pressure.** Manual `/compact` after a real multi-tool turn,
   then a continuation turn that must still satisfy the original instruction.
   The `CompactionCheckpoint` is the artifact. A token-count fill is not the
   claim.
6. **Scripted drives.** Named here with pass signals. Implementing missing
   drives is follow-up work that gates the first live run, not approval of
   this record.
7. **Severity cohort.** Cohort 1 fixes every S1 and S2, then re-runs. S3 is
   recorded and does not block the cohort gate.
8. **Failure acceptance (review clarification, 2026-10-15).** `L5-failure`
   passes only for `unavailable_model`; any other classification is a
   failed or blocked run with evidence. A classified provider failure that
   prevents a required-success scenario from completing is S2 (fix and
   re-run), not merely `provider-invalid`. S1 takes precedence for failures
   to start or resume and the other S1 cases below.
9. **Live failure classification (owner, 2026-10-20, from PR review).**
   `L5-failure` stays strict: only `unavailable_model` passes. Through the
   gateway today, an unknown model id classifies as `invalid_request` on
   both families (see Provider failure). That is a known S2 for cohort 1:
   yach must classify the gateway's unknown-model response as
   `unavailable_model` before the cohort gate can close. Loosening the
   pass to any classified failure was rejected.
10. **Autonomy signal (owner, 2026-10-20, from PR review).** `L3-autonomous`
   uses `L2-review`'s change request without its instruction to run
   `uv run pytest`. If the prompt asks for the verifier, whether the agent
   runs it unprompted cannot be observed.

## Repository

Primary, and the only cohort-1 host: `ylog`. It is a small Python (uv,
pytest, Textual) browser for yach session JSONL. A two-file change plus the
verifier is a real task with a fast check, and it dogfoods the evidence
consumer. The coupling risk is real: a failure may be a fixture or schema
mismatch rather than a harness defect. The evidence note must say which.

Verifier, from `ylog`'s `README.md:147-150` and `devenv.nix` `test-project`
script:

```sh
uv run pytest
```

`pyproject.toml` sets `testpaths = ["tests"]` and lists `pytest>=9.1.1` in
the `dev` dependency group, so `uv run pytest` from the ylog workspace root
is the real command. `uv run ruff check .` and `uv run ruff format --check .`
are the lint pair in the same README block. They are not the scenario
verifier. A scenario that claims "verifier exits 0" means `uv run pytest`.

Dogfood runs use a dedicated jj workspace of ylog, never
`/srv/agent-dev/projects/ylog`. The record stores the workspace path and the
ylog seed commit id the run started from. Edits in the main checkout are other
people's work, not the agent's.

Named, not in cohort 1: `sjujperpowers` (second language, later). `yach`
itself is a later cohort only, and only in a dedicated jj workspace, because
dogfooding the harness on its main checkout hides setup failures and
contends for `flock ~/.local/state/yach-lanes/build.lock`.

## Provider families

`ProviderKind` is `anthropic`, `openai`, `openai-compatible`, and
`openai-codex` (`crates/yach-connections/src/lib.rs:84-97`). Two families
means two of these, not two models on one kind. Cohort 1 uses
`openai-compatible` and `anthropic`. The native `openai` API-key provider
and the native `openai-codex` ChatGPT login are not cohort-1 families.

| Family | `YACH_RIG_PROVIDER` | Adapter and wire | Example model | Gateway base URL |
| --- | --- | --- | --- | --- |
| OpenAI Codex via gateway | `openai-compatible` | OpenAI-compatible chat completions (`.completions_api()`, `crates/yach-backend/src/rig_adapter.rs:494-506`) | `openai-codex/gpt-6-sol` | `YACH_RIG_OPENAI_COMPAT_BASE_URL=http://omp-subscriptions.home.lan:4000/v1` |
| Claude via gateway | `anthropic` | Anthropic Messages (`rig_adapter.rs:467-478`) | `claude-sonnet-4-5` | `YACH_RIG_ANTHROPIC_BASE_URL=http://omp-subscriptions.home.lan:4000` |

Both launches go through:

```sh
secretspec run --provider omp-subscriptions --scope <scope> --reason "..." -- <cmd>
```

`secretspec` resolves `secretspec.toml` from the yach repository, so the
wrapper starts in a yach checkout and the dogfood command runs in the
dedicated ylog workspace. The 2026-09-30 smoke recorded the failure mode of
starting `secretspec` in the scratch directory
(`docs/project/records/2026-09-30-session-diagnostics-gateway-smoke.md:107-112`).

The Codex base URL includes `/v1` because that is the form the 2026-09-30
gateway smoke used, and the captured body was chat-completions shaped
(`max_tokens`, `messages`, `stream`, `tools`;
`docs/project/records/2026-09-30-session-diagnostics-gateway-smoke.md:31-33`,
`:89-91`). `RigProviderConfig::OpenAiCompatible` is the chat-completions
adapter for aggregators (`rig_adapter.rs:232-236`). Responses is the native
`openai` provider (`rig_adapter.rs:221-226`) and the ChatGPT-subscription
client (`rig_adapter.rs:508-520`). Cohort 1 does not switch the Codex family
onto either of those. If a run's capture is not chat-completions shaped, the
family label is wrong and the run is not cohort 1.

The family distinction is the adapter and the vendor line:
openai-compatible chat completions versus anthropic Messages. A Codex model
id on the anthropic adapter, or a Claude model id on the openai-compatible
adapter, is not a second family. Two models on one adapter do not meet the
issue. Native Responses (`openai` or `openai-codex`) is a third wire, not a
relabeling of the gateway Codex family. The owner named this family as
Responses; this tree does not send that wire on `openai-compatible`, so the
record follows the code. A Responses-speaking gateway cohort is a later
change, not a relabel of these runs.

**Auth prerequisite (landed).** Rig's Anthropic client used to send the key
only as `x-api-key`, and the gateway's `/v1/messages` route rejects that
header; the diagnostics spec left this path out of scope for that reason
(`docs/project/specs/2026-09-30-session-diagnostics-design.md:358-360`).
Kata `yach#q6rj` fixed it on 2026-10-20 (#331, `4b95c442`): the vendored
`AnthropicKey` sends exactly one credential header, `x-api-key` by default
or `Authorization: Bearer` for an auth token
(`vendor/rig-core/src/providers/anthropic/client.rs:44-81`). The anthropic
family launches with `--scope anthropic-gateway`, which supplies
`YACH_RIG_ANTHROPIC_AUTH_TOKEN`, and the gateway smoke completed a turn with
tools (`docs/project/records/2026-10-19-anthropic-auth-token-gateway-smoke.md:32-56`).
A 403 on an anthropic run is now a failure to classify, not a known open
prerequisite.

**What cohort 1 does not exercise.** The native `openai-codex` ChatGPT login
(`YACH_RIG_PROVIDER=openai-codex` / `chatgpt-subscription`) is not a cohort-1
family. Gateway Codex traffic is the `openai-compatible` adapter. Do not
record a gateway Codex run as `openai-codex`.

**Capture gap.** `YACH_CAPTURE_REQUESTS` writes OpenAI, OpenAI-compatible,
ChatGPT-subscription, and native-compaction bodies. Anthropic requests are
not captured (`README.md:191`, `README.md:437-444`). A Claude-family claim
about a model decision cites `yach sessions show --json` and the screen
capture. It does not invent a request-body file. That absence is expected,
not missing evidence, unless the scenario's claim was specifically about the
request body.

## Task classes

One class per required coverage row. Cohort 1 runs each live scenario on
both families unless a row below says otherwise. It does not run the full
cross product of class × mode × family beyond that.

| Class | Layer | What "done" is |
| --- | --- | --- |
| Fresh start | S, and L once per family | New `YACH_SESSION_DIR`, no resume. Prompt produces a session file and a completed turn. |
| Multi-tool change | S contract, L real task | Two or more files differ from the seed, and `uv run pytest` exits 0. Response text is not the pass signal. |
| Review mode | S, and L on the multi-tool task | Default `review`. S covers approve-once and reject. L covers a real edit approval. |
| Autonomous mode | L only | Session-only `full-access`. Same change request as the multi-tool task, without the instruction to run the verifier. Score whether the agent ran the verifier without being asked, and whether it stopped at a failure. |
| Context pressure | L only | After a real multi-tool turn, manual `/compact`, then one continuation turn whose edit still matches the pre-compaction instruction. A `CompactionCheckpoint` is required. |
| Provider failure | S terminal path; L one induced failure per family | S: a `Failed` round with a retryable kind and one with a non-retryable kind; session shows `ProviderAttemptFinished`. L: unknown model id, classified as `unavailable_model` with `classification_source` recorded. |
| Cancellation | S; L only if a live run is cancelled for cause | S: `Cancelled` round; outcome is cancelled, not failed. Do not cancel a healthy live run just to fill the cell. |
| Restart and resume | S, and L once per family | Quit, resume the same session id, complete one more turn. |

## Layer S — scripted drives

Credless. Drives a real `yach` binary in a private tmux server against
`YACH_RIG_PROVIDER=scripted` (`.agents/skills/verify-yach/SKILL.md:8-16`).
Evidence lands outside the repo at
`$XDG_STATE_HOME/sjujperpowers/evidence/yach/<UTC>-<pid>/`
(`SKILL.md:42-43`). These are harness-contract checks. They do not count as
the two provider families.

Reply scripts are JSON arrays of provider rounds (`SKILL.md:38`).
`ScriptedProvider` pops one round per request and returns
`ProviderErrorKind::InvalidRequest` ("scripted provider exhausted") when the
queue is empty (`crates/yach-backend/src/bench_loop.rs:106-125`).
`ProviderStreamEvent::Failed` and `::Cancelled` already exist
(`crates/yach-backend/src/provider.rs:352-359`), and `ProviderError` is
serializable (`:233-239`), so a drive script can inject a failed or cancelled
round without new harness code. Helpers emit text, bash, failed, cancelled
and multi-call tool rounds (`.agents/skills/verify-yach/scripts/lib/common.sh`).
A new behavior is a new drive script plus a feature file (`SKILL.md`).

### Exists today

| Drive | Feature | Pass signal |
| --- | --- | --- |
| `scripts/drive-prompt-reply` | `features/prompt-reply.md:9-11` | Transcript shows the prompt and the scripted Markdown reply with delimiters consumed. `/status` reports `messages: 2 (user 1, assistant 1, tool 0)`. Status bar shows `scripted-model` and `approval:review`. |
| `scripts/drive-tool-review` | `features/tool-review.md:9-11` | Approve-once runs `wc -l notes.txt` and the transcript shows the completed command. Reject leaves `notes.txt` on disk. The file check is the proof, not the transcript text alone (`:17`). |
| `scripts/drive-resume` | `features/resume.md:9-13` | After `/quit` and `yach tui --resume`, the prior transcript is restored before input, `/status` reports four messages, and exactly one session file remains. |
| `scripts/drive-headless` | `features/headless.md:9-11` | `yach run` exits 0 with outcome JSON `schema: "yach-run-outcome/1"`, `outcome: "completed"`. `sessions list` shows one completed turn. `sessions show latest` shows the prompt. |
| `scripts/drive-provider-failure` (retryable) | `features/provider-failure.md` | Injects `ProviderStreamEvent::Failed` with `ProviderErrorKind::RateLimited`. Turn ends failed, not retried into success. `yach sessions show latest --json` has one `provider_attempt` item (the inspector's view of the `ProviderAttemptFinished` event) with `error_kind` `rate_limited`. A scripted `Failed` event is not retried by the attempt layer (`docs/project/specs/2026-08-28-provider-attempt-reliability-design.md:77-81`), so this proves the terminal path, not the live retry ladder. |
| `scripts/drive-provider-failure` (non-retryable) | `features/provider-failure.md` | Injects `ProviderStreamEvent::Failed` with `ProviderErrorKind::Authentication`. Same shape, `error_kind` `authentication`. Outcome is failed, not a retry storm. |
| `scripts/drive-cancellation` | `features/cancellation.md` | Injects `ProviderStreamEvent::Cancelled`. The session's turn outcome is cancelled, not failed. Session attempt `outcome` is `cancelled`. `yach run`'s outcome JSON has no cancelled value and reports a provider cancellation as `failed` (`crates/yach-cli/src/headless.rs:931-934`), so the session inspector is the signal. |
| `scripts/drive-multi-file-edit` | `features/multi-file-edit.md` | Scripted tool rounds change two workspace files and run the workspace verifier. Two paths differ from the seed, verifier command exits 0, response text is not the pass signal. The drive's own workspace is the verify-yach scratch workspace, not the ylog dogfood workspace. |

The full pack is `doctor && launch && drive-prompt-reply &&
drive-tool-review && drive-resume && drive-headless &&
drive-provider-failure && drive-cancellation && drive-multi-file-edit`
(`SKILL.md:12-16`). All four previously missing coverage rows now exist,
implemented by the three new scripts under
`docs/project/plans/2026-10-20-m1-scripted-drives.md`.

Retry policy, for reading the live session later: at most three attempts,
delays of 1s then 2s, 30s cumulative cap; timeout, network, rate limit, and
provider-internal are retryable; the rest are not
(`docs/project/specs/2026-08-28-provider-attempt-reliability-design.md:173-191`).
That spec is implemented (status line at `:3`). `ProviderErrorKind` is the
closed set at `crates/yach-backend/src/provider.rs:143-157`.

Exhausted-script `InvalidRequest` ("scripted provider exhausted",
`bench_loop.rs:120-125`) stays the existing helper behavior. It is not a
separate named drive.

## Layer L — live cohort

Six scenarios. Cohort 1 runs `L1-fresh`, `L2-review`, `L3-autonomous`,
`L4-compact`, and `L6-resume` on both families (10 runs), and `L5-failure`
once per family (2 runs). Cancellation is not a scheduled live cell.
The person scores interaction quality and autonomy. Everything else is an
artifact check against the recorded task's acceptance assertions, including
the continuation edit in `L4-compact`.

Dogfood workspace: a dedicated jj workspace of ylog, created for the cohort,
never `/srv/agent-dev/projects/ylog` and never a yach checkout. Seed is a
clean ylog change, identified by its Git commit id. Every run starts from the
seed: before each scenario and family, the operator runs
`jj new <seed-commit-id>` in the dogfood workspace, so no run
inherits an earlier run's edits and `jj diff` shows only that run's changes.
Earlier runs' edits stay in their own changes as evidence. Runs that continue
a session on purpose start from that session's change instead
(`jj new <change>`): `L4-compact` when it continues the `L2-review` session,
and `L6-resume`.

| Id | Scenario | Families | Mode | Pass signal |
| --- | --- | --- | --- | --- |
| `L1-fresh` | Fresh start. New `YACH_SESSION_DIR`, no `--resume`. One prompt that completes a turn without editing the repo. | both | `review` | One new session file. `sessions show --json` has a completed turn. Workspace diff is empty. |
| `L2-review` | Multi-tool change of 2+ files, in review mode. The prompt names the two files and requires `uv run pytest`. | both | `review` (default) | At least one review decision gates an edit or shell tool. Two or more files differ. `uv run pytest` exits 0. A review rejection that the agent then respects is a pass on the gate even if the scenario needs a second approval to finish. |
| `L3-autonomous` | The `L2-review` change request, session-only `full-access`, without the sentence that asks for `uv run pytest`. | both | `full-access` | Same file artifact as `L2-review`; the operator runs `uv run pytest` after the turn, and it must exit 0. Manual score: the agent ran `uv run pytest` (or the project's tests) without being asked, and stopped rather than inventing a pass if the verifier failed. |
| `L4-compact` | After a real multi-tool turn (the `L2-review` session, or a fresh one if that session is unusable), `/compact`, then one continuation turn that must still satisfy the original instruction. | both | `review` | Session log contains a `CompactionCheckpoint`. The continuation turn completes; its diff and verifier output meet the continuation acceptance assertions recorded before compaction. "No compaction happened" is a fail. |
| `L5-failure` | One non-destructive provider failure: unknown model id. Do not pull a credential, revoke a live credential, inject a billing failure, or wait for a natural 429. | once per family | `review` | The harness surfaces `ProviderErrorKind` `unavailable_model` with `classification_source` recorded. Only that kind passes; any other classification is failed or blocked with evidence. Success is the expected classified failure, not a completed coding turn. |
| `L6-resume` | Quit the TUI, run `yach tui --resume`, and complete one more turn in the resumed session. `yach run` is not a substitute: it is headless and cannot show a restored transcript (`crates/yach-cli/src/headless.rs:1-4`). | both | `review` | Same session id before and after. The continuation turn completes. Transcript is restored. |

`L3-autonomous` uses `L2-review`'s change request with the verifier
instruction removed, so the mode and that one sentence are the only
intended differences; the operator records both prompt texts.
`L4-compact` may continue the `L2-review` session
when that session is healthy; if `L2-review` did not complete, `L4-compact`
starts a fresh multi-tool turn first, and the evidence note says so. A
failed `L2-review` does not get papered over by skipping compaction.

Before the initial multi-tool turn for `L4-compact`, the operator records
the original task text, the deferred continuation instruction, the expected
changed paths and observable edit, and the artifact assertions that will
check it (including `uv run pytest`). The continuation requirement must be
in the pre-compaction instruction, not invented after `/compact`. After
compaction the continuation prompt asks the agent to perform that deferred
edit; the recorded assertions judge its diff and verifier output. A fluent
claim of equivalent intent is not a pass. This is an artifact check, not an
additional subjective manual score.

### Modes

`ApprovalMode` is `review`, `accept-edits`, `auto-review`, `full-access`
(`crates/yach-proto/src/lib.rs:266-290`). Default is review.
`load_project_approval_mode` ignores a stored `full-access` or `auto-review`
(`crates/yach-backend/src/permission.rs:30-31`, `:41`).
`persist_project_approval_mode` refuses to store either
(`permission.rs:70-74`). "Autonomous" in the issue is session-only
`full-access`, matching the roadmap's advanced unsandboxed tier
(`docs/project/roadmap.md:70-72`, `:76-78`). Cohort 1 does not run
`accept-edits` or `auto-review`.

### Context pressure

Compaction is implemented. Checkpoints append, and the log is never
truncated (`crates/yach-backend/src/compaction.rs:5-7`). `/compact` is a
slash command (`crates/yach-ui/src/slash_commands.rs:80-83`).
`/responses/compact` exists for native OpenAI
(`crates/yach-backend/src/compaction.rs:895-927`). The 2026-07-20
design's "not yet implemented" status line
(`docs/project/specs/2026-07-20-context-compaction-design.md:5-6`) is stale
and is not the pass signal.

On ylog the automatic threshold may never trip. That is expected. The pass
signal is a `CompactionCheckpoint` plus a continuation turn whose diff and
verifier output meet the recorded pre-compaction acceptance assertions.
Treating "no compaction happened" as a pass fails the requirement.

### Provider failure

Live injection is an unknown model id, once per family. It is
non-destructive: no credential is removed, and a healthy run is not
cancelled to fill the cell.

The only passing error kind is `unavailable_model`, with
`classification_source` recorded. Another classified error is not an
equivalent pass: record failed if the attempted injection returned the wrong
kind, or blocked if a prerequisite prevented the injection, and preserve
the actual error and evidence. Apply S1/S2 when their definitions hold.

A scripted `Failed` round proves the terminal path only. Live retry behavior
is whatever the landed policy does
(`docs/project/specs/2026-08-28-provider-attempt-reliability-design.md:173-191`).
Cohort 1 does not try to force a retryable live 429.

**Known S2: unknown-model classification through the gateway.** Observed
2026-10-20 on the `4b95c442` binary: an unknown model id through
`omp-subscriptions` returns HTTP 404 with an `invalid_request_error` body
and the message `Unknown model: <id>`, on both `/v1/chat/completions` and
`/v1/messages`.

- **openai-compatible:** classified `invalid_request` from the status.
  There is no baked dialect for this provider by design
  (`crates/yach-catalog/src/lib.rs:2210-2221`;
  `docs/project/specs/2026-08-28-provider-attempt-reliability-design.md:34-36`),
  and a 404 without a dialect is `invalid_request`
  (`crates/yach-backend/src/error_dialect.rs:505-515`).
- **anthropic:** classified `invalid_request` from the typed dialect. The
  Anthropic parser maps `not_found_error` to `unavailable_model`, but the
  gateway sends `invalid_request_error` (`error_dialect.rs:216-237`).

As things stand, both `L5-failure` runs record `failed`. Under decision 9
that is an S2 found before the first run: the classification fix is
cohort-1 work, and both families' `L5-failure` are re-run after it. The fix
is not chosen here. A gateway-specific dialect and a rule for a 404 that
says `Unknown model` are both open; either must keep
`openai-compatible` from inferring a dialect from the model id.

### Resume gap

`features/resume.md:16` documents that a resumed process does not reattach
the scripted provider; `drive-resume` still shows that on 2026-10-20
(`backend: no provider configured`, reply `fixture response: second
question`). The scripted drive asserts the restored transcript
and the message count, not the second reply's text. A live resume scenario
that assumes the provider is still selected will fail for a product gap, not
a dogfood mistake. `L6-resume` records that gap if it is still true: the
operator may re-select the provider on resume, and the evidence note says
whether re-selection was required. The scenario does not paper over a drop
by calling it a pass without that note.

## Failure injections

Scripted, every re-run, once the drives exist:

- `ProviderStreamEvent::Failed` with `rate_limited` (retryable kind) and
  `authentication` (not retryable).
- `ProviderStreamEvent::Cancelled`.
- Exhausted script remains the existing `InvalidRequest` path at
  `bench_loop.rs:120-125`. It is not given its own drive.

Live, once per family per cohort:

- Unknown model id on `L5-failure`. Record a missing credential as
  `blocked`, matching the checkpoint rule at
  `docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md:24-25`,
  not as a completed failure scenario. Do not inject billing failures, do
  not revoke a live credential mid-turn, and do not depend on a naturally
  occurring 429.

Scenarios that require a successful provider round: `L1-fresh`, `L2-review`,
`L3-autonomous`, `L4-compact`, `L6-resume`. A classified provider failure
that prevents completion is S2 and must be fixed and re-run, even when its
failure class is `provider-invalid`; an S1 condition takes precedence.
`L5-failure` exists to observe `unavailable_model` specifically, not just any
classified error, and requires no successful coding turn. Until the known
S2 above is fixed, it records `failed` on both families.

## Automation / manual split

Automated, artifact-only:

- files changed (`jj diff` stat in the dogfood workspace);
- verifier command and exit code;
- session id stability across resume;
- `sessions show --json` contains the expected event kinds
  (`ProviderAttemptFinished`, `CompactionCheckpoint`, review decision);
- approval mode in the session log;
- compaction checkpoint present after `/compact`.
- continuation diff and verifier output against the recorded pre-compaction
  acceptance assertions for `L4-compact`.

Manual, person required:

- interaction quality: was the review understandable, did status text tell
  the truth;
- autonomy: did `full-access` act without stalling, and did it stay inside
  the task;
- resume: did the resumed screen show the prior turn's transcript.

Manual notes cite the session id and the screen-capture path. They do not
replace the artifact check. A fluent reply with a failing verifier is a
failed artifact check.

## Evidence format

One directory per scenario run, outside the repo. The versioned record
stores the summary and the classification. Raw capture stays outside the
repo. Screen captures under the verify-yach evidence dir are not a record;
cleanup keeps them only under `$XDG_STATE_HOME`
(`.agents/skills/verify-yach/SKILL.md:44-46`).

Each Layer L run records the fields below. A Layer S drive records a shorter
set: drive name, portfolio version, yach build, the drive's exit status,
the evidence directory it printed, and run result. For a failed drive, it
also records failure class, severity and finding note under the same rules.
Its provider is the scripted provider and its workspace is the verify-yach
scratch workspace, so the repo, family, model and verifier fields do not
apply.

| Field | Rule |
| --- | --- |
| Scenario id | `L1-fresh` … `L6-resume`. |
| Portfolio version | `2` until this record is revised and the version bumped. |
| Yach build | The Git commit id of the yach checkout the binary was built from, with a clean tree (`jj diff` empty in that workspace) at build time. A dirty build is not a cohort run: `evals/scripts/source-digest.sh` covers only crate sources and manifests, not `vendor/` or compile-time data files, so neither it nor a dirty flag pins every input. Record the digest alongside as a check. A jj change id may be shown for context but is not provenance: it survives rewrites while the tree changes (`docs/project/specs/2026-09-08-performance-measurement-framework-design.md:149-168`). Not "main". |
| Repo | `ylog`, plus the dedicated workspace path and the seed's Git commit id. The seed is never rewritten while the cohort uses it; a changed seed is a new seed with a new commit id. |
| Family | `openai-compatible` or `anthropic`. Not the model id. |
| Model id | The id actually requested, for example `openai-codex/gpt-6-sol` or `claude-sonnet-4-5`. |
| Approval mode | `review` or `full-access`. |
| Session inspector | `yach sessions show <id\|latest> --json` saved beside the run. `sessions list` and `sessions show` are the inspector (`crates/yach-cli/src/sessions.rs:18-22`; design at `docs/project/specs/2026-09-30-session-diagnostics-design.md:274-294`). |
| Verifier | Command (`uv run pytest`) and exit status. `n/a` only for scenarios that must not edit (`L1-fresh`, `L5-failure`). |
| Diff stat | `jj diff` stat of the dogfood workspace, not the yach checkout. |
| Capture dir name | Directory name only, when the claim is about a model decision and the transport captures bodies. Never the request body. Omitted when capture does not apply (anthropic family; scenarios whose claim is not a model decision). |
| Run result | `pass`, `failed`, `blocked`, or `cancelled`, with the observed task or provider outcome. Cancellation is not a provider failure. |
| Failure class | For each observed harness or provider failure, exactly one of `reproducible`, `intermittent-valid` (repeat count and recorded rule), or `provider-invalid`, chosen by the precedence under Failure classes. `n/a` when no failure occurred, a prerequisite blocked the run before any provider attempt, or the only failure is a task failure (see Failure classes). If evidence cannot support a class, record `unclassified` as a pending evidence state and S2, not as a fourth failure class. |
| Severity | S1, S2, S3, or not a blocker, independently of failure class. Record all findings, including completed-run S3 observations and non-blocking task failures. |
| Finding / blocker note | Explain every failed or blocked result, provider-invalid classification, S3 finding, and task failure; cite evidence and the unmet acceptance criterion. For a pass with no finding, `none`. |
| Manual assessment | Required for every Layer L run, one entry per check. Each check is `ok`, a finding, or `n/a` with the reason it could not occur in this run. Interaction quality has two checks. **Review understandable** applies when a review prompt was shown; it is `n/a` for `L1-fresh`, `L5-failure`, and an `L3-autonomous` run that showed none. **Status text truthful** applies to every Layer L run. `L3-autonomous` adds four autonomy checks. Three apply to every L3 run: ran `uv run pytest` (or the project's tests) without being asked, acted without stalling, and stayed inside the task. **Stopped rather than inventing a pass** applies only when the verifier failed during the turn; it is `n/a` otherwise. `L6-resume` adds **Transcript restored**: the resumed screen showed the prior turn's messages before the continuation prompt. A failure is a finding, and the `L6-resume` pass signal is unmet. Each entry cites the session id and the screen-capture path. A finding takes a severity under Blocker severity and is explained in the finding note. Apart from Transcript restored, it does not change the artifact result. `n/a` for Layer S drives. |
| Task acceptance | Original task text and artifact assertions; for `L4-compact`, also the deferred continuation instruction, expected edit and paths recorded before compaction, and the assertion results. |

`ProviderAttemptFinished` is the session event for a settled attempt
(`crates/yach-backend/src/session.rs:336-371`, written as
`SessionEvent::ProviderAttemptFinished` at `:544-548`). Fields include
purpose, sequence, retry index, outcome, error kind, classification source,
status, timeout phase, delays, timings, model, and optional capture name
(`docs/project/specs/2026-09-30-session-diagnostics-design.md:110-130`).

`YACH_CAPTURE_REQUESTS=<absolute dir>` writes the exact request body beside
the session, never into JSONL (`crates/yach-backend/src/recording_http.rs:3-6`;
`docs/project/specs/2026-09-30-session-diagnostics-design.md:248-266`). The
portfolio must not copy those files into `docs/project/records/`. The record
stores the capture file name from the attempt event, not the body.
Authentication headers are never written
(`README.md:440-443`); bodies still contain prompts and file contents, which is
why they stay outside the repo.

## Failure classes

Three failure classes, and no others. Run result and severity are separate
axes: `blocked` is not a failure class, and `provider-invalid` does not
exempt an S1 or S2 from the fix-and-re-run gate.

| Class | Meaning |
| --- | --- |
| `reproducible` | The failure recurs the same way on every classification repeat. An unexplained `variant` / `provider_internal` failure is a harness gap, not an excuse: it is `unclassified` and S2 until its declared repeats run, then `reproducible` or `intermittent-valid` by their outcome. |
| `intermittent-valid` | Under a recorded repeat rule, the failure did not recur on every classification repeat, including a failure that did not recur at all. The run author records the rule before repeating: how many repeats and what counts as the same failure, then records the actual repeat count and outcomes. A failure with no repeat is not this class. A portfolio-wide default repeat count remains unspecified; each case must declare its rule. |
| `provider-invalid` | The provider or credential failed and the harness surfaced a classified `ProviderErrorKind` with `classification_source` other than an unexplained `variant` / `provider_internal` pair (`docs/project/specs/2026-09-30-session-diagnostics-design.md:186-188`). |

A classification repeat reuses every input of the run it classifies: yach
build commit, repo and seed commit, scenario, exact task text and
assertions, family, model id and approval mode. A repeat that changes any of
them is a new run, not a repeat. A run after a fix is a re-run under the
Re-run gate, not a classification repeat, and does not reclassify the
original failure.

Precedence makes the class unique. A failure that meets the
`provider-invalid` definition takes that class whether or not it recurs, and
its repeat outcomes go in the finding note. `reproducible` and
`intermittent-valid` cover every other failure: harness failures, and
provider failures without an honest classification, such as an unexplained
`provider_internal`.

A classified provider failure on a scenario that required a successful
round is `provider-invalid` only when the harness surfaced it honestly.
If it prevents completion, it is also S2 (or S1 when an S1 definition
applies), and must be fixed and re-run. The class describes the evidence;
it does not make the scenario green or determine severity.

The classes describe harness and provider failures, not task results. A
task failure is a wrong change that the verifier or an artifact check
rejected visibly, with the harness surfacing every tool result and the
verifier's exit status honestly. It has failure class `n/a` and severity
not a blocker; the finding note records the task result and the assertion
it failed. If the harness hid or misreported the failure, it is a harness
failure and takes one of the three classes.

A prerequisite that blocks a run before a provider attempt is `blocked`,
failure class `n/a`, with the prerequisite and severity recorded. If an
attempt exists, classify its observed harness or provider failure from
evidence instead of using `n/a`. When evidence is missing, record the class
as `unclassified` pending evidence, assign S2, and fix and re-run; this is
not a fourth class and cannot satisfy the cohort gate.

## Blocker severity

| Severity | Definition | Cohort gate |
| --- | --- | --- |
| S1 | Cannot start, cannot resume, data loss, or a review decision did not gate the tool. | Fix, then re-run. |
| S2 | A required scenario cannot be completed (including a classified provider failure on a required-success scenario), or evidence is missing so the run cannot be classified. S1 conditions take precedence. | Fix, then re-run. |
| S3 | Scenario completed, but interaction was misleading or the agent needed a correction that a better harness would not have needed. | Record only. Does not block. |
| Not a blocker | The model produced a wrong patch and the verifier failed visibly. Or `provider-invalid` with the expected `unavailable_model` and recorded classification source on `L5-failure`. Any other L5 classification is failed or blocked, not this passing case. | Do not fix the model. Repeat the expected injection when checking its failure path; fix and re-run any S1/S2. |

A wrong model edit that the verifier catches is a task result, not a
blocker, unless the harness hid the failure. A review approval that was not
required before the tool ran is S1, even if the edit was correct.

Cohort 1 resolves every S1 and S2 from the first recorded run, then re-runs
those scenarios. S3 is recorded and does not block the cohort gate.

## Re-run gate

A portfolio re-run is:

1. the full scripted pack under "Exists today", including the drives from
   `docs/project/plans/2026-10-20-m1-scripted-drives.md`, and
2. every live scenario with S1 or S2, regardless of failure class or whether
   a prerequisite blocked the prior run, and every live scenario whose
   previous result was `reproducible` or `intermittent-valid`.

A re-run keeps every input of the original run except the yach build, which
is the build with the fix: repo and seed commit, scenario, exact task
text and assertions, family, model id and approval mode. A run that changes
any other input does not resolve the original blocker. A fix that needs a
different input, such as a credential or gateway setting outside yach,
records that input and why it changed.

`provider-invalid` is not a re-run exemption for S1/S2: fix the blocking
provider or prerequisite issue and re-run the required scenario. A passing
`L5-failure` (`unavailable_model` with classification source) is re-run when
the cohort is checking that injection path. An L5 run with any other
classification is failed or blocked with evidence; if S1/S2 applies it must
be fixed and re-run before the gate can close.

The cohort gate is met when that re-run has no open S1 or S2.

The eval portfolio's `just eval-gate`
(`docs/project/specs/2026-07-28-eval-portfolio-design.md:51-53`) is adjacent
evidence. A green eval gate does not close CEW-147.

## Where evidence lives

| What | Where |
| --- | --- |
| This portfolio (version, classes, pass signals, classification rules) | This file, in the repo. |
| Per-run summary and failure classification | A later record under `docs/project/records/`, citing scenario id, portfolio version, yach build commit, and session id. Summaries only. |
| `yach sessions show --json`, verifier logs, `jj diff` stat, screen captures | The run's capture directory, outside the repo. |
| Request bodies | `YACH_CAPTURE_REQUESTS` directory, outside the repo. Never copied here. The in-repo note stores the capture file name only. |
| Scripted drive screens | `$XDG_STATE_HOME/sjujperpowers/evidence/yach/<UTC>-<pid>/`. |

If a capture directory is deleted, the in-repo summary remains, and the run
cannot be re-classified from raw events. That is acceptable. It is not
acceptable to omit the summary fields above and point only at a path.

## Scope risks

Still in force after the owner decisions:

- **Dogfooding on a shared checkout.** Other agents build in
  `/srv/agent-dev/projects/yach` and sibling workspaces. Cohort 1 does not
  use that checkout, and it does not use `/srv/agent-dev/projects/ylog`.
  Even a later yach cohort must use a dedicated jj workspace and
  `flock ~/.local/state/yach-lanes/build.lock`.
- **Confusing `provider-invalid` with a pass.** The checkpoint recorded a
  missing API key as `blocked`
  (`docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md:34`).
  A classified auth failure is not a green scenario. `L1-fresh`,
  `L2-review`, `L3-autonomous`, `L4-compact`, and `L6-resume` require a
  successful provider round; a provider failure preventing completion is S2
  (S1 if it prevents start or resume). Only `L5-failure` is green when the
  error is `unavailable_model` with classification source recorded.
- **Resume drops the scripted provider.** `features/resume.md:16`. A live
  resume that assumes the provider is still selected fails for a product
  gap. Record the gap if it is still true; do not paper over it.
- **Context pressure on ylog may never trip the threshold.** "No compaction
  happened" is not a pass. The pass signal is a `CompactionCheckpoint` plus
  a faithful continuation.
- **Evidence that cannot be re-read.** Screen captures and tmux scrollback
  are not the record. The versioned record stores the `sessions show --json`
  summary and the classification, and names the capture directory. It does
  not depend on scrollback that cleanup does not preserve.
- **Secret capture.** `YACH_CAPTURE_REQUESTS` writes request bodies to a
  private directory. This record must not gain those files. Store the
  capture file name from the attempt event, not the body.
- **Anthropic gateway auth is a prerequisite, not a silent third family.**
  Adding anthropic is an owner decision, not an accident of credentials.
  The prerequisite, Kata `yach#q6rj`, landed on 2026-10-20. Do not
  substitute a second Codex model id and call it the second family.
- **Eval gate substituting for the cohort.** A green `just eval-gate`
  satisfies the eval-portfolio spec, not CEW-147.
- **Anthropic capture is absent by design.** Do not fail an anthropic run
  for a missing request-body file. Do not copy a body into git to fill the
  gap.
- **ylog schema coupling.** A two-file ylog change can fail because the
  fixture does not match yach's log, not because the harness mis-edited.
  The blocker note says which. A fixture mismatch that the verifier shows
  is a task result, not an S1, unless the harness hid the verifier failure.

## Follow-up, not this record

The four scripted coverage rows (retryable failure, non-retryable failure,
cancellation, two-file edit plus verifier) now exist under
`docs/project/plans/2026-10-20-m1-scripted-drives.md`. They no longer block
the first live run; run the full pack before the cohort.

Kata `yach#q6rj` (anthropic Bearer auth for the gateway) landed on
2026-10-20 (#331, `4b95c442`). It no longer blocks the anthropic half of the
live cohort. Both families can start once the full scripted pack passes.

Classifying the gateway's unknown-model response as `unavailable_model`
(the known S2 under Provider failure) is cohort-1 work. It does not block the
first live run, which records `L5-failure` as `failed` until the fix lands,
but the cohort gate cannot close without it.
