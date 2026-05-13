# Native Edit Transactions Design

Date: 2026-05-13
Status: proposed

## Context

Native yach now owns sessions, project resource roots, read-only project
inspection, provider tool continuation, tool advertising, extension-owned
metadata tools, and static context assembly. The next Native MVP gap is local
file mutation: yach needs to safely edit existing files and create new files
without delegating mutation semantics to providers, adapters, or extensions.

The accepted Native MVP definition requires patch/edit transactions rather than
unstructured writes. The current read-only primitives already enforce
project-relative paths, root escape prevention, bounded text reads, local-only
resource visibility, shaped tool results, redacted session evidence, and
append-only JSONL persistence. Edit transactions should extend those seams
instead of inventing a parallel path.

This design deliberately does not make arbitrary file mutation provider-visible
by default. File edits are higher risk than metadata reads: they change the
workspace, can corrupt work if partially applied, and need durable evidence.
The first implementation should land a yach-owned transaction engine and
session evidence model. Provider-advertised edit tools, extension-owned edit
tools, approval UI, shell/process tools, delete, rename, and verification
actions remain separate follow-up designs.

## Goal

Design a native edit transaction primitive that can modify existing UTF-8 text
files and create new UTF-8 text files through reviewable, policy-governed
transactions with durable evidence, race checks, no known partial mutation for
the supported first-slice shape, and benchmarkable timing.

The design must leave room for the extension ecosystem. Extensions will
eventually need to contribute tools, including mutation-capable tools in some
form, but they should plug into the same yach-owned transaction boundary rather
than receiving arbitrary write access or bypassing evidence and policy.

## Non-Goals

- No implementation in this slice.
- No provider-advertised edit tool in the first implementation.
- No extension-owned mutation tools.
- No interactive approval UI.
- No shell/process execution.
- No delete, rename, chmod, symlink, binary edit, or directory tree mutation.
- No network access.
- No provider-native tool result block support.
- No raw provider payload persistence.
- No broad config language for edit policy.
- No automatic verification command execution after edits.

## Terminology

- **Edit transaction:** one yach-owned request to apply one or more file
  operations. The first implementation permits one operation per transaction.
- **Edit operation:** one intended file change inside a transaction. The first
  implementation supports `modify_text_file` and `create_text_file`.
- **Patch hunk:** a reviewable text replacement against an existing file,
  expressed with expected old text and replacement new text.
- **Create operation:** a transaction operation that creates one new text file
  if it does not already exist.
- **Transaction preview:** a validated, reviewable diff and summary produced
  before filesystem mutation.
- **Applied result:** the durable record of what yach actually changed.
- **Edit evidence:** append-only session records that summarize intended and
  applied changes without storing unbounded full file contents.

## Approach Options

### Option A: Provider edit tool first

Yach could immediately advertise an `edit` or `write` tool to native providers
and rely on the existing one-round tool loop to execute edits.

This is too much risk for the next slice. Provider-visible mutation requires a
separate approval and policy design, plus careful continuation semantics. If
the transaction engine is wrong, provider integration will make failures harder
to isolate.

### Option B: Transaction engine first

Yach implements a backend-native edit transaction module with path validation,
patch application, guarded write behavior, summaries, session evidence, and
benchmarks. It can be exercised from tests and later from a local harness path,
but it is not provider-advertised yet.

This is the recommended path. It satisfies the Native MVP architecture
requirement that yach owns mutation semantics and creates a stable primitive
for future provider tools, extension tools, approval UI, and verification
actions.

### Option C: Full-file write first

Yach could start with `write_file(path, content)` and add patch support later.

This is simpler, but it loses the core property the MVP calls for:
reviewable patch-like transactions. Full-file writes make accidental large
rewrites easier, increase provider payload sizes, and produce less useful
evidence. They should only appear as the create-file payload or as a later
explicit overwrite operation with stricter policy.

## Harness Reference Point

Other coding harnesses commonly expose file tools such as `read`, `edit`,
`write`, `grep`, `find`, and `ls`. That is the right general UX family for
yach, but this design keeps the native backend primitive narrower than the
eventual user-facing tool list.

For yach, `edit` and `write` should not be two unrelated low-level filesystem
capabilities. They should compile into the same edit transaction model:
`edit` becomes a modify operation, while `write` for a new file becomes a
create operation. Future compatibility or extension surfaces can choose names
that feel familiar, but the backend should keep one policy and evidence model
for local file mutation.

## Recommended Shape

Create a backend module, likely `crates/yach-backend/src/edit.rs`, that owns
edit transaction validation and application.

The first transaction model should support:

1. one project root per transaction;
2. one project-relative file operation per transaction in the first
   implementation, with the data model able to grow to multiple operations
   later;
3. existing UTF-8 text file modifications through ordered exact-match hunks;
4. new UTF-8 text file creation through the same transaction envelope;
5. root escape, absolute path, symlink escape, and existing-path checks;
6. per-file and per-transaction byte limits;
7. preview generation before write;
8. no partial mutation for the supported single-operation transaction shape;
9. redacted append-only session evidence;
10. edit timing metrics and Criterion benchmark coverage.

The patch format should be structured, not a raw provider-supplied shell patch.
For the first implementation, use exact-match text hunks:

```text
path: crates/example/src/lib.rs
operation: modify_text_file
hunk:
  find: "old text"
  replace: "new text"
```

Exact-match hunks are patch-like, reviewable, and easier to validate than full
unified diff parsing. They fail safely if the expected text is not present or
is ambiguous. A later provider-facing tool can expose a unified-diff-like
surface and compile it into the same internal hunk model, but the backend
primitive should start with deterministic structured operations.

## Transaction Model

The core types should be yach-owned and provider-neutral:

```rust
pub struct NativeEditTransactionRequest {
    pub operations: Vec<NativeEditOperation>,
}

pub struct PreparedNativeEditTransaction {
    pub transaction_id: NativeEditTransactionId,
    pub operations: Vec<PreparedNativeEditOperation>,
}

pub enum NativeEditOperation {
    ModifyTextFile {
        path: String,
        expected_sha256: String,
        hunks: Vec<NativeEditHunk>,
    },
    CreateTextFile {
        path: String,
        content: String,
    },
}

pub struct NativeEditHunk {
    pub find: String,
    pub replace: String,
}
```

Callers provide operations; yach mints `NativeEditTransactionId` when preparing
or accepting the transaction. Provider or extension supplied IDs must not become
durable authority because edit IDs are correlation evidence owned by yach.

Modify operations must include `expected_sha256` for the full current file
bytes. Yach validates that hash during preview/preparation and validates it
again immediately before replacing the target. This makes stale edits fail
closed if a user, tool, formatter, or extension changes the file between
preview and apply.

Even though the model uses `Vec<NativeEditOperation>`, first-slice policy should
set `max_operations = 1`. Multi-operation transactions are a follow-up once yach
has an explicit rollback or journal story. Keeping the envelope general avoids
renaming the primitive later while avoiding false confidence about multi-file
atomicity now.

Transaction preview should produce:

- transaction id;
- per-operation relative path;
- operation kind;
- before/after byte counts;
- before/after content hashes when applicable;
- rendered unified-diff-style preview for review;
- estimated total changed bytes;
- validation warnings or errors.

Transaction apply should produce:

- `completed`, `validation_failed`, `permission_denied`, or `failed`;
- per-operation applied status;
- before/after hashes;
- bytes read and written;
- rendered diff summary;
- categorical failure reason.

The minimum engine boundary for the first implementation should look like:

```rust
pub struct NativeEditEngine;

impl NativeEditEngine {
    pub fn preview(
        root: &NativeResourceRoot,
        request: NativeEditTransactionRequest,
        policy: &NativeEditPolicy,
    ) -> Result<PreparedNativeEditTransaction, NativeEditError>;

    pub fn apply(
        root: &NativeResourceRoot,
        transaction: PreparedNativeEditTransaction,
        policy: &NativeEditPolicy,
    ) -> Result<NativeEditApplyResult, NativeEditError>;
}
```

`preview` is responsible for path validation, policy validation, hunk
application in memory, diff summary creation, and yach-owned transaction ID
assignment. `apply` is responsible for final race guards, filesystem mutation,
and applied-result evidence. The implementation plan can adjust names, but it
should keep this separation between reviewable preparation and guarded apply.

## Hunk Semantics

Hunks should be exact byte-string replacements over UTF-8 text. The first
implementation should use deterministic sequential semantics:

- at least one hunk is required for `modify_text_file`;
- empty `find` text is rejected;
- hunks are applied in caller-provided order against the progressively modified
  in-memory text;
- each hunk's `find` text must appear exactly once at the time that hunk is
  applied;
- zero matches produce `hunk_not_found`;
- more than one match produces `hunk_ambiguous`;
- replacement text may be empty;
- if an earlier hunk removes or changes text needed by a later hunk, the later
  hunk fails normally under the same exact-match rule;
- validation computes the full after-image in memory before any filesystem
  mutation.

This is intentionally stricter than editor-style fuzzy patching. Fuzzy matching,
line-number anchors, and unified-diff parsing can be compiled into this model
later only if they preserve fail-closed behavior.

## Path And File Policy

Edit path policy should extend `NativeResourceRoot` rather than bypass it.
Existing resource resolution only works for existing final paths because it
canonicalizes the whole target. Create operations need a new helper that
validates the canonical parent directory plus a normalized final file name.

Rules:

- all operation paths must be project-relative;
- absolute paths are rejected;
- `..` traversal is rejected before filesystem access;
- final resolved paths must remain under the canonical project root;
- symlinked parent directories that escape the root are rejected;
- modifying symlinked files is rejected in the first implementation;
- creating through a symlinked parent is rejected;
- create target must not already exist;
- modify target must exist and be a regular UTF-8 text file;
- paths whose first component is `.git` are rejected;
- paths under `.yach/native-sessions` are rejected;
- paths whose first component is `target` are rejected by default unless a
  later policy explicitly allows generated/build output edits.

The first implementation should not try to infer ignore files or VCS state.
Yach can add `.gitignore` or VCS-aware policy later if dogfooding shows a need.

## Guarded Apply And Failure Behavior

The transaction engine should validate every operation before writing anything.
Validation includes path policy, existence/type checks, UTF-8 checks,
per-file/per-transaction size limits, exact hunk match checks, ambiguous match
checks, and duplicate-target checks.

If validation fails, no files are written.

Application should prevent known partial mutation for the supported
single-operation transaction shape. A practical first implementation can:

1. read and validate all affected files;
2. compute all after-images in memory;
3. for modify operations, re-read and re-hash the target immediately before
   replacement, failing with `hash_mismatch` if it changed;
4. write the after-image to a temporary file in the target directory;
5. preserve the target file's permissions for modify operations unless a later
   design explicitly changes metadata behavior;
6. fsync best-effort where available;
7. replace the target with the temporary file.

Create operations must use no-overwrite publish semantics. The implementation
should either use a temp-file persist operation that fails if the target exists,
or an equivalent platform-specific primitive. A concurrent create between
validation and publish must fail with `target_exists`, not overwrite the other
writer's file.

Temp-file replacement can affect ownership, ACLs, extended attributes, and
timestamps on some platforms. The first implementation should preserve ordinary
Unix permissions for modify operations and record that richer metadata
preservation is not guaranteed. If dogfooding shows this is too weak, richer
metadata preservation should be designed explicitly before provider-visible
edits.

If replacement fails after the final guard, yach must record the failure with
the exact relative path and whether the target content hash changed. Full
crash-safe replacement is not a first-slice requirement, but no known partial
mutation should be hidden.

Because full crash-safe multi-file replacement is not a first-slice
requirement, `max_operations = 1` is the recommended initial policy. This still
supports meaningful create and modify workflows, creates the evidence model,
and makes benchmark results easier to interpret. Multi-operation transactions
should become accepted only after a later design covers rollback, journaling, or
another explicit strategy for partial replacement failures.

Create operations should create parent directories only if a later design
explicitly allows directory creation. For the first implementation, parent
directories must already exist.

## Policy And Permission

Edit transactions are `NativeToolRisk::MutatesLocalState` when surfaced through
the tool system.

The edit engine should have its own `NativeEditPolicy` for operation count,
byte limits, path deny rules, create/modify enablement, and diff truncation.
The native tool policy should continue to deny `MutatesLocalState` by default.
If a hidden/local built-in tool is added, it should require an explicit
test/local harness policy that delegates to `NativeEditPolicy`; normal provider
tool execution must remain denied. It should not be provider-advertised through
`yach.provider_tool_advertising.v1` yet.

Initial policy defaults should be conservative and expressed in bytes, not
characters:

```rust
pub struct NativeEditPolicy {
    pub max_operations: usize,          // default: 1
    pub max_file_bytes: u64,            // default: 256 * 1024
    pub max_transaction_bytes: usize,   // default: 128 * 1024
    pub max_diff_summary_bytes: usize,  // default: 32 * 1024
    pub allow_create: bool,             // default: true
    pub allow_modify: bool,             // default: true
}
```

`max_file_bytes` applies to the full before-image and after-image for each
edited file. `max_transaction_bytes` applies to the serialized request payload
accepted by the edit engine. `max_diff_summary_bytes` applies to persisted
preview/evidence diff text after rendering; larger diffs are truncated with
explicit evidence. These numbers can be tuned during implementation, but the
tests should assert that the limits are enforced by category rather than rely
on exact default values.

Future provider exposure should require a separate design for:

- tool schema;
- model-facing result shape;
- explicit allow policy;
- approval or confirmation UX;
- one-round versus multi-round continuation behavior;
- provider-visible diff size limits;
- cancellation semantics.

This preserves the existing boundary: providers may request actions, but yach
owns validation, permission, execution, evidence, and continuation.

Extension exposure should follow the same rule. An extension may eventually
register an edit-like tool, but the actual file mutation should be routed
through this backend transaction primitive or an equivalent yach-owned
capability boundary. Extension manifests can describe intent and schema; they
should not silently grant direct workspace write privileges during startup.
This keeps extension flexibility while preserving startup performance, policy
checks, and durable local-effect evidence.

## Session Evidence

Generic tool records are not quite enough for file mutation. They show that a
tool ran, but edit transactions need durable, inspectable local-effect
evidence. Add edit-specific session events or tool-result summaries that
capture:

- transaction id;
- turn id and optional tool request id;
- operation count;
- relative paths;
- operation kinds;
- before/after byte counts;
- before/after hashes;
- diff summary bytes;
- outcome;
- failure category;
- whether full content was redacted or truncated.

Do not persist full file bodies by default. Diff previews may be persisted only
under a strict byte limit and should be truncated with explicit evidence when
too large. Absolute paths and raw provider arguments should not be persisted.

Useful event shape:

```rust
NativeSessionEvent::EditTransactionPrepared { ... }
NativeSessionEvent::EditTransactionFinished { ... }
```

If the implementation plan prefers fewer event variants, it can encode the
same information in `ToolRequestRecorded` and `ToolExecutionFinished` summaries
for the first slice, but the design should reserve explicit edit events because
file mutation is a first-class MVP primitive.

## Provider And Tool Integration

The first implementation should not advertise an edit tool to providers.

It should still design toward eventual tool integration:

- built-in owner: `NativeToolOwner::BuiltIn`;
- risk: `MutatesLocalState`;
- provider visibility: hidden by default;
- executor: a `NativeEditTransactionExecutor` behind the existing
  `NativeToolExecutor` seam or a sibling backend seam that can later be wrapped
  as a tool executor;
- result shape: concise transaction status plus relative paths, byte counts,
  hashes, and truncated diff summary.

Provider adapters must remain projection-only. They should not parse patches,
write files, decide permissions, or record edit evidence.

## Errors

Use categorical errors suitable for session evidence and provider-facing
summaries later:

- `path_outside_root`;
- `absolute_path`;
- `parent_missing`;
- `target_missing`;
- `target_exists`;
- `expected_file`;
- `unsupported_file_type`;
- `not_utf8`;
- `file_too_large`;
- `transaction_too_large`;
- `duplicate_target`;
- `hunk_not_found`;
- `hunk_ambiguous`;
- `hash_mismatch`;
- `permission_denied`;
- `io`;
- `metadata_preservation_unsupported`;
- `partial_apply`.

Failures should include a relative path when safe. They should not include
absolute paths or full local file contents.

## Benchmarking And Metrics

Add Criterion coverage for:

- validating a no-op-sized transaction;
- modifying one small file;
- creating one small file;
- rejecting a hunk mismatch;
- rejecting a path escape.

Multi-file benchmark cases should be added only after multi-operation
transactions are implemented.

Runtime metrics should use low-cardinality names and attributes, such as:

- `edit_transaction_validate`;
- `edit_transaction_apply`;
- attributes: operation count bucket, outcome, changed bytes bucket.

Metrics should not include file paths or diff contents.

## Test Coverage

Implementation should include focused tests for:

- path traversal rejection;
- symlink escape rejection;
- create parent missing;
- create target exists;
- modify target missing;
- non-UTF-8 rejection;
- file too large;
- transaction too large;
- `.git`, `.yach/native-sessions`, and root `target` deny rules;
- exact hunk apply;
- zero-hunk rejection;
- empty-`find` rejection;
- hunk not found;
- ambiguous hunk;
- hash mismatch before apply;
- concurrent create no-overwrite behavior;
- duplicate target rejection;
- policy rejection for more than one operation in the first implementation;
- no partial mutation on failed validation;
- ordinary permission preservation for modify operations;
- redacted session evidence;
- append-only JSONL round trip for edit evidence;
- benchmark target compiles and runs.

## Implementation Slices

Recommended follow-up implementation sequence:

1. Add edit model, policy, path validation, preview API, error categories, and
   preview-only tests. Done means `NativeEditEngine::preview` can accept create
   and modify requests, reject invalid policy/path/hunk cases, mint an edit ID,
   and return a bounded diff summary without writing files.
2. Add single-operation apply for `modify_text_file` and `create_text_file`,
   including mandatory modify hash guards, no-overwrite create publish,
   temp-file replacement, metadata expectations, and
   no-write-on-validation-failure tests.
3. Add edit-specific session evidence and JSONL round-trip coverage.
4. Add a local harness or hidden built-in tool path that is not
   provider-advertised and remains denied by default in normal tool policy.
5. Add Criterion benchmarks for validate, apply, create, mismatch, and path
   rejection.
6. Write the separate provider-visible edit tool and approval design only after
   the backend primitive is proven locally.

## Acceptance Criteria

- A native backend module can validate and apply patch-like text edit
  transactions for existing files and new files, with first-slice policy
  limited to one operation per transaction.
- Mutations are project-root scoped and deny root escapes, symlink escapes,
  unsafe metadata paths, and unsupported file types.
- Failed validation does not write any files.
- Modify apply fails if the current file hash no longer matches the prepared
  transaction hash.
- Create apply does not overwrite a file created concurrently.
- Applied transactions produce durable evidence with relative paths, hashes,
  byte counts, diff summaries, and categorical outcomes.
- Full file bodies are not persisted in session logs by default.
- Provider adapters remain free of mutation logic.
- Extension-owned future mutation tools have a clear path through yach-owned
  transaction semantics rather than direct writes.
- Edit transaction validation/application is benchmarked.
- Project docs are updated to mark this design as the accepted basis for the
  next implementation plan once reviewed.

## Open Questions

1. Should the first local harness path expose edit transactions through a CLI
   smoke command, a backend-only test helper, or the native tool workflow with a
   non-provider-visible built-in tool?
2. Should parent directory creation be deferred completely, or allowed only for
   create operations behind explicit policy?
3. Should the future provider-facing patch surface be structured hunks, a
   unified-diff parser, or both compiled into the same internal operation
   model?
4. Do we need to preserve ACLs, xattrs, and platform-specific metadata before
   provider-visible edits, or is ordinary permission preservation enough for
   the native MVP?
