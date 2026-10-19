# Anthropic Auth Token Gateway Smoke

Date: 2026-10-19. Spec:
`docs/project/specs/2026-10-19-anthropic-auth-token-design.md`. Kata
`yach#q6rj`.

## Question

Does yach reach the host subscription gateway
(`http://omp-subscriptions.home.lan:4000`) over the Anthropic Messages API
when the credential is sent as `Authorization: Bearer`, from the environment
and from a stored `/connect` connection?

## Gateway behavior

Direct `curl` to `/v1/messages` with `claude-sonnet-4-5`, before any yach
change:

| Headers sent | Status |
| --- | --- |
| `Authorization: Bearer <token>` | 200 |
| `x-api-key: <token>` | 403 |
| both | 200 |

`GET /v1/models` with Bearer returned 200 with 693 rows, `"has_more": null`,
ids prefixed `anthropic/` and others, and an `api` field per row
(`anthropic-messages` 28, `devin-agent` 645, others). The model ids
`claude-sonnet-4-5` and `anthropic/claude-sonnet-4-5` both completed a
message.

## Runs

Binary: debug build of the change at `crates/yach-cli`. All three runs used
`secretspec run --provider omp-subscriptions --scope anthropic-gateway`,
which supplies `YACH_RIG_ANTHROPIC_AUTH_TOKEN`.

1. **Smoke command.**

   ```bash
   YACH_RIG_ANTHROPIC_BASE_URL=http://omp-subscriptions.home.lan:4000 \
   YACH_RIG_ANTHROPIC_MODEL=claude-sonnet-4-5 \
   secretspec run --provider omp-subscriptions --scope anthropic-gateway \
     --reason "yach anthropic auth token gateway smoke" -- \
     yach smoke-rig-anthropic
   ```

   `rig_smoke_outcome=Completed`, 4 events, 2 text deltas,
   `matched_expected_text=true`. With both `YACH_RIG_ANTHROPIC_API_KEY` and
   `YACH_RIG_ANTHROPIC_AUTH_TOKEN` set, the same command reported
   `env vars YACH_RIG_ANTHROPIC_API_KEY and YACH_RIG_ANTHROPIC_AUTH_TOKEN are
   both set; set only one` and made no request.

2. **Headless turn with tools.** Scratch project with `README.md` and
   `main.rs`, fresh `HOME`, `YACH_RIG_PROVIDER=anthropic`, the same base URL,
   and `yach run --model claude-sonnet-4-5 --prompt "List the files here,
   read README.md, and summarize it in one sentence."`. Exit 0, outcome
   `completed`, one turn in 3.4 s, calling `list_project_paths` and
   `read_text_file`; the answer summarized the README correctly.

3. **Stored connection through `/connect`.** A throwaway script drove
   `yach rpc` (fresh `HOME`, no `YACH_*` variables) through the dialog flow:
   provider `anthropic`, label `Gateway`, authentication "Auth token
   (Authorization: Bearer)", base URL `http://omp-subscriptions.home.lan:4000`,
   then the token at the secret prompt ("Enter the auth token"). The flow
   returned to the root list with the connection saved. Model discovery
   then listed 693 connection models. Activating
   `anthropic/claude-sonnet-4-5` succeeded, and a prompt finished with
   outcome `completed`. The saved registry entry:

   ```json
   {"provider": "anthropic", "base_url": "http://omp-subscriptions.home.lan:4000",
    "authentication": {"kind": "auth_token", "source": "system"}, "state": "ready"}
   ```

## Result

All three paths complete through the gateway with Bearer auth. Run 3
exercises both vendored-client changes live: discovery authenticated with
Bearer and parsed the gateway's `"has_more": null` listing, and the turn
authenticated with Bearer.

## Not covered

- Direct Anthropic API with an API key was not re-run live; the header
  fixtures cover it (`x-api-key` only, no `Authorization`).
- The gateway's model list is not filtered to `anthropic-messages` rows, so a
  stored gateway connection lists all 693 models, though only 28 are
  `anthropic-messages`. Selecting one of the others was not tried; it is
  expected to fail at request time.
