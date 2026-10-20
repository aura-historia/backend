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
| `data` | PostgreSQL, CDC transport, durable queues, scoped worker policies and dev CDC alarms |
| `initialize` | Private migrator and FX functions; deployment does not invoke them |
| `compute` | Application/worker Lambdas, Cognito, eventing and scheduled work |
| `api` | REST and MCP HTTP APIs, regional domains, CloudFront and WAF |
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

MCP has its own Lambda reserved concurrency: **2 dev / 5 prod**, with one PostgreSQL
connection per process, independently configured from REST. This bounds active MCP
invocations and adds a configured steady-state pool budget of 2/5 connections when
use cases open their pool. Frozen warm environments, old versions and deployments can retain
idle connections beyond active concurrency, so this is not a database-enforced global connection ceiling. Confirm
actual PostgreSQL/account headroom including REST, workers, migration/admin reserves
and version/drain overlap before traffic or increasing either setting. Recycle/drain
old environments and measure real connections during promotion; Gateway rate limits
are not a database capacity guarantee. MCP receives private database/TLS and shared listing
provider configuration, while IAM retains only its runtime PostgreSQL secret access. Its public
catalogue requires no billing, newsletter or Cognito configuration. Add provider credentials only with their selected family.
Certificate/DNS prerequisites and the generic stage smoke belong to the
[MCP front-door procedure](http-api-front-door.md#mcp-deployment-and-stage-verification).

The declared single NAT, Single-AZ database/CDC resources and single stage search
host are outage dependencies, not a high-availability promise. Review current
engine availability, backups, retention/removal policies, quotas and cost before
execution; dev resources may be destructive on deletion. Retained resources still
require ownership and recovery planning.

### Local Cognito session revocation

Apply [the additive session-cutoff migration](../migrations/20261018063800_cognito_session_revocation.sql)
with the coordinated Release workflow before deploying the API version
that reads it. Deploy all API instances/versions that can serve traffic before relying
on local revocation; older versions ignore the cutoff. Do not roll back to a version
that ignores it while affected JWTs can still be valid. Keep issuer, API and database
clocks synchronized within the 60-second allowance. The
[authentication boundary](http-api-front-door.md#route-and-authentication-boundary)
owns cutoff, provider-failure and out-of-band revocation semantics.

### Removing geography creation receipts

The [receipt cleanup migration](../migrations/20261014120000_remove_geography_creation_receipts.sql)
requires the keyless address/Party-location runtime to be deployed first. That runtime
works with both schemas. After foundation preparation and compatibility review, run the
exceptional Release workflow at the same ref with `scope=all` against the current schema. Drain invocations of
receipt-dependent versions, then invoke Migrate at the deployed initialization SHA to
apply cleanup. After cleanup, rollback targets must also support the keyless schema.
Address ID tombstones, identity guards and private-resolution erasure/fences remain required.

## Deployment inputs

**Release (CD)** in [`deploy.yml`](../.github/workflows/deploy.yml) is the routine forward
release entry point. It replaces the former `auto → Migrate → all` sequence.

| Trigger | Selected source and stage |
| --- | --- |
| Eligible `develop` push | Exact push SHA → dev, routine release |
| Immutable UTC CalVer `YYYYMMDD-HHMM` tag push | Existing tag → prod, routine release |
| Manual Release from current `develop` | Explicit stage/ref; continuation or exceptional operation |

Manual dev `ref` is blank/`develop` or a full lowercase SHA. Prod requires an existing
valid UTC CalVer tag. Sources must be reachable from fetched `origin/develop` and
contain release protocol v1 (`releases/protocol.json`). Historical helpers are rejected
before publication; never retag a release. The selected SHA owns the lockfile, catalogs,
Lambda ZIPs, container digests, mail templates, CDK assembly and release requirements.

| Scope | Contract |
| --- | --- |
| `routine` (default) | Prepare, approve once, pause capture, foundation, migrate, application, verify, resume owned capture, verify recovery |
| `foundation` | Exceptional bootstrap/repair: network/data/initialize; no migration or DMS operation |
| `all` | Exceptional activation or reviewed compatible rollback: foundation plus application; no migration or DMS operation |

`auto` is retired. Exceptional scopes explicitly acknowledge their separate prerequisites;
they do not provide a second routine release path. All helpers require durable stage
ownership. Direct unowned `deploy-stacks.sh` or `invoke-operation.sh` invocations fail.
CloudFormation uses prepared assemblies, change sets and preserved parameters, without
hotswap, imports or previous-template artifact substitution.

### Operator prerequisites

Before relying on these workflows, the repository/AWS owners must verify:

- Protected `develop` with required CI and immutable release tags. Restrict tag
  creation to release maintainers and updates/deletion even for administrators.
  Do not require path-filtered checks without an always-running fallback.
- Both `aws-dev` and `aws-prod` require reviewers and disable administrator bypass.
  Whether operators can approve their own release follows the environment's existing
  self-review policy; the coordinator always requires an actual recorded approval.
  Restrict sources to protected `develop` and approved immutable
  release tags containing the coordinator. The helper checks required reviewers and
  actual run approval history; the bypass setting must also be verified in GitHub's UI
  because the environment REST response does not consistently expose it. GitHub plans
  without these environment protections cannot provide this release contract.
- Retire queued/rerunnable historical Deploy/Migrate/Initialize runs during adoption;
  old workflow code predates ownership checks. Restrict environment sources/tag creation
  and OIDC access accordingly. Do not run historical workflow definitions: dispatch the
  current workflow with a supported source input. Helper protocol checks cannot retrofit
  code already stored in an old workflow run.
- OIDC audience `sts.amazonaws.com` and narrowly scoped subjects: publishers use
  `repo:aura-historia/backend:ref:refs/heads/develop` or the IAM `StringLike`
  pattern `repo:aura-historia/backend:ref:refs/tags/????????-????`; protected tag
  rules and workflow validation enforce digits/date/ancestry. Final Release and
  manual operations use exact `repo:aura-historia/backend:environment:aws-dev`
  or `...:environment:aws-prod` subjects. Checkout does not change that subject.
- Use AWS CLI v2 with conditional S3 writes, Node 26 and GitHub CLI (as on the workflow runner).
- The existing `CI_DEPLOY_ROLE_ARN` and publisher region/bucket variables are
  available outside environments. Verify their account/region/bucket targets
  against [`config.ts`](../infra/src/config.ts). Publishers need scoped immutable
  artifact writes; deployment also needs CDK/CloudFormation inspection/execution,
  parameter resolution and exact migrator/FX invocation permissions.
- Set repository variables `AWS_ACCOUNT_ID` (12 digits), `AWS_REGION`, and
  `RELEASE_STATE_BUCKET` from the independent operational stack below. The approved deployment account is explicit and 12 digits; workload region is
  `eu-central-1`. Template-only synth may omit the account, deployment may not.
- Certificates, DNS/aliases and routing meet the [front-door gate](http-api-front-door.md#deployment-cutover-and-rollback).
  Verify external provider credentials, permissions, callbacks, sender identities,
  recipients, quotas and acknowledgment deadlines before enabling traffic.

Only the final Release job enters `aws-<stage>` after image/ZIP/mail/CDK asset publication and planning. Migrate
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

Release prepares the selected source's Lambda/image catalogs and compiled mail
templates even for foundation-only releases. The MCP ZIP is selected from the same
[`lambda-binaries.json`](../ci/lambda-binaries.json) catalog and stage/SHA artifact
keys as REST; compute publishes its immutable version and `live` alias before the
API stack can send requests to it. ZIPs and templates are stage/SHA-local;
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

1. Verify operator prerequisites and repositories above. Run Release for the selected ref
   with exceptional `scope=foundation`; record the resolved SHA and completed
   network/data/initialize stacks.
2. Run **Migrate (CD)** for that stage and exact deployed initialization SHA;
   require confirmed success under [manual operations](#manual-operations).
3. Have the OpenSearch owner complete [host setup/compatibility checks](opensearch-stage.md#release-and-schema-change-contract)
   and approve private-workload connectivity, scoped credentials and trusted TLS.
4. Run **Initialize (CD)** with the same stage/SHA; verify persisted initial FX.
5. Before same-ref manual Release `scope=all`, verify schema/job compatibility,
   FX, OpenSearch, provider/SES prerequisites, certificate/DNS and
   [native-consumer handoff](durable-worker-runbook.md#activation-and-legacy-handoff).
   Stop old consumers and settle or preserve in-flight/backlog custody first.
   Required pre-activation probes need an approved private path; if unavailable,
   **hold `all`**, rather than deploy active compute as a speculative test.
6. Continue with the pinned full dev SHA or same immutable prod tag. Record actual
   function versions/aliases, mapping/rule/schedule states, front-door/readiness and
   relevant private product-reader/projector and periodic-matcher smoke. Report
   failed/omitted checks explicitly and hold dependent traffic. Complete CDC activation separately below.

Compute creation activates worker mappings, partner integrations and FX/cleanup
schedules together. It is **not an inactive preview** and has no readiness marker.
Foundation success does not prove these prerequisites. Never run two consumers of
one queue or rename/purge a queue to perform handoff.

## Routine and schema-dependent releases

Preparation builds/tests/publishes or reuses every artifact, then produces a read-only
plan with account, region, stage, completed baseline, selected SHA/ref, template change
paths, preserved activation state, requirements and operational phases. Exact OpenSearch
asset differences appear in the summary and downloadable plan artifact. Plan failures
and rejected approvals leave capture untouched.

The one protected `aws-<stage>` approval authorizes the displayed plan. When manual
OpenSearch preparation is required, the approval job explicitly says **Confirm OpenSearch
… completed and release**. Approval also attests the displayed actions/validations for
that stage, SHA and prerequisite digest; complete them while the old application is
running. No additional approval is needed for ordinary releases.

After approval the coordinator atomically owns the stage and rechecks the baseline,
stack/resource identities, task lifecycle, requirements, reviewer policy and activation
state. Any change invalidates the plan. It records pause intent, stops a running DMS task
and waits for stable `stopped`, deploys network/data/initialize once, invokes the exact
initialization release's private migrator and requires its strict success response,
then deploys compute/API and production observability. It checks templates, SHA, image
digests and existing mapping/rule/schedule activation before resuming an owned pause
with `resume-processing`. An intentionally stopped, previously executed task stays stopped.
Every forward release invokes the migrator, including when no SQL is pending; SQLx's
ledger and advisory lock remain authoritative. Initial FX is not invoked.

Capture recovery has a 15-minute deadline. It requires a running task without table
errors, fresh source/target latency at most 300 seconds, and checkpoint movement or
fresh zero source throughput. An enabled router must have its stage-owned mapping enabled
and iterator age at most 60 seconds. When invocation metrics are absent, fresh zero incoming
stream records can prove idleness. Streams can also omit write metrics while idle: fresh
zero source throughput, successful empty Kinesis reads and zero stream read iterator age
provide an alternative idle proof. Missing or stale metrics never count as zero, and
observed router lag cannot be waived by idle evidence. Allow publication delay within the
deadline; progress logs identify unmet observations every 30 seconds and include the last
observation on timeout. This supports idle stages without requiring empty queues.
Disabled routers keep their activation state and are exempt from router lag checks.
Failure after resumption is **deployed but unverified** and retains ownership. Adoption
uses the same read-only health check and records no baseline unless it succeeds.

Traffic, callbacks, workers, schedules, existing messages and running invocations keep
working throughout. PostgreSQL writes can accumulate in WAL during the pause. Review
expand-and-contract compatibility with running code, retained messages and rollback
sources. Crawler-local migrations are outside this contract. Capture topology changes,
DMS replacement, source-slot/endpoint/instance changes, consumer retirement, durable
resource replacement and existing activation changes use dedicated procedures. The
initial guard conservatively rejects DMS table-selection changes and durable-resource
property changes as well; compatible task-settings updates are supported while stopped.

### Release requirements

Append immutable JSON declarations to `releases/requirements/<id>.json`; modifying or
removing an existing declaration, even in an intermediate skipped commit, is rejected.
The planner compares the known completed release with the target and evaluates every
new declaration. Relevant assets include all tracked `opensearch/**` files. Every changed
business migration SQL file also needs explicit running-application/retained-work review.
Unclassified changes block before capture is paused. Rust semantic compatibility cannot
be inferred automatically; add an application-only declaration when an external prerequisite
exists, even if assets did not change. Review remains responsible for prerequisites in
migration bootstrap code, provider configuration and other application changes.

A declaration contains `version: 1`, a stable `id` matching its filename, `kind`
(`OPENSEARCH` or `COMPATIBILITY`), `classification` (`NONE`, `MANUAL`, `UNSUPPORTED`),
`reason` and a canonical repository `runbook` link. `NONE` requires a reviewed rationale.
`MANUAL` is only for compatible OpenSearch preparation and also requires `action` and
`validation`. `UNSUPPORTED` blocks the routine path and identifies the dedicated procedure.
A compatibility declaration must set both `runningApplicationCompatible` and
`retainedWorkCompatible` to `true`. Set `requiresDeployed` to the full prerequisite SHA
for a contract/removal migration: it must already be an ancestor of the completed baseline,
not merely present in the target's history. This prevents skipping the prerequisite release.

Use `assets: {"path": {"from": "<Git blob SHA>", "to": "<Git blob SHA>"}}` to classify
exact content transitions; use JSON `null` for an absent file. Obtain hashes with
`git rev-parse <baseline>:<path>` and `git hash-object <path>`. Several skipped declarations
must form an unambiguous chain from the baseline blob to the target blob. For example:

```json
{
  "version": 1,
  "id": "add-search-field",
  "kind": "OPENSEARCH",
  "classification": "MANUAL",
  "reason": "The new writer needs an additive field before deployment",
  "runbook": "docs/opensearch-stage.md#release-and-schema-change-contract",
  "action": "Apply the reviewed additive mapping to this stage's existing index",
  "validation": "Read the effective mapping and verify the required field type",
  "assets": {
    "opensearch/mappings/product_listings.json": {"from": "<old blob SHA>", "to": "<new blob SHA>"}
  }
}
```

Do not use a `NONE` classification for incompatible index cutovers, fenced writers,
drains, destructive rebuilds or engine transitions that cannot precede the release.
Currency expansion still needs business/crawler migrations, additive search mappings and
a complete expanded FX snapshot before activating inputs; declare its dedicated ordering
as unsupported when ordinary migration-before-application sequencing cannot satisfy it.

### Release ownership setup

Provision the separate `aura-historia-release-operations` stack from
[`infra/bin/releases.ts`](../infra/bin/releases.ts) under an approved setup operation,
using the existing staging bucket. It is outside the application stack sequence. It
provides a private encrypted, versioned, retained S3 bucket and a minute-based pause
monitor. Confirm the `ReleaseAlarmTopicArn` SNS subscription and alarm delivery; both
stage monitor alarms must be healthy before a routine release. Existing RDS WAL/storage
and CDC alarms remain active. The monitor can read state and publish metrics, never
stop/start DMS or clear ownership.

`RELEASE_PAUSE_BUDGET_SECONDS` defaults to 10800 (three hours), configurable from 300 to
86400 seconds. Set it from measured write rate, slot retention and free-storage headroom;
the default is not a capacity guarantee. An overdue owned pause alerts even after runner
loss and blocks further mutations. It never causes automatic resumption.

The state key is `stages/<account>/<region>/<stage>.json`. S3 strongly consistent reads
and conditional `PutObject` (`If-None-Match` initially, `If-Match` subsequently) fence
writers, with a monotonically increasing revision. Bucket policy rejects unconditional
writes and state deletion. Each version retains baseline/target, run/attempt, phase/intent,
confirmed deployment/migration evidence, task identity/checkpoints, original task state,
pause ownership/time, approval/prerequisite evidence and failure/completion status.
There is no expiring lease. An incomplete operation excludes different SHAs and every
exceptional entry point. A continuation can replace a prior run owner only after GitHub
reports that exact attempt completed; AWS operations are reconciled separately.

Add these narrow permissions to the existing approved GitHub OIDC deployment model:

| Permission | Scope |
| --- | --- |
| `s3:GetObject`, `s3:PutObject` | Exact stage state key in the operational bucket |
| `s3:ListBucket` | Dedicated operational bucket (allows distinguishing absent initial state) |
| `dms:StopReplicationTask`, `dms:StartReplicationTask` | Exact stage-owned task ARN from CloudFormation |
| DMS describe-task/endpoint/instance/table-statistics actions | Read-only; use resource scope where the service supports it |
| `cloudformation:DescribeStacks`, `GetTemplate`, `ListStackResources` | Stage stacks, in addition to existing deployment permissions |
| `cloudwatch:DescribeAlarms`, `GetMetricStatistics` | Monitoring reads (AWS requires `*` for these calls) |
| `lambda:GetEventSourceMapping`, `events:DescribeRule`, `scheduler:GetSchedule` | Stage activation inspection; service-supported resource scope |
| `ssm:GetParameter` | Plain String partner-bus names under `/eventbridge/<stage>/`; no decryption |

Retain immutable artifact publication, existing CDK/CloudFormation/DMS-role preparation,
and exact private migrator/FX invocation permissions. The GitHub token needs actions read
for environment protections, approval history and previous run attempts. Publishers share
the CI role; workflow approval is not an additional IAM isolation boundary. No routine
root login or long-lived operator key is needed.

Before the first routine release, use **Adopt release baseline** from current `develop`.
Supply the verified deployed compute/initialize SHA. It checks healthy stacks/resources,
matching SHAs, initialized task/checkpoint, capture configuration and CDC health; its
protected approval attests all application stacks and actual OpenSearch state correspond
to that source. This explicit operator attestation is necessary for pre-coordinator
releases with no durable marker. Missing or mismatched baselines fail closed. Adoption
cannot discard an incomplete release and never starts a new task. After a successful
exceptional operation, finish its dedicated procedure and adopt the verified baseline again.

### Release recovery

Read the summary and versioned stage record before acting. A failed migration never
permits application deployment; failed/partial application deployment never permits blind
resumption. Resume failure may mean AWS already accepted the request. Verification failure
can leave capture running but the release incomplete. Never infer state from the workflow
conclusion alone, delete the record, reset a checkpoint or start with a fresh LSN.

Cancel an abandoned owner and wait for its GitHub attempt to finish. Inspect the recorded
stack operation and exact task/slot identity. Rerun **all jobs**, or manually dispatch
Release for the **same pinned SHA/tag**; a new attempt needs a fresh plan and approval.
Preparation waits up to 30 minutes for a recorded CloudFormation update and up to 15 minutes
for a recorded DMS transition. Confirmed phases are preserved; ambiguous stack updates are
checked against live templates/parameters before redeployment. An unconfirmed migration is
invoked again through its existing SQLx lock/ledger validation. Never mark success from a
workflow flag. Changed task identity, failed stacks or unsupported capture state require the
owning dedicated repair procedure before continuation.

If the pause is overdue, inspect retained WAL, available storage, slot validity and the
alert. A measured budget increase in the repository variable is allowed only after that
review and a fresh approval; the same release retains its original pause time. Repair
monitoring failures rather than treating missing telemetry as healthy. If the original
release cannot be completed, a separately authorized operator recovery must establish a
verified compatible deployment and CDC outcome, fence the old runner, retain its versioned
evidence, then conditionally reconcile state with the current ETag. There is deliberately
no force-unlock or automatic rollback command. Do not use Adopt to erase uncertain work.

## Manual operations

[`migrate.yml`](../.github/workflows/migrate.yml) and
[`initialize.yml`](../.github/workflows/initialize.yml) accept stage and a full
lowercase `commit_sha`. This asserts the **already deployed** initialize stack's
`CommitSHA`; it does not select/build/deploy source. These exceptional operations acquire
the same durable ownership record and refuse an unfinished routine release, even at the
same SHA. Migrate rejects an initialization source older than deployed compute. Their shared helper requires
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
let a normal release create colliding groups. Under separate approval, remove expiry
without deleting events and execute an **import-only** change set for the two
verified `AWS::Logs::LogGroup` resources, with no unrelated changes. Require only
`Import` actions before execution. `GetTemplate` can lose non-ASCII characters;
verify the import template against deployed source and live configuration rather
than assuming a lossless round trip. Then review the normal release separately:
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
Release verifies/bootstraps the shared account-level `dms-vpc-role`, refusing to
replace unexpected trust. Manual data-stack deployment must verify that role first.
Routine Release automatically pauses/resumes its owned running task as described above.
Exceptional selection changes need approval before data deployment and coordination with task
state/update constraints; deploying configuration does not start/restart the task.
Before task configuration updates, require a stable `stopped` task, or `ready` for a
new task. Treat `failed` as a release blocker even when direct modification succeeds:
CloudFormation can fail both update and rollback while waiting for stabilization.
Complete the [source/destination-retirement gate](durable-worker-runbook.md#activation-and-legacy-handoff)
**before data deployment removes worker queues or access policies**; foundation updates precede
compute updates and do not pause the old router. Exclude `search_filters` from effective capture,
drain its captured records through the old router, pause old publication and settle retiring
job/archive custody before resource removal. Apply the default-replica-identity migration only
after capture exclusion, and resume routing with the compatible destination catalog.
Saved-filter matching reads PostgreSQL on its periodic schedule; only persisted
`search_filter_matches` continue to drive matching notifications through CDC.

First start needs a separately approved actual source slot/LSN and capture/recovery
plan. Follow [first CDC start](durable-worker-runbook.md#first-cdc-start-on-a-new-stage)
for slot inspection/creation, existing-state reconciliation and delivery verification.
Preserve approved start-position compatibility parameters on updates. Later
recovery uses `resume-processing` and the DMS checkpoint, not a new initial LSN.
A lost/invalid slot requires a fenced replay/rebuild plan. Monitor slot lag, retained
WAL, free storage and transport retention; a configured WAL cap does not prevent
storage exhaustion or prove recoverability.

DMS heartbeat advances idle capture in its dedicated `dms_heartbeat` schema. Apply
the owning migration before starting capture; the replication role needs schema
usage/create rights, not general database DDL rights. Heartbeat runs only while
DMS runs. Keep capture pauses within the measured WAL budget; build artifacts before
stopping capture and avoid leaving a healthy slot unattended through a long rollout.

Dev CDC alarms belong to data; prod CDC alarms use the production observability topic.
Alarm on slot WAL lag before the retention limit as well as task failure and free
storage. DMS metric dimensions use the task ARN resource ID, not its friendly name.
Confirm SNS subscriptions separately; an alarm action does not prove operator delivery.

`CdcRouterEnabled` and `PeriodicMatcherEnabled` independently default off at first
compute creation; Release preserves prior values. Scope is not an activation override.
Follow [activation and legacy handoff](durable-worker-runbook.md#activation-and-legacy-handoff)
and [failure custody/redrive](durable-worker-runbook.md#failure-custody-and-controlled-redrive)
for routing, archives, native ownership and recovery. Runtime roles do not receive
operator purge/redrive powers.

The periodic matcher is the only saved-filter matching path: a one-shot private Fargate
task using PostgreSQL active-filter candidates and ProductListing search. Its [matching contract](events/flow.md#periodic-saved-filter-matching)
and [recovery checks](durable-worker-runbook.md#periodic-matching-recovery) are independent of
CDC activation; persisted match notifications still require CDC. Enable/pause its parameter
through the approved CloudFormation path, preserving other parameters; do not
partially rewrite its Scheduler target or add a restart loop. Pausing does not stop
accepted tasks. Scheduler delivery DLQ and ECS/application failure evidence are
different channels; neither proves an expected run completed. Inspect actual task
revision, exit/report and next scheduled outcome before retiring an old trigger.

The [public catalogue overview](swagger.yaml) is a rebuildable single-row PostgreSQL
calculation. The hourly backend cleanup run recalculates it as its last step once the stored
calculation is at least 5 h 45 min old by the database clock, so the full scan of live
listings runs about every six hours under its own statement timeout, which must stay below
that Lambda's timeout. Reads never aggregate listings: a failed refresh keeps serving the
previous calculation, fails the invocation and is retried on the next run while still due.
A new stage answers `503` until its first hourly run. Refresh duration grows with the live
catalogue; watch its logged duration against the statement timeout before the catalogue
outgrows it, because a refresh that keeps timing out repeats its scan every hour.

## Rollback and stack recovery

After compatibility review, manual Release from current `develop` selects an older
supported prod tag or merged dev SHA with exceptional `scope=all`. It requires idle
stage ownership and never invokes the older migrator. Complete the dedicated DMS
coordination and compatibility review first; adopt the verified baseline afterward.
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
