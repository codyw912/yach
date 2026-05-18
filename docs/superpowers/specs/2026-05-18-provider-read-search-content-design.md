# Provider-Visible Read/Search Content Design

Date: 2026-05-18
Status: proposed

## Context

Yach now has a narrow provider-visible agent edit surface:

- `edit_text_file` and `create_text_file` are advertised only through explicit
  native-provider policy.
- Provider-originated edits route through yach-owned validation, permission,
  preview, review, apply/reject, redacted evidence, and production edit
  tracing.
- The provider can inspect path metadata through `project_path_info`.
- Local backend primitives already exist for explicit text reads, bounded text
  search, project path metadata, and local-only context packages.

The gap is practical edit usefulness. Exact replacement edits assume the model
already knows the text to replace. That can work for user-supplied snippets or
static context, but a coding harness needs safe ways for the agent to inspect
target files before editing.

Comparable harnesses suggest the same direction. Pi exposes familiar built-ins
such as `read`, `grep`, `find`, `ls`, `edit`, `write`, and `bash`. OpenCode
separates read, grep/glob/list, and edit/write/patch permission families. Yach
should preserve the familiar read/search UX where it helps agents, but keep
resource policy, result shaping, session evidence, and provider continuation in
the Rust core.

References:

- Pi usage docs: <https://pi.dev/docs/latest/usage>
- OpenCode tools: <https://open-code.ai/en/docs/tools>
- OpenCode permissions: <https://opencode.ai/docs/permissions/>

## Goal

Design the first provider-visible local content tools for native yach so agents
can read and search project files before using the existing exact/create edit
tools.

The design should preserve:

- project-root-only resource access;
- explicit provider-visible content policy;
- bounded result size and match count;
- redacted durable session evidence;
- yach-owned execution and continuation;
- extension compatibility;
- no extra TUI startup work.

## Non-Goals

- No implementation in this slice.
- No file mutation tools beyond the existing `edit_text_file` and
  `create_text_file`.
- No broad `write`, patch, delete, rename, move, chmod, binary edit, or
  multi-operation atomicity.
- No shell/process, network, web fetch, LSP, or MCP tools.
- No extension-owned content tools yet.
- No content indexing service or background crawler.
- No multi-round autonomous provider loop.
- No new TUI review UI.
- No raw provider argument persistence, absolute path exposure, full directory
  dumps, binary file exposure, or unbounded file bodies.

## Design Principles

### Familiar Tool Names, Yach-Owned Enforcement

The provider-facing tool names should be easy for coding agents to understand:
`read_text_file`, `search_project`, and `list_project_paths` are explicit and
map cleanly to existing yach resource primitives.

These tools are not generic filesystem handles. They are yach-owned operations
that:

1. validate schema through the native tool registry;
2. authorize through explicit provider-visible content policy;
3. resolve paths through `NativeResourceRoot`;
4. execute with bounded read/search/list policies;
5. shape compact provider results;
6. record redacted tool evidence;
7. continue through the existing provider tool-result path.

### Content Is A Distinct Read Risk

`project_path_info` is metadata-only and should stay classified as
`ReadsLocalMetadata`. File bodies, matching lines, and path listings reveal more
local project data, so the new tools should use `NativeToolRisk::ReadsLocalContent`.

The permission policy should keep metadata and content separate. Enabling
`project_path_info` should not automatically enable file contents or search
snippets. Enabling content tools should still be narrower than mutation,
process, or network authority.

### Provider Results Are Bounded Context, Not Session Evidence

The provider needs useful content to act on, so `read_text_file` may return file
text and `search_project` may return matching lines. Durable session evidence
should not persist those bodies. Session logs should record summaries:

- tool name;
- provider call ID;
- argument byte count with redacted summary;
- relative path or query classification only if already safe and bounded;
- result byte count;
- match count or listed path count;
- truncation flags;
- categorical failure reason.

Provider-visible content is a runtime result, not canonical session evidence.

## Approach Options

### Option A: Expose Existing Local Context Packages Directly

Yach already has `NativeResourceContextPackage`, so one option is a single tool
that accepts several paths and returns a bundled context package.

This is compact, but it hides user intent. A model choosing to read one file,
search for a symbol, or list a directory should use distinct tools with
distinct validation, bounds, and evidence. A bundle tool can come later if
provider/tool call overhead becomes the bottleneck.

### Option B: Mirror Pi Names Exactly

Expose `read`, `grep`, `find`, and `ls` as the initial read/search surface.

This is familiar, but the names are ambiguous inside yach's typed risk model.
`find` can mean file-name search, filesystem traversal, or shell-like
predicate execution. `grep` implies regex details and flags that yach has not
designed yet. `ls` can become an unbounded directory dump if copied literally.

### Option C: Yach-Native Content Tools With Familiar Semantics

Add `read_text_file`, `search_project`, and `list_project_paths` as canonical
built-ins. They map to the familiar read/grep/find/ls family while keeping
yach's semantics explicit, bounded, and project-root scoped.

This is the recommended option. It gives agents the minimum content acquisition
surface needed for exact edits without importing shell semantics or broad
filesystem behavior.

## Recommended Tool Set

### `read_text_file`

Reads one UTF-8 project file.

Schema:

```json
{
  "path": "crates/example/src/lib.rs"
}
```

Execution:

- resolve `path` through `NativeResourceRoot`;
- require an existing regular file;
- reject absolute paths, parent traversal, symlink escapes, directories,
  missing paths, oversized files, and non-UTF-8 content;
- use a provider-content read policy, initially smaller than local-only reads;
- return bounded content and metadata.

Provider result shape:

```json
{
  "outcome": "read",
  "path": "crates/example/src/lib.rs",
  "text": "bounded file text",
  "byte_count": 1234,
  "truncated": false
}
```

The result should be marked provider-visible and may contain file text. The
session event summary must not persist `text`.

### `search_project`

Searches project text files for a literal query.

Schema:

```json
{
  "query": "NativeEditTraceId"
}
```

The first version should be literal substring search, not regex. Regex, case
sensitivity controls, path filters, and glob syntax can be added later after
their safety and performance behavior is explicit.

Execution:

- traverse from the project root using existing generated/heavy directory skips;
- skip oversized and non-UTF-8 files;
- enforce maximum scanned files and maximum matches;
- return stable project-relative path order;
- include line numbers and bounded line snippets;
- report truncation when limits stop traversal or match collection.

Provider result shape:

```json
{
  "outcome": "search",
  "query": "NativeEditTraceId",
  "matches": [
    {
      "path": "crates/yach-backend/src/session.rs",
      "line_number": 42,
      "line": "pub struct NativeEditTraceId(pub String);"
    }
  ],
  "searched_files": 120,
  "truncated": false
}
```

The session event summary should persist counts and truncation only, not matched
line bodies.

### `list_project_paths`

Lists immediate entries or bounded recursive paths under one project-relative
directory.

Schema:

```json
{
  "path": "crates/yach-backend/src"
}
```

The first implementation should list one directory level only. A later
`recursive` boolean or depth field can be added after path-count bounds and UX
are proven.

Execution:

- resolve `path` through `NativeResourceRoot::resolve_directory`;
- reject missing paths, files, absolute paths, parent traversal, and symlink
  escapes;
- skip generated/heavy entries consistently with search;
- return sorted project-relative entries with kind and optional byte size;
- enforce a maximum entry count and report truncation.

Provider result shape:

```json
{
  "outcome": "list",
  "path": "crates/yach-backend/src",
  "entries": [
    { "path": "crates/yach-backend/src/lib.rs", "kind": "file", "byte_size": 1000 },
    { "path": "crates/yach-backend/src/native_runner.rs", "kind": "file", "byte_size": 2000 }
  ],
  "truncated": false
}
```

Session evidence should persist entry count and truncation only.

## Policy And Registry Shape

Add canonical built-in definitions:

```rust
NativeToolDefinition::read_text_file()
NativeToolDefinition::search_project()
NativeToolDefinition::list_project_paths()
```

Each should use:

```rust
risk: NativeToolRisk::ReadsLocalContent
owner: NativeToolOwner::BuiltIn
provider_visibility: ProviderToolVisibility::Visible
```

Extend `NativeToolPermissionPolicy` with a content allowlist separate from
metadata and mutation:

```rust
content_advertising: BTreeSet<String>
```

Add constructors shaped like:

```rust
NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
    metadata_names,
    content_names,
    edit_names,
)
```

`allows_provider_advertising` should allow `ReadsLocalContent` only when the
tool name is in `content_advertising`. `authorize` should likewise permit
content execution only through this explicit set.

Provider advertising should fail closed for forged content tool definitions.
Canonical built-ins may be projected only when name, risk, description, and
schema match the core definitions. Extension-owned content tools remain out of
scope until extension content risk and execution policy are designed.

## Execution Boundary

`ProjectReadOnlyToolExecutor` should become the executor for both metadata and
content read-only tools. It can dispatch internally by canonical tool name:

- `project_path_info` -> existing metadata output;
- `read_text_file` -> provider-content read helper;
- `search_project` -> provider-content search helper;
- `list_project_paths` -> provider-content list helper.

If the executor receives a tool with unsupported risk, unknown owner, denied
permission, malformed arguments, or unavailable root, it must fail closed with
existing categorical tool execution errors or a small new categorical error.

Do not introduce provider adapter execution. Rig or any future provider adapter
should continue to receive schema-only tool definitions, emit tool-call events,
and rely on yach to validate and execute.

## Native Provider Flow

The explicit native-provider agent tool path should advertise:

- `project_path_info`
- `read_text_file`
- `search_project`
- `list_project_paths`
- `edit_text_file`
- `create_text_file`

only when the current policy enables the corresponding risk classes and the
runtime has a project root.

The one-round continuation boundary should remain unchanged:

1. initial provider request includes schema-only tool advertising;
2. provider emits zero or more completed tool calls;
3. yach validates, authorizes, executes, records evidence, and shapes results;
4. yach sends one continuation request with tool results;
5. continuation requests strip tool advertising;
6. second-round tool calls still fail closed.

This means read/search/list and edit calls can appear in the same first tool
round if the provider emits them. The implementation should preserve the
existing max tool call count and result byte limits. If a content tool result is
too large, the turn should fail closed rather than sending partial unplanned
content, unless the specific tool already marked its provider result as
truncated within the allowed byte limit.

## Bounds

Initial provider-content bounds should be conservative and centralized:

- `read_text_file`: max 32 KiB provider result text;
- `search_project`: max 512 scanned files, max 64 matches, max 240 bytes per
  returned line snippet, max 64 KiB per scanned file;
- `list_project_paths`: max 200 entries;
- provider continuation result: keep the existing continuation max result byte
  enforcement.

If these limits are too small in dogfood, they can be adjusted with evidence.
The first implementation should optimize for safety and predictable
continuation payloads, not maximal context extraction.

## Privacy And Evidence

Provider-visible content tools necessarily send local content to the provider.
That must be explicit in the risk class and policy. The durable session log
should not store file bodies, search match lines, directory dumps, or raw
queries if a query could contain sensitive content.

Recommended evidence summary fields:

- `ToolRequestRecorded`: redacted argument summary, byte count, provider call
  ID, tool name, validation outcome, permission outcome.
- `ToolExecutionFinished`: outcome, categorical reason, result byte count,
  redacted summary such as `read_text_file result redacted`,
  `search_project matches=7 truncated=false`, or
  `list_project_paths entries=23 truncated=false`.

Do not add new session event families for read/search tracing in the first
implementation. Existing tool evidence is enough. If read/search performance
becomes hard to diagnose, a later tracing slice can add bounded content-tool
phase records.

## Extension Compatibility

The design should not reserve content tools permanently for built-ins. The
future extension path should be:

1. extension declares a tool with a read-content risk class;
2. yach validates its schema and activation policy;
3. yach decides whether the tool can be provider-visible;
4. yach executes through a host boundary that returns bounded provider content
   and redacted evidence;
5. adapters still receive schemas only.

This slice should not implement that path. It should only avoid hard-coding
provider handling in a way that makes extension-owned content tools impossible.

## Error Handling

Fail closed for:

- unknown tool;
- malformed or oversized arguments;
- unsupported content schema projection;
- missing project root;
- absolute paths or parent traversal;
- symlink escapes;
- missing path;
- directory passed to `read_text_file`;
- file passed to `list_project_paths`;
- oversized file;
- non-UTF-8 file;
- too many listed paths;
- too many searched files or matches;
- provider result exceeding continuation bounds.

Errors should use categorical labels such as:

- `resource_path_missing`
- `resource_path_outside_root`
- `resource_path_directory`
- `resource_path_not_directory`
- `resource_read_too_large`
- `resource_read_not_utf8`
- `resource_search_truncated`
- `resource_list_truncated`
- `tool_round_result_too_large`

Do not include absolute paths, raw queries, file bodies, or raw provider payloads
in error messages or durable evidence.

## Testing

Add focused backend tests for:

- canonical content tool definitions and provider schemas;
- content tools are not advertised by metadata-only policy;
- forged/mutated built-in content definitions are rejected;
- Rig projects schema-only definitions for content tools without executable
  provider tools;
- native-provider initial requests advertise content tools only when policy and
  project root allow them;
- continuation requests strip advertising;
- `read_text_file` returns bounded provider content while session evidence
  stays redacted;
- `read_text_file` rejects traversal, directories, non-UTF-8, and oversized
  files without leaking local paths or bodies;
- `search_project` returns stable bounded matches and redacted evidence;
- `list_project_paths` returns sorted bounded entries and redacted evidence;
- content and edit tool calls can both contribute to one provider continuation;
- second-round tool calls still fail closed.

Tests should use temporary project roots, fake provider requesters, and direct
projection helpers. No network or provider credentials should be required.

## Acceptance Criteria

This design is complete when it gives yach a narrow, implementable first
provider-visible content surface that lets agents inspect project files before
editing, keeps yach-owned execution authoritative, preserves the one-round
provider continuation boundary, records only redacted durable evidence, and
leaves shell/process, broad mutation, network, and extension-owned content tools
for later focused designs.

## Follow-Up

After the implementation lands, likely next slices are:

1. dogfood the native-provider edit loop with read/search/list enabled;
2. tune content bounds from measured sessions;
3. design broader mutation tools only if exact/create editing remains too
   limited;
4. design extension-owned content tool registration and execution policy.
