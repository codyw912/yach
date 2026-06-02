# Extension Activation Manager Design

Date: 2026-06-02
Status: draft

## Context

Yach now has most of the low-level pieces needed for a real extension runtime:

- local-path user/project install records;
- post-first-paint manifest scanning;
- process host/session protocol primitives;
- extension executor routing through the native tool workflow;
- provider-turn catalog resolution with explicit replacement policy;
- extension static-context placeholders and profiling evidence.

The remaining gap is lifecycle ownership. Installed package roots can be scanned,
and host sessions can register or invoke tools, but yach does not yet have one
runtime component that owns extension activation state, start/stop/reload,
diagnostics, failed-host behavior, and the rule that provider-visible tools come
only from active executable registrations.

Comparable harnesses point at the shape:

- Pi makes extensions feel immediately hackable, including install refs and
  drop-in local files, but each loaded extension can add startup latency.
- OpenCode gives plugins broad hooks and TypeScript ergonomics, but plugin
  loading is startup-coupled.
- Codex keeps tools, MCP servers, apps, plugins, trust, and permissions under
  explicit policy surfaces, which better matches yach's safety and provenance
  goals.

Yach should keep the Pi-like developer loop while making activation a
Rust-owned, observable, post-first-paint state machine.

## Goal

Design an extension activation manager that coordinates discovered manifests,
install records, host sessions, active registrations, reloads, and diagnostics
without regressing TUI first paint.

The design should be sufficient for implementation planning. The first
implementation should focus on local-path records and background metadata-tool
activation, using the existing process host/session primitives where possible.

## Non-Goals

- No implementation in this slice.
- No npm/git package adapters or package-manager integration.
- No TypeScript or Rust extension templates.
- No sandbox implementation.
- No project trust implementation beyond preserving blocked-state seams.
- No extension-owned shell/process, network, broad mutation, or high-risk tools.
- No hidden system-prompt mutation.
- No in-process plugin ABI.
- No broad hook system that can arbitrarily mutate yach internals.

## Design Principles

### First Paint Remains Extension-Cold

The TUI first render should not wait on extension host spawn, dependency
install, network access, deep package scans, or host registration. Startup may
read a small cached index only when it fits an explicit budget; otherwise
extension work starts after first paint.

After first paint, the activation manager may load install records, consume
manifest scan results, and schedule background activation for eligible
extensions. Users should be able to type immediately even when many extensions
are installed.

### Active Means Executable And Accepted

Provider-visible tools must come only from active registrations. A manifest-only
tool declaration can help diagnostics and activation planning, but it should not
be advertised to the provider until a host process has registered the tool and
yach policy has accepted it.

Late activation affects future provider turns only. Yach should not mutate an
in-flight provider request because an extension finishes loading in the
background.

### The Manager Owns State, The Session Owns Transport

The activation manager owns lifecycle state and routes runtime events:

- install/discovery records;
- activation eligibility;
- host start/stop/reload;
- accepted registrations;
- active catalog projection;
- diagnostic projection.

`ExtensionHostSession` and `ExtensionHostTransport` own protocol I/O for one
host process. They should not decide provider advertisement, replacement
policy, session evidence, or user-facing lifecycle state.

## State Model

Extensions move through explicit states:

```text
installed -> discovered -> blocked
installed -> discovered -> starting -> registering -> active
installed -> discovered -> starting -> failed
active -> stopping -> stopped
active -> reload_requested -> stopping -> starting
```

State names:

- `installed`: an enabled or disabled install record exists.
- `discovered`: manifest scan validated a package root and manifest.
- `blocked`: policy, trust, missing dependency, disabled record, or unsupported
  contribution prevents activation.
- `starting`: the manager is spawning or connecting to a host process.
- `registering`: the host handshake is complete and yach is validating
  registrations.
- `active`: accepted registrations are available for future turns.
- `failed`: startup, handshake, registration, policy, or protocol validation
  failed.
- `stopping`: yach is shutting down the host.
- `stopped`: the host is not running and has no active registrations.
- `reload_requested`: a stop/start transition has been requested.

A diagnostic state record should include:

```text
extension_id
version
scope
source_ref
install_source
package_root
manifest_path
activation_state
generation
last_error_kind
last_error_summary
registered_tool_count
registered_tools
provider_visible_tools
started_at
registered_at
stopped_at
```

Diagnostics should avoid raw commands, raw stderr, environment values, provider
payloads, secrets, and unbounded text. Errors should be categorical with bounded
summaries.

## Inputs

The activation manager consumes:

- enabled user install records;
- enabled project install records only after project trust is established;
- post-first-paint manifest scan results;
- future policy/profile decisions;
- manual reload, enable, disable, and activate commands;
- first-render completion;
- shutdown/cancellation events.

Project-scoped extensions should remain `blocked` until project trust exists.
This design does not implement project trust, but it keeps the state boundary
explicit so project activation does not become implicit later.

## Activation Triggers

### Background Metadata Activation

After first paint, enabled user extensions with safe metadata/provider-visible
tool contributions may activate in the background. This is the first recommended
trigger because it proves the host lifecycle while preserving startup
performance.

Background activation should use small timeouts and bounded concurrency. A slow
or failed extension should transition to `failed` or remain inactive without
blocking the main binary or input loop.

### Manual Reload Or Activate

Developers need a fast local loop:

```text
yach extension reload <id>
yach extension activate <id>
```

Reload stops the current host, rescans the package root, increments the
generation, starts the new host, and updates future-turn catalog snapshots if
registration succeeds. Stale events from the old generation are ignored.

### Turn-Ready Activation

A future trigger may let yach spend a small explicit budget before a provider
turn to activate tools likely needed by that turn. This should not be in the
first implementation unless the policy and latency budget are explicit.

## Runtime Flow

Startup:

1. Render the TUI.
2. Load lightweight install records and cached index data.
3. Scan enabled package roots in the background.
4. Send discovered manifests to the activation manager.
5. Schedule eligible background metadata activation.
6. Register accepted tools into the active extension catalog.

Provider turn:

1. Snapshot the active built-in and extension catalog before request assembly.
2. Apply explicit replacement/alias policy with provenance.
3. Advertise only active provider-visible tools.
4. Route extension tool invocations through the active host route.
5. Persist yach-owned evidence and shape provider-visible tool results.

Reload:

1. Mark the extension `reload_requested`.
2. Stop the current host.
3. Clear active registrations for that generation.
4. Rescan the package root.
5. Start and register a new host.
6. Publish the new active catalog projection for future turns.

## Host Lifecycle Rules

Host process behavior:

- spawn cwd defaults to the package root;
- command and args come from the validated manifest;
- environment inheritance is minimal and later allowlisted;
- stdout is the JSONL protocol channel;
- stderr is captured only as bounded, redacted diagnostics;
- startup, handshake, registration, invocation, idle, and shutdown have
  explicit timeouts;
- one invocation at a time per host is acceptable for the first implementation;
- stale events are ignored by generation and request id;
- graceful shutdown can use a reserved `extension.shutdown` message when the
  protocol grows it;
- hard kill is the fallback after shutdown timeout.

Host processes do not own provider continuation, transcript mutation, session
evidence, permissions, or replacement policy. Yach owns those surfaces.

## Diagnostics

`yach extension list` and `yach extension doctor` should show enough state for a
user to understand why a tool is or is not available:

- install scope and enabled flag;
- package root and manifest path;
- activation state;
- last categorical error;
- registered tool names;
- provider-visible tool names;
- replacement provenance when a tool replaces or aliases another tool.

The diagnostic output should distinguish:

- installed but disabled;
- installed but not discovered;
- discovered but blocked;
- starting/registering;
- active;
- failed;
- stopped.

## Approach Options

### Option A: Activate Everything After Scan

Start every discovered extension host after first paint.

This is simple but not recommended. It can reproduce the Pi startup pain in the
background, make slow extensions noisy, and waste work for extensions that only
contribute specialized features.

### Option B: Manual Activation Only

Require users to activate or reload every extension explicitly.

This protects performance, but it makes normal installed extensions feel broken
because provider-visible tools would not appear until a manual command runs.

### Option C: Activation Manager With Background Metadata Activation

Introduce the manager, activate eligible user metadata tools after first paint,
keep project extensions blocked until trust, and provide manual reload for
developer flow.

This is the recommended option. It proves host lifecycle and provider-visible
tool registration without putting extension work on the first-frame path.

## Implementation Slices

1. Add activation state records and diagnostic projection without spawning
   hosts.
2. Wire post-first-paint discovered manifests into the manager and mark enabled,
   disabled, blocked, and failed states.
3. Add background metadata activation using the existing host session/transport
   primitives.
4. Route active registrations into provider-turn catalog snapshots and ensure
   inactive/failed extensions are omitted.
5. Add reload/stop command handling for local developer workflow.
6. Add profiling evidence for first paint, activation latency, and reload.

## Test Plan

- Unit-test state transitions, including failure and reload generation changes.
- Use fake hosts for activation success, timeout, protocol error, and tool-name
  collision behavior.
- Verify no host spawn occurs before first render in the native runner path.
- Verify inactive, failed, disabled, and blocked extensions are omitted from
  provider-visible tool snapshots.
- Verify active registrations route invocations through the expected host
  generation.
- Verify list/doctor render categorical activation state without leaking raw
  stderr or environment data.
- Add profiling/report evidence once host activation is wired.

## Acceptance Criteria

- TUI first paint remains extension-cold: no host spawn before first render.
- Enabled user local-path extensions can activate metadata/provider-visible
  tools after first paint.
- Provider turns advertise only active accepted registrations.
- Disabled, blocked, failed, and inactive extensions are omitted from
  provider-visible tools.
- Extension diagnostics show activation state, last categorical error, and
  provider-visible tools.
- A local extension can be stopped/reloaded without restarting yach in a later
  slice.
- Stale host events cannot affect the active catalog after reload.
