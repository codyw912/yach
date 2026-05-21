# Extension Runtime First Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the first conservative extension runtime path: installed extension package discovery after first paint, a persistent process-host metadata `toy_tool` invocation path, provider-turn catalog resolution from activated extensions, and explicit tool alias/replacement policy with provenance. Keep broad mutation, shell/process, network, hidden system-prompt mutation, in-process plugins, and implicit replacement out of scope.

**Architecture:** Build on the existing `crates/yach-backend/src/extension.rs` manifest/catalog parser and one-shot host-registration helpers instead of replacing them. Add package-root/indexing and activation state as backend-owned primitives, then wire the native runner/TUI path only after there are backend tests proving extension discovery does not spawn hosts. Keep provider adapters schema-only. Tool invocation remains yach-owned: validate, authorize, record evidence, call host route, shape result, and only then include provider-visible tool results.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `tokio`, existing yach backend/provider/TUI seams, `just dev cargo test`, `just lint`, startup benchmark harness.

---

## File Structure

- Modify `crates/yach-backend/src/extension.rs`: package roots, package manifests, extension index cache model, persistent host session protocol, activation state, and replacement policy parsing helpers.
- Modify `crates/yach-backend/src/tools.rs`: resolved tool catalog, provenance metadata, alias/replacement resolution, extension executor route abstraction, provider-advertising candidate behavior.
- Modify `crates/yach-backend/src/native_runner.rs`: after backend primitives exist, schedule post-first-paint extension discovery and use resolved provider-turn tool catalogs.
- Modify `crates/yach-backend/src/lib.rs`: add integration-style tests that span extension/tool/native-runner seams.
- Modify `crates/yach-cli/src/main.rs`: only for CLI commands such as `extension list`, `extension doctor`, and future install stubs; no host activation before first render.
- Modify `crates/yach-bench/src/main.rs`: add real manifest-scan startup profiling once discovery is wired.
- Modify `docs/benchmarks/`: record startup and activation evidence.
- Modify `docs/project/state.md` and `docs/project/next.md`: only after implementation slices land and change current project status.

## Implementation Sequence

Do this as small PRs. Stop after each PR if behavior or risk looks larger than expected.

1. Package roots and manifest index cache.
2. Post-first-paint manifest scan and diagnostics.
3. Persistent extension host `tool.invoke`/`tool.result` for metadata tools.
4. Provider-turn resolved catalog from built-ins plus activated extension tools.
5. Explicit alias/replacement policy with provenance.
6. Extension static-context file contribution.
7. Startup and activation profiling evidence.

## Task 1: Package Roots And Manifest Index Cache

**Files:**
- Modify: `crates/yach-backend/src/extension.rs`
- Test: `crates/yach-backend/src/extension.rs`

- [ ] **Step 1: Add failing package-root tests**

Add tests for:

- `extension_package_root_loads_yach_extension_manifest`
- `extension_package_root_loads_package_json_yach_manifest_pointer`
- `extension_package_index_records_source_scope_and_manifest_path`
- `extension_package_index_rejects_duplicate_extension_ids`
- `extension_package_index_does_not_start_hosts`

The tests should create temporary package roots containing either:

```text
yach.extension.json
```

or:

```json
{
  "name": "@example/yach-toy-tools",
  "version": "0.1.0",
  "yach": {
    "manifests": ["./extensions/toy/yach.extension.json"]
  }
}
```

Assert that package metadata includes:

- extension id;
- extension version;
- package root;
- manifest path;
- scope: user, project, or ephemeral;
- source ref, if provided;
- host start count is still zero.

- [ ] **Step 2: Implement package-root discovery primitives**

Add conservative structs:

```rust
pub enum ExtensionInstallScope {
    User,
    Project,
    Ephemeral,
}

pub struct ExtensionPackageRoot {
    pub root: PathBuf,
    pub scope: ExtensionInstallScope,
    pub source_ref: Option<String>,
}

pub struct ExtensionPackageRecord {
    pub manifest: ExtensionManifest,
    pub scope: ExtensionInstallScope,
    pub package_root: PathBuf,
    pub manifest_path: PathBuf,
    pub source_ref: Option<String>,
}

pub struct ExtensionManifestIndex {
    records: Vec<ExtensionPackageRecord>,
}
```

Keep path handling strict:

- manifest paths must stay inside the package root;
- `package.json` may point only to relative manifest paths;
- missing or malformed manifests produce categorical errors;
- discovery never executes extension code.

- [ ] **Step 3: Add JSON cache model without startup wiring**

Add a serializable index snapshot type for future first-paint cache loading. It
should store enough data to display installed/discovered extension diagnostics,
but it must not grant provider advertising or execution authority. A cached
manifest entry is advisory until the real manifest is validated or a trusted
cache policy is separately designed.

Tests:

- cache round-trips valid records;
- cache with stale/missing manifest path is marked stale;
- cache load does not spawn hosts.

- [ ] **Step 4: Verify**

Run:

```bash
just dev cargo test -p yach-backend extension_package_ -- --nocapture
just dev cargo test -p yach-backend extension_manifest_ -- --nocapture
git diff --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/extension.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add extension package manifest index"
```

## Task 2: Post-First-Paint Manifest Scan And Diagnostics

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-cli/src/main.rs`
- Test: `crates/yach-backend/src/native_runner.rs`, `crates/yach-cli/src/main.rs`

- [ ] **Step 1: Add backend event-loop tests**

Add tests proving:

- native backend sends the first ready/state events before extension scan starts;
- extension scan runs as background work after initialization;
- manifest scan failures become redacted status/diagnostic entries;
- host start count stays zero during scan.

Use fake package roots and injected scan providers. Do not require real user
home directories.

- [ ] **Step 2: Add startup trace marks**

Add trace labels:

```text
extension_manifest_scan_scheduled
extension_manifest_scan_started
extension_manifest_scan_finished
extension_manifest_scan_failed
```

Ensure `extension_manifest_scan_started` happens after `tui_first_render_end` in
the benchmark harness once the TUI path is wired.

- [ ] **Step 3: Add minimal CLI diagnostics**

Add read-only commands:

```text
yach extension list
yach extension doctor [<id>]
```

For this task, these may inspect discovered package roots/index state only. They
must not install dependencies or spawn hosts.

- [ ] **Step 4: Verify**

Run:

```bash
just dev cargo test -p yach-backend extension_manifest_scan -- --nocapture
just dev cargo test -p yach-cli extension_ -- --nocapture
git diff --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs crates/yach-cli/src/main.rs
git commit -m "feat: discover extensions after first paint"
```

## Task 3: Persistent Metadata Tool Host Invocation

**Files:**
- Modify: `crates/yach-backend/src/extension.rs`
- Modify: `crates/yach-backend/src/tools.rs`
- Test: `crates/yach-backend/src/extension.rs`, `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add protocol tests**

Extend the host protocol from one-shot registration toward a persistent session.
Tests should cover:

- yach writes `extension.initialize`;
- host replies `extension.ready`;
- host registers `toy_tool`;
- yach sends `tool.invoke` with request id and validated args;
- host replies `tool.result`;
- timeout, crash, malformed result, oversized result, and mismatched request id
  are categorized.

Use a fixture host process or an in-memory fake host seam. Keep the public
protocol JSONL-shaped even when tests use an in-memory transport.

- [ ] **Step 2: Implement host session abstraction**

Add a small host abstraction:

```rust
pub trait ExtensionHostTransport {
    fn send(&mut self, message: ExtensionHostClientMessage) -> Result<(), ExtensionHostError>;
    fn recv(&mut self, timeout: Duration) -> Result<ExtensionHostServerMessage, ExtensionHostError>;
}
```

Then add `ExtensionHostSession` for:

- initialize/ready handshake;
- registration collection;
- one tool invocation at a time;
- bounded result bytes;
- shutdown/kill behavior.

Avoid adding concurrency inside a host session in this slice. Parallel host calls
can be a later design if profiling demands it.

- [ ] **Step 3: Route extension metadata execution through native workflow**

Replace or augment the current in-memory `ExtensionToolExecutorRouter` so it can
call an activated `ExtensionHostSession` for `ReadsLocalMetadata` tools. The
existing native workflow remains authoritative:

```text
schema validation -> permission -> evidence -> host invoke -> result shaping
```

Host output must be bounded and redacted in durable evidence.

- [ ] **Step 4: Verify**

Run:

```bash
just dev cargo test -p yach-backend extension_host_ -- --nocapture
just dev cargo test -p yach-backend extension_executor_ -- --nocapture
git diff --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/extension.rs crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
git commit -m "feat: invoke metadata tools through extension hosts"
```

## Task 4: Provider-Turn Resolved Catalog

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Test: `crates/yach-backend/src/lib.rs`, `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Add catalog resolution tests**

Add tests proving:

- built-ins resolve exactly as today when no extensions are active;
- activated provider-visible `toy_tool` appears in provider advertising;
- inactive extension tools do not appear in provider advertising;
- provider adapters receive schema-only definitions, not executable tools;
- late activation affects the next provider turn, not an in-flight turn.

- [ ] **Step 2: Add resolved catalog structs**

Add:

```rust
pub struct ResolvedNativeTool {
    pub provider_name: String,
    pub implementation_name: String,
    pub definition: NativeToolDefinition,
    pub provenance: NativeToolProvenance,
}

pub enum NativeToolProvenance {
    BuiltIn,
    Extension {
        extension_id: String,
        extension_version: String,
    },
    ExtensionReplacement {
        extension_id: String,
        extension_version: String,
        replaced_builtin: String,
        replacement_source: String,
    },
}
```

Keep this additive. Existing `NativeToolRegistry` may remain as the underlying
definition store until replacement policy forces a richer model.

- [ ] **Step 3: Use resolved definitions in native-provider turn setup**

Before each provider turn, build a resolved catalog from:

- built-ins;
- activated extension registrations;
- provider visibility policy;
- executable route availability.

The provider loop should route by resolved tool, not by hardcoded extension
metadata assumptions. Existing hardcoded built-in dispatch can stay for built-in
executor routes while extension metadata calls go through the extension route.

- [ ] **Step 4: Verify**

Run:

```bash
just dev cargo test -p yach-backend provider_tool_advertising -- --nocapture
just dev cargo test -p yach-backend native_provider -- --nocapture
git diff --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/native_runner.rs crates/yach-backend/src/rig_adapter.rs crates/yach-backend/src/lib.rs
git commit -m "feat: resolve provider tools from active catalog"
```

## Task 5: Explicit Alias And Replacement Policy

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/extension.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add replacement policy tests**

Add tests for:

- accidental built-in collision fails closed;
- `alias_only` exposes extension implementation under its own name;
- `replace_builtin` keeps provider-facing built-in name and routes to extension;
- `disable_builtin` removes a built-in from the active catalog;
- replacement provenance is recorded;
- replacement cannot lower risk;
- project replacement policy is ignored or blocked when project is untrusted.

- [ ] **Step 2: Implement explicit policy model**

Add a conservative policy object that can later be loaded from config:

```rust
pub enum NativeToolResolutionMode {
    Deny,
    AliasOnly,
    ReplaceBuiltin,
    DisableBuiltin,
}

pub struct NativeToolReplacementRule {
    pub builtin_name: String,
    pub extension_id: String,
    pub extension_tool: String,
    pub mode: NativeToolResolutionMode,
    pub source: NativeToolReplacementSource,
}
```

Do not parse project/user TOML in this task unless the config seam already
exists. Tests may construct policy directly.

- [ ] **Step 3: Wire provenance into evidence summaries**

Tool request/execution evidence should include enough structured metadata to
distinguish:

- core built-in;
- extension-owned alias;
- extension replacement of a built-in.

Keep raw args/results redacted as before.

- [ ] **Step 4: Verify**

Run:

```bash
just dev cargo test -p yach-backend native_tool_replacement -- --nocapture
git diff --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/extension.rs crates/yach-backend/src/lib.rs
git commit -m "feat: resolve explicit tool replacements"
```

## Task 6: Extension Static-Context Files

**Files:**
- Modify: `crates/yach-backend/src/static_context.rs`
- Modify: `crates/yach-backend/src/extension.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`
- Test: `crates/yach-backend/src/lib.rs`, `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Add static-context contribution tests**

Add tests proving:

- manifest-referenced `background_context` file is included only after
  extension discovery;
- file path must stay inside package root;
- max bytes are enforced;
- source extension id and relative path are recorded;
- omissions are recorded without raw file contents.

- [ ] **Step 2: Integrate with static context assembly**

Extend static context assembly to accept activated/discovered extension context
items as input. Keep dynamic extension code out of static context assembly. This
task is only for manifest-referenced package files.

- [ ] **Step 3: Verify**

Run:

```bash
just dev cargo test -p yach-backend static_context_extension -- --nocapture
just dev cargo test -p yach-backend native_provider_messages_include -- --nocapture
git diff --check
```

- [ ] **Step 4: Commit**

```bash
git add crates/yach-backend/src/static_context.rs crates/yach-backend/src/extension.rs crates/yach-backend/src/native_runner.rs crates/yach-backend/src/lib.rs
git commit -m "feat: include extension static context files"
```

## Task 7: Startup And Activation Profiling

**Files:**
- Modify: `crates/yach-bench/src/main.rs`
- Create/modify: `docs/benchmarks/extension-runtime-profile-YYYY-MM-DD.md`
- Test: benchmark command and focused backend tests

- [ ] **Step 1: Add benchmark modes**

Add or update benchmark modes for:

- baseline native TUI startup;
- installed inactive extension with real manifest scan;
- many installed manifests scan after first paint;
- background metadata host activation;
- metadata extension tool invocation latency.

- [ ] **Step 2: Record evidence**

Run release-profile benchmark commands and record:

- first render p50/p95/p99;
- manifest scan start relative to first render;
- scan duration for one and many manifests;
- host activation duration;
- tool invocation duration;
- whether any host spawned before first render.

- [ ] **Step 3: Verify**

Run:

```bash
just dev cargo test -p yach-bench
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-report --samples 100
git diff --check
```

- [ ] **Step 4: Commit**

```bash
git add crates/yach-bench/src/main.rs docs/benchmarks/extension-runtime-profile-*.md
git commit -m "docs: record extension runtime startup profile"
```

## Cross-Cutting Verification

Before each PR:

```bash
just dev cargo fmt --all
just dev cargo test -p yach-backend
git diff --check
```

Before the final PR in this sequence:

```bash
just lint
just test
```

If `just lint` exposes unrelated existing diagnostics, fix them if they are in
the touched area. Do not bypass pre-commit hooks.

## Out Of Scope Until Later Specs

- Broad `write`/patch/delete/rename extension tools.
- Shell/process tools.
- Network/web tools.
- In-process Rust plugins.
- Hidden extension system-prompt mutation.
- Automatic provider advertising from manifest-only schemas.
- Marketplace ranking or trust scoring.
- Pi extension API compatibility.
- A working auto-review reviewer runtime.

## Completion Criteria

This plan is complete when yach can discover installed extension manifests after
first paint, activate a metadata extension host, invoke a `toy_tool` through the
yach-owned tool workflow, include activated provider-visible extension tools in
future provider turns, resolve explicit alias/replacement policy with provenance,
include manifest-referenced extension static context files, and show profiling
evidence that installed inactive extensions do not slow first paint.
