# AWS infrastructure and releases

This runbook owns deployment order, operator prerequisites and recovery boundaries.
[Local CDK tooling](../infra/README.md) owns build/test/synth commands;
[architecture](arch.md) owns design guardrails. Checked-in configuration and a
successful synth are **not evidence of deployed state or workload readiness**.
Verify the approved account, physical resources and effective configuration at each release.

## Releases

Application releases build in GitHub Actions and deploy to the host; the
[host deploy runbook](../host/prod/README.md#deploys) owns that path, its approval and its
rollback. The CDK app below is never part of a release: an operator deploys its stacks with
`cdk deploy` as described in [deploying the reduced app](#deploying-the-reduced-app).

## Infrastructure boundary

The application runs on the self-hosted host. The CDK app declares only what stays on AWS,
for `prod` in `eu-central-1`; there is no AWS staging copy. [`application-stack.ts`](../infra/src/application-stack.ts)
composes three stacks:

| Stack suffix under `application-prod` | Ownership |
| --- | --- |
| `host` | The host's IAM user, the versioned backup bucket, the CloudFront origin secret, the Cognito triggers' signing key and the role that publishes their code |
| `identity` | Cognito pool, client, domains, identity providers and both Cognito triggers; the SES configuration set; the host user's Cognito admin grant |
| `api` | CloudFront distributions and Web ACLs for `api.` and `mcp.`, both on the host origin |

The `assets.` distribution and its bucket, the SES identity, DNS and the host itself are
externally owned; CDK does not reconcile them. The two consent-evidence log groups
(`/aws/lambda/aura-historia-api-prod` and `/aws/lambda/cognito-post-confirmation-prod`) never
expire and are retained when the stack that created them is deleted. The app only references
them by name, in the host user's policy; never delete them.

### Host access

The `host` stack deploys independently of the others and before the host serves traffic.

- **Backup bucket.** pgBackRest's repository lives under `pgbackrest/` and the encrypted copy of
  the host's secret files under `secrets/`. The bucket is versioned, TLS-only and private, and is
  retained on deletion. pgBackRest expires its own backups; a replaced or deleted object stays
  recoverable as a noncurrent version for the configured number of days. The host user may read,
  write and delete current objects under those prefixes; the bucket policy denies it removing
  object versions or changing versioning, lifecycle or the policy itself.
- **Host IAM user.** One user with least privilege: SES `SendEmail` through the managed
  configuration set, the Amazon Location calls of the REST API and the geographic worker, the
  Cognito admin calls of the REST API and the identity deletion worker (granted by `identity` on
  the exact pool), `CreateLogStream`/`PutLogEvents` on the consent-evidence groups and the
  container groups under `/aura-historia/prod/host/`, and `PutMetricData` in the
  `AuraHistoria/Host` namespace. CloudFormation never creates its access key: an operator creates
  it, installs it in the host's root-only secret files and rotates it by creating the new key
  before deleting the old one.
- **Origin secret.** `/aura-historia/prod/edge/origin-verify` is generated once and retained.
  An operator installs its `value` in the host proxy's secret file. To rotate, put a new value,
  let the proxy accept both, deploy `api` with the new version in `OriginVerifyCredentialVersion`,
  then drop the old value from the proxy.
- **Trigger artifact publisher.** The Deploy workflow's approved `prod` job assumes this role
  through the account's GitHub OIDC provider and publishes both triggers' zips as
  `<binary>-prod-<commit>.zip` in `aura-historia-binary-artifacts-eu-central-1`, a versioned
  bucket outside the app. The role may only create such objects with `If-None-Match: *`, so it
  can never replace or delete them; set its ARN (`TriggerArtifactPublisherRoleArn`) as the `prod`
  environment's `PROD_TRIGGER_PUBLISHER_ROLE_ARN` secret, a secret because the ARN names the
  account and Actions logs are public.
- **Cognito trigger signing key.** `/aura-historia/prod/edge/post-confirmation-signing` is
  generated once and retained; its name predates the pre-sign-up trigger. Both triggers sign
  their requests to the host with it, each under its own purpose label, so a request signed for
  one internal endpoint never verifies at the other. A second key would add no isolation: both
  triggers' configurations hold the key in plaintext and are readable by the same principals. An
  operator installs its `value` as the API's `COGNITO_TRIGGER_SIGNING_KEY`, which verifies both
  endpoints; `identity` resolves it into both triggers at deployment through
  `PostConfirmationSigningCredentialVersion`. To rotate, put a new value, set the old one as the
  API's `COGNITO_TRIGGER_PREVIOUS_SIGNING_KEY` and the new one as its current key, deploy
  `identity` with the new version, which updates both triggers, then remove the previous key.

### Host runtime credentials

**AWS.** Every AWS call of the host (SES, Amazon Location, the Cognito admin calls, the
consent-evidence groups) uses the host IAM user through the SDK's default credential chain. The
processes need `AWS_REGION=eu-central-1`, `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` in
their environment, from a root-only env file. Pass the key itself, not a `_FILE` variable or a
shared credentials file: the Google client below reads only the environment.

### Vertex AI credentials

Vertex AI stays keyless. `GOOGLE_APPLICATION_CREDENTIALS` names a Workload Identity Federation
configuration of type `external_account` with an AWS credential source: the Google client signs an
STS `GetCallerIdentity` request with the host user's keys and exchanges it for a Google token.
On the Google side, in the project that runs Vertex AI:

- The workload identity pool's AWS provider trusts the workload account and maps
  `google.subject=assertion.arn`. If it has an attribute condition, the condition must admit
  `arn:aws:iam::<account>:user/aura-historia-prod-host`.
- The host user's principal,
  `principal://iam.googleapis.com/projects/<project-number>/locations/global/workloadIdentityPools/<pool>/subject/arn:aws:iam::<account>:user/aura-historia-prod-host`,
  holds the Vertex AI role the Lambda roles held, directly or through
  `roles/iam.workloadIdentityUser` on the service account they impersonated.
- Remove the retired Lambda roles' bindings at teardown.

On the host, generate the configuration with
`gcloud iam workload-identity-pools create-cred-config <provider resource> --aws` (plus
`--service-account` when impersonating) and delete `region_url` and `url` from its
`credential_source`: without them the client reads the region and keys from the environment
instead of an EC2 metadata server. Install it read-only and root-owned, and mount it into every
container that calls Vertex AI.

Fallback, only if federation is unavailable: point `GOOGLE_APPLICATION_CREDENTIALS` at a
service-account key file installed the same way, and rotate it by creating the new key before
deleting the old one.

### CloudFront origin lock

Both distributions use one HTTPS-only custom origin: a dedicated, unproxied DNS name for the
host, read from the SSM `String` parameter `/origin/prod/host-domain-name`. Do not publish that
name. CloudFront forwards the viewer `Host` (AllViewer), so the host's proxy serves and routes
by the public hostnames and holds their certificates; the origin name only locates the host.
CloudFront adds the `X-Origin-Verify` header to every origin request, overwriting any viewer
value. The host accepts a request only with that header and only on port 443 from CloudFront's
origin-facing IP ranges, refreshed daily. Cache policies, response headers, selective caching
and Web ACLs are unchanged by the origin; see the [HTTP front door](http-api-front-door.md).

CloudFormation reads the origin name and the secret only when it updates a distribution. After
changing either, deploy `api` with a template change, such as the new secret version.

The Cognito triggers run outside any VPC, so neither can reach the host's database. Each calls
its internal endpoint on the host through the public API hostname, because the host accepts only
CloudFront, and signs the request with HMAC-SHA256 over its purpose, timestamp and body. The host
accepts a request only within five minutes of its own clock and only for the configured user
pool, and any failure, refusal or timeout fails the sign-up or confirmation. The endpoints are not
part of the public OpenAPI contract.

- **Post-confirmation** forwards each confirmation event to
  `/internal/cognito/post-confirmation`, which registers the user.
- **Pre-sign-up** returns early for native sign-ups. For a federated sign-up it applies the
  provider policy, then asks `/internal/cognito/pre-sign-up` whether the email already has an
  account; the host answers only the decision (allow, reject, or link to the persisted Cognito
  identity and canonical email). The trigger performs the link itself with its own Cognito grant,
  so the host user needs none. The lookup must answer within 2.5 seconds so that the link call
  still fits Cognito's five-second limit. The answer reveals whether an email has an account, so
  only signed requests get one, and neither side logs the email or the decision.

Their code is pinned to objects in the release bundle bucket, which therefore stays, through each
function's artifact key, version and SHA-256 parameters; Cognito invokes their `live` aliases.

### Production deletion protection

CDK enforces termination protection on every stack. The user pool has native deletion
protection, and the pool, SES configuration set, backup bucket and origin secret retain their
physical resources on deletion and replacement. **Never replace the user pool**: it holds every
account, and password hashes cannot be exported. Stack termination protection does not prevent
destructive updates or direct service API calls.

The `identity` stack requires the checked-in
[platform stack policy](../infra/stack-policies/protected-platform.json), which allows
modification but denies resource replacement and removal during updates. Apply it under
approved platform maintenance, from the repository root:

```sh
aws cloudformation set-stack-policy --region eu-central-1 \
  --stack-name application-prod-identity \
  --stack-policy-body file://infra/stack-policies/protected-platform.json
```

An update that intentionally replaces or removes an `identity` resource other than the pool
(such as a trigger permission) needs a reviewed, resource-specific
`--stack-policy-during-update-body`; never one that covers the pool.

### Deploying the reduced app

The reduced `identity` and `api` stacks reach prod only at the cutover, after the host serves
the application; `host` may deploy earlier. Keep the retained stack names. Deploy each stack
with `cdk deploy` from `infra/`, passing its parameters: `identity` takes the
`CognitoIdentityProvidersCredentialVersion`, the `PostConfirmationSigningCredentialVersion` and
`CognitoTriggersRevision`, a commit the [Deploy workflow](../host/prod/README.md#deploys)
deployed, which published both triggers' zips; `api` takes `OriginVerifyCredentialVersion`.
Before the first reduced deployment:

- Both trigger artifacts must be built from code that runs without the database: a
  pre-sign-up trigger that asks the host's endpoint and a configured post-confirmation
  forwarder. Cognito invokes them for every sign-up and confirmation, so a trigger that cannot
  start fails them all; the host must serve both internal endpoints before `identity` deploys.
- `identity` keeps the exports the retired `compute` stack imports, so `compute` may stay until
  teardown. Remove those exports only after `compute` is deleted.
- The retired `observability` stack imports the API Gateway ID, which the reduced `api` stack
  removes. Delete `observability` before deploying `api`; its alarms watch the retired
  serverless runtime.
- Deploying `api` switches the origins and deletes the API Gateway APIs and their regional
  domains in one update. Until writes land on the host, roll back by redeploying the previous
  `api` template, which recreates them in front of the still-deployed Lambdas.
- The trigger move replaces the two Cognito invoke permissions in `identity`; review the
  change set and allow exactly those replacements in the stack-policy override.

Deleting a stack from the app does not delete it from AWS; retired stacks are torn down
separately once nothing depends on them.

### Fresh database baseline

The [initial business schema](../migrations/20260725090000_initial_business_schema.sql)
is the complete baseline for a fresh database. Bootstrap provisions roles and applies the
baseline through the normal reviewed deployment path.

Never edit ledger checksums or mark the baseline applied to bypass the migrator's
ledger validation. An environment with existing business state needs a separately
reviewed preservation or reset plan before adopting this baseline. Subsequent schema
changes use new forward migrations without rewriting applied history.

Address ID tombstones, identity guards and private-resolution erasure/fences are required
by the baseline.

### Local Cognito session revocation

Local Cognito session revocation requires all serving API versions to honor the persisted
cutoff. Do not roll back to a version that ignores it while affected JWTs can still be
valid. Keep issuer, API and database clocks synchronized within the 60-second allowance.
The [authentication boundary](http-api-front-door.md#route-and-authentication-boundary)
owns cutoff, provider-failure and out-of-band revocation semantics.

### Content assessment categories

Every serving version must know every category stored in
`product_listing_content_assessments`: readers fail closed on an unknown one, so listing,
search, watchlist and notification reads that touch such a row fail. Do not roll back to a
version that predates a stored category (`NUDITY` was added by
[#2830](https://github.com/aura-historia/backend/issues/2830)) while rows with it exist.
If a rollback is unavoidable, first delete those rows. Their listings then count as
unassessed, so their images stay hidden until assessment runs again.

## Deployment inputs

### Runtime credentials

Runtime credentials are deployment inputs. Templates reference fields of Secrets Manager
secrets, each pinned to a `<Name>CredentialVersion` stack parameter that the template's
`AuraHistoria::CredentialVersions` metadata maps to its secret. CloudFormation resolves the
values into Lambda and ECS task configuration when it updates those resources, and each
published Lambda version's description carries the pinned versions it uses, so a
credential-only release also moves aliases and versioned consumers. Runtime code never reads a
secret store per request; a changed credential takes effect with a release. The release reviewer
reads the Anthropic secret's key when it runs.

[`credentials.ts`](../infra/src/credentials.ts) owns the secret names and JSON fields; provision
app secrets before the first release that references them, and keep only runtime credentials
under the `app/` prefix. Templates accept only the
UUID version IDs Secrets Manager generates; never supply a client request token. When deploying a
stack, pass each of its `<Name>CredentialVersion` parameters the secret's `AWSCURRENT` version ID
(`aws secretsmanager describe-secret --secret-id <name> --query VersionIdsToStages`).

Plain SSM `String` parameters hold non-secret configuration only: change sets report their
resolved values, which would otherwise reach release evidence. Planning still redacts every
value that may come from a dynamic reference. Database administration, migration, inspection
and replication secrets remain invocation-time reads of the database operations and DMS.
Deployed function and task configuration holds plaintext: restrict its read access. Never put
credentials in source, CLI arguments, logs or release evidence.

To change a credential, write a new secret version from a request file, release, verify the new
credential in use and only then revoke the old one. Third-party keys overlap, so this is
interruption-free. The runtime PostgreSQL role has one password: run the approved bootstrap
operation, which applies role passwords from their secrets, immediately before the release;
until the release completes, new connections from deployed versions fail with SQLSTATE `28P01`.
If that release fails and rolls back, restore the previous secret version to `AWSCURRENT` and run
bootstrap again before retrying.

### Identity setup

Production managed login uses `https://auth.aura-historia.com`. The identity stack
imports an ACM certificate from `us-east-1` and publishes
`CognitoCustomDomainCloudFrontDomainName`; create a DNS-only CNAME from the custom
hostname to that output after provisioning identity. The parent domain must resolve
before Cognito can create the custom domain. Register
`https://auth.aura-historia.com/oauth2/idpresponse` in both Google and Meta. The
retained Amazon Cognito prefix domain can also be registered as a redirect URI;
frontend app callbacks are separate from these provider-to-Cognito redirects.

For federated login, verify the provider's enabled/live use case and required scopes.
Review the pinned provider version in configuration and test hosted sign-in. User-pool/schema/client replacement needs an explicit identity and
callback migration; never silently replace a production pool.

A fresh environment creates protected identity with `AttachIdentityTriggers=false`,
then compute creates stable pre/post trigger aliases, then separately approved identity
sets `AttachIdentityTriggers=true`. Do not detach or replace an existing pool to perform
this sequence.

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
provider/SES, certificate/DNS and [consumer activation](durable-worker-runbook.md#activation-and-cdc-source-retirement).
Initial FX is a separately approved private invocation; retries retain
`deployment:fxrate:initial:{stage}:v1`. Persisted capture is deduplicated, provider
calls are not exactly once. Function success alone does not prove workload readiness.
Never run duplicate consumers or purge/rename queues to simplify handoff.

## Manual platform operations

For a federated registration conflict, preserve the existing PostgreSQL account.
Verify the losing Cognito profile's exact immutable subject and issuer against
persisted bindings; only a proven unbound profile may be removed under separately
authorized Cognito administration. Never choose a deletion target by email, link
a Facebook collision or delete when binding/identity cannot be established.

### CloudWatch log cost controls

CloudWatch bills log ingestion and custom metrics, so production logs only what needs attention
and publishes no per-source metric.

- Every Lambda reads `LOG_LEVEL` (`warn` in production, `info` in dev) and logs only at that
  level and above; spans stay at INFO so a WARN or ERROR line keeps its request and correlation
  IDs. Consent evidence stays at INFO whatever the level, and the ingestion outcome lines the
  metric filters read are not level-filtered. Raise the level for one function by changing its
  `LOG_LEVEL` in CDK and releasing. Consumers invoke published versions, which snapshot the
  configuration, so editing `$LATEST` in the console changes nothing for them. The API, MCP and
  Cognito triggers get a new version from the change; the queue workers and the CDC router
  run explicit versions identified by artifact and credentials only, so a change of `LOG_LEVEL`
  alone reaches them with their next artifact release.
- Lambda's JSON log format and system log level `WARN` drop the START, END and REPORT lines, so
  REPORT's memory and init figures are gone; set the system level to `INFO` on one function to see
  them again. The application log level is deliberately not set to `WARN`: Lambda filters a custom
  runtime's logs by each line's `level` and treats a line without an RFC 3339 `timestamp`, which
  the shared formatter omits, and plain-text panic output as INFO, so it would drop the errors. The
  application filters itself.
- Every Lambda declares its log group with seven days of retention, except the ingestion and MCP
  groups (30 days) and the two consent evidence groups, which never expire. The retention handler
  gives 30 days to any other new group, unless it was created with a retention.
- Neither API Gateway writes an access log.
- The crawler's log group lives outside CloudFormation and keeps seven days of retention.

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
Complete the [source/destination-retirement gate](durable-worker-runbook.md#activation-and-cdc-source-retirement)
**before data deployment removes worker queues or access policies**; foundation updates precede
compute updates and do not pause the old router.
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
Follow [activation and source retirement](durable-worker-runbook.md#activation-and-cdc-source-retirement)
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
revision, exit/report and next scheduled outcome.

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
