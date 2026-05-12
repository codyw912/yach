# Extension Tool Registration Design

Date: 2026-05-12
Status: proposed

## Context

Native yach now owns the first safe provider tool path: `project_path_info`
is registered as a backend-native tool, advertised to providers as a schema
only, executed through yach-owned validation/policy/execution, and recorded as
native session evidence. The next architecture step is letting extensions add
tools to that same pipeline without giving provider adapters or extension code
authority over sessions, transcripts, or provider continuation semantics.

This design is intentionally about extension-owned tool registration, not the
full extension platform. The broader extension system should eventually support
context providers, slash commands, UI contributions, hooks, and richer workflows.
Tool registration is the first contribution surface because the Native MVP needs
a `toy_tool` smoke extension and because the provider advertising path already
has a typed, yach-owned boundary.

Startup performance is a hard constraint. The native startup profile from
2026-05-12 shows the Rust `main` to first render path is sub-millisecond p95 in
the traced run. Extension discovery and activation must not move back onto that
path. The TUI should first paint and accept input before extension hosts are
spawned unless a future design explicitly allows a bounded early-activation
extension class.

## Goal

Design the first extension-owned tool registration path so extensions can
contribute yach-owned tool definitions that enter the existing native registry,
policy, provider advertising, execution, result-shaping, and session-evidence
pipeline.

The design should preserve three product goals:

- a fast default startup path;
- a powerful extension ecosystem that can grow beyond tools;
- Rust-native core ownership of safety, performance, and runtime semantics.

## Non-Goals

- No implementation in this slice.
- No package manager, marketplace, registry resolution, or `npm`-style install
  command implementation.
- No Pi extension compatibility.
- No hot reload.
- No in-process arbitrary code loading.
- No approval UI.
- No new provider SDK tool execution path.
- No mutating, shell/process, or network tools enabled by default.
- No `yach-proto` UI/backend capability changes.
- No broad lifecycle hook API.
- No MCP-in-core design.

## Approach Options

### Option A: Core-only Rust plugin traits

Rust extensions could compile against a `yach-extension-api` crate and register
tools through native trait objects or dynamic libraries. This is fast and can
eventually support high-performance Rust extensions, but it is a poor first
extension boundary: dynamic loading is platform-sensitive, ABI stability is hard,
and it does not help TypeScript extension authors.

### Option B: Process-hosted extension protocol

Extensions declare lightweight metadata in files and run as separate host
processes only when activated. The host can be written in Rust, TypeScript, or
another language as long as it speaks a small yach extension protocol. Yach owns
registration, policy, provider advertising, execution routing, result shaping,
session records, timeouts, and failure handling.

This is the recommended first path. It matches the multi-language extension goal,
keeps untrusted extension code out of the core process, and avoids startup
latency by making manifest discovery cheap and host activation lazy.

### Option C: Embedded scripting runtime

Yach could embed a JS/WASM/Lua runtime and load single-file extensions directly.
That can produce a Pi-like drop-file experience, but it adds startup and sandbox
complexity to the Rust core. It may still be useful later as an extension-host
implementation, but it should not be the first core boundary.

## Recommended Shape

Use a two-phase extension model:

1. **Discovery:** yach reads extension manifests and builds a lightweight
   extension catalog.
2. **Activation:** yach starts an extension host only when an activation event
   requires executable extension code.

Discovery must be manifest-first and code-free. A manifest can declare package
identity, version, protocol compatibility, contribution metadata, activation
events, and the command needed to start the extension host. Reading a manifest
does not register executable authority by itself; it only creates candidates for
later validation and activation.

Activation starts a host process and performs a versioned handshake. The host
then sends registration messages such as `tool.register`. Yach validates each
registration, classifies risk, applies local policy, and inserts approved tools
into the native tool catalog. Provider-visible schemas are projected only after
that approval step.

For TypeScript support, the manifest may point at a command such as `node`,
`deno`, `bun`, or a package-local wrapper. Yach core should not require `npm` or
own dependency installation in the first implementation. A future installer can
resolve Git/package references into an extension directory, but runtime loading
should stay language-agnostic.

For Rust extensions, the same protocol can be served by a compiled binary. A
later Rust SDK can make that ergonomic without changing the core runtime model.

## Extension Manifest

The first manifest should be explicit and versioned. Example shape:

```json
{
  "schema": "yach.extension.v1",
  "id": "example.toy-tools",
  "version": "0.1.0",
  "main": {
    "command": "node",
    "args": ["./extension.js"]
  },
  "activation": {
    "events": ["onCommand:yach.extensions.activate.example.toy-tools"]
  },
  "contributes": {
    "tools": [
      {
        "name": "toy_tool",
        "description": "Return static fixture metadata.",
        "risk": "reads_local_metadata",
        "provider_visible": false
      }
    ]
  }
}
```

The manifest contribution section is advisory. The authoritative definition is
the registration message received from the activated host and accepted by yach.
Keeping both is still useful: manifests let yach show installed extension
capabilities, decide activation events, and preflight obvious policy failures
without spawning code.

For provider-visible tools, `onTool:<name>` is not sufficient as the only
activation event because the provider cannot request a tool until yach has
advertised it, and yach should not advertise it until the extension host has
registered an accepted definition. Provider-visible tools need one of these
non-circular registration paths:

- manual activation, such as a command that starts the extension and registers
  its tools for future turns;
- post-first-paint background activation for extensions that opt into
  provider-tool registration and pass manifest preflight;
- a future strictly validated manifest-authoritative schema path, if yach later
  decides some provider schemas can be advertised before host activation.

The first implementation should prefer manual activation or post-first-paint
background activation. Manifest-authoritative provider advertising is powerful
but should be a later design because it makes manifest validation part of the
provider safety boundary.

Tool names should be globally unique after normalization. The first design
should reserve built-in names and reject extension registrations that collide
with built-ins or already-registered extension tools. A later aliasing or
namespacing design can add friendlier UX, but the first implementation should
prefer predictable failure over implicit shadowing.

## Registration Protocol

The extension host protocol should be transport-agnostic enough to run over
stdio JSON lines first. Initial messages:

- `extension.initialize`: yach sends protocol version, workspace root metadata
  that is safe to reveal, extension id, and enabled capabilities.
- `extension.ready`: host confirms protocol version and process metadata.
- `tool.register`: host registers one tool definition.
- `tool.unregister`: optional later message; not required for first MVP.
- `tool.invoke`: yach asks the host to execute an approved tool call.
- `tool.result`: host returns a bounded result or structured failure.

`tool.register` should map into an extended native tool definition:

```rust
pub struct NativeToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: NativeToolInputSchema,
    pub risk: NativeToolRisk,
    pub owner: NativeToolOwner,
    pub provider_visibility: ProviderToolVisibility,
}

pub enum NativeToolOwner {
    BuiltIn,
    Extension { extension_id: String },
}
```

The existing `NativeToolInputSchema` is intentionally small. Extension tools will
need richer JSON Schema eventually, but the first implementation should accept
only a conservative subset that can be validated locally and projected safely.
Unknown schema features should fail closed.

## Registry And Policy

Split the current registry concept into two responsibilities without changing
the external provider flow:

- **Tool catalog:** accepted definitions from built-ins and activated extensions.
- **Executor router:** maps an accepted tool owner to a built-in executor or an
  extension host process.

All tool calls still enter yach as untrusted input. The existing flow remains
authoritative:

1. provider or UI requests a tool by name and JSON arguments;
2. yach looks up the accepted definition;
3. yach validates arguments;
4. yach evaluates policy;
5. yach records redacted session evidence;
6. yach invokes the built-in executor or extension host;
7. yach validates and shapes the result before any provider continuation.

Extension code must not append session events directly, mutate canonical
transcripts, call provider adapters, or decide provider continuation boundaries.
It may return structured output to yach. Yach decides what becomes durable
session evidence and what, if anything, is provider-visible.

The default policy remains deny-by-default for higher-risk classes. For the first
tool-registration implementation, only `reads_local_metadata` extension tools
should be eligible for automatic approval, and only if their schema and result
contract are conservative. `reads_local_content`, `mutates_local_state`,
`runs_process`, `uses_network`, and similar risks need separate approval and UI
design before they are provider-visible or auto-executable.

## Provider Advertising

Provider advertising remains schema-only. Extensions do not register Rig tools,
provider SDK tools, or adapter-owned executors.

The provider-advertising candidate set should be built from accepted yach tool
definitions after policy classification. A tool is provider-advertisable only
when all of these are true:

- the definition is accepted in the yach tool catalog;
- the current session/profile policy permits provider visibility;
- the risk class is allowed for provider exposure;
- the schema can be projected into the existing `ProviderAdvertisedToolSchema`
  representation without local data leakage;
- the tool is already registered with an executable route for the provider turn
  being built.

Lazy extension registration means a newly installed tool may not appear in the
first provider request of a just-started process. That is acceptable. Fast first
paint is more important than complete first-request extension availability.
Future UX can show extension activation state and allow a manual reload or
activation command.

For provider-visible extension tools, the first safe availability model is:

1. extension manifests are discovered after first paint;
2. eligible provider-tool extensions are either manually activated or activated
   by a background post-first-paint task;
3. accepted tool registrations become candidates for subsequent provider turns;
4. the current in-flight provider request is not mutated when a late extension
   finishes activation.

This keeps provider advertising non-circular without blocking startup or letting
manifest-only data grant execution authority.

Continuation behavior should stay one-round and fail-closed until a separate
multi-round tool loop design changes it. Continuation requests should not inherit
provider advertising by default.

## Startup And Activation Policy

Default startup must not spawn extension hosts. The first TUI frame and input box
should be ready before extension activation work starts.

Recommended phases:

1. **Before first paint:** no extension host startup. Optionally load a cached
   manifest index if it can be done inside a tight budget without blocking
   rendering.
2. **After first paint:** scan extension manifests in a background task and
   update the extension catalog.
3. **Provider-tool registration activation:** for extensions that declare
   provider-visible tool contributions, either wait for manual activation or run
   a bounded post-first-paint background activation path. Successful
   registrations affect future turns only.
4. **On activation event:** spawn the host and perform registration handshake.
5. **On tool invocation:** if a tool requires activation and is not ready, either
   activate within a bounded timeout or fail with a clear deferred-activation
   error.

Some future extensions may need earlier activation. They should declare an
explicit `startup` activation class and be subject to a small budget, telemetry,
and fail-open behavior where the TUI still paints if the extension is slow.
Early activation should be opt-in, visible in diagnostics, and rare.

Startup profiling should add extension-specific marks before accepting any early
activation feature:

- manifest cache load start/end;
- background scan start/end;
- host spawn start/end;
- registration complete;
- skipped due to startup budget.

## Failure Handling

Extension failures should degrade the owning extension, not the core TUI.

Fail closed when:

- manifest schema is malformed;
- extension id or tool name is invalid;
- host handshake protocol is unsupported;
- registration uses unsupported schema features;
- registration declares a risk not allowed by current policy;
- registration collides with a built-in or existing tool;
- host exits during invocation;
- invocation times out;
- result exceeds size limits;
- result shape does not match the registered output contract.

Session evidence should record categorical extension/tool failures without raw
arguments, raw local data, command lines containing secrets, or provider payloads.
The user-facing error can name the extension id and tool name, but detailed
debug output must stay redacted.

## Testing

The implementation plan should include tests for:

- manifest parsing accepts a minimal `toy_tool` extension and rejects malformed
  manifests;
- discovery builds a catalog without spawning extension code;
- startup path does not block on extension discovery or host startup;
- host handshake accepts compatible protocol versions and rejects incompatible
  versions;
- `tool.register` inserts an extension-owned tool definition into the same
  registry path as built-ins;
- built-in tool names cannot be shadowed;
- unsupported schema features fail closed;
- provider advertising includes an approved provider-visible extension tool only
  after policy approval;
- provider advertising omits or rejects denied extension tools without silently
  granting execution;
- extension tool invocation records redacted session evidence;
- host crash, timeout, oversized result, malformed result, and policy denial are
  categorized;
- no Rig `Tool`, `ToolSet`, or provider-adapter executor is introduced.

Performance tests should include a startup profile with extensions installed but
not activated. The first-render numbers should stay within the existing native
startup envelope, with any manifest scan happening after first paint or within a
documented cache-load budget.

## Acceptance Criteria

This design is ready for implementation when it is accepted and a follow-up plan
can decompose it into small slices:

1. extension manifest parser and catalog, no host execution;
2. process-host handshake and `toy_tool` registration;
3. extension-owned executor routing through native tool workflow;
4. policy-gated provider advertising for approved extension tools;
5. startup profiling/diagnostics for installed but inactive extensions.

The first implementation should prove a `toy_tool` extension can register a safe
read-only metadata tool through the same yach-owned pipeline as built-ins, while
the TUI first paint remains effectively instant and provider adapters remain
schema-only.

## Follow-Up

After tool registration, likely extension-system follow-ups are:

- `static_context_provider` contribution surface;
- install UX for Git/local/package references;
- a Rust SDK and a TypeScript SDK over the same host protocol;
- manual reload and diagnostics commands;
- approval UI for higher-risk tools;
- richer provider-visible read/search/edit tool policies.
