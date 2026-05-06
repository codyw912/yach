# First Non-fixture Native Tool Candidate Plan

Date: 2026-05-05
Status: planning recommendation; implementation not started
Related: `.project/phases/05-native-tools-resources-session-hardening.md`, `docs/plans/2026-05-05-001-plan-resource-config-root-policy.md`, `docs/plans/2026-05-05-002-plan-native-tool-lifecycle-permissions.md`, `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`

## Goal

Select the first non-fixture native tool candidate and define the narrowest implementation slice that exercises real yach-owned tool/resource/session machinery without crossing into unsafe provider-visible local data, file mutation, process execution, network access, or permission UI.

This plan does not approve implementation. It gives the owner a concrete candidate to approve or reject before code changes.

## Current baseline

`yach-backend` already has backend-internal primitives for the safe half of native tool usage:

- project resource roots canonicalize paths and reject traversal/symlink escapes;
- local-only text reads exist but are fixed to `NativeResourceProviderVisibility::Never`;
- native tool definitions, schema validation, permission policy, execution boundary, and fixture-only executor exist;
- provider tool calls can be translated into yach-owned pending tool requests;
- provisional native session tool records persist redacted summaries and outcomes;
- fixture continuation primitives can validate, execute, size-limit, and map provider-bound tool results without real provider SDK continuation.

The missing next step is a real, non-fixture tool definition that uses local project state while preserving the same trust boundaries.

## Options considered

### Recommended: `project_path_info`

`project_path_info` accepts a project-relative path and returns normalized metadata only:

- canonical relative path after root policy checks;
- entry kind: file, directory, or other;
- byte size for files;
- provider visibility marker fixed to `never`;
- optional redacted error kind for missing, escaped, non-UTF-8 path input, or IO failure.

It does not read file contents, write files, run commands, access network, inspect credentials, or mutate session state beyond provisional redacted tool records.

Why this is the best first non-fixture candidate:

- It exercises the already-built resource root/path policy with real filesystem metadata.
- It is useful before file-read/edit tools because an agent can verify target existence and size first.
- It creates session/tool evidence without exposing source contents.
- It keeps provider-visible local data off by default; even metadata is not sent to a provider until a later continuation integration is approved.

### Alternative: `project_text_preview`

This would read a capped UTF-8 snippet from a project file using existing local-only read helpers. It would prove more resource behavior, but it exposes file contents and should wait until provider-visible resource policy, redaction expectations, and user consent are sharper.

### Alternative: `session_turn_summary`

This would summarize existing native session metadata without touching project files. It is lower risk, but it does not advance the local resource/tool trust boundary that Phase 5 needs.

### Rejected for first slice: command/process/network tools

Shell, process, network, file-write, and edit tools are out of scope. They need explicit permission UX/policy, stronger sandboxing decisions, and destructive-action audit rules before implementation.

## Recommended implementation slice, if approved

Add backend-internal support only:

- Define a non-fixture built-in tool definition named `project_path_info`.
- Add a new risk class for read-only project metadata, separate from fixture-safe.
- Validate arguments with a strict object schema: required `path` string, no unexpected fields, size cap inherited from tool validation policy.
- Execute through the existing `NativeToolExecutor` boundary using `NativeResourceRoot` path resolution and filesystem metadata APIs.
- Return structured JSON content capped by the existing provider-bound result limits, even though no provider submission is added in this slice.
- Record provisional native session tool request/execution records using existing redacted `NativeToolPayloadSummary` values.
- Add unit tests for success, missing path, traversal escape, symlink escape, directory metadata, oversized arguments, permission denied by default, and explicit allow policy.

No `yach-proto`, `yach-ui`, `yach-cli`, real provider SDK, live provider call, native-provider runtime loop, or permission UI change is needed for the first implementation.

## Permission and data policy

Initial policy should remain deny-by-default. Tests and later explicit dogfood wiring may allow exactly `project_path_info` under a named policy. The tool result should be classified as local metadata, not file content.

Provider visibility remains `never` for the first implementation. A later approval may decide whether path existence/type/size can be sent to a provider as tool-result continuation data.

Session persistence should store only:

- tool request id and yach tool name;
- provider call id metadata when present;
- validation/permission/execution outcome;
- path argument summary, not raw unbounded JSON;
- result byte counts and compact metadata summary.

Absolute host paths, raw provider payloads, credentials, and file contents should not be persisted.

## Protocol and UI impact

None for the first backend-only implementation. Protocol/UI work becomes relevant only if native dogfood exposes live tool progress, user approval/denial, or transcript-visible tool result rendering.

## Validation, if implemented

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```

Optional broader confidence after integration with `yach-cli` or native dogfood:

```bash
just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings
just dev cargo test -p yach-backend -p yach-cli
```

## Stop gates

Stop before:

- implementing without owner approval;
- sending `project_path_info` results to a real provider;
- integrating tool execution into `--backend native-provider`;
- exposing local file contents, absolute paths, credentials, command output, or network data;
- adding file mutation, process execution, network access, or edit tools;
- adding protocol approval UI;
- declaring native JSONL stable.

## Recommendation

Ask for owner approval to implement the backend-only `project_path_info` tool skeleton next. It is the smallest non-fixture tool that exercises real resource policy and native session evidence while keeping provider-visible local data and destructive behavior out of scope.
