# Extension-First Product Posture Design

Status: accepted 2026-08-19 — all owner forks decided
Date: 2026-08-19
Motivation: owner decision 2026-08-18 — challenge yach's core boundary before
Wave 2, using Pi and OpenCode 2 as evidence for an extension-first harness that
can extend itself without surrendering yach-owned invariants.

## Question

What must remain core, and what should be expressible through the same public
extension contracts available to users?

This is a product-posture design, not an implementation plan. It defines the
boundary that later contribution-surface designs must honor. It does not add
new host messages, hooks, UI APIs, or provider-visible capabilities.

## Current yach boundary

Yach already has a conservative extension runtime:

- manifest-first packages and user/project install records;
- process-hosted, language-neutral stdio JSONL hosts;
- post-first-paint discovery and activation;
- active executable registration before provider advertisement;
- metadata-only extension tools and visible static-context files;
- explicit built-in replacement policy with provenance and inherited risk;
- live stop, reload, status, and diagnostics through `yach-proto`;
- an allowlisted host environment, but no OS sandbox claim;
- a full `ClientEvent`/`ServerEvent` stdio boundary and deterministic invariant
  matrix through `yach rpc`.

The current manifest exposes only tools and static context. The runtime has no
result-transform hook, result-retention contract, command contribution, model
role or subagent contribution, status entry, provider contribution, or generic
lifecycle hook.

This is intentionally narrower than the accepted direction in
`2026-05-20-extension-runtime-tool-replacement-design.md`. The question now is
whether that narrowness is a permanent boundary or only the safe first slice.

## Cohort evidence

### Pi: the extension API is much of the product API

Pi loads trusted TypeScript modules in process with the user's full system
permissions. Its extension surface includes:

- custom and replacement tools;
- ordered lifecycle events across input, prompt construction, provider calls,
  messages, tools, compaction, sessions, and resources;
- pre-tool blocking and chained post-tool result transforms;
- context filtering, system-prompt replacement, and custom compaction;
- provider registration and custom streaming implementations;
- commands, shortcuts, flags, session entries, and extension state;
- transcript renderers, tool renderers, footer status, widgets, custom TUI
  components, and a full custom footer;
- UI behavior bridged into RPC mode.

Complex features such as plan mode, presets, sandbox routing, custom providers,
and custom compaction ship as extensions. Pi therefore proves the strongest
version of the self-extension story: an extension can reshape most harness
behavior without a core fork.

The costs are equally concrete:

- extension factories are awaited during startup;
- extension code shares the process and full user authority;
- mutable lifecycle hooks expose broad internal shapes;
- ordering is extension load order;
- compatibility depends on a large TypeScript API surface.

Pi does isolate many non-security hook failures: its runner catches errors,
reports the extension and event, and continues with the current value. Ordered
`tool_result` handlers see prior handlers' changes. Pre-tool blocking is a
stricter path and may fail the call instead of silently weakening a gate.

### OpenCode 2: a broad server plugin layer, not an everything-UI layer

OpenCode loads JavaScript/TypeScript plugins in process. Plugins receive an SDK
client plus Bun's shell API and can contribute tools, auth and provider behavior.
Typed hooks cover configuration, messages, model parameters and headers,
permissions, commands, tool arguments and results, shell environment, message
and system transforms, compaction, and tool definitions.

OpenCode also dogfoods the plugin shape inside the product: built-in auth and
provider integrations are assembled through the same plugin interface used by
external packages. That is strong evidence for making public contribution
contracts capable enough to host first-party features.

OpenCode is not literally "everything is a plugin":

- its plugin module type explicitly has no TUI implementation (`tui?: never`);
- the server, session state machine, tool registry, permission engine, event
  transport, and SDK boundary remain core;
- plugins mutate typed outputs at named seams rather than receiving arbitrary
  references to every subsystem.

The costs differ from Pi but remain real:

- npm plugins and dependencies may be installed with Bun during startup;
- plugins execute in process with broad authority;
- hooks execute sequentially in load order;
- ordinary trigger failures propagate unless the caller handles them;
- mutable hook payloads make behavior composition order-sensitive.

### What the cohort actually establishes

Both harnesses support a stronger extension story than yach today. Neither
eliminates core. Their cores retain the state machine and expose named seams
around it. The useful challenge is therefore not "core or extensions". It is:

> Can first-party and third-party product behavior use the same explicit,
> versioned contracts while the core remains the authority for invariants?

For yach, the process and protocol boundaries make that harder than an
in-process TypeScript callback, but also more durable and testable.

## Decision: extension-first microkernel

Yach should adopt an **extension-first microkernel** posture.

The default rule is that product behavior belongs in a public contribution,
interception, or replacement contract when it can be expressed without giving
up a core invariant. First-party features should dogfood those contracts.
Exceptions require a named reason: bootstrap dependency, security authority,
canonical-state ownership, transport semantics, or measured performance.
"It is easier to call an internal Rust function" is not a sufficient reason.

This is stronger than "extensions may add tools" and weaker than "arbitrary
code may mutate every internal object".

## The irreducible core

Core owns mechanisms whose duplication or disagreement would break the product:

1. **Canonical state and identity** — sessions, turns, messages, tool calls,
   correlation IDs, persistence, replay, and migrations.
2. **Protocol semantics** — `ClientEvent`/`ServerEvent`, capability negotiation,
   event ordering, cancellation, backpressure, and transport-independent DTOs.
3. **Provider-loop state machine** — round progression, tool-call correlation,
   retries, continuation validity, usage accounting, and terminal outcomes.
4. **Authority and policy** — project trust, sensitive-path policy, risk classes,
   permission decisions, review gating, and future isolation selection.
5. **Execution brokerage** — validation, cancellation, deadlines, bounded
   results, provenance, durable evidence, and fail-closed routing.
6. **Context accounting** — model-window accounting, compaction checkpoints,
   result masking, replay validity, and provider-native compaction state.
7. **Extension lifecycle** — install records, host activation, protocol
   negotiation, contribution validation, ordering, failure isolation, and stop.

These are kernel responsibilities, not necessarily hard-coded product
features. Core may delegate work through a typed contract while retaining the
state transition and validating the result.

## Public extension planes

### Additive contributions

Extensions should eventually be able to contribute declarative or executable
capabilities without replacing an existing owner:

- tools;
- visible static context;
- commands and key-bound actions;
- model roles, model tags, and subagent definitions;
- provider/catalog adapters;
- protocol-renderable status entries and transcript row descriptors;
- future skills, prompts, and themes.

Each surface needs its own schema, scope precedence, trust rule, lifecycle, and
capability negotiation. Listing it here is directional, not permission to add
untyped manifest fields.

### Explicit replacements

A first-party or third-party extension may replace a named implementation only
through a compatibility contract and explicit policy. Tool replacement already
has this shape. Future candidates include provider adapters, compactors, and
resource strategies.

Replacement never transfers ownership of canonical state. For example, a
compactor may return a replacement artifact, while core still selects the cut,
validates the artifact, records the checkpoint, updates accounting, and owns
replay.

Coordinated replacement is first-class. Hashline read plus hashline edit is one
feature and must be activated atomically as a declared set, not as two unrelated
best-effort collisions.

### Typed interceptors

Yach should allow narrowly typed interceptors where replacement is the wrong
shape. An interceptor may observe a core-owned value and return a bounded patch
at a named phase. It does not receive a mutable backend handle.

Candidate phases, each requiring a separate design before implementation:

- post-execution tool-result transform;
- prompt/context contribution;
- compaction context contribution;
- provider request metadata/header contribution;
- observational lifecycle events.

There is no generic `on(any_event, mutate_anything)` API. Adding a phase means
specifying input/output schemas, ordering, timeouts, provenance, failure mode,
persistence, replay behavior, and RPC matrix scenarios.

## Immediate contract conclusions

### Tool-result transforms

The OMP-class shell minimizer should be reachable without replacing the shell
executor. The eventual seam should be a post-execution, pre-provider-result
transform with these invariants:

- core captures the bounded original before calling transforms;
- transforms receive a typed result envelope, not executor internals;
- transforms may change provider-visible content and display metadata, but may
  not rewrite outcome, permission, provenance, byte accounting, or evidence;
- activation and ordering are explicit policy, not incidental load order;
- every transformed result records the transform chain and original digest;
- failure disables or skips the transform and falls back to the bounded
  original; it never converts a failed tool into success;
- retaining full original output needs a separate local artifact-store design;
  the current session log stores bounded provider-visible content and is not
  silently repurposed as an artifact store.

No transform host protocol is added by this posture pass.

### Tool-result retention

Compaction currently treats eligible completed tool results uniformly. Tool
contracts should eventually declare one of two core-enforced retention classes:

- `maskable` — default; core may replace an old result with the standard marker;
- `protected` — keep through masking while the tool's declared invariant needs
  it.

Extensions may request `protected`; core validates and enforces it. Extensions
do not delete or pin session events themselves. More states such as
"mask after consumed" should be added only when a concrete lifecycle can define
and test "consumed" without model-behavior inference.

No retention field is added by this posture pass.

### Roles, subagents, providers, and status

These are legitimate extension-first surfaces, but they are not tool hooks:

- roles/tags belong to model-catalog resolution;
- subagents are declarative resources referencing roles and allowed tools;
- provider adapters require a typed streaming/auth/catalog contract;
- status entries are protocol data with priority and width behavior, not TUI
  component code.

The posture keeps all four reachable. Each remains deferred until its own design
has a product need and invariant matrix.

## UI boundary

Yach should not copy Pi's arbitrary in-process TUI component injection into a
remote-capable architecture.

Extension hosts may contribute protocol-renderable descriptors and issue typed
UI requests. The backend validates and emits them; clients decide how to render
the negotiated capability. A future client-side extension system can be
separately designed if descriptor-based UI proves insufficient.

This follows OpenCode's useful split: a broad server plugin surface does not
require its TUI to load the same plugin module. It also preserves the headless
design's rule that backend behavior remains reachable through protocol events.

## Authority and trust

Process hosting is a fault and language boundary, not a sandbox. Today an
extension host can still use its OS account to access local resources even
though yach strips ambient secrets and brokers only narrow APIs.

Therefore:

- manifests and diagnostics must state requested contribution capabilities;
- yach must not claim filesystem or network isolation without a later sandbox;
- project extension activation remains trust-gated;
- brokered callbacks should be preferred because they preserve yach policy and
  evidence, even before OS isolation exists;
- a future trusted/unrestricted profile may deliberately grant broader host
  capabilities, but it must be explicit rather than inferred from installation.

## Ordering and failures

Load order is too weak for durable composition. Mutating interceptors use an
explicit ordered policy. Additive contributions use deterministic scope and ID
ordering where order is semantically irrelevant. Conflicts fail closed unless a
replacement or composition rule resolves them.

Failure policy is phase-specific:

- observational and display contributions: disable/degrade the extension and
  continue;
- result transforms: use the validated original and continue;
- replacement execution: return a structured tool failure; do not silently run
  a different implementation after side effects may have begun;
- policy/security interceptors: fail closed;
- canonical-state validation: reject the returned patch or artifact.

This combines Pi's useful per-extension isolation with yach's requirement that a
failure cannot weaken policy.

## Invariant-matrix rule

The new stdio RPC boundary becomes the acceptance surface for extension work.
Every new contribution, interceptor, or replacement contract must add exact
scenarios covering its observable invariants, including as applicable:

- inactive/discovered/active/stopped lifecycle transitions;
- capability negotiation and headless reachability;
- deterministic ordering and collision behavior;
- extension timeout, crash, malformed output, and restart;
- policy-denied and untrusted-project paths;
- original/transformed result provenance and failure fallback;
- retention through masking, compaction, resume, and replay;
- first-party bundled and external implementations obeying the same contract;
- TUI and RPC clients observing the same backend event semantics.

Unit tests remain appropriate for parsers and reducers. The matrix owns composed
behavior across protocol, runner, session log, host fixture, and project state.

## Sequencing

1. Do not implement result transforms, retention classes, roles, subagents,
   providers, or status entries merely because this design names them.
2. Start the Wave 2 review-UX spec.
3. When a future feature needs one of these surfaces, write a focused design and
   use the RPC matrix as its acceptance harness.

## Owner decisions

1. **DECIDED — public-contract dogfooding defaults on, with named exceptions.**
   First-party behavior uses public extension contracts by default. Bootstrap,
   security authority, canonical state, transport semantics, or measured
   performance may justify a core implementation; convenience does not.
2. **DECIDED — typed interceptors are part of the product direction.** Yach may
   expose named, versioned, bounded mutation phases. It will not expose a generic
   mutable lifecycle event bus.
3. **DECIDED — UI extensions use protocol descriptors and typed requests.**
   Arbitrary client component code remains undecided. Backend contributions must
   remain renderable by negotiated TUI, headless, and future remote clients.

## Sources

- Pi extension documentation:
  `https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md`
- Pi extension runner:
  `https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/extensions/runner.ts`
- OpenCode plugin documentation: `https://opencode.ai/docs/plugins/`
- OpenCode plugin contract:
  `https://github.com/anomalyco/opencode/blob/dev/packages/plugin/src/index.ts`
- OpenCode plugin loader:
  `https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/plugin/index.ts`
- Existing yach runtime/replacement design:
  `docs/superpowers/specs/2026-05-20-extension-runtime-tool-replacement-design.md`
- Headless protocol boundary:
  `docs/superpowers/specs/2026-08-18-headless-protocol-boundary-design.md`
