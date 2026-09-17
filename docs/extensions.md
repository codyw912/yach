# Extensions

Yach loads extensions from a `yach.extension.json` manifest and runs each
host as an ordinary subprocess. This page is for people who write or install
those packages.

**The declaration is consented to, not enforced.** Yach refuses to
*activate* an extension whose declared capabilities are not granted. It does
not constrain what a running host does. An extension can open a socket
without declaring `uses_network`, or spawn a process without declaring
`runs_process`, and Yach will neither prevent nor detect that. A grant means
you agreed the extension may start with that declaration. It does not mean
Yach watches the host.

## Declaring a tool's risk

Each contributed tool names a `risk` in the manifest. There is no separate
capabilities field: Yach derives the requested set from those risks.

These two request a capability grant:

| `risk` | Meaning |
| --- | --- |
| `uses_network` | The tool is declared to use the network. |
| `runs_process` | The tool is declared to run a process. |

These three are file-scoped. They request nothing, write no grant file, and
activate without `/extension-trust`:

| `risk` | Meaning |
| --- | --- |
| `reads_local_metadata` | Reads names, paths, or other metadata. |
| `reads_local_content` | Reads file contents. |
| `mutates_local_state` | Changes local files or other local state. |

A network tool looks like this:

```json
{
  "name": "fetch_url",
  "description": "Fetch a URL.",
  "risk": "uses_network",
  "provider_visible": true
}
```

If you later add `uses_network` or `runs_process` to a package that did not
request them, activation is blocked until you grant again. Changing the
manifest `version` string without changing those risks does not require a
new grant.

The bundled `yach.hashline` package declares only file-scoped tools, so it
does not need a grant.

## Activation without a grant

If the derived set includes `uses_network` or `runs_process` and no covering
grant is on disk, Yach does not start a host process. The extension
activates as blocked (`activation_state=blocked`,
`last_error_kind=policy_blocked`). The diagnostic names the missing
capability, for example:

```text
extension requests ungranted capabilities: uses_network. Grant with `/extension-trust example.network-tools`.
```

The same check runs at startup and on `/extension-reload`. Reloading does
not bypass it.

Grants live only in user home, at `~/.yach/extensions/<extension-id>.json`.
That private JSON document is the inspectable audit artifact; it is not a
project-controlled file. The versioned `yach.extension-authority.v1` shape
contains `extension_id`, nullable `current` authority, nullable
`legacy_baseline`, and an ordered `history`. Each history decision records a
unique `operation_id`, `recorded_at`, `action` (`grant` or `revoke`), a fixed
reason, its originating `surface` (`cli` or `lifecycle`), and nullable `before`
and `after` grant snapshots. A grant snapshot contains the approved capability
set plus `version_at_grant` and `granted_at` provenance. These fields describe
the decision Yach recorded; they do not identify or authenticate a particular
human.

Older three-field grant files (`approved`, `version_at_grant`, `granted_at`)
still authorize. They have no decision history, so Yach does not invent one.
The next explicit trust or revoke preserves that imported grant as
`legacy_baseline` and writes the versioned document with the new decision.
Revoke sets `current` to null and appends a revoke decision; it deliberately
does not delete the document or its prior evidence.

A failure before replacement leaves the previous document authoritative and
unchanged. If replacement has become visible but Yach cannot confirm directory
durability, the command reports that the update occurred with unknown storage
durability; it does not claim success or rollback. On platforms that cannot
sync directories, crash durability of the directory entry is weaker than the
file contents.

## Granting and revoking

In the TUI:

```text
/extension-trust <id>
/extension-revoke <id>
```

From the CLI:

```text
yach extension trust <id>
yach extension revoke <id>
```

Trust needs a discovered package (the selector must resolve to a loaded
manifest). It records the derived capability set. In the TUI it then
reloads the extension so activation can proceed. The CLI command writes the
grant only; the host starts on the next session, or after
`/extension-reload` in a running TUI.

Extension commands that report `extension_outcome=Failed` exit with status 1.
Successful commands and diagnostic no-ops exit with status 0; malformed command
usage exits with status 2. This lets scripts distinguish a printed diagnostic
failure from a completed command without parsing its human-readable message.

A successful grant prints the approved capabilities and the tools that
requested each, then:

```text
The extension may activate; Yach does not observe whether the host uses those capabilities.
```

An extension that requests no capabilities prints that nothing needed a
grant, and writes no file.

Revoke keeps the document and clears current authority, so activation stays
denied even though history remains. If the package is still loaded, the TUI
also stops the host. After you have already uninstalled the package, pass the
extension id from the manifest — a path or other selector is rejected when
nothing is discovered:

```text
yach extension revoke example.network-tools
```

In the TUI, `/extension-status` and `/extension-status <selector>` report the
live native activation snapshot. Its records reflect the current session's
activation state, generation, errors, and registered/provider-visible tools.
The optional selector can match id, source reference, install source, package
root, or manifest path; an empty or whitespace-only selector is unfiltered.

From the CLI, `yach extension list` and `yach extension doctor`
(`yach extension doctor <id>` to filter) perform a fresh package and install
scan. They do not start extension hosts and do not claim to be the TUI's live
snapshot. Both surfaces expose the capability fields, but their other state is
obtained differently.

Look at `capabilities=` and `capability_grant=`:

| Value | Meaning |
| --- | --- |
| `unknown` | The manifest or authority was not consulted. This is not the same as `none`. |
| `none` on `capabilities=` | The known manifest's tools request no grant-requiring capabilities. |
| `none` on `capability_grant=` | There is no current approval, including after revoke when the authority document and history remain. |
| `uses_network`, `runs_process`, or both, comma-separated | The derived request or current approved set. |

Treat `unknown` as “not known here,” not as “requests nothing” or “ungranted.”
A record whose package or authority has not been consulted renders `unknown`
rather than inventing `none`.

## Registration follows the manifest

When a host does start, each tool it registers must match a manifest entry
in both name and risk. Registering an undeclared name, or a declared name
at a different risk, is rejected before any tool from that host is
committed. The diagnostic then shows `last_error_kind=protocol_error` with
`undeclared_tool` or `tool_risk_mismatch`.

That check keeps the *registered tool list* aligned with the declaration
you granted. It still does not observe sockets or child processes the host
opens on its own.
