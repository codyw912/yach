# Rig Upstream Reconciliation

Date: 2026-08-24

## Question

Did merged Rig work after issue
[0xPlaygrounds/rig#2269](https://github.com/0xPlaygrounds/rig/issues/2269)
make Yach's vendored `rig-core` patch removable or materially smaller?

## Result

Partially, but not enough to remove the vendor boundary or unblock publishing.

Rig 0.42.0 includes PR
[#2295](https://github.com/0xPlaygrounds/rig/pull/2295), which fixes the
`ResponseOutputMessage.phase` half of #2269 for blocking stateless replay. The
0.42 changelog explicitly credits #2269/#2295. The issue correctly remains open:
PR #2295 says the compaction-item and caller-built raw-request asks were not
implemented and overlap still-open PR
[#2234](https://github.com/0xPlaygrounds/rig/pull/2234).

## #2269 Matrix

| Requested capability | Rig 0.42 / current main | Yach vendor |
| --- | --- | --- |
| Preserve Responses message `phase` on replay | **Fixed** by #2295; blocking path only | Not backported to the current 0.41 vendor |
| Accept opaque `compaction`/unknown input items | Missing: `InputItem` remains a closed typed shape with private fields and no unknown variant | `InputContent::Unknown(Value)` passthrough |
| Expose terminal ordered raw `response.output` | Missing: provider-native `StreamingCompletionResponse` has usage/reasoning/status/ids but no output list; generic `StreamFinal.raw` serializes that incomplete terminal type | Captures raw ordered `Vec<Value>` from `response.completed` |
| Send a caller-built Responses request | Missing: `create_completion_request` remains `pub(crate)` and `raw_stream` accepts Rig's generic `CompletionRequest`, not a caller-built native request | Public completion/streaming entry points over native `responses_api::CompletionRequest` |

PR [#2367](https://github.com/0xPlaygrounds/rig/pull/2367), released in
0.42, adds a valuable `raw: serde_json::Value` side channel and public typed raw
completion methods. Its contract is the provider terminal type serialized after
Rig parses it; it deliberately does not preserve fields the terminal type never
modeled and does not expose raw stream frames. It therefore does not replace
Yach's terminal `response.output` capture.

## Compatibility Probes

An isolated copy of `yach-backend`, `yach-connections`, and `yach-proto` was
compiled without the workspace `[patch.crates-io]`.

- Registry `rig-core 0.42.0`: **failed with 41 compile errors**.
- Current Rig `main` at `b5dafc03`: **failed with the same substantive gaps**
  after adapting the dependency declaration to main's removed `reqwest`/`rustls`
  features.

Some failures are expected 0.42 migration work (`OneOrMany` -> `Vec`, streaming
type changes, removed `GetTokenUsage`). The remaining failures prove unreleased
Yach-specific APIs:

- public ChatGPT auth module and `AuthFileGuard` transaction/fencing API;
- expected-account authorization and typed auth outcomes/errors;
- ChatGPT/Codex model listing and catalog client version;
- the three Responses passthrough capabilities above.

## Vendored Patch Families Still Load-Bearing

1. **Responses stateless replay**: opaque input, complete ordered terminal
   output, native caller-built requests.
2. **ChatGPT auth safety and lifecycle**: atomic 0600 writes, cross-process lock,
   path revalidation, fencing tokens, public guard API, bounded device flow,
   expected-account enforcement, typed auth errors/outcomes.
3. **ChatGPT/Codex catalog discovery**: model-listing client and protocol-version
   request surface.

No matching upstream issue or PR was found for the Yach auth guard or model
listing APIs.

## Decision

- Keep #2269 open; its remaining asks are still real on 0.42 and current main.
- Do not remove `[patch.crates-io]` or relax the release preflight.
- Do not treat a 0.42 migration as a release unblocker by itself; it is a
  breaking migration plus a rebase of still-required vendor patches.
- Consider backporting #2295 to the 0.41 vendor for phase fidelity independently
  of release work.
- For the durable release path, upstream small coherent patch families against
  current Rig main: Responses passthrough (coordinated with #2234), ChatGPT auth
  safety/public guard, then ChatGPT model listing. A published Yach-owned Rig
  crate remains the fallback if upstream timing blocks a Yach release.
