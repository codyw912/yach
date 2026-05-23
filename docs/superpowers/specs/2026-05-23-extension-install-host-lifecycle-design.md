# Extension Install And Host Lifecycle Design

Date: 2026-05-23
Status: draft

## Context

Yach now has the first conservative extension runtime slice:

- package roots and manifest indexing;
- post-first-paint manifest scanning;
- extension metadata tool registration and invocation primitives;
- provider-turn resolved catalogs from built-ins plus activated extension tools;
- explicit built-in replacement policy with provenance;
- extension static-context file contributions;
- startup and activation profiling evidence.

The remaining gap is product-shaped extension usage. Today, package roots are
fed through `YACH_EXTENSION_PACKAGE_ROOTS`, diagnostics can list/doctor scanned
manifests, and the host protocol has tests for registration and invocation. But
yach does not yet have durable install records, normal install/remove/update
commands, a long-lived spawned host manager, activation state, or a local
developer workflow for quickly trying a TypeScript or Rust extension.

Comparable harnesses sharpen the tradeoff:

- Pi supports `install` from npm refs, git URLs, local directories, and local
  files. It also supports project-scoped package settings, ephemeral `-e`
  installs, conventional package directories, and automatic dependency install.
  The UX is strong, but packages run with full system access and missing
  project packages may install on startup.
- OpenCode loads JavaScript/TypeScript plugins from project/global plugin
  directories and npm config. It supports TypeScript directly and lets plugins
  hook behavior, but npm plugin installation and local dependency installation
  happen at startup, which is not acceptable for yach's first-paint target.
- Codex separates user and trusted-project config, keeps project-local config
  out of untrusted projects, and models per-tool enablement/approval for MCP,
  apps, plugins, and permission profiles. That is closer to yach's desired
  trust and control model.

Yach should preserve Pi-like ergonomics where they matter, while keeping
installation, activation, execution, permission, provenance, and performance
semantics owned by the Rust core.

## Goal

Design the next extension-runtime work so yach can install or reference real
extension packages, launch real extension host processes, maintain activation
state, and support a fast local developer loop without regressing TUI first
paint.

The design should be sufficient to plan small implementation slices. It should
cover the durable shape for npm/git/local refs, but the first implementation
should prioritize local path installs and process-host lifecycle before network
package adapters.

## Non-Goals

- No implementation in this slice.
- No production marketplace, gallery, package ranking, signing, or provenance
  attestation service.
- No in-process plugin ABI or embedded JavaScript runtime.
- No automatic install, update, dependency resolution, network access, or host
  spawn before TUI first render.
- No project-scoped activation before project trust is established.
- No extension-owned mutation, shell/process, or network tools beyond already
  designed conservative metadata/static-context behavior.
- No hidden system-prompt mutation.
- No implicit built-in replacement.
- No broad lifecycle hook system that can arbitrarily mutate yach internals.
- No sandbox implementation beyond preserving the policy seams needed for one.

## Design Principles

### Pi-Like UX, Yach-Owned Authority

The install UX should feel familiar:

```text
yach install <ref>
yach extension install <ref>
yach extension remove <id-or-ref>
yach extension update [<id-or-ref>]
yach extension list
yach extension doctor [<id>]
```

`yach install` is a convenience alias for `yach extension install`. It should
not imply that install is safe or startup-bound. Install commands are explicit
management operations. Runtime startup should read lightweight records and scan
after first paint; it should not fetch packages, run package managers, or spawn
extension hosts before the input box is usable.

### Install Records Are Data, Packages Are Materialized Separately

Yach should distinguish the install record from the package root on disk.

An install record stores:

- normalized source ref;
- resolved package root;
- scope: user, project, or ephemeral;
- enabled flag;
- optional selected resources or filters;
- resolved extension ids from the most recent scan;
- install/update timestamps;
- categorical last error.

Package materialization is source-adapter specific:

- local path refs point directly at the path;
- git refs clone or update into a yach-owned package directory;
- npm refs install into a yach-owned package directory through a configured
  package-manager command;
- ephemeral refs materialize under a temporary run directory and are not written
  to persistent settings.

The first implementation should support durable local path records plus the ref
parser and data model for git/npm. Git and npm adapters can follow after host
launch is working.

### Activation Is A Runtime State Machine

Extensions should move through explicit states:

```text
installed -> discovered -> blocked -> starting -> registering -> active
                              \-> failed
active -> stopping -> stopped
```

`installed` means yach has an install record. `discovered` means the manifest
scan validated a package root. `blocked` means policy or trust prevents
activation. `active` means yach has a live host route and accepted
registrations for this process.

Provider-visible tools are advertised only from active registrations. Late
activation affects future provider turns only. The active provider request is
not mutated when a host finishes registration in the background.

### Process Hosts Are Long-Lived, But Not Privileged

The current registration helper can spawn a process and parse registration
messages, but it is not yet the runtime host manager. The next runtime should
introduce a process-backed `ExtensionHostTransport` that owns stdin/stdout JSONL
for a long-lived child process.

Host startup rules:

- cwd is the package root unless the manifest specifies a stricter relative
  host cwd;
- command and args come from the validated manifest;
- environment inheritance is minimal and later configurable by allowlist;
- stdout is the protocol channel;
- stderr is captured only as bounded redacted diagnostics;
- every inbound/outbound message has size limits;
- startup, registration, invoke, idle, and shutdown all have timeouts;
- process termination cleans up descendants where the platform supports it.

Hosts cannot append session events, mutate transcripts, talk to providers, or
decide provider continuation. They return registration metadata and tool
results to yach. Yach owns validation, permissions, review, result shaping,
session evidence, provider continuation, and replacement provenance.

## Approach Options

### Option A: Copy Pi Install And Load Semantics Now

Implement npm/git/local install, conventional directories, local single-file
loading, dependency install, and missing project package auto-install in one
slice.

This gives the best immediate UX, but it puts the riskiest work on the critical
path early. It also increases the chance that package-manager latency or host
startup sneaks back into TUI startup behavior.

### Option B: Keep Environment-Only Package Roots

Avoid install work and continue using `YACH_EXTENSION_PACKAGE_ROOTS` plus
diagnostics while building more host protocol features.

This is too small. It is useful for tests and dogfooding, but it does not shape
the product API or make extensions feel real to users.

### Option C: Staged Install Records Plus Host Lifecycle

Add durable local-path install records, explicit enable/disable/remove/list
commands, real process-host activation, and diagnostics first. Keep npm/git ref
parsing and record shape in the design, but defer network/package-manager
adapters until the local path and host lifecycle are proven.

This is the recommended option. It preserves the eventual Pi-like UX while
limiting the first implementation to file-local behavior that can be tested
without network access and without touching first paint.

## Install Scopes And Files

User scope:

```text
~/.yach/extensions.json
~/.yach/extensions/packages/
```

Project scope:

```text
.yach/extensions.json
.yach/extensions/packages/
```

Ephemeral scope:

```text
$TMPDIR/yach/extensions/<session-id>/
```

The settings file should be inspectable JSON with a version field:

```json
{
  "schema": "yach.extensions.v1",
  "packages": [
    {
      "source": "./extensions/fff",
      "kind": "local_path",
      "scope": "project",
      "enabled": true,
      "package_root": "./extensions/fff",
      "resources": {
        "extensions": "all",
        "static_context": "all"
      }
    }
  ]
}
```

Relative project paths resolve relative to `.yach/extensions.json`. Relative
user paths resolve relative to the current working directory at install time and
should be stored as absolute paths unless the user explicitly asks for a
portable project record.

Project scope should only participate after project trust. In an untrusted
project, yach may report project extension records as blocked, but it should not
activate them or run install/update hooks.

## Ref Model

Supported ref syntax should be:

```text
npm:@scope/pkg@1.2.3
npm:pkg
git:github.com/user/repo@v1
git:git@github.com:user/repo@v1
https://github.com/user/repo
ssh://git@github.com/user/repo@v1
/absolute/path/to/package
./relative/path/to/package
```

The first implementation should:

- parse and normalize every supported ref family;
- install local path refs only;
- reject npm/git install attempts with categorical "adapter unavailable"
  diagnostics;
- preserve the normalized ref shape so npm/git adapters can be added without
  changing settings format.

Npm installation should later shell out to a configured package manager during
explicit install/update only. Git installation should later shell out to `git`
during explicit install/update only. Neither adapter should run from TUI startup
or background extension scan.

## Command UX

Initial command surface:

```text
yach install <ref> [--user|--project] [--disabled]
yach extension install <ref> [--user|--project] [--disabled]
yach extension remove <id-or-ref> [--user|--project]
yach extension enable <id-or-ref> [--user|--project]
yach extension disable <id-or-ref> [--user|--project]
yach extension list [--all]
yach extension doctor [<id-or-ref>]
yach extension reload [<id-or-ref>]
```

Recommended defaults:

- user scope by default for global invocations;
- project scope only when `--project` is explicit;
- local path installs enabled by default;
- npm/git installs disabled until adapters land;
- project extension activation blocked until trust;
- `reload` is a development command that rescans and restarts matching hosts
  after first paint or in CLI diagnostics mode.

`list` should show install record state, manifest discovery state, activation
state, provider-visible tool state, and replacement provenance. `doctor` should
include categorical failures without raw command lines or secrets.

## Local Developer Workflow

The first good developer loop should be:

```text
yach extension init --ts ./my-ext
yach extension install ./my-ext --user
yach extension reload my.extension.id
```

The generated TypeScript template can be manifest-first:

```text
my-ext/
  yach.extension.json
  package.json
  src/index.ts
```

The manifest should point to an explicit command such as:

```json
{
  "main": {
    "command": "npm",
    "args": ["run", "yach-host"]
  }
}
```

This keeps Rust core language-agnostic. A future SDK can make authoring easier,
but yach should validate manifests without executing TypeScript.

Single-file `.ts` or `.js` loading should not be the first implementation. To
preserve the Pi-like drop-in path without hidden execution, add it later as a
development helper that synthesizes a manifest file on disk or requires a
neighboring manifest. Yach should not execute a file just to discover its
manifest.

Rust extension templates should use the same stdio protocol:

```text
yach extension init --rust ./my-rust-ext
```

Compiled Rust hosts do not get a privileged in-process route by default. If
process overhead later becomes measurable, an in-process Rust plugin design can
be evaluated separately.

## Activation Lifecycle

Activation triggers should be explicit and conservative:

- `background_metadata`: after first paint, for enabled user-scope extensions
  that declare provider-visible metadata tools;
- `manual`: from `yach extension reload/activate`;
- `on_command:<command>`: when a user or future TUI command explicitly requests
  an extension command;
- `turn_ready`: before a provider turn only when a tool is explicitly requested
  by policy and the wait budget allows it.

Project-scope activation requires project trust. Missing project packages
should be reported as missing, not installed automatically at startup. A future
prompt may offer to install missing project packages, but it must be explicit
and outside first paint.

Activation should be cancellable. Slow or failed activation degrades the
extension, not the TUI. If a provider-visible extension tool is not active by
turn start, it is omitted from that turn's provider request and can appear in a
later turn after activation succeeds.

## Host Protocol Additions

The existing protocol already has:

- `extension.initialize`
- `extension.ready`
- `tool.register`
- `tool.invoke`
- `tool.result`

The host lifecycle slice should add or reserve:

- `extension.registered`: host finished initial registration;
- `extension.shutdown`: yach requests graceful exit;
- `extension.error`: host reports a categorical startup/runtime error;
- `tool.error`: host returns a categorical tool failure.

`extension.registered` is useful because manifest-declared contribution counts
should not be the long-term only way to know registration is complete. The first
implementation may still use manifest-declared expected tool counts to keep the
slice small, but the protocol should reserve an explicit completion message.

All failures should map to categorical labels:

- `spawn_failed`
- `unsupported_protocol`
- `extension_id_mismatch`
- `registration_timeout`
- `registration_malformed`
- `registration_collision`
- `invoke_timeout`
- `invoke_malformed`
- `result_too_large`
- `host_exited`

## Trust And Permissions

Separate these decisions:

- package installation permission;
- project install record trust;
- activation permission;
- provider visibility;
- tool risk permission;
- replacement permission;
- invocation approval.

Initial defaults:

- user local-path extensions can be discovered and activated after first paint;
- project extensions are discovered only after trust and activated only after
  trust;
- metadata tools can be provider-visible only after registration and policy;
- replacement remains disabled unless explicitly configured;
- content, mutation, shell/process, and network extension tools remain out of
  provider-visible scope until separately designed.

This mirrors the important Codex-style separation: changing who approves an
action must not expand the sandbox, and enabling a package must not silently
grant every tool risk.

## Runtime Data Flow

Startup:

```text
yach main
  -> create native backend/session
  -> render TUI first frame
  -> send FirstRenderCompleted
  -> load extension install records
  -> scan enabled package roots
  -> update discovered catalog and diagnostics
  -> activate eligible metadata extensions in background budget
```

Provider turn:

```text
user prompt
  -> snapshot active tool catalog
  -> apply replacement policy and permissions
  -> advertise schema-only provider tools
  -> provider emits tool call
  -> yach validates and authorizes
  -> extension host invoked when route is extension-owned
  -> yach records redacted evidence and provenance
  -> provider receives bounded result
```

Reload:

```text
yach extension reload <id>
  -> stop active host
  -> rescan package root
  -> start host if policy allows
  -> replace active registrations for future turns
```

## Diagnostics And Observability

Diagnostics should report:

- install record source, scope, enabled state, and package root;
- manifest discovery state and manifest path;
- activation state and latest categorical failure;
- host pid only in local diagnostics, never provider evidence;
- registered tools and provider visibility;
- replacement policy and resolved provenance;
- scan/activation timings.

Startup trace marks should expand to include:

```text
extension_install_records_load_started
extension_install_records_load_finished
extension_host_activation_scheduled
extension_host_spawn_started
extension_host_ready
extension_host_registration_finished
extension_host_activation_failed
```

Benchmark evidence should keep the same first-frame checks:

- baseline native TUI first render;
- installed disabled local extension;
- installed enabled local metadata extension;
- many local install records;
- background host activation latency;
- provider-turn invocation latency through a real process transport.

## Error Handling

Fail closed when:

- settings JSON is malformed;
- a ref is unsupported or ambiguous;
- a local path does not exist;
- npm/git adapter is unavailable;
- package root scan fails;
- project extension is requested before trust;
- host command escapes package-root policy when a relative cwd is specified;
- host spawn, registration, or invocation times out;
- host emits malformed JSONL or mismatched ids;
- registration collides without explicit replacement policy;
- provider visibility is requested before executable route exists.

User-facing errors should be categorical and include extension id or source ref
when safe. Session evidence should avoid raw command lines, raw stderr, raw
arguments, secrets, and unbounded extension output.

## Implementation Slices

1. **Install record model and local path commands.** Add ref parsing, JSON
   settings read/write, local path `install/remove/enable/disable/list/doctor`,
   and package-root collection from user/project/ephemeral records. Keep
   npm/git as parsed but unavailable adapters.
2. **Process-backed persistent host transport.** Replace the current one-shot
   stdout registration helper in runtime paths with a stdin/stdout JSONL
   transport and host session manager. Keep existing helper tests as lower-level
   process protocol coverage if useful.
3. **Activation manager and diagnostics.** Track activation states, background
   metadata activation after first paint, reload/stop behavior, and richer
   diagnostics.
4. **Developer templates.** Add `yach extension init --ts` and `--rust` with
   manifest-first starter projects. Defer single-file synthesis until after the
   manifest template flow is dogfooded.
5. **Package adapters.** Add git and npm materialization behind explicit
   install/update commands, never startup.
6. **Profiling evidence.** Record first-paint, record-load, scan, real host
   activation, and real process invocation latency.

## Acceptance Criteria

This design is ready for implementation planning when it can drive small PRs
that:

- install a local extension package into user or project scope;
- list and doctor install records without spawning hosts;
- preserve first-paint behavior with many installed records;
- launch a real process host after first paint;
- keep provider-visible extension tools unavailable until active registration;
- reload a local development extension without restarting yach;
- record activation/invocation evidence with provenance and redaction;
- leave npm/git adapters and broad high-risk extension tools for later slices.

## Sources

- Pi package docs:
  `https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/packages.md`
- OpenCode plugins:
  `https://opencode.ai/docs/plugins/`
- OpenCode tools:
  `https://dev.opencode.ai/docs/tools/`
- Codex configuration reference:
  `https://developers.openai.com/codex/config-reference`
- Local runtime/replacement design:
  `docs/superpowers/specs/2026-05-20-extension-runtime-tool-replacement-design.md`
- Local startup evidence:
  `docs/benchmarks/extension-runtime-profile-2026-05-23.md`
