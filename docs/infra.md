# AWS infrastructure and releases

This runbook owns deployment order, operator prerequisites and recovery boundaries.
[Local CDK tooling](../infra/README.md) owns build/test/synth commands;
[architecture](arch.md) owns design guardrails. Checked-in configuration and a
successful synth are **not evidence of deployed state or workload readiness**.
Verify the approved account, physical resources and effective configuration at each release.

## Infrastructure boundary

The CDK app declares `dev` and `prod` in `eu-central-1`. AWS `dev` serves the
staging API; it is not a stage named `stage`. Application stacks are composed in
[`application-stack.ts`](../infra/src/application-stack.ts):

| Stack suffix under `application-<stage>` | Ownership |
| --- | --- |
| `network` | Private workload/database networking and outbound access |
| `data` | PostgreSQL, CDC transport, durable queues and scoped worker policies |
| `initialize` | Private migrator and FX functions; deployment does not invoke them |
| `compute` | Application/worker Lambdas, Cognito, eventing and scheduled work |
| `api` | HTTP API, regional domain, CloudFront and WAF |
| `observability` | Production alarms and alarm topic |

Network precedes data and initialize; compute precedes API and prod observability.
The standalone container-repository setup stack is outside normal releases.
OpenSearch host administration, external DNS, provider dashboards and native-worker
process/identity deployment are externally owned; CDK does not reconcile them.

PostgreSQL is authoritative and private. Application/migration runtimes use scoped
generated secrets and hostname-verifying TLS with the committed public RDS CA bundle.
Never substitute plaintext, a raw-IP URL or disabled verification. CA refresh is
a reviewed asset release. Per-process connection limits do not cap aggregate
Lambda/ECS database usage. Inspect capacity before activating consumers.

The declared single NAT, Single-AZ database/CDC resources and single stage search
host are outage dependencies, not a high-availability promise. Review current
engine availability, backups, retention/removal policies, quotas and cost before
execution; dev resources may be destructive on deletion. Retained resources still
require ownership and recovery planning.

## Deployment inputs

[`deploy.yml`](../.github/workflows/deploy.yml) and the versioned
[`ci/`](../ci/) helpers define the executable release contract:

| Trigger | Selected source and stage |
| --- | --- |
| Eligible `develop` push | Push SHA → `dev`, `scope=auto` |
| Release-tag push | Existing UTC CalVer `YYYYMMDD-HHMM` tag → `prod`, `scope=auto` |
| Manual Deploy from `develop` | Explicit stage/ref/scope; also used for rollback |

Manual dev `ref` is blank/`develop` or a full lowercase 40-character SHA. Prod
requires an existing CalVer tag with a real UTC date/time, not a branch or raw SHA.
Every source must be reachable from fetched `origin/develop`. Resolve once and pin
builds, lockfiles, catalogs, templates and stage CDK to that full SHA. Historical
sources without the current layout/helpers are unsupported; never retag a release.

| Scope | After artifact preparation and stack preflight |
| --- | --- |
| `auto` | Deploy foundation; stop there if compute is absent or migration sources differ; otherwise deploy application stacks |
| `foundation` | Network/data/initialize only; invoke neither migration nor FX |
| `all` | Foundation plus compute/API/prod observability; explicitly acknowledge compatibility and activation prerequisites |

`auto` compares the deployed compute `CommitSHA` with the selected source over
`migrations`, `infra/sql` and `src/database-migration-lambda`. Lookup/diff errors
fail before stage-stack changes. This is a **source-change guard**, not database
introspection. A successful migration does not make another `auto` run advance
compute; the same-ref `all` continuation is required. OpenSearch-only and other
changes outside those paths are not covered.

Deploy uses the selected source's CloudFormation change sets, not hotswap or
previous-template artifact replacement. It never invokes business migrations,
initial FX, OpenSearch administration, DMS start/reset or retained-resource import.

### Operator prerequisites

Before relying on these workflows, the repository/AWS owners must verify:

- Protected `develop` with required CI and immutable release tags. Restrict tag
  creation to release maintainers and updates/deletion even for administrators.
  Do not require path-filtered checks without an always-running fallback.
- `aws-prod` required reviewers and source rules allowing release tags plus
  `develop` for manual dispatch; `aws-dev` follows its approved branch/reviewer
  policy. Manual dispatch permission does not bypass source validation.
- OIDC audience `sts.amazonaws.com` and narrowly scoped subjects: publishers use
  `repo:aura-historia/backend:ref:refs/heads/develop` or the IAM `StringLike`
  pattern `repo:aura-historia/backend:ref:refs/tags/????????-????`; protected tag
  rules and workflow validation enforce digits/date/ancestry. Final Deploy and
  manual operations use exact `repo:aura-historia/backend:environment:aws-dev`
  or `...:environment:aws-prod` subjects. Checkout does not change that subject.
- The existing `CI_DEPLOY_ROLE_ARN` and publisher region/bucket variables are
  available outside environments. Verify their account/region/bucket targets
  against [`config.ts`](../infra/src/config.ts). Publishers need scoped immutable
  artifact writes; deployment also needs CDK/CloudFormation inspection/execution,
  parameter resolution and exact migrator/FX invocation permissions.
- The approved deployment account is explicit and 12 digits; workload region is
  `eu-central-1`. Template-only synth may omit the account, deployment may not.
- Certificates, DNS/aliases and routing meet the [front-door gate](http-api-front-door.md#deployment-cutover-and-rollback).
  Verify external provider credentials, permissions, callbacks, sender identities,
  recipients, quotas and acknowledgment deadlines before enabling traffic.

Only final Deploy enters `aws-<stage>` after image/ZIP/mail publication. Migrate
and Initialize enter it before invocation. Publishers currently share the CI role:
environment approval is a workflow checkpoint, **not an isolated IAM boundary**.
Do not reuse that role as a native-worker identity.

Parameter names and types belong in configuration/constructs, not a second
inventory here. Some credentials use plain SSM `String` dynamic references and
are copied into function configuration; a reference is not runtime secret
isolation. Restrict parameter, function-configuration and deployment access.
Changing a parameter alone does not refresh consumers: deploy/recycle affected
configurations and verify the new credential before revoking the old one.
Never put credentials in source, CLI arguments, logs or release evidence.
Provider/consent-specific requirements live in [marketing consent](marketing-consent.md).

For federated login, verify the provider's enabled/live use case, required scopes
and Meta-to-Cognito redirect URI separately from frontend callbacks. Review the
pinned provider version in configuration and test hosted sign-in. Before rollout,
measure cold/warm private pre-sign-up paths and failure cases with margin under
Cognito's fixed trigger deadline. Hold if timing or fail-closed behavior is
unverified. User-pool/schema/client replacement needs an explicit identity and
callback migration; never silently replace a production pool.

### Release artifacts

Deploy prepares the selected source's Lambda/image catalogs and compiled mail
templates even for foundation-only releases. ZIPs and templates are stage/SHA-local;
existing objects are reused, never overwritten. Missing objects use conditional
creation; authorization/transport failures are not evidence of absence.

ECR images use immutable full-SHA tags and compute receives resolved registry
digests, never `latest`. Missing images are built and smoke-tested before publishing;
unresolved artifacts block deployment. Existing artifact/template/staging buckets
are prerequisites from configuration. The CLI-credentials synthesizer uses the
existing staging bucket; it does not require a full CDK bootstrap stack.

### Container repository setup

Provisioning owners prepare catalog ECR repositories **outside normal releases**:

1. Inspect each physical repository's account/region, URI, immutable-tag/AES256
   configuration and any CloudFormation owner. Valid existing repositories may
   be reused without moving ownership.
2. For missing repositories, review the pinned setup source, synthesized catalog
   and change set from [`bin/artifacts.ts`](../infra/bin/artifacts.ts) against
   physical inventory. The setup stack retains repositories; releases never create them.
3. Import/ownership changes and failed setup-stack shells require separate approval.
   Never delete a repository or image history, create duplicates, or treat an empty
   stack shell as proof of ownership. Keep the setup catalog append-only absent a
   reviewed removal/migration.

## First-time stage

1. Verify operator prerequisites and repositories above. Deploy the selected ref
   with `auto` or manual `foundation`; record the resolved SHA and completed
   network/data/initialize stacks.
2. Run **Migrate (CD)** for that stage and exact deployed initialization SHA;
   require confirmed success under [manual operations](#manual-operations).
3. Have the OpenSearch owner complete [host setup/compatibility checks](opensearch-stage.md#release-and-schema-change-contract)
   and approve private-workload connectivity, scoped credentials and trusted TLS.
4. Run **Initialize (CD)** with the same stage/SHA; verify persisted initial FX.
5. Before same-ref manual Deploy `scope=all`, verify schema/job compatibility,
   FX, OpenSearch, provider/SES prerequisites, certificate/DNS and
   [native-consumer handoff](durable-worker-runbook.md#activation-and-legacy-handoff).
   Stop old consumers and settle or preserve in-flight/backlog custody first.
   Required pre-activation probes need an approved private path; if unavailable,
   **hold `all`**, rather than deploy active compute as a speculative test.
6. Continue with the pinned full dev SHA or same immutable prod tag. Record actual
   function versions/aliases, mapping/rule/schedule states, front-door/readiness and
   relevant private reader/projector/percolator smoke. Report failed/omitted checks
   explicitly and hold dependent traffic. Complete CDC activation separately below.

Compute creation activates worker mappings, partner integrations and FX/cleanup
schedules together. It is **not an inactive preview** and has no readiness marker.
Foundation success does not prove these prerequisites. Never run two consumers of
one queue or rename/purge a queue to perform handoff.

## Routine and schema-dependent releases

Eligible develop pushes release dev; reviewed merged sources are promoted to prod
by immutable UTC CalVer tags. For compatible routine changes, `auto` can advance
all application stacks. Deploy never invokes migration or initial FX.

For a forward release whose migration sources changed:

1. Deploy prepares artifacts, updates foundation and reports **application not deployed**.
   Record its resolved SHA.
2. Run Migrate at that deployed initialization SHA and require confirmed success.
   Complete separately approved OpenSearch/provider changes where needed.
3. Review compatibility and prerequisites, then manually Deploy the **same ref**
   with `scope=all`; use the pinned dev SHA or same prod tag, not a moving branch.

Use expand-and-contract across running binaries, retained jobs and rollback
sources. Foundation does not pause existing consumers/schedules; updated FX code
at its stable unqualified ARN must tolerate the currently deployed schema.
Contract/removal waits until old binaries, replay and the rollback window no longer
need the old schema. Crawler-local migrations are outside this release contract.

Deploy/Migrate/Initialize serialize per stage, not across the whole multi-run
procedure. If a release needs additional manual gating, cancel blocked automatic
runs and confirm the stage slot is free before dispatching operations; do not
leave an approval-waiting run blocking the run needed to satisfy its prerequisites.

When expanding supported currencies, apply business and crawler currency migrations
and add OpenSearch mapping fields before activating the new currency inputs. Capture
and verify a complete expanded FX snapshot before current-price reads use the new
currencies. Preserve immutable older snapshots and rebuild affected projections under
the existing projection fences; historical sale prices retain only their captured quotes.

## Manual operations

[`migrate.yml`](../.github/workflows/migrate.yml) and
[`initialize.yml`](../.github/workflows/initialize.yml) accept stage and a full
lowercase `commit_sha`. This asserts the **already deployed** initialize stack's
`CommitSHA`; it does not select/build/deploy source. Their shared helper requires
a complete stack and exact SHA, waits for the function update, then invokes it
synchronously. Success requires status 200 without `FunctionError`, plus exactly
`{"status":"ready"}` for migration or JSON `null` for FX. CLI success alone is insufficient.

The private migrator bootstraps scoped roles/extensions and applies embedded SQLx
migrations under an advisory lock. There is no public route, schedule, startup
migration or SQL-running CloudFormation custom resource. The break-glass
[`psql wrapper`](../infra/sql/rds-bootstrap-roles.sql) requires separate approval
and private secret injection, never command-line passwords.

Initialize is first-time FX capture only. Retries reuse
`deployment:fxrate:initial:{stage}:v1`, even at a later SHA; persisted capture is
deduplicated, provider calls are not exactly once. Neither operation activates
compute, manages OpenSearch or repairs stacks. On timeout/ambiguous completion,
inspect restricted logs and persisted state before retrying; publish no raw payloads.

Before a release adopts existing API/PostConfirmation evidence log groups, inspect
selected templates and exact names/ownership from the Lambda construct. Do not
let normal Deploy create colliding groups. Under separate approval, remove expiry
without deleting events and execute an **import-only** change set for the two
verified `AWS::Logs::LogGroup` resources, with no unrelated changes. Require only
`Import` actions before execution. `GetTemplate` can lose non-ASCII characters;
verify the import template against deployed source and live configuration rather
than assuming a lossless round trip. Then review normal Deploy separately:
no replacement/deletion, retention unset, retained
resources and exact retention-handler exemptions. Consent access/retention duties
belong in [marketing consent](marketing-consent.md), not this ownership migration.

For a federated registration conflict, preserve the existing PostgreSQL account.
Verify the losing Cognito profile's exact immutable subject and issuer against
persisted bindings; only a proven unbound profile may be removed under separately
authorized Cognito administration. Never choose a deletion target by email, link
a Facebook collision or delete when binding/identity cannot be established.

## CDC and scheduled work

DMS declares a stopped CDC-only task, not a full load or automatic slot creation.
Deploy verifies/bootstraps the shared account-level `dms-vpc-role`, refusing to
replace unexpected trust. Manual data-stack deployment must verify that role first.
Selection changes need approval before data deployment and coordination with task
state/update constraints; deploying configuration does not start/restart the task.

First start needs a separately approved actual source slot/LSN and capture/recovery
plan. Follow [first CDC start](durable-worker-runbook.md#first-cdc-start-on-a-new-stage)
for slot inspection/creation, existing-state reconciliation and delivery verification.
Preserve approved start-position compatibility parameters on updates. Later
recovery uses `resume-processing` and the DMS checkpoint, not a new initial LSN.
A lost/invalid slot requires a fenced replay/rebuild plan. Monitor slot lag, retained
WAL, free storage and transport retention; a configured WAL cap does not prevent
storage exhaustion or prove recoverability.

`CdcRouterEnabled` and `PeriodicMatcherEnabled` independently default off at first
compute creation; Deploy preserves prior values. Scope is not an activation override.
Follow [activation and legacy handoff](durable-worker-runbook.md#activation-and-legacy-handoff)
and [failure custody/redrive](durable-worker-runbook.md#failure-custody-and-controlled-redrive)
for routing, archives, native ownership and recovery. Runtime roles do not receive
operator purge/redrive powers.

The periodic matcher is a one-shot private Fargate task. Enable/pause its parameter
through the approved CloudFormation path, preserving other parameters; do not
partially rewrite its Scheduler target or add a restart loop. Pausing does not stop
accepted tasks. Scheduler delivery DLQ and ECS/application failure evidence are
different channels; neither proves an expected run completed. Inspect actual task
revision, exit/report and next scheduled outcome before retiring an old trigger.

## Rollback and stack recovery

After compatibility review, manual Deploy from `develop` selects an older supported
prod tag or merged dev SHA; prefer `scope=all`. `auto` may stop at foundation on
migration-source differences: its Migrate instruction is **forward-release only**.
**Never invoke an older migrator or run down migrations as rollback.**

Rollback rebuilds/reuses artifacts and uses that source's CDK, so it may change
infrastructure and API policy, not just code. Review actual diffs/replacements,
newer database schema, retained jobs, provider/template contracts and committed
effects. Require separate prepared-change-set inspection when needed; environment
approval is not that inspection. This is not atomic across stacks and does not
undo database writes, email/provider effects, external DNS or OpenSearch changes.

Failed/in-progress/rollback-failed/incomplete-import/empty-shell stacks need operator
CloudFormation recovery before release. A usable `UPDATE_ROLLBACK_COMPLETE` or
`IMPORT_COMPLETE` may be deployed after inspecting resource ownership and stack
completeness; status alone does not prove readiness. Failed creation cannot simply
be updated. Inventory
retained buckets, queues, repositories, log groups and SES configuration sets before
cleanup/import; normal workflows neither import nor delete them. Stage-owned email
configuration-set cutover and retained-name recovery requirements are documented in
[marketing consent](marketing-consent.md#operations-and-recovery). Inspect already active mappings
and schedules after partial creation. If only output lookup failed, verify actual
stack state and redo missing checks instead of blindly redeploying.

Cognito schema additions are **forward-only**: custom attributes cannot be removed
or redefined after creation, including during CloudFormation rollback. Retain the
existing pool and permanent attributes; do not delete/replace it to unblock a
release. For an irreversible schema rollback failure, inspect rollback events and
continue rollback with only the minimum confirmed rollback-failed resource skipped.
Skipping does not reconcile live state: review and deploy a forward template that
retains the permanent schema before considering recovery complete. A cancelled
forward update alone is not grounds to skip a resource.

Preserve failure custody under [controlled recovery](durable-worker-runbook.md#failure-custody-and-controlled-redrive).
Never purge queues or replace RDS to get a release through. Database recovery needs
an isolated snapshot/PITR restore, verified engine/roles/schema/password alignment
and endpoint/secret handoff before traffic; do not assume logical slots survive.
Use a reviewed forward fix when an old source is unsupported or incompatible.
