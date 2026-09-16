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
A project checkout cannot grant capability.

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

A successful grant prints the approved capabilities and the tools that
requested each, then:

```text
The extension may activate; Yach does not observe whether the host uses those capabilities.
```

An extension that requests no capabilities prints that nothing needed a
grant, and writes no file.

Revoke deletes the grant. If the package is still loaded, the TUI also
stops the host. After you have already uninstalled the package, pass the
extension id from the manifest — a path or other selector is rejected when
nothing is discovered:

```text
yach extension revoke example.network-tools
```

## Reading diagnostics

In the TUI, `/extension-status` and `/extension-status <id>` print live
records. From the CLI, `yach extension list` and `yach extension doctor`
(`yach extension doctor <id>` to filter) print the same fields.

Look at `capabilities=` and `capability_grant=`:

| Value | Meaning |
| --- | --- |
| `unknown` | The manifest is not yet known. This is not the same as `none`. |
| `none` on `capabilities=` | The tools request no grant (file-scoped only). |
| `none` on `capability_grant=` | No grant file is recorded. |
| `uses_network`, `runs_process`, or both, comma-separated | The derived or approved set. |

Treat `unknown` as "not consulted," not as "requests nothing" or "ungranted."
A record whose package has not been scanned still renders `unknown` rather
than inventing `none`.

## Registration follows the manifest

When a host does start, each tool it registers must match a manifest entry
in both name and risk. Registering an undeclared name, or a declared name
at a different risk, is rejected before any tool from that host is
committed. The diagnostic then shows `last_error_kind=protocol_error` with
`undeclared_tool` or `tool_risk_mismatch`.

That check keeps the *registered tool list* aligned with the declaration
you granted. It still does not observe sockets or child processes the host
opens on its own.
