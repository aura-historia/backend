# AWS infrastructure and releases

This runbook owns deployment order, operator prerequisites and recovery boundaries.
[Local CDK tooling](../infra/README.md) owns build/test/synth commands;
[architecture](arch.md) owns design guardrails. Checked-in configuration and a
successful synth are **not evidence of deployed state or workload readiness**.
Verify the approved account, physical resources and effective configuration at each release.

## Delivery evidence and ownership

The versioned [delivery contracts](../ci/delivery/contracts.schema.json) and
[pure validators](../ci/delivery/contracts.cjs) own release evidence. Canonical JSON
sorts object keys and retains array order; its SHA-256 binds every request, report
and immutable bundle. SQLx migration checksums remain SHA-384 and are never
substituted with artifact hashes.

A completed bundle binds its exact source, complete Lambda/image catalogs,
content-addressed versioned objects, registry digests, migration checksums,
OpenSearch/source assets, per-target assemblies and original full-suite tests.
Build identity is source SHA plus approved build-configuration digest. Publish
completion metadata last. Missing/corrupt objects fail reuse. A new deployment
request carries fresh tests and target approval without changing the bundle.

Compatibility evidence binds the target account/region, last verified deployment,
actual schema ledger, cumulative changes, effective templates/configuration,
reviewer model/policy and manual prerequisite digest. Unknown baselines and
uncertain review require maintenance. Manual OpenSearch administration remains
operator work under the [OpenSearch contract](opensearch-stage.md); retained
worker/message compatibility follows the [worker runbook](durable-worker-runbook.md).

The model receives pending migration SQL, schema differences, changed infrastructure
properties, source/contract patches and merged PR descriptions from immutable squash
commit bodies. Unchanged templates and schemas use verified digests; applied SQL and
unchanged implementation files are omitted. The full inventory remains immutable
evidence. Admission recomputes the focused context and verifies both its digest and
its binding to that inventory. Release notes cannot override native facts or hard
policy. The retained source inventory has a separate 8 MiB bound; full before/after
file contents in that inventory do not consume the model's focused input budget.
Oversized relevant context still holds delivery; evidence is never truncated.
Planning and admission read the same native property-level change-set view, resolving
available references instead of treating every possible dependent update as a change.

Healthy idle CDC requires fresh successful polling, explicit zero returned records
and zero reader age. Missing metric samples remain unknown; an absent producer
metric alone does not establish idleness.

The environment record holds only its last verified application bundle, separately
observed SQL state, active execution and an explicit operational hold. Successful
SQL followed by failed application deployment updates schema evidence and preserves
the previous verified bundle. Holds have no expiry or automatic unlock; maintenance
and routine delivery share the same exclusion boundary. Request approvals authorize
only their exact build or target scope and cannot be transferred between environments.

## Infrastructure boundary

The CDK app declares `dev` and `prod` in `eu-central-1`. AWS `dev` serves the
staging API; it is not a stage named `stage`. Application stacks are composed in
[`application-stack.ts`](../infra/src/application-stack.ts):

| Stack suffix under `application-<stage>` | Ownership |
| --- | --- |
| `network` | Private workload/database networking and outbound access |
| `data` | PostgreSQL, CDC transport, durable queues, scoped worker policies and dev CDC alarms |
| `identity` | Durable Cognito pool/client/domain/providers and SES configuration |
| `initialize` | Protected bootstrap, read-only inspector and initial FX functions |
| `migrate` | Candidate forward migrator; routine deployment publishes it before invocation |
| `compute` | Application/worker Lambdas, eventing and scheduled work |
| `api` | REST and MCP HTTP APIs, regional domains, CloudFront and WAF |
| `observability` | Production alarms and alarm topic |

Network/data/identity/initialize are protected platform boundaries; routine order is
migrate, reviewed SQL, compute, API and prod observability.
The standalone container-repository setup stack is outside normal releases.
OpenSearch host administration, external DNS, provider dashboards and native-worker
process/identity deployment are externally owned; CDK does not reconcile them.

PostgreSQL is authoritative and private. Application/migration runtimes use scoped
generated secrets and hostname-verifying TLS with the committed public RDS CA bundle.
Never substitute plaintext, a raw-IP URL or disabled verification. CA refresh is
a reviewed asset release. Per-process connection limits do not cap aggregate
Lambda/ECS database usage. Inspect capacity before activating consumers.

Production REST and MCP enable `PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED`
for concurrent listing-search enrichment. Each process retains its existing PostgreSQL
pool limit; other stages keep sequential enrichment.

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

### Fresh database baseline

The [initial business schema](../migrations/20260725090000_initial_business_schema.sql)
is the complete baseline for a fresh database. Bootstrap provisions roles and applies the
baseline through the normal reviewed deployment path.

The pre-production consolidation replaces migration history and resets application-owned
wire and evaluator versions to 1. Existing pre-consolidation SQLx ledgers are incompatible;
the migrator must reject them. Never edit ledger checksums or mark the baseline applied
to bypass that check. An environment with existing business state needs a separately
reviewed preservation or reset plan before adopting this baseline. Subsequent schema
changes use new forward migrations without rewriting applied history.

Address ID tombstones, identity guards and private-resolution erasure/fences are required
by the baseline. Retained CDC, queue and archive custody follows the
[activation and handoff procedure](durable-worker-runbook.md#activation-and-legacy-handoff).

### Local Cognito session revocation

Local Cognito session revocation requires all serving API versions to honor the persisted
cutoff. Do not roll back to a version that ignores it while affected JWTs can still be
valid. Keep issuer, API and database clocks synchronized within the 60-second allowance.
The [authentication boundary](http-api-front-door.md#route-and-authentication-boundary)
owns cutoff, provider-failure and out-of-band revocation semantics.

## Deployment inputs and approvals

[Release](../.github/workflows/deploy.yml) is a **manual** workflow dispatched from
protected `develop`. Pushes, tags, PRs and schedules never request AWS release
compilation, publication or deployment. Ordinary CI remains automatic.

| Mode | Selection | Protected approvals |
| --- | --- | --- |
| `build` | Full immutable SHA merged into protected `develop`, plus dev/prod | `aws-release-build`, then `aws-dev` or `aws-prod` |
| `promote` | Existing completed bundle digest, plus dev/prod | Target approval only |

Every request runs the [complete exact-source test suite](ci.md#exact-source-release-eligibility),
including promotions and documentation-only changes. Candidate tests receive no
cloud or model credentials. The build approval authorizes compilation/publication
cost. Only its isolated requester role can submit a shared CodePipeline build.
One CodeBuild job compiles the entire Lambda deployment catalog in one Cargo
invocation, builds each catalog image, compiles mail, and synthesizes configured
target assemblies. Lambda builds retain Cargo Lambda’s ThinLTO and one codegen unit;
its command-line profile settings override the LTO environment variable. The matcher
uses its existing GNU target separately from the Lambda MUSL target. Selecting the
deployment catalog avoids including development tools and their dependency features.
Rust, cargo-lambda, Zig, Node, sccache, lockfiles and the prebuilt tool image digest
are pinned by the approved configuration.
Candidate Cargo/build scripts and Dockerfiles execute without forwarded AWS
credentials; compilation uses a network-isolated container. Source mounts remain
read-only. The builder role can publish artifacts, with no database secrets,
application mutation, maintenance or role-passing powers.

The release builder alone restores and saves dependency snapshots in a separate
private S3 cache bucket, with 14-day expiration. Snapshots contain downloaded Cargo
sources and a bounded 10 GB local sccache; they contain no workspace target tree,
application binaries, credentials or deployment evidence. Workspace compilation
bypasses sccache and final binary links always run. The compiler container accesses
only the local cache; CodeBuild handles its S3 transfer outside that container.
Keys separate builder/control configuration, lockfile and Lambda catalog. A new
source SHA saves a new snapshot and restores a compatible earlier snapshot by
prefix. Dependency compilation additionally uses sccache's compiler/input/flag
identity. Cache misses and expired snapshots require ordinary compilation.

To update tools, change the [builder Dockerfile](../ci/delivery/builder/Dockerfile),
run `bash ci/delivery/publish-builder.sh` as a platform operator, and pin its printed
ECR digest in `ci/delivery/build-config.json`. Publication reuses an existing
immutable Dockerfile tag. Review the configuration and update the trusted control
revision through the platform procedure; ordinary releases only pull the pinned
image. Tagged builder images are retained for builds using earlier configurations.

A fresh cumulative target report is prepared after bundle creation/reuse. Review
its exact account/region, source, bundle, baseline, SQL, stack changes, worker
compatibility and **blocking OpenSearch checklist** before target approval.
`MAINTENANCE_REQUIRED`, uncertainty and missing baselines hold routine delivery.
The reviewer has read/inspection/model access and cannot execute generated commands,
weaken hard restrictions or authorize maintenance.

Target approval attests the exact report and any completed manual prerequisite.
OpenSearch commands run locally on the named host under the
[OpenSearch contract](opensearch-stage.md#release-and-schema-change-contract).
Provide a fresh sanitized before-state through dispatch `manual_evidence` as
`{"beforeObservation":{"target":"prod","targetHost":"…","index":"…","alias":"…","mapping":{},"aliases":{},"observedAt":"…"}}`
when private read access is unavailable. Also provide expected after observations as
`manual_evidence` JSON `{"observations":[{"prerequisiteId":"…","mapping":{},"aliases":{}}]}`.
If observations become available while approval waits, an approved operator can
publish the immutable `reports/manual/<request-id>.json` observation object to the
delivery bucket with conditional creation. The protected target job binds those
observations to the displayed target/report/prerequisite digests. Missing or
incorrect expected after-state evidence holds deployment. No pipeline has search
administration credentials.

Optional dispatch `manual_evidence.nativeChecks` requests additional read-only review
evidence. `retiredEventRules` selects `{eventBusName,name}` pairs on an already observed
target bus; the planner verifies absence and execution rechecks it. `databaseInstanceId`
and `lambdaFunctions` select observed target resources for one-hour capacity metrics
and allowlisted pool settings. `sourcePaths` selects supporting tracked Rust files at
the candidate SHA. Requests supply selectors, never observed values or verdicts; missing
metrics remain missing. These checks add no mutation or approval authority.

The review also compares compiled mail-template identifiers and content hashes with
the last verified bundle. A build-specific `MAIL_TEMPLATE_PREFIX` rotation can select
identical template bytes; changed content remains explicit in the report.

The target requester submits pinned request, bundle, report and control-source
versions to its native pipeline. All mutations and verification share one ordered
`Deploy` stage in QUEUED mode, plus the durable environment exclusion record.
Native CloudFormation action roles trust only their owning pipeline role. Notification
rules wait for their SNS publish policy before creation.
CodePipeline stage queueing alone is not a whole-release lock. The candidate
migrator is deployed before invocation, pending SQL runs before application
changes, then native API/MCP CodeDeploy alias rollout and bounded health checks
must pass before the application baseline advances. Worker/message compatibility
is separately reviewed. Routine delivery preserves DMS task identity, configuration
and intentional consumer activation. It cannot stop/start/modify capture or replace
protected durable resources.

Rollout hooks select the qualified candidate from the deployment's registered AppSpec,
verifying its SHA-256 and the exact function/live alias. CodeDeploy target summaries
may omit version fields; neither the current alias nor the latest published version
is evidence of the deployment's intended candidate.

GitHub monitoring is bounded. Cancelling a GitHub runner does not cancel AWS work,
resume DMS or release an environment hold. Inspect native execution links and the
versioned environment record after failure; do not infer success from a workflow
badge. The monitor retries an initially missing native execution for at most two
minutes; authorization errors and terminal failure statuses still fail immediately.
SQL may have committed while application deployment failed. Reconciliation
must preserve that schema evidence and the previous verified application baseline.

### Operator setup and cutover

Delivery infrastructure and stack-boundary migration are separately approved
platform work; implementing this branch does not authorize application deployment.
Before enabling normal requests:

1. Inventory existing physical resources, logical IDs, stack references, artifacts,
   active native operations and any legacy DMS pause. Preserve existing data,
   Cognito identities, queues/backlogs, slot/checkpoint identity and recovery evidence.
   Import/move retained resources through reviewed CloudFormation operations; do
   not deploy the new stack layout blindly over a live deployment.
2. Protect `develop`. Configure required CI checks as described in [CI](ci.md).
   Configure `aws-release-build`, `aws-dev` (if enabled), `aws-prod`, and each enabled
   `aws-<target>-maintenance` with required
   reviewers, protected-branch deployment policy, and administrator bypass disabled.
   The owner chooses whether self-approval is allowed. Verify these in GitHub's UI
   and API, then set `DELIVERY_PROTECTIONS_VERIFIED=true`; helpers also reject missing
   reviewer/branch protections. YAML does not create environment protections.
3. **Revoke or narrow the old broad `CI_DEPLOY_ROLE_ARN` OIDC trust before cutover.**
   Account for historical rerunnable workflows and queued runs. Deleting YAML or
   rotating a repository variable does not revoke an old run's ability to assume
   a still-trusted role. Confirm old writers are fenced before enabling the new path.
4. Review the immutable trusted control ZIP, publish it under `control/<digest>.zip`
   with a version ID, and configure the standalone delivery stack with its key/version,
   exact protected workflow revision, approved build-config digest and CodeConnections
   connection. Source polling, source webhooks, restart-on-update and automatic builds
   are disabled. Admission rejects bootstrap/default executions without authorization.
   Application bundles cannot update the control plane or its permissions boundaries.
5. Use the outputs from [delivery-stack.ts](../infra/src/delivery-stack.ts) for the
   repository variables below. Pin `DELIVERY_WORKFLOW_REVISION` to the approved control commit for helper
   checkouts and test evidence; actual invocation HEAD is recorded separately.
   Trusted helpers compare control-sensitive paths to that pin and reject changed
   workflow entries/helpers until the owner updates the approved control plane.
   App-only commits can advance develop without changing this pin. The protected
   develop entry YAML grants no authority by itself. Keep control-source version,
   pinned workflow revision, build configuration and target files aligned; changing them is platform work.
   Configure a strong review model explicitly, qualified read-only inspector and
   target inventory under `ci/delivery/targets/`. Missing private facts require
   sanitized operator evidence and hold deployment until reviewed.
   The reviewer calls the Anthropic Messages API directly with Opus 5.5 at high
   effort. Its planner role reads only the target's Anthropic API key parameter
   at runtime; never copy that key into GitHub secrets, logs or release evidence.
   The verdict uses schema-constrained JSON. Thinking and response text share
   the bounded output budget. Thinking blocks
   are discarded; malformed, truncated, refused or tool-bearing responses hold
   deployment. Token usage and effort are retained with the review evidence.
6. Establish a verified environment baseline and current schema observation under
   the protected maintenance procedure. A baseline assertion must reflect the
   deployed versions and artifacts, not merely the requested source. Confirm SNS
   subscriptions and budget/failure notification delivery. Record real approval,
   cold/warm build timing, action duration, artifact size, model usage and postcheck
   evidence before declaring an end-to-end cutover verified.

| Repository variable | Delivery stack output / ownership |
| --- | --- |
| `AWS_ACCOUNT_ID`, `AWS_REGION` | Explicit approved workload account; `eu-central-1` |
| `DELIVERY_BUCKET` | `DeliveryBucket` |
| `DELIVERY_CONTROL_VERSION` | Approved immutable control ZIP version |
| `DELIVERY_WORKFLOW_REVISION` | Approved full control-source commit, independent of candidate source |
| `DELIVERY_READER_ROLE_ARN` | `BundleReaderRoleArn` |
| `DELIVERY_BUILD_PIPELINE` | `BuildPipelineName` |
| `DELIVERY_BUILD_REQUESTER_ROLE_ARN` | `BuildRequesterRoleArn` |
| `DELIVERY_PROD_PLANNER_ROLE_ARN`, `DELIVERY_DEV_PLANNER_ROLE_ARN` | `<target>PlannerRoleArn` |
| `DELIVERY_PROD_REQUESTER_ROLE_ARN`, `DELIVERY_DEV_REQUESTER_ROLE_ARN` | `<target>DeploymentRequesterRoleArn` |
| `DELIVERY_PROD_PIPELINE`, `DELIVERY_DEV_PIPELINE` | `<target>DeploymentPipelineName` |
| `DELIVERY_PROD_MAINTENANCE_ROLE_ARN`, `DELIVERY_DEV_MAINTENANCE_ROLE_ARN` | `<target>MaintenanceRoleArn` |
| `DELIVERY_REVIEW_MODEL_ID` | `claude-opus-5-5` for the direct Anthropic reviewer |
| `DELIVERY_DEV_ENABLED` | Same explicit flag as delivery provisioning; default false |

Parameter names and types belong in configuration/constructs, not a second
inventory here. Some credentials use plain SSM `String` dynamic references and
are copied into function configuration; a reference is not runtime secret
isolation. Restrict parameter, function-configuration and deployment access.
Changing a parameter alone does not refresh consumers: deploy/recycle affected
configurations and verify the new credential before revoking the old one.
Never put credentials in source, CLI arguments, logs or release evidence.
Provider/consent-specific requirements live in [marketing consent](marketing-consent.md).

Production managed login uses `https://auth.aura-historia.com`. The identity stack
imports an ACM certificate from `us-east-1` and publishes
`CognitoCustomDomainCloudFrontDomainName`; create a DNS-only CNAME from the custom
hostname to that output after provisioning identity. The parent domain must resolve
before Cognito can create the custom domain. Register
`https://auth.aura-historia.com/oauth2/idpresponse` in both Google and Meta. The
retained Amazon Cognito prefix domain can also be registered as a redirect URI;
frontend app callbacks are separate from these provider-to-Cognito redirects.

For federated login, verify the provider's enabled/live use case and required scopes.
Review the pinned provider version in configuration and test hosted sign-in. Before rollout,
measure cold/warm private pre-sign-up paths and failure cases with margin under
Cognito's fixed trigger deadline. Hold if timing or fail-closed behavior is
unverified. User-pool/schema/client replacement needs an explicit identity and
callback migration; never silently replace a production pool.

### Existing-stack ownership transition

The final split is not a direct safe update of deployed stacks. A separately approved
maintenance plan must freeze both writers, inspect native settlement and retain the
source templates, physical identities, exports/consumers and resource histories.
Prepare ownership-only refactor/import templates that preserve all existing resource
properties and identities. Keep existing exports until consumers switch. Validate
CloudFormation support and import/refactor change sets before execution; a normal
update that deletes the moved source resources is not the transition procedure.

| Existing owner | New owner | Preserve |
| --- | --- | --- |
| compute Cognito/SES | identity | Pool/client/domain/providers and email configuration identities |
| compute scheduler DLQs/router archive | data | Existing queue/bucket physical names, backlog and archived failure custody |
| initialize routine migration Lambda | migrate | Existing `database-migration-lambda-<stage>` function identity |

After ownership is verified, review the inspector secret/private endpoint permissions,
new release-bucket endpoint access, separate bootstrap/inspection functions, artifact
parameters and permissions boundaries. IAM path changes replace roles: retain old roles
**and their inline policies** while any retained Lambda version still uses them. Cut over
stable aliases and drain old versions before retiring policies. DMS/Kinesis/RDS,
replication slots and worker queue identities/settings must remain unchanged.

A fresh environment creates protected identity with `AttachIdentityTriggers=false`,
then compute creates stable pre/post trigger aliases, then separately approved identity
sets `AttachIdentityTriggers=true`. Do not detach or replace an existing pool to perform
this sequence. No existing-stack ownership transition is implied by a successful synth.

### Immutable artifacts and build evidence

The retained delivery bucket owns shared content-addressed bytes, versioned
manifests, exact test evidence, requests and reports. Complete manifests and build
completion markers publish only after every object/digest/catalog validates.
Partially uploaded objects cannot make a bundle eligible. Reuse reads and validates
existing immutable versions; corruption is a failure, never permission to overwrite.
Registry references use immutable digests. Mail uses `mail/<build-id>/mjml/…` for
both environments. Per-target assemblies may differ while application bytes remain
identical. Shared CDK file assets are registered once per target; repeated asset
identities must name the same source. Assets are published and referenced by exact key/version before
assembly completion; deployment performs no Rust, image, mail or synth rebuild.

CloudFormation templates use JSON Unicode escapes so native template readback
preserves their exact values. Baseline and change-set comparisons explicitly read
the `Original` template; processed dynamic references are not the submitted template.
Native pipeline input copies the verified immutable template bytes without JSON
serialization, including for a verified unchanged stack's `CREATE_UPDATE` action.

One on-demand shared build group uses a Linux XLarge worker (36 vCPUs, 72 GiB).
This sizing is provisional, not an optimal performance/capacity claim. Before enabling paid release builds,
an explicitly approved benchmark must record cold/warm duration, peak memory,
cache behavior and final native billed compute minutes; adjust compute if needed. Real CodeBuild executions export observed duration/compute minutes, per-container
cgroup memory peaks when available, pipeline/build IDs and artifact sizes under `builds/<build-id>/metrics.json`; inspect CloudWatch/CodeBuild for compute
minutes, effective timeout/compute type and cache behavior; project settings alone do not prove the actual build's limits. Benchmark cold/warm builds through the protected build
approval before changing group count or compute size. There is no reserved build
fleet, build VPC or automatic source-triggered paid work. Metrics include compiler
cache hits/misses, compilation CPU/resource usage and elapsed build phases. S3
cache transfer time and final billing require native CodeBuild evidence; a local
cache proof is not a production performance benchmark. Pipeline scratch artifacts
and dependency caches have bounded retention; completed bundles and recovery evidence are
retained. Configure the delivery budget and confirm notification subscriptions.

### Optional dev and idle costs

Production works independently of dev and never requires a successful dev rollout.
`devEnabled=false` omits its delivery pipeline and rejects dev requests; this does
not delete a running dev environment. Disabling workflows/pipelines does **not**
remove RDS, DMS, NAT, interface endpoint, storage or self-hosted search-host costs.

Decommission is a separate approved operation: inventory backups/restore evidence,
retained data and keys, queues/backlogs, DNS/provider callbacks, scoped credentials,
search storage, active tasks and replication slot/WAL ownership. Do not leave an
active database with an abandoned slot accumulating WAL. Drain/fence consumers and
settle capture custody before removing dependencies. Later reactivation uses the
same platform/bootstrap procedures and fresh target evidence; this is not an
on-demand ephemeral environment mechanism.

## First-time environment and maintenance

Bootstrap prepares protected network/data/identity resources and the private
bootstrap/inspection functions under separate platform approval. The BOOTSTRAP
function alone reads administrator/runtime/migrator/replication/inspector secrets,
creates scoped roles/extensions, applies migrations and grants the inspector only
catalog access plus ledger version/checksum/success columns. Routine MIGRATE reads
only the migrator secret, validates the approved ledger and pending SQLx checksums
under its advisory lock, and applies forward SQL or no-ops. INSPECT is a qualified,
read-only function. The operation is fixed in each function configuration, never
selected by untrusted invocation input. No startup or CloudFormation SQL runner exists.

Before application activation, complete OpenSearch connectivity/TLS, initial FX,
provider/SES, certificate/DNS and [native-consumer handoff](durable-worker-runbook.md#activation-and-legacy-handoff).
Initial FX is a separately approved private invocation; retries retain
`deployment:fxrate:initial:{stage}:v1`. Persisted capture is deduplicated, provider
calls are not exactly once. Function success alone does not prove workload readiness.
Never run duplicate consumers or purge/rename queues to simplify handoff.

[Maintenance](../.github/workflows/maintenance.yml) is a protected, narrow manual
entry for status, explicit baseline adoption/exclusion, reviewed DMS pause/resume and reconciliation.
It does not execute arbitrary SQL, shell commands or stack updates. It uses the same
non-expiring environment record as routine delivery. Entry and reconciliation may
observe a started CDC task without an optional recovery checkpoint; pause and resume
still require verified recovery checkpoints. A maintenance pause needs fresh
WAL/storage headroom and monitoring evidence; persist exact task/slot/checkpoint
identity and intent before mutation. Resume only the owned normal stop using
`resume-processing`; never invent a new start LSN or automatically clear an overdue
pause. Follow the [worker recovery procedure](durable-worker-runbook.md) for native
custody and capture changes.

Dispatch `status` first to obtain the current state digest. Mutations require an
`evidence` JSON object with that `expectedStateDigest` and a reviewed `reason`.
`enter` acquires maintenance exclusion and records the actual capture baseline;
`pause` additionally requires fresh `wal` evidence (`observedAt`, `retainedBytes`,
`freeBytes`, `maxPauseSeconds`, `monitoringUri`). `resume` reads the exact retained
pause record. Retries reconcile retained intent/results and never issue a blind
second stop/start. `reconcile` additionally requires `migrationInvocationSettled:true`
and a restricted `migrationEvidenceUri`; native settlement, actual schema and restored
intended DMS/capture state plus health are checked before clearing the hold.

Before the first routine request, associate `migrate` with the delivery stack's
migration CloudFormation service role, and `compute`, `api` and `observability` with
its runtime CloudFormation service role. A stack still using the bootstrap role
needs a separately reviewed platform handoff: use the exact versioned bundle
`TemplateURL` and unchanged live parameters, require zero resource changes, then
verify the resulting role, original template, parameter values and every physical
resource identity. Avoid `UsePreviousTemplate` for this handoff: native readback can
materialize dynamic SSM references. An available change set with no resource changes
remains ambiguous to routine delivery; complete this handoff before retrying it.

For the first baseline only, `adopt` takes `bundleDigest`, `reason` and explicit intended
`expectedDmsStatus` (`running`, `stopped` or `ready`). Deploy the
reviewed bootstrap/platform templates and exact bundle through the separately
approved platform procedure first. Adoption checks actual stack templates and
artifact/source parameters, Lambda ZIP and serving alias bytes, immutable container
digests, complete migration checksums, reconstructed schema and native settlement.
It publishes a held record before verification; failure retains that hold and cannot
label an asserted baseline verified. It cannot overwrite an existing verified baseline.

Failed native operations require fresh actual schema plus settled CloudFormation,
CodeBuild, CodeDeploy and pipeline evidence before reconciliation. A manually owned
operation has no invented pipeline ID; inspect its native resources separately.
Unknown migration completion or in-flight native work preserves the hold. Never
mark a failed release verified, erase committed SQL, force-unlock by age, invoke an
older migrator or run down migrations. The
[private psql bootstrap wrapper](../infra/sql/rds-bootstrap-roles.sql) remains separately
approved administration with private secret injection, never passwords in CLI logs.

## Manual platform operations

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
Platform setup verifies/bootstraps the shared account-level `dms-vpc-role`, refusing to
replace unexpected trust. Manual data-stack deployment must verify that role first.
Routine delivery observes DMS without changing its lifecycle. Only reviewed maintenance may pause/resume owned capture.
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
The shared schema inspector excludes only the regular
`dms_heartbeat.awsdms_heartbeat` table owned by `aura_replication`, which DMS creates
and maintains. The live inspector and migration reconstruction use that same rule;
unexpected names, kinds, ownership or other schema changes remain visible to admission.

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
A new stage answers `503` until its first calculation. Before public activation,
invoke the existing backend cleanup once and verify that overview reads succeed.
Refresh duration grows with the live
catalogue; watch its logged duration against the statement timeout before the catalogue
outgrows it, because a refresh that keeps timing out repeats its scan every hour.

## Rollback and stack recovery

Native CloudFormation failure handling and the scoped API/MCP CodeDeploy alias
rollback are the routine recovery mechanisms. They do not undo SQL, messages,
provider effects or search changes. Reconciliation requires actual deployed state
and fresh schema evidence; use a reviewed forward fix when older code is incompatible.
Broader stack repair, protected resource changes and capture changes are separately
approved maintenance. There is no database rollback framework or routine rebuild of
an older source as a rollback shortcut.

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
