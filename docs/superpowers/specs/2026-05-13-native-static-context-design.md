# Native Static Context Design

Date: 2026-05-13
Status: proposed

## Context

Native yach can now build provider requests, preserve native session history,
execute safe read-only tools, advertise approved tool schemas, and accept
extension-owned metadata tool registrations. The next Native MVP gap is static
context: standing instructions and guidance that should shape provider requests
without requiring the model to call a tool.

This is not an extension replacement for `AGENTS.md`. `AGENTS.md` is now a
cross-harness convention and should be a first-class yach core feature. Codex
injects discovered `AGENTS.md` files as instruction messages. opencode has a
similar rules mechanism around `AGENTS.md`, global rules, and configured
instruction files. Pi extensions can contribute prompt/context paths and hook
into context assembly. Yach should support the common `AGENTS.md` workflow in
core, then let extensions contribute additional static context through the same
typed assembly pipeline later.

The existing backend already has local-only context packaging primitives in
`resource.rs`, but those primitives explicitly do not make local content
provider-visible. This design adds the missing provider-request assembly model:
what static context is discovered, how it is ordered, how size limits are
applied, how provenance is preserved, and where extension-provided context will
plug in without owning the prompt.

## Goal

Design native static context assembly so yach can discover and inject
`AGENTS.md` instructions into provider requests in a predictable,
provider-neutral, policy-governed way, while leaving a clean path for
extension-provided context packs.

## Non-Goals

- No implementation in this slice.
- No arbitrary project file glob inclusion.
- No extension-host execution for context contribution.
- No install UX, package manager, marketplace, or registry resolution.
- No Pi extension compatibility.
- No MCP resource integration.
- No user-global instructions file until yach home/config layout is designed.
- No user approval UI.
- No provider-specific prompt templating beyond the current adapter projection.
- No broad memory system, summarizer, retrieval index, or vector search.
- No mutating local state.

## Terminology

- **Static context:** model-visible guidance assembled before a provider request
  starts, not a tool result produced during the turn.
- **Core context source:** a yach-owned source such as `AGENTS.md` discovery.
- **Extension context source:** an installed extension contribution that points
  at extension-packaged context files or later project-file selectors.
- **Context item:** one validated, bounded, provenance-labeled unit of static
  context.
- **Context bundle:** the ordered set of context items selected for a provider
  request.
- **Provider projection:** conversion from the context bundle into
  `ProviderMessage`s and adapter-specific prompt/preamble behavior.

## Relationship To Tools

Static context and tools solve different problems.

A tool is requested by the model during a turn. Yach validates the request,
checks policy, executes the tool, records evidence, and may continue the
provider turn with shaped tool results.

Static context is assembled before the provider request. The model sees it as
guidance or background context from the beginning. It does not grant execution
authority, does not call extension code during the turn, and does not let
extensions mutate the transcript or provider continuation semantics.

## Approach Options

### Option A: Extension context first

Yach could implement `static_context_provider` directly as an extension
contribution surface, then add `AGENTS.md` later as one built-in provider.

This is too abstract for the next slice. It risks making a common harness
convention feel optional or extension-owned, and it forces extension activation
questions before yach has a native context assembly pipeline.

### Option B: Core `AGENTS.md` plus shared context assembly

Yach implements `AGENTS.md` discovery and a typed context bundle in core first.
The bundle has provenance, priority, byte limits, provider visibility, and
session evidence. Extension-packaged context can later enter as another source
without changing provider adapters or prompt ordering.

This is the recommended path. It delivers expected harness behavior, grounds
the design in a concrete user-facing feature, and creates the extension context
seam without making extensions responsible for base project instructions.

### Option C: General project-file context selectors

Yach could immediately support configured files/globs such as
`docs/**/*.md`, `CONTRIBUTING.md`, or arbitrary extension-provided selectors.

This is powerful, but it expands the provider-visible local data surface too
early. It should come after `AGENTS.md` and packaged extension context establish
ordering, budgeting, provenance, and failure behavior.

## Recommended Shape

Implement core native static context assembly around `AGENTS.md` first.

At provider request construction time, yach should gather eligible static
context items, build an ordered context bundle, and prepend that bundle to the
provider request as system-role guidance. The Rig adapter already projects
system messages into the provider preamble, so this preserves the existing
provider-neutral `ProviderRequest` shape while making the static context path
explicit and testable.

The first implementation should support:

1. discover `AGENTS.md` from the project root to the current working directory;
2. read only UTF-8 text files under explicit byte limits;
3. label each item with source, path, scope, priority, and byte count;
4. merge items in deterministic root-to-leaf order;
5. inject the assembled bundle as one or more system messages before transcript
   messages;
6. record redacted session evidence that context was included without writing
   full local file contents to metrics or debug logs.

Extension-provided static context should be designed as an additive source into
the same bundle, not as a separate prompt mutation mechanism. The first
extension context source should be extension-packaged files only:

```json
{
  "contributes": {
    "static_context": [
      {
        "id": "rust-style-guide",
        "title": "Rust style guide",
        "source": {
          "type": "extension_file",
          "path": "context/rust.md"
        },
        "placement": "developer_instructions",
        "max_bytes": 12000
      }
    ]
  }
}
```

Project-file selectors and globs should remain a follow-up design because they
make arbitrary project content provider-visible.

## Discovery And Ordering

For core `AGENTS.md`, yach should follow the common harness shape:

1. determine the canonical project root;
2. determine the canonical current working directory for the session;
3. walk from project root to cwd;
4. include each `AGENTS.md` found on that path;
5. order root before leaf so more specific instructions come later.

The provider-visible content should be labeled in a stable envelope such as:

```text
# AGENTS.md instructions for .

<root AGENTS.md content>

# AGENTS.md instructions for crates/yach-backend

<nested AGENTS.md content>
```

Yach should not invent override semantics in the first implementation. If both
root and nested instructions exist, both are included in order. Later, yach can
support explicit override files if a separate design justifies it.

Extension context ordering should be lower priority than project-owned
`AGENTS.md` by default. A safe initial order is:

1. yach built-in provider/runtime preamble;
2. project-owned `AGENTS.md` from root to leaf;
3. extension-packaged context items sorted by extension id then item id;
4. transcript messages.

This means project instructions remain authoritative over extension guidance.

## Provider Visibility And Policy

`AGENTS.md` is intentionally provider-visible: users place it in a repository
because they want harnesses to treat it as model instructions. Yach should still
treat it as local file content with explicit limits and provenance.

The first policy should be conservative:

- only filenames exactly named `AGENTS.md`;
- only files inside the project root and along the root-to-cwd path;
- UTF-8 text only;
- per-file byte limit;
- total static context byte limit;
- omit oversized or unreadable context items with a visible diagnostic unless a
  later required-context policy explicitly says to fail the turn;
- no globs, symlink escape, binary content, hidden arbitrary files, or network
  access.

When a context item is omitted, yach should record the category:

- `context_file_missing`;
- `context_file_not_utf8`;
- `context_file_too_large`;
- `context_bundle_too_large`;
- `context_path_outside_root`;
- `context_source_disabled`.

The first implementation can omit oversized items instead of failing the whole
turn, as long as the omission is visible in session evidence and diagnostics.
Provider requests should not include partial file content unless the context
item is explicitly truncated with a visible marker and the truncation policy is
covered by tests.

## Provider Request Assembly

Add a yach-owned context assembly boundary before native provider requests are
created. Conceptually:

```rust
pub struct NativeStaticContextItem {
    pub source: NativeStaticContextSource,
    pub scope: NativeStaticContextScope,
    pub title: String,
    pub content: String,
    pub byte_count: usize,
    pub priority: NativeStaticContextPriority,
}

pub struct NativeStaticContextBundle {
    pub items: Vec<NativeStaticContextItem>,
    pub total_bytes: usize,
}
```

`native_provider_messages_from_log` should stop being the only message assembly
step. The provider path should become:

1. load completed transcript messages for the current turn;
2. discover static context for the session root/cwd and active profile;
3. build a bounded context bundle;
4. prepend the bundle as system `ProviderMessage`s;
5. preserve existing transcript ordering after context messages.

This keeps provider adapters simple. The Rig adapter can continue to convert
system messages into the preamble and non-system messages into the prompt.

## Extension Context Contributions

Extensions should eventually contribute static context through manifests and
post-first-paint discovery. They should not need to run host code to contribute
packaged static files.

The first extension-compatible model should be:

- manifest declares `contributes.static_context`;
- source type is `extension_file`;
- path is relative to the extension root;
- yach reads the file after manifest validation;
- yach applies the same UTF-8, byte, provenance, and ordering rules;
- extension context affects future provider requests only;
- late discovery never mutates an in-flight provider request.

This parallels the extension-owned tool registration model: manifests can
declare contributions, but yach owns validation, policy, provider projection,
and session evidence. Unlike tools, packaged static context does not require
host activation or execution.

Project-file context selectors should be a separate extension contribution
type, because they expose repo content selected by an extension. That later
design should decide whether users must opt in per extension, per workspace, or
per selector.

## Startup And Performance

Static context must not regress TUI first paint.

The first safe implementation can assemble context lazily when the first native
provider request is built, not during TUI startup. This avoids adding file reads
to the first-frame path. If yach later preloads a context index, that preload
must be post-first-paint or covered by startup profiling.

Required timing evidence for implementation:

- no `AGENTS.md` startup profile remains in the existing sub-millisecond traced
  first-render envelope;
- a provider-request assembly benchmark records context discovery and assembly
  time for zero, one, and nested `AGENTS.md` files;
- static context assembly emits low-frequency metrics such as
  `static_context_discovery_ms`, `static_context_read_ms`, and
  `static_context_total_bytes` when a provider request is built.

## Failure Handling

Static context failures should degrade context inclusion, not the whole TUI.

Provider request construction may continue when an optional context item is
omitted due to size, encoding, or path policy. It should fail only when the
provider request would otherwise be malformed, such as when there is no user
message after context assembly or when policy explicitly requires a context
source that cannot be read.

All diagnostics must avoid leaking full local file contents. Session evidence
may include source kind, relative path, byte count, omission category, and item
id. It should not duplicate the entire context body unless the session log has
an explicit model-visible prompt transcript mode in a later design.

## Testing

The implementation plan should include tests for:

- discovers root `AGENTS.md`;
- discovers nested `AGENTS.md` from project root to cwd in deterministic order;
- ignores sibling or unrelated `AGENTS.md` files not on the root-to-cwd path;
- rejects path traversal and symlink escapes;
- rejects non-UTF-8 content;
- handles per-file and total bundle byte limits;
- injects context before transcript messages as system `ProviderMessage`s;
- preserves existing transcript ordering after injected context;
- keeps Rig preamble behavior stable for system messages;
- records redacted context inclusion and omission evidence;
- does not read or spawn extension code for core `AGENTS.md`;
- leaves extension static context as manifest-only contribution data until a
  later implementation slice enables it.

Performance tests should include provider-request assembly benchmarks for no
context, one `AGENTS.md`, and nested `AGENTS.md`.

## Acceptance Criteria

This design is ready for implementation when accepted and a follow-up plan can
decompose it into small slices:

1. static context model and `AGENTS.md` discovery tests;
2. bounded context file reading and root/cwd path policy;
3. provider request assembly integration;
4. session evidence and diagnostics;
5. provider-request assembly benchmark;
6. project docs update to mark `AGENTS.md` support as core and extension static
   context as a follow-up contribution source.

## Open Questions

- Should yach support a user-global instructions file in the same slice as
  project `AGENTS.md`, or defer global context until config/home layout is
  designed?
- Should oversized `AGENTS.md` files be omitted entirely or truncated with an
  explicit marker?
- Should context inclusion be recorded only as redacted evidence, or should
  yach eventually persist the exact model-visible prompt for replay/debugging?

## Reference Inputs

- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/specs/2026-05-12-extension-tool-registration-design.md`
- `docs/project/state.md`
- `docs/project/next.md`
- Codex `AGENTS.md` guidance:
  `https://developers.openai.com/api/docs/guides/prompt-guidance#using-agentsmd`
- opencode rules documentation: `https://dev.opencode.ai/docs/rules/`
- Pi extension documentation: `https://pi.dev/docs/latest/extensions`
