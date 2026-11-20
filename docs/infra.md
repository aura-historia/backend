# Infrastructure and operations

This runbook owns how production is operated: the host's runtime, its credentials, scheduled
work and recovery, and the resources that stay on AWS. The [host guide](../host/README.md) and
the [prod Compose project](../host/prod/README.md) own the host's procedures;
[local CDK tooling](../infra/README.md) owns build, test and synth commands;
[architecture](arch.md) owns design guardrails. Checked-in configuration and a successful synth
are **not evidence of deployed state or workload readiness**: verify the running host and the
physical AWS resources at each change.

## Releases

Application releases build in GitHub Actions and deploy to the host; the
[host deploy runbook](../host/prod/README.md#deploys) owns that path, its approval and its
rollback. The CDK app is never part of a release: an operator deploys its stacks with
`cdk deploy` as described in [deploying the CDK stacks](#deploying-the-cdk-stacks).

## Host

One host runs the whole application next to OpenSearch and the crawler:

| Concern | Owner |
| --- | --- |
| cgroup tiers, memory plan, firewall, SSH and the deploy user | [Host guide](../host/README.md) |
| The `aura-prod` Compose project: API replicas, MCP, worker, PostgreSQL, Caddy and its networks | [Prod Compose project](../host/prod/README.md) |
| Secret files and their encrypted copy | [Secrets](../host/prod/README.md#secrets) and [runtime credentials](#runtime-credentials) |
| Deploys and rollback | [Deploys](../host/prod/README.md#deploys) |
| Timers for scheduled work and backups | [Scheduled jobs](../host/prod/README.md#scheduled-jobs) and [scheduled work](#scheduled-work) |
| pgBackRest backups and restore | [Backups](../host/prod/README.md#backups) |
| OpenSearch | [OpenSearch operations](opensearch-stage.md) and [rebuilding OpenSearch](#rebuilding-opensearch) |
| Outbox, job queue, custody and redrive | [Worker runbook](durable-worker-runbook.md) |
| Container logs, health metrics and alarms | [Monitoring](#monitoring), [logs](../host/prod/README.md#logs) and [alarm tests](../host/prod/README.md#monitoring) |

The API, MCP and the Cognito triggers' internal endpoints answer only requests that carry
CloudFront's origin secret ([origin lock](#cloudfront-origin-lock)); PostgreSQL and OpenSearch
never leave the host's networks.

## Runtime credentials

Every runtime credential is a root-owned file on the host, one credential per file; the
[secrets section](../host/prod/README.md#secrets) lists them and their sources. Compose passes a
service only the files it lists, and a process reads its settings and credentials once at
startup ([process startup](../src/platform-process/README.md)), so a changed credential takes
effect when the services that list it are recreated (`compose.sh up -d <service>`). Runtime
code never fetches a secret per request or job. `settings.env` holds non-secret settings only.
Never put credentials in the repository, CLI arguments, logs or deploy output.

To change a third-party credential, install the new value, recreate the services that list it,
verify it in use and only then revoke the old one; keys overlap, so this is interruption-free.
A PostgreSQL role password is shared by every file that carries it: change it in PostgreSQL and
in each of those files together, then recreate the services that list them. Until they are
recreated, their new connections fail with SQLSTATE `28P01`.

### AWS credentials

Every AWS call of the host (SES, Amazon Location, the Cognito admin calls, the consent-evidence
groups, metrics and pgBackRest's S3 traffic) uses the host IAM user's access key
([host access](#host-access)) through the SDK's default credential chain. The processes need
`AWS_REGION=eu-central-1`, `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` in their environment,
from a root-only env file. Pass the key itself, not a `_FILE` variable or a shared credentials
file: the Google client below reads only the environment.

### Edge credentials

The resources that stay on AWS take their credentials at deployment, never at runtime:
CloudFront's origin secret, the identity providers' client secrets and the Cognito triggers'
signing key are Secrets Manager secrets that CloudFormation resolves into those resources'
configuration when it updates them. Each is pinned by a `<Name>CredentialVersion` stack
parameter that the template's `AuraHistoria::CredentialVersions` metadata maps to its secret;
[`credentials.ts`](../infra/src/credentials.ts) owns the names and JSON fields. Templates accept
only the UUID version IDs Secrets Manager generates; never supply a client request token. When
deploying a stack, pass each of its `<Name>CredentialVersion` parameters the secret's
`AWSCURRENT` version ID (`aws secretsmanager describe-secret --secret-id <name> --query VersionIdsToStages`).
The deployed configuration holds plaintext: restrict read access to it. The SSM `String`
parameters the app reads hold non-secret configuration only, because change sets report their
resolved values.

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
  holds the Vertex AI role the application needs, directly or through
  `roles/iam.workloadIdentityUser` on a service account that holds it.

On the host, generate the configuration with
`gcloud iam workload-identity-pools create-cred-config <provider resource> --aws` (plus
`--service-account` when impersonating) and delete `region_url` and `url` from its
`credential_source`: without them the client reads the region and keys from the environment
instead of an EC2 metadata server. Install it read-only and root-owned, and mount it into every
container that calls Vertex AI.

Fallback, only if federation is unavailable: point `GOOGLE_APPLICATION_CREDENTIALS` at a
service-account key file installed the same way, and rotate it by creating the new key before
deleting the old one.

## Remaining AWS resources

The application runs on the self-hosted host. The CDK app declares only what stays on AWS,
for `prod` in `eu-central-1`, except the CloudFront alarms; there is no AWS staging copy.
[`application-stack.ts`](../infra/src/application-stack.ts) composes five stacks:

| Stack suffix under `application-prod` | Ownership |
| --- | --- |
| `host` | The host's IAM user, the versioned backup bucket, the CloudFront origin secret, the Cognito triggers' signing key and the role that publishes their code |
| `identity` | Cognito pool, client, domains, identity providers and both Cognito triggers; the SES configuration set; the host user's Cognito admin grant |
| `api` | CloudFront distributions and Web ACLs for `api.` and `mcp.`, both on the host origin |
| `monitoring` | The host's container log groups, its health alarms and their email topic |
| `edge-monitoring` | In `us-east-1`: the CloudFront 5xx alarms and their own email topic |

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

### CloudFront origin lock

Both distributions use one HTTPS-only custom origin: a dedicated, unproxied DNS name for the
host, read from the SSM `String` parameter `/origin/prod/host-domain-name`. Do not publish that
name. CloudFront forwards the viewer `Host` (AllViewer), so the host's proxy serves and routes
by the public hostnames and holds their certificates; the origin name only locates the host.
CloudFront adds the `X-Origin-Verify` header to every origin request, overwriting any viewer
value. The host's proxy accepts a request only with that header. Port 443 deliberately accepts
every source rather than only CloudFront's address ranges, so the header is the origin's only
barrier and is a credential: whoever holds it reaches the origin directly, past the Web ACLs,
though still subject to the application's authentication and rate limits. Rotate it as above
when it may have leaked. Cache policies, response headers, selective caching and Web ACLs are
unchanged by the origin; see the [HTTP front door](http-api-front-door.md).

CloudFormation reads the origin name and the secret only when it updates a distribution. After
changing either, deploy `api` with a template change, such as the new secret version.

The Cognito triggers run outside any VPC, so neither can reach the host's database. Each calls
its internal endpoint on the host through the public API hostname, because the host answers only
requests carrying CloudFront's origin secret, and signs the request with HMAC-SHA256 over its
purpose, timestamp and body. The host accepts a request only within five minutes of its own clock
and only for the configured user pool, and any failure, refusal or timeout fails the sign-up or
confirmation. The endpoints are not
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

Their code is the zips the [Deploy workflow](../host/prod/README.md#deploys) published for the
commit `identity` names in `CognitoTriggersRevision`; each new commit publishes a function version,
whose description is that commit, and Cognito invokes each function's `live` alias.

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

### Monitoring

The alarms watch the host from AWS, so they fire when the host itself is gone. The worker
publishes the host's [health metrics](../src/aura-historia-worker/README.md#health-metrics) to
`AuraHistoria/Host` once a minute, and a metric filter counts rejected ingestion commands in the
worker's log; Docker ships each service's [logs](../host/prod/README.md#logs)
to `/aura-historia/prod/host/<service>`, kept 30 days at the `warn` level. Thresholds are in
[`config.ts`](../infra/src/config.ts):

| Alarm | Fires when |
| --- | --- |
| `…-host-heartbeat-missing` | No heartbeat for 5 minutes; missing data breaches, so a dead host, Docker or worker, or a host cut off from AWS, fires it |
| `…-host-postgres-unreachable` | PostgreSQL did not answer the worker for 3 minutes |
| `…-host-normal-backlog-age`, `…-host-bulk-backlog-age` | The oldest waiting job of the class is at least 15 minutes, or 12 hours for bulk, old |
| `…-host-parked-jobs` | More jobs or outbox snapshots entered failure custody in the last 5 minutes; it clears 5 minutes later, while custody is kept until an operator acts (see the [worker runbook](durable-worker-runbook.md)) |
| `…-host-ingestion-rejected` | ProductListing ingestion rejected a command in the last 5 minutes. A rejection is acknowledged without effect, so neither custody nor the backlog shows it; the worker log names its command and error code |
| `…-host-low-free-disk` | The host filesystem has at most 20% free, above OpenSearch's low watermark |
| `…-host-backend-cleanup-stale`, `…-host-fx-refresh-stale`, `…-host-periodic-match-stale` | The [scheduled job](../host/prod/README.md#scheduled-jobs) has not succeeded for 2.5 hours (cleanup), 13 hours (FX refresh) or 27 hours (matching): a missed FX refresh or matching run, or two missed cleanups; a missing figure counts as stale |
| `…-host-backup-stale` | The last successful backup is at least 26 hours old; no recorded backup counts as stale |
| `…-api-cloudfront-5xx-rate`, `…-mcp-cloudfront-5xx-rate` | At least 5% of a distribution's requests failed with 5xx in two consecutive 5-minute periods of at least 10 requests |

Every alarm emails its topic when it fires and when it clears. Each stack's topic subscribes the
address in the operator-owned SSM `String` parameter `/monitoring/prod/alarm-email` of its own
region, so the parameter exists in both `eu-central-1` and `us-east-1`; do not publish the
address. A subscription delivers nothing until the address confirms it, and CloudFormation
reads the parameter only when the subscription changes. CloudFront publishes its metrics only
in `us-east-1`, where CloudFormation cannot reference the `api` stack's distributions, so their
IDs are pinned in the configuration; replacing a distribution must update them.

Deploy `monitoring` before the host ships logs to its groups. `edge-monitoring` has no assets
and names no bootstrap roles, so `cdk deploy application-prod-edge-monitoring` deploys its
template inline to `us-east-1` with the CLI's credentials; that region has no CDK bootstrap.

### Deploying the CDK stacks

Deploy each stack with `cdk deploy` from `infra/`, passing its parameters: `identity` takes the
`CognitoIdentityProvidersCredentialVersion`, the `PostConfirmationSigningCredentialVersion` and
`CognitoTriggersRevision`, a commit the [Deploy workflow](../host/prod/README.md#deploys)
deployed, which published both triggers' zips; `api` takes `OriginVerifyCredentialVersion`.
Cognito invokes the triggers for every sign-up and confirmation, so a trigger that cannot
start fails them all: name only a revision whose host already serves both internal endpoints.
Review each change set; `identity` updates run under its
[stack policy](#production-deletion-protection).

Deleting a stack from the app does not delete it from AWS; a retired stack is torn down
separately once nothing depends on it.

## Release compatibility

These constraints limit which commits a deploy or rollback may move between.

### Fresh database baseline

The [initial business schema](../migrations/20260725090000_initial_business_schema.sql)
is the complete baseline for a fresh database. `bootstrap` creates the roles and applies the
baseline ([first start](../host/prod/README.md#first-start)).

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

## Identity

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

### Federated registration conflicts

Preserve the existing PostgreSQL account.
Verify the losing Cognito profile's exact immutable subject and issuer against
persisted bindings; only a proven unbound profile may be removed under separately
authorized Cognito administration. Never choose a deletion target by email, link
a Facebook collision or delete when binding/identity cannot be established.

## Log levels

Every application process reads `LOG_LEVEL`; production runs at `warn`
([compose project](../host/prod/compose.yaml)), except the one-shot `ops`, `migrate` and
`bootstrap` runs, whose reports are info events. Spans stay at INFO, so a WARN or ERROR line keeps
its request and correlation IDs. Consent evidence stays at INFO whatever the level. Raise a level for one service by changing its `LOG_LEVEL` and recreating it.
Container logs go to CloudWatch Logs, one group per service, and Docker keeps a local copy for
`compose.sh logs <service>` ([logs](../host/prod/README.md#logs)).

The Cognito triggers log in Lambda's JSON format with system level `WARN` and filter themselves at
`warn` ([trigger logging](../infra/src/constructs/lambda-logging.ts)); their groups keep seven
days. The two consent-evidence groups never expire. The crawler's logs stay in the host's
journal ([#3079](https://github.com/aura-historia/backend/issues/3079)).

## Scheduled work

Host timers run the scheduled `ops` commands ([schedule](../host/prod/README.md#scheduled-jobs)):
expired-data cleanup and recovery hourly, FX capture twice a day and
[periodic saved-filter matching](events/flow.md#periodic-saved-filter-matching) once a day. A job
never starts the stack and never runs twice at once; a started timer is not completed work, so
read the unit's exit status and the command's JSON report. Periodic matching stops itself after
two hours; its [recovery checks](durable-worker-runbook.md#periodic-matching-recovery) inspect
what a run completed. Before a new database serves traffic, run `ops fx-refresh` once, since
listing reads need a persisted FX snapshot, and `ops backend-cleanup` once, for the public
catalogue overview.

The [public catalogue overview](swagger.yaml) is a rebuildable single-row PostgreSQL
calculation. The hourly cleanup recalculates it as its last step once the stored calculation is
at least 5 h 45 min old by the database clock, so the full scan of live listings runs about
every six hours under its own statement timeout. Reads never aggregate listings: a failed refresh
keeps serving the previous calculation, fails the run and is retried on the next one while still
due. A new database answers `503` until its first calculation. Refresh duration grows with the
live catalogue; watch its logged duration against the statement timeout before the catalogue
outgrows it, because a refresh that keeps timing out repeats its scan every hour.

## Rebuilding OpenSearch

OpenSearch is a rebuildable projection and is not backed up. Rebuild it from PostgreSQL with
`ops reproject-all`, into an isolated index generation where practical; the
[projection runbook](durable-worker-runbook.md#projection-fences-and-rebuild) owns the
procedure, its fences and catch-up, and [OpenSearch operations](opensearch-stage.md) own the
cluster, its security configuration and index assets.

## Rollback and stack recovery

An application rollback is a deploy of an earlier commit ([deploys](../host/prod/README.md#deploys)).
It does not undo SQL, jobs, provider effects or search changes: migrations only expand the
schema, and a reviewed forward fix is the remedy when older code is incompatible. There is no
database rollback framework. Restore the database only from pgBackRest
([restore](../host/prod/README.md#restore)), after a restore check, never by rebuilding an older
source.

A failed, in-progress, rollback-failed or incomplete-import stack needs operator CloudFormation
recovery before the next `cdk deploy`. A usable `UPDATE_ROLLBACK_COMPLETE` or `IMPORT_COMPLETE`
stack may be deployed after inspecting resource ownership and completeness; status alone does not
prove readiness. Failed creation cannot simply be updated. Inventory retained buckets, log groups,
secrets and SES configuration sets before cleanup or import; never delete or import them to get a
deployment through. Stage-owned email configuration-set recovery is documented in
[marketing consent](marketing-consent.md#operations-and-recovery).

Cognito schema additions are **forward-only**: custom attributes cannot be removed
or redefined after creation, including during CloudFormation rollback. Retain the
existing pool and permanent attributes; do not delete/replace it to unblock a
deployment. For an irreversible schema rollback failure, inspect rollback events and
continue rollback with only the minimum confirmed rollback-failed resource skipped.
Skipping does not reconcile live state: review and deploy a forward template that
retains the permanent schema before considering recovery complete. A cancelled
forward update alone is not grounds to skip a resource.

Preserve job and outbox custody under [controlled recovery](durable-worker-runbook.md#failure-custody-and-controlled-redrive);
never delete custody to get a deploy through.
