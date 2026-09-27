# Distribution Presets and Reference Components

**Outcome:** plane:YACH-12

Status: draft for owner review, 2026-09-27.

## Problem and outcome

Yach is meant to be a minimal, compiled, extension-first harness: a Pi-like
core without Pi's TypeScript startup cost. Today it ships as one binary whose
model-facing behavior is fixed in core:

- a hard-coded system guidance message on every provider request
  (`PROVIDER_BASELINE_GUIDANCE`, `crates/yach-backend/src/runner.rs:3902`);
- a hard-coded list of seven provider-visible tools
  (`provider_approved_tools`, `runner.rs:4096`);
- two first-party extensions, hashline and the Jev reviewer, linked into the
  binary and re-seeded as install records on every start, so users can disable
  them but not remove them (`crates/yach-cli/src/main.rs:4215,4280`;
  `crates/yach-backend/src/extension_install.rs:210-219`);
- a hard-coded compaction summary prompt
  (`crates/yach-backend/src/compaction.rs:909-935`).

New models arrive often, and harnesses tuned to one model's habits behave
differently, better or worse, on the next. Users need to strip yach back to a
model's native behavior and build up only what their workflow needs. First-party
features are **reference implementations**: we build what we want to use, but
nobody is locked into it.

Deliver:

1. a kernel that adds no model-facing guidance of its own;
2. first-party **reference components** that are enabled, disabled, and
   removed as units;
3. named **presets** that select components, with `full` as the default
   install and `minimal` as the strip-back option.

## Relationship to existing designs

- Honors the extension-first microkernel posture
  (`2026-08-19-extension-first-product-posture-design.md`): kernel owns state,
  protocol, the provider loop, authority, execution brokerage, context
  accounting, and extension lifecycle. Everything here is a mechanism or a
  component; no new host message, hook, or interceptor is added.
- Keeps the coordinated-replacement rules of the hashline bundle design
  (`2026-08-21-hashline-extension-bundle-design.md:215-222`) unchanged.
- Skills are specified separately (plane:YACH-14). This spec reserves a
  `skill-index` component slot for them.
- Binary-size targets per preset and release-profile tuning are plane:YACH-13.

## Layers

### Kernel

Unchanged responsibilities. For this design the kernel:

- owns the `bash` tool, because execution brokerage (permission decisions,
  auto-review, bounded output, evidence) is a kernel responsibility. A preset
  may still leave `bash` unadvertised;
- brokers every tool, whoever defines it;
- assembles provider context from the session log, static context, and active
  components;
- emits **no guidance text**. Structural framing the kernel must own remains:
  tool schemas for tools it defines, the static-context and compaction-summary
  wrappers (`runner.rs:3745`), and provider-protocol envelopes. Those carry
  data, not instructions about how to behave.

### Reference components

First-party, versioned with the binary, each toggled as one unit.

| Component | Provides | Implementation |
|---|---|---|
| `project-tools` | `project_path_info`, `read_text_file`, `search_project`, `list_project_paths`, `edit_text_file`, `create_text_file` | In-process (see below) |
| `baseline-guidance` | The text now in `PROVIDER_BASELINE_GUIDANCE`, as a system-placement static-context item | In-process static-context contribution |
| `hashline` | Coordinated replacement of `read_text_file` / `edit_text_file` | Existing subprocess extension, unchanged |
| `jev-reviewer` | Automatic-review reviewer | Existing subprocess extension, unchanged |
| `skill-index` | Reserved for plane:YACH-14 | Defined by the skills spec |

`project-tools` stays in-process under the posture spec's measured-performance
exception. The 2026-09-12 baseline measures a four-tool-call scripted turn at
27.48 ms p50 with built-in tools and 42.16 ms p50 with hashline's subprocess
tools, both across the same child-process boundary
(`docs/benchmarks/baseline-2026-09-12.md:219-220,233`). The ~15 ms gap is not
yet attributed between host round-trip, activation, and tool-path differences;
attribution needs `extension_id` on tool trace marks and is a follow-up. The
exception covers first-party convenience only: a third-party replacement for
any project tool uses the ordinary subprocess extension contract.

`baseline-guidance` is a component rather than kernel text because it was
tuned from dogfooding specific models
(`docs/project/records/2026-07-20-baseline-prompt-cohort-check.md`); it is
exactly the kind of opinion a user studying a new model must be able to drop.

### Third-party extensions

Unchanged: manifest-declared subprocess hosts with user/project install
records, trust gating, and capability grants.

## Presets

A preset is a named set of components. The term is deliberately not
"profile": yach already uses model profile for catalog resolution
(`yach_catalog::ModelProfile`), and Cargo profiles are a separate build
concept (plane:YACH-13). Two presets ship:

| Preset | Components |
|---|---|
| `minimal` | `bash` (kernel), `skill-index` |
| `full` (default install) | `bash`, `project-tools`, `baseline-guidance`, `hashline`, `jev-reviewer`, `skill-index` |

Rules:

1. **Presets select components only.** They never change approval mode,
   project trust, capability grants, shell allowlists, or the auto-review
   execution gate. Those remain kernel policy owned by the user.
2. **Applying a preset is an action, not a live rule.** `yach preset use
   <name>` writes the component set and the bundled install records once.
   Afterwards each component is ordinary user state: users add or remove
   individual components without the preset reasserting itself.
3. **First run applies `full`, preserving existing choices.** A missing
   `[preset]` record means "apply `full` and record it". For installs that
   predate this design, the first apply keeps every existing bundled record's
   `enabled` value: a user who disabled hashline or Jev keeps it disabled.
   Only components with no prior record take the preset's default. Later
   starts never re-apply a preset; see "Bundled records across upgrades" for
   the one startup action that remains.
4. **Ephemeral selection for evaluation.** `yach run --preset <name>` (and
   the equivalent `yach rpc` initialization option) uses a preset for that
   session only, without writing user state. This lets evaluation harnesses
   compare a model under `minimal` and `full` on the same checkout.
5. **`jev-reviewer` under `full`** is installed and enabled, but still needs
   its existing network capability grant, and automatic execution stays behind
   `AUTO_REVIEW_EXECUTION_ENABLED`. Installing a component grants nothing.

### What `minimal` means for approval modes

With only `bash`, every file read and change goes through the shell path:

- `review` and `auto-review` treat each read and write as a shell command
  (subject to the allowlist and reviewer);
- `accept-edits` has no hash-checked structured edits to auto-apply;
- reads lose the bounded-result and resource-broker path of `read_text_file`.

This is the intended behavior for observing a model's native tool use. The
preset does not change the approval mode to compensate; user documentation and
`yach preset use minimal` output state these consequences.

## Component state and storage

- **Kernel-provided components** (`project-tools`, `baseline-guidance`) are
  toggled in `~/.yach/config.toml`:

  ```toml
  [preset]
  applied = "full"          # last preset applied; absent = first run

  [components]
  project-tools = true
  baseline-guidance = true

  [bundled]
  removed = []              # bundled extension ids the user removed
  ```

  `[components]` holds only component-name booleans; unknown names produce a
  diagnostic and are ignored. `[bundled] removed` is a separate table so it is
  never mistaken for a component.
- **Extension components** (`hashline`, `jev-reviewer`) stay install records in
  `~/.yach/extensions.json`. Applying a preset installs and enables, or
  removes, their bundled records.
- **Bundled records become removable.** `ExtensionInstallStore::remove` stops
  rejecting `Bundled` records, and normal startup and management commands stop
  calling `ensure_bundled_*` to seed missing records. Seeding happens only when
  a preset is applied or a user runs `yach extension install --bundled <id>`.
- **Removal is remembered.** Removing a bundled extension deletes its install
  record and adds its id to `[bundled] removed` in `~/.yach/config.toml`.
  Neither startup refresh nor a later preset apply re-adds a removed id; only
  `yach extension install --bundled <id>` or `yach preset use <name> --reset`
  clears it. `--reset` reapplies the preset's defaults to every component.
- **Scope.** Component toggles are user scope in this slice. Project-scoped
  component restrictions are deferred.
- **When changes apply.** CLI changes take effect for new sessions. A running
  session keeps the catalog it started with. In-session toggling is deferred.

CLI surface:

```text
yach preset list | show | use <minimal|full> [--reset]
yach component list
yach component enable <name> | disable <name>
yach extension install --bundled <hashline|jev-reviewer>
```

`yach component list` reports each component's source (kernel, bundled
extension, external extension), state, and, for the core build, whether it is
compiled in.

### Bundled records across upgrades

Bundled manifests are materialized per yach version under
`~/.yach/bundled/<package>/<version>` (`crates/yach-cli/src/main.rs:4167-4200`),
and today's per-start `install_bundled` call is what repoints an existing
record's `package_root` to the new version (`extension_install.rs:192-195`).
Dropping per-start seeding must not drop that refresh.

On startup, when the running yach version differs from a bundled record's
materialized version, the kernel re-materializes that package's manifest for
the current version and updates the record's `package_root`, keeping its
`enabled` value. The materialized version is the last path component of
`package_root` (`~/.yach/bundled/<package>/<version>`); no new record field is
added. This refresh only touches records that exist: it never creates a record,
and never touches ids in `[bundled] removed`. It is cheap (one
manifest write per bundled package per upgrade) and runs before extension
discovery, which is already post-first-paint.

### Components not compiled into this build

A preset or component command naming a bundled extension that this build
omits (see Build packaging) records the preference, reports "not compiled in"
in `yach component list` and the extension diagnostics, and continues. It is
never a startup failure. If the same user state is later used by a build that
includes the component, the recorded preference applies.

## Tool resolution

Per provider turn, the advertised tool set is resolved in this order:

1. kernel `bash`;
2. `project-tools` members, if the component is enabled;
3. activated extension registrations;
4. declared replacements, under the existing rules.

The hashline bundle design already defines replacement
(`2026-08-21-hashline-extension-bundle-design.md:215-222`); this spec only
states the consequences under presets:

- **`full`:** hashline replaces `read_text_file` (`preserve`) and
  `edit_text_file` (`replace`, advertised under the built-in name with the
  extension's schema). The other four project tools stay active.
- **A disabled replacement target is a member failure.** If `project-tools` is
  disabled (for example `minimal` plus hashline), the bundle's builtin targets
  are not registered, so the whole bundle does not activate. A diagnostic names
  the missing target and the disabled component. A replacement never supplies a
  built-in name the preset turned off.
- **Accidental collisions fail closed**, as today. An extension tool whose name
  matches an active tool without a declared replacement is rejected with a
  diagnostic; there is no silent shadowing.

## Compaction prompt (interim)

A typed compaction interceptor would let a component own the summary prompt,
but the summary becomes canonical session state, so that interface needs its
own validation and failure design. It is deferred.

Interim mechanism: a `summary_prompt` field in the `compaction` configuration
(`crates/yach-backend/src/compaction.rs:37-86`) names a UTF-8 file whose
contents replace the instruction preamble and section schema in
`build_summary_prompt`. The kernel keeps ownership of the previous-summary,
focus, and conversation framing, the cut selection, checkpoint recording, and
accounting. An unreadable or empty file fails closed to the built-in prompt
with a diagnostic.

**`summary_prompt` is user scope only.** `CompactionConfig` otherwise lets
project config override user config (`compaction.rs:80-85`), but the summary
becomes canonical session state that later turns treat as authoritative, so a
repository must not be able to rewrite how history is condensed. A
`summary_prompt` key in `<project>/.yach/config.json` is ignored with a
diagnostic. This is stricter than `.yach/APPEND_SYSTEM.md`, which adds project
guidance to the live turn but never rewrites persisted state.

The prompt can be replaced, not omitted: `build_summary_prompt` is the entire
instruction to the summarizing model, and without it the model would answer
the conversation instead of summarizing it. Users who want no yach
summarization prompt set `compaction.enabled = false` or select the native
`compactor = "openai-responses"`. `minimal` does not change compaction.

## Build packaging

`crates/yach-cli` gains default-on features `bundled-hashline` and
`bundled-jev`, gating the extension crate dependencies, their
`__extension-host` dispatch branches (`main.rs:65-84`), and their manifest
materialization. `cargo install yach --no-default-features` builds a core
binary without them. In a core build, applying `full` installs the kernel
components, reports the missing bundled extensions as "not compiled in", and
succeeds.

The size effect is small (hashline and Jev are ~100 KB of symbols together);
the purpose is that a core build contains no first-party extension code at all.
Size targets are plane:YACH-13.

## Acceptance

Covered by the stdio RPC invariant matrix unless noted:

1. `full` (default first run) advertises the same tools and sends the same
   guidance as today's build.
2. `minimal` advertises only `bash` and sends no baseline guidance.
3. Disabling `project-tools` removes all six members atomically in the next
   session; enabling restores them.
4. `minimal` plus enabled hashline: the bundle does not activate, native
   targets are absent, and a diagnostic names the disabled component.
5. An undeclared extension tool named like an active tool fails closed.
6. Removing a bundled extension persists across restarts; neither startup nor
   a later `yach preset use` re-adds it without `--reset` or an explicit
   `yach extension install --bundled`.
7. `yach run --preset minimal` affects only that session; persisted user
   state is unchanged afterwards.
8. Applying a preset changes no approval mode, trust, grant, or allowlist.
9. A custom user-scope `compaction.summary_prompt` replaces the preamble and
   schema and keeps kernel framing; an unreadable file falls back with a
   diagnostic; the same key in project config is ignored with a diagnostic.
10. First apply on an existing install (no `[preset]` record, hashline record
    present and disabled) records `full` and leaves hashline disabled.
11. Upgrade: a bundled record materialized for version N is repointed to the
    version N+1 manifest on the first N+1 start with `enabled` unchanged; a
    removed id stays absent.
12. A preset naming a component this build omits reports "not compiled in"
    and the session starts normally.
13. Unit/build: `--no-default-features` compiles and links neither bundled
    extension crate; `yach component list` reports them as not compiled in.

## Out of scope

- Skills and the `yach-guide` skill (plane:YACH-14).
- Per-preset size targets and release-profile tuning (plane:YACH-13).
- A compaction interceptor or any other new interceptor phase.
- Provider adapters as extensions.
- Project-scoped component restrictions and in-session component toggling.
- npm/git install adapters, package signing, marketplace.
- A first-run preset picker.

## Open follow-ups

- Attribute the built-in versus subprocess tool gap with `extension_id` on
  tool trace marks.
- Decide whether a third preset (for example `full` without `jev-reviewer`)
  earns its place once component toggling is in use.

## Sources

- `docs/project/specs/2026-08-19-extension-first-product-posture-design.md`
- `docs/project/specs/2026-08-21-hashline-extension-bundle-design.md`
- `docs/project/specs/2026-05-20-extension-runtime-tool-replacement-design.md`
- `docs/benchmarks/baseline-2026-09-12.md`
- Comparable harnesses reviewed 2026-09-27: Pi (replaceable `SYSTEM.md`,
  extension-first), mini-swe-agent (bash-only baseline), Terminus (single
  tool), fx (fixed tools and prompt, published performance), Strands harness
  (batteries-included factory over a composable SDK; built-ins disabled with
  `tools=[]`, replacements fail on name collision).
