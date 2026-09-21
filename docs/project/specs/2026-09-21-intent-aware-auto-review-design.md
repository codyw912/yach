# Intent-Aware Automatic Review

**Outcome:** plane:YACH-11

Status: accepted 2026-09-21; implementation pending.

## Problem and outcome

Repeated approvals for ordinary commands and edits make sustained work in Yach
intolerable. Blanket host authority removes the interruptions but also removes the
judgment the user wants. Useful dogfood therefore needs a middle ground before it
can produce evidence about the rest of the product.

Deliver automatic approval of ordinary work authorized by the user's request,
with meaningful human intervention for substantial danger, uncertain scope, and
user-reserved actions. Jev is the first reviewer implementation, not the owner of
authority and not a mandatory vendor dependency in the core.

This is an early governed-operation slice using a public extension contract. It
does not require completing the whole extension platform first. Sandboxing is a
separate, complementary design; this slice must not claim to provide it.

## Decision

Separate three responsibilities:

1. **Core policy and authorization:** validate actions, retain user-owned limits,
   select review routes, bind approvals to actions, and record evidence.
2. **Reviewer extension:** assess a bounded request against user intent and risk;
   return typed judgments, uncertainty, and evidence references. The first
   implementation calls Jev.
3. **Execution:** existing core-owned edit and command paths enforce the final
   decision and their own correctness checks. A reviewer never executes its
   proposed action or supplies an executable replacement.

Use deterministic decisions where policy already settles the request. Invoke the
reviewer only for unresolved, eligible requests. The product delivered is actual
automatic approval, not a side panel of recommendations requiring the same clicks.

Rejected alternatives:

- More allowlists alone remain brittle for varied legitimate commands and do not
  interpret task intent.
- Reviewing every read, edit, and command adds unnecessary cost, disclosure, and
  latency without strengthening an already-settled decision.
- A model-owned final permission switch obscures authority and turns model errors
  into either blanket access or unappealable vetoes.

## Authorization and user ownership

### Task intent

A request authorizes its reasonable necessary implementation steps, not just the
literal command syntax the user happened to mention. It does not authorize
unrelated activity or any possible means of reaching the requested end state.
The reviewer assesses the actual target and material side effects, not merely the
coding agent's justification. Low intrinsic risk is not sufficient authorization
for unrelated work or an action prohibited by a standing user restriction.

Use these concrete examples as acceptance cases:

| Action | Default interpretation under automatic review |
| --- | --- |
| Add a necessary Cargo crate or use `uv add` for the requested feature | Eligible as project work, including manifest/lockfile changes |
| Synchronize project dependencies, build, lint, or run tests | Eligible when relevant to the task and consistent with the declared environment |
| Edit authorized Nix/devenv configuration | Assess as a configuration edit, not as permission to activate it |
| Install a persistent user-wide tool, modify a system package set, or activate host configuration | Requires separately expressed authority; not implied by ordinary project work |
| Publish a requested PR or perform another explicitly requested external action | Eligible on its actual authorization and consequences, not categorically forbidden because it uses the network |
| Delete important data or perform a costly-to-reverse operation | Hold for concrete risk confirmation even when related to the task |

Project-local installation can execute build/install scripts, access registries,
and write shared caches. It is not inherently safe because of its command name.
Conversely, cache writes outside the repository do not alone make an operation a
persistent system installation. Assess declared environment, destination, package
source, scripts where relevant, and material effects. Never rewrite the user's
environment workflow merely to make an approval easier.

### Durable restrictions and human-only actions

User-owned policy supports global restrictions and project-specific restrictions
keyed by the existing canonical project identity. A project restriction cannot
silently relax a global restriction. Only explicit user policy editing can grant
an exception. Repository files may restrict execution or provide context; they
cannot grant approval authority, select the reviewer, or broaden disclosure.

Preserve the distinction between:

- **Ask first:** the user may authorize execution of the displayed action.
- **Human performs:** Yach holds and hands off; the user performs the action outside
  the agent. For example: "Edit my Nix configuration; I run the rebuild."

A normal task request or an ordinary approval button does not silently erase a
human-performs restriction. The user can explicitly change that restriction or
make an acknowledged one-action exception. The model cannot make that change.

Durable restrictions live in user-owned permission state, not solely in a prompt
summary. Compaction, restart, extension reload, and provider changes do not discard
them. User/client policy changes are validated and acknowledged only after durable
persistence; failures leave the previous policy active. This is a narrow permission
policy extension, not a general settings redesign or a new generic rules language.

## Approval mode and policy ordering

Extend the existing approval-mode/client flow with explicit automatic review.
Selecting it binds one user-selected reviewer and policy revision for the session.
Selection names the hosted reviewer and its disclosure boundary. This slice does
not silently enable automatic review or restore it after restart: use explicit
session selection, retaining existing stored non-dangerous mode behavior.

Existing `review`, `accept-edits`, and session-only `full-access` remain distinct.
`full-access` must not be renamed or advertised as model-reviewed. Standing
human-only/ask restrictions introduced by this design take precedence over mode
shortcuts and broad allowlists, including full-access. Full-access removes ordinary
review, not a user's separately recorded restriction. Changing such a restriction
requires a user action with its own evidence.

For each action, core applies the following order:

1. Validate request structure, supported operations, resource scope, and execution
   integrity. Invalid actions are not made valid by approval.
2. Apply user-owned prohibitions and human checkpoints. Match known restrictions
   deterministically where possible. If their applicability requires semantic
   judgment, assess it before taking any automatic allow path; uncertainty asks.
3. Honor a valid, fresh, exact-action user approval or override.
4. Apply deterministic mode/policy decisions for actions already authorized.
5. In automatic-review mode, assess unresolved eligible actions using the reviewer.
6. Automatically execute only when authorization, evidence sufficiency, and risk
   meet the versioned routing policy. Otherwise hold for the user or return a
   truthful non-execution result to a client that cannot ask.

Do not average strong authorization against serious danger to cancel the danger.
Missing context, reviewer failure, and dangerous action are distinct reasons for
holding execution. The coding agent may gather relevant context or propose a
materially safer action, but must not retry an equivalent denied action through a
different tool to evade the decision. Do not add an autonomous retry loop.

## Amendment to existing authority contracts

The accepted [approval-modes design](2026-08-24-approval-modes-design.md) treats
sensitive paths and outside-project writes as hard denials in every mode. The
[full-access design](2026-08-24-full-access-approval-design.md) retains these checks
for structured tools while acknowledging that bash has wider reach.

This design supersedes those provisions only where needed to distinguish
permission policy from correctness:

- **Permission restrictions** such as a sensitive target or an outside-project
  write may be reviewed and explicitly overridden by the user through a supported
  core operation. Any required resource grant must be separately represented and
  bounded to the target; neither an assessment nor an override bypasses a broker's
  resource-root validation.
- **Integrity invariants** remain mandatory: schema validation, valid operation
  types, file preconditions, current preview, transaction consistency, resource
  handle validity, evidence persistence, and actual OS/service permissions.
- **Authority control-plane operations** remain user/client-only: changing policy,
  granting extension/reviewer authority, and selecting a reviewer are not ordinary
  provider edits. A generic file-write override cannot rewrite future permissions.

Approval does not add support for an operation the executor cannot perform. Report
that limitation rather than directing the model to use a less governed route.
Outside-project resource grants must preserve structured edit guarantees; do not
replace them with unrestricted shell execution.

## Public reviewer contract

Add a versioned reviewer contribution to the existing extension protocol. It is
separate from model-visible tools and generic mutable hooks. The first-party Jev
adapter uses the same contract available to other reviewers; no private approval
path is permitted. Select one reviewer per session in this slice, not a voting
ensemble or cascading reviewer framework.

### Request

Core constructs an immutable request containing:

- Request/decision identity, action kind, issuing session/turn, policy revision,
  authorization revision, and selected reviewer identity/generation.
- Exact tool/command arguments, cwd, relevant declared environment identifiers,
  targets, proposed diff and file preconditions where applicable, requested resource
  access, and actual sandbox/execution restrictions or their absence.
- Trusted user messages relevant to the task, explicit grants, standing restrictions,
  and authenticated prior risk confirmations, with stable source references.
- Separately labeled untrusted evidence: assistant explanations, repository content,
  tool results, extension descriptions, and retrieved material.
- Explicit indication of omitted, truncated, redacted, or unavailable evidence.

Do not accept a provider-written summary as the sole record of user authorization.
Core retains original authorization records outside lossy conversation compaction.
Repository instructions may explain implementation; they do not become authority
merely because they use an authoritative filename or tone.

Requests and responses are bounded. Evidence gathering uses existing core read-only
resource mechanisms with explicit targets and limits, not arbitrary shell or network
access granted to the reviewer. If evidence cannot be supplied within the review
budget or disclosure policy, escalate. Do not silently trim decision-critical data
and interpret the remaining request as complete. Initial contract limits are 64 KiB
of serialized review state, 16 KiB of assessment data, and a 15-second end-to-end
review deadline including evidence gathering and provider calls. Provider context
limits may lower the effective input budget, never silently truncate it. Exceeding
any limit escalates without execution. These are failure bounds, not latency targets;
normal latency must be measured. No automatic inference retry extends the deadline.

### Assessment

Return typed dimensions rather than a single unexplained "safe" flag:

- Authorization: exact/substantive authorization, insufficient authorization, or
  ambiguity, with references to trusted request evidence.
- Applicable user restriction or reserved-action indicators.
- Consequence categories and severity: significant data loss, sensitive disclosure,
  persistent environment/security changes, service disruption, and costly external
  effects.
- Evidence sufficiency and suspected instruction-origin confusion.
- Probabilities/uncertainty when provided by the model; model, adapter, rubric, and
  policy identifiers; measured request duration and reported usage.

Core rejects malformed, unsupported, out-of-range, non-finite, mismatched, or stale
responses. Evidence references must refer to supplied items; a reference identifies
the basis claimed by the assessment, not proof that its judgment is true.

Jev does not generate explanations. Human-facing messages are rendered from bounded
reason codes, target/action details, cited request excerpts, and uncertainty. Do not
fabricate a natural-language rationale or add another generative model merely to
make the reviewer look articulate.

### Data disclosure and bootstrap authority

Enabling a hosted reviewer explicitly authorizes a defined class of request context
to the selected endpoint. That is distinct from permission to execute the proposed
action. Do not send credentials, environment secrets, or unrestricted transcripts by
default. Secret redaction is not proof that arbitrary proprietary content is public.
If safe minimization removes necessary context, ask the user without sending it.

The adapter uses normal provider credentials and endpoint configuration, with no
host-specific credential wrapper in the project. Reviewer installation, network
capability, selection, and disclosure authorization are established by the user;
the reviewer cannot approve its own activation, grant expansion, policy changes,
or calls needed to bootstrap itself. Its request path must not recursively ask the
same reviewer to approve its own inference call. Network permission does not by
itself authorize all data disclosure.

An ordinary process-hosted reviewer remains trusted executable code with the host's
privileges. The typed contract limits what core accepts from it; it is not an OS
containment mechanism. Activation must state this limitation just as other extension
activation does.

## Jev adapter and routing

Use a batch of narrow questions against the same bounded state: authorization,
restriction applicability, relevant consequences, and evidence sufficiency. Keep
exact arithmetic, identity comparisons, and routing in code. Do not multiply
question probabilities as if their errors were statistically independent.

Preserve raw assessments with their semantics. A confidence statistic derived from
an answer distribution is not a measured probability of safe execution. Thresholds
are versioned per question/risk class and model/rubric revision, not borrowed from a
content-moderation cookbook or transferred between Choice and Noul questions.

Eligible ordinary authorized work proceeds when no restriction or substantial risk
requires human intervention and evidence is adequate. Significant destructive risk
requires informed confirmation rather than a higher opaque auto-approval threshold.
Uncertain authorization or incomplete evidence asks for clarification. Review errors
ask for human approval; they are neither evidence of malice nor permission to run.

The adapter and core record the model identifier actually returned. Unknown or
changed model/rubric combinations do not inherit an earlier calibration implicitly.
No untested fallback model may silently supply execution authority.

## Exact-action approval and human override

Core owns a pending action with its original request identity and applicable
preconditions. An authenticated UI/RPC user can approve that action once, reject it,
or clarify the request. The provider and reviewer extension cannot emit the user
response event or forge an approval by placing text in tool output.

A risk confirmation displays the proposed action and concrete consequence, not
just a generic danger label. A reviewer rejection is a hold, not an unappealable
veto. After explicit human override, core bypasses model reconsideration for that
exact action and checks its integrity again before execution.

The approval is consumed once. It is invalid after a relevant action, target,
file-precondition, policy, authorization, session, or reviewer-generation change,
or cancellation. Returning from slow review does not resurrect an interrupted
command. A stale edit needs a new preview, not an override of the failed hash check.
Concurrent user rejection or policy revocation wins over an in-flight model allow.

Binding a command string does not freeze everything an arbitrary process may later
read or execute. Revalidate known script/file inputs where captured; report the
remaining limitation rather than claim elimination of all command-time races.

An override records its scope and does not create a standing grant. Human-only
exceptions use an explicit user acknowledgement that changes that boundary for this
action; generic approval must not silently convert them into ordinary asks.

## Coverage and execution limitations

Initial coverage includes core-brokered command requests, structured edit
transactions, extension edit proposals re-entering that transaction path, and any
other existing permission ask through the shared decision vocabulary. Preserve
existing preparation/execution seams rather than invent a second executor.

Capability grants at extension activation remain separate. This feature does not
intercept arbitrary filesystem/network/process effects inside an already-active
extension host or shell process. An extension's declared risk is context, not a
verified inventory of its effects. Where Yach lacks per-call mediation, status and
documentation must say so; auto-review is not advertised as comprehensive monitoring.

Sandbox design, brokered process execution for all extension effects, and OS-enforced
network/filesystem limits are separate work. Their eventual interfaces should supply
actual restrictions to the reviewer without changing who owns authorization.

## UI, RPC, headless, and evidence

Use the existing approval selector and correlated backend events. Automatic review
is visible in session status, along with reviewer identity and the absence/presence
of isolation. Routine allows do not open modal dialogs; their decision trail remains
inspectable. Pending human reviews expose exact action, concern, and one-action
approval/rejection. Reviewer errors are visibly different from risk findings.

TUI and negotiated stdio RPC share the same backend decisions, override rules,
correlation, and evidence. An RPC connection is an authority-bearing client; user
approval events are never exposed as model tools. Older clients that cannot negotiate
automatic review remain on supported manual behavior.

Headless mode can explicitly select the same configured automatic reviewer. If it
cannot obtain a required human decision, it reports non-execution and a failing
result rather than hangs or grants blanket permission. A caller-provided trusted
approval channel may answer through the same protocol. Do not reinterpret existing
`--full-auto` as automatic review; its full-access meaning remains explicit.

Persist decision evidence before effects: action reference, authorization/policy
revision, deterministic or model route, reviewer/model/rubric identity, assessment
and uncertainty, final core decision, human override when present, and outcome.
Failure to persist authorization evidence prevents execution. Reuse existing durable
event/permission evidence patterns; do not log raw credentials or entire remote
review payloads by default. Pending decisions and one-action approvals do not become
reusable authority after restart.

## Evaluation and acceptance

The implementation plan must include a frozen, labeled Yach-specific corpus and
held-out evaluation before enabling model-derived automatic execution. Labels cover
expected routing and consequences, not agreement with another model. Include:

- Routine multi-file edits, tests, builds, lockfile synchronization, new dependencies,
  unfamiliar commands, allowed registries, and known installation script effects.
- Persistent user/system tool installation, Nix configuration edits versus activation,
  project caches versus persistent environment changes, and explicitly authorized
  exceptions to those defaults.
- Authorized publication and external actions, important data deletion, ambiguous
  scope, secret-bearing context, misleading repository/tool instructions, and absent
  or truncated evidence.
- Timeouts, malformed/non-finite assessments, unavailable credentials/models, review
  cancellation, policy revocation, changed action/file state, extension reload,
  evidence persistence failure, and duplicate/stale client replies.

Record incorrect approvals (including severity), unnecessary intervention rate,
automation coverage of ordinary authorized work, model/threshold calibration, p50/p95
review latency, request size, cost, and end-to-end interruption time. Report sample
counts and limitations; zero observed dangerous approvals does not prove zero risk.
No critical acceptance scenario may execute without the required human decision.
The mandatory labeled denial/confirmation cases must have zero observed automatic
executions; every designated routine workflow must complete its ordinary steps
without per-action human approval. Freeze the evaluated model, rubric, routing
thresholds, and corpus revision together before enabling automatic execution.
Report latency and cost as measured results, not invented guarantees. If results
do not support these safety and usability conditions, report that outcome and
revisit the adapter/rubric rather than mark shadow evaluation as feature completion.

Deterministic regression cases must prove:

1. User-owned restrictions outrank modes/allowlists and survive restart/compaction;
   project/model/extension content cannot create approval authority.
2. Ordinary authorized project work can actually execute automatically through the
   real command/edit seams, including extension edit proposals.
3. Significant danger, human-only policy, insufficient evidence, and reviewer failure
   do not execute automatically, and have distinguishable user-visible outcomes.
4. Explicit human override executes the fresh exact action once without another model
   veto, while unsupported or stale operations remain unexecutable.
5. Cancellation, changed authorization, and stale responses cannot race into execution;
   policy/evidence write failures cannot produce an unrecorded allow.
6. TUI, RPC, and headless exercise the same rules, with an actual interactive surface
   check and truthful headless failure when human input is unavailable.
7. Hosted-review consent, minimization, and unavailable-context handling are exercised;
   private context is not sent merely because the proposed action was authorized.
8. The existing performance framework measures deterministic overhead and paired
   local review-routing cost; separate live measurements record Jev latency/cost.
   Network/model variability is not disguised as a deterministic CI gate.

A short shadow phase is a calibration method, not the delivered product. Completion
requires real Jev-backed automatic approval and a recorded representative coding
workflow that no longer needs repeated human clicks for its ordinary authorized
steps, together with the exception/override scenarios above.

## Non-goals

- OS sandboxing or a claim that classification enforces subprocess behavior.
- Marketplace, generic lifecycle hooks, arbitrary UI injection, reviewer ensembles,
  or automatic retry cascades.
- General code-quality review or a content-moderation system for all conversation.
- Silent full-access fallback, broad permanent grants inferred from overrides, or
  automatic reviewer self-approval.
- A wholesale settings redesign, changed provider API credentials architecture, or
  automatic installation/activation of system tools.

## Sources and interpretation

- [Yach approval modes](2026-08-24-approval-modes-design.md),
  [full access](2026-08-24-full-access-approval-design.md), and
  [extension-first posture](2026-08-19-extension-first-product-posture-design.md).
- [TypeSafe announcement](https://typesafe.ai/blog/introducing-system-one-models-and-jev),
  [typed API](https://docs.typesafe.ai/introduction),
  [confidence semantics](https://docs.typesafe.ai/confidence),
  [Jev 1.13 limitations](https://docs.typesafe.ai/model-jaggedness/jev-1.13.md), and
  [guardrail cookbook](https://docs.typesafe.ai/cookbooks/llm_guardrails).
  The cookbook is a small cached content-screening demonstration, not evidence of
  reliable execution authorization. Vendor speed/calibration claims require local
  workload evaluation; schema correctness is not decision correctness.
- [Codex auto-review documentation](https://learn.chatgpt.com/docs/sandboxing/auto-review.md)
  and pinned source at `a86631502d49274cb47208925c7d3dcece032029`:
  [policy template](https://github.com/openai/codex/blob/a86631502d49274cb47208925c7d3dcece032029/codex-rs/prompts/templates/guardian/policy_template.md),
  [request freshness](https://github.com/openai/codex/blob/a86631502d49274cb47208925c7d3dcece032029/codex-rs/core/src/guardian/review_request.rs).
  Borrow the risk/authorization split and fresh action-bound decisions, not its
  default low-risk authorization rule or model-reconsidered human override.
- [Claude Code auto mode](https://code.claude.com/docs/en/permission-modes),
  [human checkpoints](https://code.claude.com/docs/en/auto-mode-config), and
  [sandbox separation](https://code.claude.com/docs/en/sandboxing).
  Living documentation illustrates the architecture; it is not a stable protocol
  dependency or evidence of a universal package-installation rule.
