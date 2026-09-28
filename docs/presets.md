# Presets and components

Yach's model-facing behavior is assembled from named **components**:
self-contained pieces that can be enabled, disabled, or removed as units.
A **preset** is a named set of components. Two presets ship:

| Preset | Components |
| --- | --- |
| `minimal` | `bash` (kernel, always present), `skill-index` (reserved) |
| `full` | `bash`, `project-tools`, `baseline-guidance`, `hashline`, `jev-reviewer`, `skill-index` |

`full` is the default install; it matches what yach has always done.
`minimal` removes the reference components so the session advertises only
the kernel `bash` tool and sends no baseline guidance — the harness's own
additions are out of the way when observing a model's native tool use.
Non-bundled extensions you installed separately still load, and project and
static context (`AGENTS.md`, `.yach/APPEND_SYSTEM.md`) is still sent; those
are configured on their own, outside the preset.

## Components

| Component | Provides | Source |
| --- | --- | --- |
| `project-tools` | `project_path_info`, `read_text_file`, `search_project`, `list_project_paths`, `edit_text_file`, `create_text_file` | kernel |
| `baseline-guidance` | The baseline system guidance text sent on every provider request | kernel |
| `hashline` | Coordinated replacement of `read_text_file` / `edit_text_file` | bundled extension `yach.hashline` |
| `jev-reviewer` | Reviewer for the `auto-review` approval mode | bundled extension `yach.jev-reviewer` |
| `skill-index` | Reserved for the skills work | kernel |

`yach component list` reports one line per component:

```text
component name=project-tools source=kernel state=enabled compiled_in=true
component name=baseline-guidance source=kernel state=enabled compiled_in=true
component name=hashline source=bundled-extension state=enabled compiled_in=true
component name=jev-reviewer source=bundled-extension state=enabled compiled_in=true
component name=skill-index source=kernel state=reserved compiled_in=true
```

Kernel components (`project-tools`, `baseline-guidance`) are `enabled` or
`disabled`. Bundled components (`hashline`, `jev-reviewer`) are `enabled`,
`disabled`, `removed`, or `not-installed`. `skill-index` is always
`reserved`. `compiled_in` reports whether this binary contains the
component's code (see [Build packaging](#build-packaging)).

## Commands

```text
yach preset list | show | use <minimal|full> [--reset]
yach component list | enable <name> | disable <name>
yach extension install --bundled <yach.hashline|yach.jev-reviewer>
```

`yach preset list` prints the preset names. `yach preset show` prints the
applied preset plus the component list. `yach preset use <name>` writes the
preset's component set once, then each component is ordinary user state —
the preset does not reassert itself. Example:

```text
$ yach preset use minimal
preset_action=use
preset=minimal
installed=
enabled=
disabled=
preserved=
not_compiled_in=
note=review and auto-review treat every file read and write as a shell command
note=accept-edits has no hash-checked structured edits to auto-apply
note=reads lose the bounded-result and resource-broker path of read_text_file
```

`yach component enable|disable` takes a component name (`project-tools`,
`baseline-guidance`, `hashline`, `jev-reviewer`; `skill-index` is
reserved). `yach extension install --bundled` takes a bundled extension
id (`yach.hashline`, `yach.jev-reviewer`) and (re)installs that record.

Changes take effect for new sessions; a running session keeps the component
set it started with.

## First run

A `~/.yach/config.toml` without a `[preset]` table means "apply `full`
once and record it". That first apply happens on session start, prints
`yach: applied preset full (first run)` on stderr, and preserves existing
choices: bundled records that already exist keep their `enabled` value
and existing `[components]` toggles are kept. Only components with no
prior record take the preset's default. Later starts never re-apply a
preset.

## Per-session preset

`yach run --preset <minimal|full>` and `yach rpc --preset <minimal|full>`
apply the preset to that session only. They change no preset or component
state: the persisted preset, `[components]` toggles, and bundled install
records are untouched. The session itself still writes normally — session
logs, and a default model if model activation saves one.

## What `minimal` means for approval modes

Presets select components only. They never change the approval mode,
project trust, capability grants, shell allowlists, or the auto-review
execution gate. Those stay kernel policy owned by the user.

With only `bash`, every file read and change goes through the shell path:

- `review` and `auto-review` treat each read and write as a shell command
  (subject to the allowlist and reviewer);
- `accept-edits` has no hash-checked structured edits to auto-apply;
- reads lose the bounded-result and resource-broker path of
  `read_text_file`.

This is the intended behavior for observing a model's native tool use.
`yach preset use minimal` prints these consequences as `note=` lines.

Note that `jev-reviewer` under `full` is installed and enabled but still
needs its existing network capability grant, and automatic execution stays
behind `AUTO_REVIEW_EXECUTION_ENABLED`; installing a component grants
nothing.

## Removal and `--reset`

Bundled extensions are removable: `yach extension remove yach.hashline`
deletes the install record and adds the id to `[bundled] removed` in
`~/.yach/config.toml`. Neither startup nor a later `yach preset use`
re-adds a removed id — `preset use` disables components outside the preset
but never deletes records. A removed id is restored by
`yach extension install --bundled <id>` or `yach component enable
<name>` (both install the bundled record through the same path), or by
`yach preset use <name> --reset`. `--reset` clears every `[bundled]
removed` marker and reapplies the preset's defaults to every component,
but it reinstalls only the bundled components that preset includes —
`preset use minimal --reset` clears hashline's removal marker without
reinstalling it.

## Compaction prompt

`compaction.summary_prompt` is a user-scope-only key in
`~/.yach/config.json`:

```json
{
  "compaction": {
    "summary_prompt": "/home/me/my-summary-prompt.md"
  }
}
```

Use an absolute path: `~` is not expanded, and a relative path is resolved
against the directory yach was started in, not against the config file.

The file must be UTF-8 and is capped at 64 KiB; its contents are trimmed
and replace the instruction preamble and section schema of the compaction
summary prompt. The kernel keeps the previous-summary, focus, and
conversation framing around it. An unreadable or empty file falls back to
the built-in prompt with a diagnostic. The same key in
`<project>/.yach/config.json` is ignored with a diagnostic — a repository
cannot rewrite how session history is condensed.

The prompt can be replaced, not omitted. `compaction.enabled = false`
disables automatic compaction only; a manual `/compact` still sends the
summarization prompt, so a session that must never send it also avoids
`/compact`. The native `compactor = "openai-responses"` still runs the
portable summary pass built from this prompt.

## Build packaging

`--no-default-features` builds yach without the bundled hashline and Jev
code — a core binary containing no first-party extension code. From a
checkout:

```sh
cargo install --path crates/yach-cli --no-default-features
```

In a core build, presets and component commands still work and report the
missing components as not compiled in — `yach component list` shows
`compiled_in=false`, `yach preset use full` lists them under
`not_compiled_in=`, and `yach extension doctor` reports
`last_error_kind=not_compiled_in` for a leftover bundled record. If a
bundled component already has an install record, the command keeps the
enabled/disabled choice it set, and a later full build honors that record.
A bundled component that was never installed stays uninstalled until you
run `yach preset use <name>` or `yach extension install --bundled <id>`
from a build that includes it — a core build creates no record for an
omitted package.
