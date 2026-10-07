# Aura Historia infrastructure

This directory contains the AWS CDK app for the Aura Historia backend.

The application is split into small CDK/CloudFormation stacks with typed
configuration objects instead of hand-written templates. The same stack code is
synthesized for:

- `prod` — real AWS production resources, production alarms enabled
- `dev` — real AWS development resources, production alarms disabled
- `ephemeral` — LocalStack resources, including a local OpenSearch domain

## Migration baseline

The [architecture contract](../docs/arch.md#12-cdc-and-projection-architecture) records target worker ownership and cutover boundaries. This README's [DMS declaration](#dms-cdc-declaration-and-evidence-1781) records checked-in infrastructure; [event flow](../docs/events/flow.md) owns routing and the [worker runbook](../docs/durable-worker-runbook.md) owns custody and handoff. None proves deployed state or substitutes for an approved DMS first-start/recovery plan.

## Structure

```text
bin/app.ts                 # CDK entrypoint and stage selection
src/application-stack.ts   # data, compute, API, observability stack composition
src/config.ts              # stage configuration, fixed buckets, RDS shape, SSM dynamic refs
src/worker-queue-config.ts # typed worker queue scopes, timing, retention, alarms
src/parameters.ts          # artifact version and shared runtime configuration inputs
src/resources/             # synth-time resources, e.g. Cognito email HTML and inline JS
src/constructs/            # focused infrastructure modules
  api.ts                   # HTTP API Gateway routes, domain, CloudFront, WAF, CORS, JWT authorizer
  cognito.ts               # Cognito user pool, public client, IdPs, hosted UI domain
  dms-cdc.ts               # private CDC-only PostgreSQL-to-Kinesis DMS resources
  eventing.ts              # EventBridge buses/rules, SQS mappings, Pipes
  lambdas.ts               # Lambda definitions, env vars, IAM grants
  network.ts               # two-AZ VPC, one NAT/EIP, S3 endpoint, workload security groups
  observability.ts         # prod-only alarms and alarm topic
  periodic-matcher.ts      # one-shot saved-filter matching task, Scheduler, DLQ, lifecycle evidence
  opensearch.ts            # external dev/prod endpoint or LocalStack domain
  queues.ts                # Shopify queue/DLQ and separate FIFO command ingress
  worker-queues.ts         # scoped worker queue ownership, unbound IAM policies, handoff outputs
  storage.ts               # private RDS PostgreSQL, generated role secrets, connection settings
sql/
  rds-bootstrap-roles.sql      # break-glass psql wrapper for role bootstrap
  rds-bootstrap-roles-core.sql # shared static role/bootstrap SQL used by private Lambda

```

### `dms-cdc` construct (#1781)

`DmsCdc` is declared for real stages. Its checked-in shape is configuration, not a deployment or live-AWS claim:

```text
DmsCdc (real stages only)
├── replication subnet group in private application subnets
├── single-AZ dms.t3.small replication instance
├── PostgreSQL source endpoint using the exact replication secret
├── Kinesis target endpoint and one provisioned, seven-day stream
├── stopped CDC-only replication task
├── Kinesis interface endpoint with private DNS
└── Secrets Manager interface endpoint with private DNS
```

It composes in the data stack after network and storage. It does not exist for `ephemeral`, full-load tables, an outbox, a custom CDC target, or a Sequin redesign. See [DMS declaration and evidence](#dms-cdc-declaration-and-evidence-1781) for checked-in slot, start-position, test and cost boundaries. DMS first start requires a separately approved operator plan.

The real-stage compute stack additionally declares the disabled-by-default `cdc-router-lambda`. It is an `x86_64`/`provided.al2023` artifact outside the VPC with no PostgreSQL or Secrets Manager configuration. Its Kinesis mapping begins at `TRIM_HORIZON`, reports partial failures, limits retries/record age, and has a private retained S3 on-failure archive. It validates all ten destination SQS queue pairs at cold start and has only Kinesis-read, source-queue publish/attribute, DLQ-attribute, and failure-archive write/list grants. `CdcRouterEnabled` independently controls only that mapping; it does not activate DMS or another consumer. R6 selects the journal plus raw revisions, saved filters, matches, and delivery intents, with explicit trigger operations enforced by the router. #1788 owns AWS evidence and controlled replay proof.

## Common commands

Use Node **26**, matching the workflow pin. `npm ci` uses `package-lock.json`;
no dependency or policy-gate overrides are needed. Run from `infra/`:

```bash
npm ci
npm run build
npm test
npm run synth -- --context stage=dev
npm run synth -- --context stage=prod
npm run synth -- --context stage=ephemeral
npm run synth:all
npm run cdk -- synth aura-historia-container-artifacts \
  --app 'npx ts-node --prefer-ts-exts bin/artifacts.ts'
```

These commands build, test, and synthesize only; they do not deploy.

### Cognito verification email

The Cognito verification body is the static English template at
`mjml/cognito/verification/en.mjml`. Its checked-in generated HTML is
`infra/src/resources/cognito-verification-email.html`; do not edit that output
by hand. With the pinned MJML dependency installed, regenerate it using the one
generator command:

```bash
npm --prefix mjml run generate:cognito-verification
```

Install the pinned compiler first with `npm --prefix mjml ci`. MJML CI
regenerates the asset and fails if it differs from the committed HTML; CDK CI
also checks freshness before building or synthesizing. Cognito currently sends
this one static English body; language is not selected dynamically. The
newsletter sentence describes verification only when a person selected the
optional newsletter choice during registration; the message itself does not
create consent. Existing five-language application templates are separate and
unchanged.

### Native signup consent attribute

The pool declares one optional immutable Cognito custom string attribute named
`marketing_consent`. Its event and Amplify name is exactly
`custom:marketing_consent`; the only accepted values are the strings `"true"`
and `"false"`, and absence means no request. The public client can write it
with its initial `SignUp` call, alongside the existing `email`, `given_name`,
`family_name`, and `locale` write permissions. It is not client-readable, so it
does not become a token claim, and Google/other IdP mappings do not include it.
The signup attribute is an immutable record of the initial choice, not current
consent. ConfirmSignUp requires no consent ClientMetadata, so resend, reload,
and another-device confirmation use the persisted Cognito attribute.

The post-confirmation Lambda grants only for
`PostConfirmation_ConfirmSignUp` with Cognito-supplied `email_verified` exactly
`"true"`, a valid exact email and canonical issuer/subject, and
`custom:marketing_consent == "true"`. Only a new registered User uses that
proof: its current consent and one C02 PostgreSQL intent commit with the User
and Cognito binding. Missing, false, malformed, unverified, forgot-password,
federated-without-proof, and registration replay cases create no grant. A
malformed optional value produces a safe diagnostic. This Lambda uses no Loops,
SES newsletter, SQS, or Step Functions integration. Cognito confirmation and
PostgreSQL do not share a transaction; a database failure is returned and does
not undo Cognito confirmation. If trigger retry does not finish registration,
the existing identity-registration recovery remains an operator concern; this
change adds no migration system.

CDK 2.272.0 emits this attribute in the user-pool `Schema` and explicitly
grants only the existing signup/profile standard attributes plus the custom
write permission. Cognito permits adding custom attributes to an existing pool,
but custom attributes cannot be removed or changed; CloudFormation declares a
`Schema` update as no interruption. See [Cognito custom attributes and app
client permissions](https://docs.aws.amazon.com/cognito/latest/developerguide/user-pool-settings-attributes.html).
If a separately approved schema correction requires a fresh User Pool or app
client, the new pool and client IDs require matching backend issuer/pool and
webapp pool/client configuration. Recheck callback/logout URL lists and any
Google/provider callback registrations affected by those IDs. Development
identity mappings must be reset and registered against the new issuer/subjects;
preserve Google linking and ordinary auth configuration. Do not infer consent
for old users or silently replace/delete a production pool.

PR/develop CI runs CDK build, tests, deployment-helper tests, and synthesis in
`.github/workflows/cdk-test.yml` only when infrastructure, deployment helpers,
workflow inputs, or files consumed by those checks change. MJML compilation runs
separately in `.github/workflows/mjml-templates.yml` for template changes;
container image checks retain their own path-filtered workflow (including Rust
workspace sources used by the Docker build). Deploy still validates the selected
release independently on every deployment. The former `Integrate (CI)` CDK and
MJML job names are replaced by `CDK checks / Test CDK infrastructure` and
`MJML template checks / Compile MJML templates`. Update branch protection if it
references the old names; do not require these path-filtered workflows without an
always-running fallback check, since skipped workflows leave required checks pending.

Synth creates these stacks per stage:

- `application-{stage}-network` — real-stage two-AZ VPC, one NAT/EIP, S3 gateway endpoint, and database/workload security groups
- `application-{stage}-data` — private RDS PostgreSQL and DMS/Kinesis CDC in real stages, Shopify/worker SQS, unbound worker IAM policies, and LocalStack OpenSearch
- `application-{stage}-initialize` — real-stage private PostgreSQL migrator and FX Lambdas; deployment does not invoke them
- `application-{stage}-compute` — Lambdas, Cognito, eventing, schedules, and (real stages only) the periodic matcher task
- `application-{stage}-api` — HTTP API Gateway routes, domain, CloudFront, integrations, authorizer
- `application-prod-observability` — prod-only alarms and alarm topic
- `aura-historia-container-artifacts` — stage-neutral retained ECR setup stack for the image catalog, synthesized separately by `bin/artifacts.ts`; not part of normal Deploy

The network stack is absent for `ephemeral`: LocalStack synthesis does not declare a VPC, NAT, EIP, gateway endpoint, or workload security groups. Real-stage stacks use `eu-central-1`; synth may omit an account only for template validation. A deployment must select the approved account explicitly:

```bash
npm run cdk -- deploy application-prod-network -c stage=prod -c account=123456789012 -c region=eu-central-1
```

The app rejects a non-12-digit account context and an explicitly selected real-stage region other than `eu-central-1`. Template-only synth without an account ignores an ambient CI runner region and uses `eu-central-1`; a deployment must select the approved account explicitly. Deploy network, then data and initialize; create compute only through the [first-time stage procedure](#first-time-stage). Data imports the VPC for RDS and compute imports the RDS endpoint plus runtime credential dynamic references. The `cloudwatch-log-retention-lambda` remains outside the VPC. This F3/F4 declaration started from `develop` SHA `dc1ae85af84ee53cf1e8c678e7453017da1ccd56`; it is not live-provisioning evidence.

The `dev` AWS environment is configured to serve the staging API at `https://api.stage.aura-historia.com`
with only the exact CloudFront alias `api.stage.aura-historia.com`; it does not own
`*.stage.aura-historia.com` (including the independently hosted OpenSearch endpoint).
Prod retains the exact alias `api.aura-historia.com`; ephemeral has no custom domain.
This is not an AWS stage rename: stack names, artifact prefixes, and `/.../dev/...`
SSM paths remain `dev`. Both dev certificate references remain under
`/certificates/dev/`: the regional API Gateway certificate must cover the new host
in `eu-central-1`, and the CloudFront viewer certificate must cover it in `us-east-1`.
Neither synth nor this README verifies live certificates, alias ownership, or DNS.
See [HTTP API front door](../docs/http-api-front-door.md#deployment-cutover-and-rollback)
for the approval, cutover, consumer handoff, smoke-check, and rollback gates.

For stage stacks, every Deploy trigger, including rollback, uses the selected
source's CDK CloudFormation change sets (`--method change-set`), never hotswap or an
artifact-only update of previous templates. See [deployment inputs](#deployment-inputs)
for source selection, scopes and operator gates.

Normal Deploy, Migrate and Initialize never import retained resources. A CDC
router failure-archive bucket left by a failed/deleted compute stack requires
[explicit operator CloudFormation resource import recovery](../docs/durable-worker-runbook.md#retained-cdc-archive-resource-import-recovery).
The former `S3_CDC_ROUTER_FAILURE_ARCHIVE_BUCKET_NAME_DEV` / `_PROD` GitHub
variables are no longer used. Automatic adoption was removed because CDK's broad
import switch can adopt unrelated named resources; normal release approval is
not resource-ownership recovery approval.

### Release artifact contract

Deployments do not require a full CDK bootstrap stack in the target account/region.
Each stack uses `CliCredentialsStackSynthesizer` with the existing staging bucket
`aura-historia-cfn-artifcats-eu-central-1`. CDK uploads large CloudFormation
templates and any future file assets under the stage prefix (`${stage}/`). Lambda ZIPs are referenced as prebuilt S3 artifacts keyed by `CommitSHA`, not as
CDK-managed assets. The periodic matcher is a separate immutable ECR artifact,
referenced by digest in each real-stage compute stack; it is not a Lambda ZIP or a
CDK Docker asset. The standalone ECR setup stack is independently synthesizable
without compute parameters, Docker, AWS lookups, or secret values.

Every Deploy trigger, including rollback, pins all application jobs to the resolved
full `CommitSHA`: source, lockfiles, helper tools and stage CDK code. **After the
selected-release checkout**, source-versioned `ci/lambda-binaries.json` and
`ci/container-images.json` generate the Lambda and container publishing matrices.
Lambda ZIPs use `<binary>-<stage>-<commit-sha>.zip`; compiled MJML uses
`<stage>/<commit-sha>/mjml/<template-path>.html` (for example,
`dev/<sha>/mjml/watchlist/product-update/price/en.html`). The MJML compiler comes
from that source's lockfile via `npm --prefix mjml ci`.

ZIPs and mail templates remain stage-local, never copied between stages. Existing
stage/SHA objects are reused and **never overwritten**; missing artifacts are
rebuilt from the selected source and first uploaded with conditional S3
`If-None-Match: *` (`--if-none-match '*'`) writes. A concurrent publisher must not
fall back to an unconditional overwrite. This rule also applies to manual Deploy
and rollback; it does not retroactively prove legacy-object provenance. For the
configured existing bucket, a confirmed S3 `HeadObject` 404 means the object is
missing; authorization, transport and other unexpected failures must block
publication, not be treated as absence.

ECR repositories are prerequisites, not release-created resources. Before image
reuse or publication, the publisher validates the existing named repository
read-only: catalog name, account/region identity and expected URI, `IMMUTABLE` tag
mutability and `AES256` encryption must match. A missing repository or invalid
configuration fails closed with a pointer to [repository setup](#container-repository-setup);
publishing never creates/imports repositories or changes their configuration.
Existing immutable ECR `git-<full-sha>` images are reused across stages. A missing
image is built from the selected release, passes its
`ci/container-images/<id>/smoke.sh`, then receives the immutable tag; never substitute
`latest`. Deploy resolves registry digests for the complete selected catalog and
passes the parameter-to-digest map to CDK. Unresolved required artifacts or failed
repository validation block deployment.

Only the final `aws-cdk-deploy` job in Deploy selects the `aws-<stage>` environment
and passes its policy before changing stage stacks. Image/ZIP/mail publishers run
without an environment, before this checkpoint; their writes only create missing
SHA-scoped immutable artifacts, not infrastructure. Normal Deploy has no
`aws-container-artifacts` job, shared latest-`develop` checkout, repository
reconciliation or global artifact-stack lock. All application jobs remain pinned
to the selected release SHA. The container PR workflow builds/tests without AWS
credentials. Migrate and FX-only Initialize retain their protected manual-operation
environments and invoke deployed functions only; neither publishes artifacts nor
updates compute. See the [admin checklist](#github--iam-repository-admin-checklist-1549)
for OIDC trust and the shared-role approval boundary.

### Container repository setup

The infrastructure provisioning owner must prepare catalog repositories in the
approved account/region **outside normal releases**, under separate setup approval:

1. Inspect each physical repository (currently `aura-historia-periodic-matcher`),
   its configuration and any CloudFormation owner. Reuse a valid existing repository
   without attempting to create it; publishing does not require ownership by
   `aura-historia-container-artifacts`.
2. If absent, provision through the retained standalone `infra/bin/artifacts.ts`
   stack (`aura-historia-container-artifacts`) from a reviewed, pinned `develop`
   source, not a historical release. The entrypoint and `infra/test/artifact-stack.test.ts`
   remain for separately approved setup. Review the synthesized catalog and change
   set against physical inventory before execution; the stack declares retained,
   immutable, AES256 repositories and must not recreate another existing catalog entry.
3. An unmanaged repository or one owned by another stack requires a separate reviewed
   import/ownership migration **if ownership is to change**. Normal publication neither
   imports nor transfers ownership. Never create a duplicate or delete repositories
   or image history to clear an `already exists` failure. Keep the setup catalog
   append-only unless removal/import is explicitly reviewed.
4. Inspect and recover any failed empty `REVIEW_IN_PROGRESS` artifact-stack shell
   separately, including its pending change sets and actual resources. A shell is
   not proof of repository ownership; normal publishing depends on the valid physical
   repository, not that stack's existence or status.

## Periodic saved-filter matcher (#1843)

The real-stage compute stack declares a standalone `aura-historia-periodic-matcher-{dev,prod}` Fargate task and a disabled-by-default EventBridge Scheduler target. The image catalog maps the `periodic-matcher` source crate/binary to the stable stage-neutral ECR repository `aura-historia-periodic-matcher`; [repository setup](#container-repository-setup) verifies physical configuration and ownership separately from releases. Images use `git-<full-source-sha>` tags, while compute task definitions use `repositoryUri@sha256:<registry-digest>`, never a mutable tag. Dev-to-prod promotion reuses the same retained digest. The independently synthesized artifact stack declares all catalog repositories but does not build or select images or run during normal Deploy. Its CloudFormation name is `aura-historia-container-artifacts`; synthesis does not establish live ownership. The matcher repository's physical name, construct identity, retention, and export name remain unchanged.

The compute stack owns one named scheduled ECS cluster per real stage and injects it into each `ScheduledEcsJob`. The cluster's logical ID `PeriodicMatcherCluster207C1F86` is frozen to preserve the CloudFormation resource during the move from matcher-owned to shared cluster; removing the override requires a CloudFormation migration. This is a migration-specific exception, not a pattern for future scheduled ECS jobs. `infra/src/constructs/scheduled-ecs-job.ts` owns each job's private Fargate task, Scheduler, IAM, DLQ, lifecycle-log and output resources. Jobs select CPU/memory and the catalog image platform (amd64 or arm64), and may add task-role grants; the matcher retains its 1024 CPU / 2048 MiB amd64 task and empty task role. The `periodic-matcher.ts` wrapper supplies the matcher image, credentials, environment, schedule and stable resource names; future jobs should use their own wrappers, not extend `aura-historia-cron`.

The matcher schedule is `cron(0 15 * * ? *)`, timezone `UTC`, flexible window `OFF`, task count one, Fargate platform `1.4.0`. `PeriodicMatcherEnabled` defaults to `false` on first creation; Deploy preserves its prior value on updates. Image publication and shared repository ownership follow the [release artifact contract](#release-artifact-contract); historical releases require the [rollback compatibility review](#rollback-and-stack-recovery).

The image is a normal Linux/amd64 executable, not a Lambda runtime. ECS starts `search-filter-periodic-match` with no arguments; it matches once and exits. Do not set an ECS command override or use an automatic restart loop. The task starts at CPU 1024 / 2048 MiB, uses the existing private application subnets and exactly the existing `ApplicationSecurityGroup`, has no public IP, and has a separate empty task role. Its execution role retrieves only the staged runtime PostgreSQL secret and OpenSearch/Google parameters, pulls this ECR repository, and writes the named application log group. The application receives PostgreSQL JSON `username`/`password` from `/aura-historia/<stage>/postgres/runtime`; OpenSearch reader credentials and ADC are injected through ECS secret references, not plaintext CloudFormation environment values. It uses the committed public RDS CA at `/opt/aura-historia/rds-ca/global-bundle.pem`, read-only root filesystem, non-root UID/GID `10001`, and a task-local writable `/tmp` volume for private ADC materialization.

The matcher preserves the existing bounded service policy and per-filter durable progress. A separate PostgreSQL advisory-lock connection excludes overlapping runs; task cancellation/timeout fails the process and does not reset committed matches or progress. PostgreSQL remains authoritative. Only committed `search_filter_matches` facts enter the existing CDC path; the matcher has no SQS, notification, SES, or schema-migration permission. The daily run is at-least-once scheduling with idempotent business writes, not an exactly-once trigger or exhaustive historical backfill.

There are two independent failure channels:

- Scheduler target-delivery failures go to `aura-historia-periodic-matcher-delivery-<stage>`, a dedicated encrypted Standard SQS DLQ with 14-day retention. Production alarms on visible messages through the existing `cloudwatch-alarms-prod` topic. The queue has no automatic consumer or replay.
- ECS STOPPED events for the task-family prefix are written to `/aura-historia/<stage>/periodic-matcher-lifecycle`; production sends whitelisted nonzero-container and startup/interruption notifications through the existing alarm topic. Each EventBridge Logs target emits a JSON envelope with exactly a `timestamp` and a string `message`, for example `classification=interruption taskArn=... taskDefinitionArn=... clusterArn=... status=STOPPED stopCode=TaskFailedToStart`. The stopped and nonzero-exit rules omit `stopCode`; only the interruption rule includes it. The application log group is `/aura-historia/<stage>/periodic-matcher`. Retention is 30 days in dev and 90 days in prod. For lifecycle evidence use Logs Insights `fields @timestamp, @message | sort @timestamp desc`; filter with `@message like /classification=interruption/` or `@message like /classification=application-container-nonzero/`. Application logs are JSON; query `job = "search-filter-periodic-match"`, `stage`, `revision`, `outcome`, `duration_ms`, `window_end`, `filters_selected`, `filters_completed`, and `filters_failed`, and inspect the `message` field values `cron.matching.started`, `cron.matching.report`, `cron.matching.finished`, and `cron.matching.terminal`. A report with failed filters is incomplete and exits nonzero. `SkippedAlreadyRunning` exits successfully as an explicit exclusion, not a matched run.

Neither a clear Scheduler DLQ nor these logs prove that the expected daily task completed; there is no missing-heartbeat detector or durable completion journal. Investigate a missing expected occurrence and inspect the named `periodic-matcher` container's exit/startup reason and task-definition revision. The compute stack exposes the stable `PeriodicMatcherTaskDefinitionArn` output for release verification, alongside the matcher cluster/task/schedule/log details. FX-only Initialize does not inspect or deploy this task. Do not depend on generated nested-construct output keys for operator automation.

Pause or enable future invocations only by updating `PeriodicMatcherEnabled` through the approved CloudFormation deployment path while preserving all other parameters. Do not issue `scheduler update-schedule` with a partial target. Pausing does not stop an accepted/in-flight task or retract a delivered invocation; inspect task state and retries before cutover or rollback. The planned rollout still requires separate owner approval for disabled infrastructure deployment, an approved manual task smoke, retirement/drain of any prior native trigger before enabling Scheduler, and inspection of the next daily result. Synthesis, image checks, and workflow completion are not live AWS acceptance evidence. Dev's standalone private-subnet connectivity test remains waived, not passed; the approved destination-CIDR, DNS and TLS review still applies (`docs/opensearch-stage.md`).

## Rust Lambda artifact contract

The current Lambda catalog uses `provided.al2023`, `x86_64`, and one executable
`bootstrap` in each ZIP. This matches the deployed CDK architecture; do not change
the target without changing the CDK definition and package smoke evidence together.
Real PostgreSQL Lambdas receive the committed public AWS RDS bundle from the
`infra/assets/rds-ca-layer/` Lambda layer at
`POSTGRES_TLS_ROOT_CERT=/opt/aura-historia/rds-ca/global-bundle.pem`. The asset is
sourced from `https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem`
and pinned in git (SHA-256 `e5bb2084ccf45087bda1c9bffdea0eb15ee67f0b91646106e466714f9de3c7e3`);
refresh it deliberately when AWS changes its trust set. Ephemeral test ZIPs instead
contain their process-generated fixture **public** CA at
`/var/task/aura-historia/test-postgres-ca.pem`. No private CA, database password,
provider token, or signed event body is packaged or logged.

CI installs `cargo-lambda 1.9.0` with `--locked` and builds each catalog
binary from `src/<binary>/` (not the workspace root, whose default-run package
does not include the Lambda binaries):

```bash
cargo lambda build --locked --release \
  --target x86_64-unknown-linux-musl \
  --output-format zip --bin <binary>
```

The ZIP lands in the shared workspace `target/lambda/<binary>/bootstrap.zip`;
CI conditionally uploads it under `<binary>-<stage>-<commit-sha>.zip`, matching
`src/constructs/lambdas.ts`. `aura-historia-api` is one `512 MiB` / `15 s` Rust
Lambda package. It uses `lambda_http` for HTTP API v2 envelopes, preserves the
native Axum router, and applies a `14 s` application request deadline. The same
artifact matrix packages `cdc-router-lambda` from its own crate: it is a
`256 MiB` / `30 s` Kinesis-to-SQS transport adapter that has no database, native
worker polling loop, cron scheduler, health daemon, API Gateway route, Function
URL, or reserved/provisioned concurrency. `notification-delivery-lambda` is a 512 MiB / 45s batch-one SQS consumer with an immutable function version. It composes only the durable PostgreSQL delivery service, versioned-template S3 reader, and one-attempt SES sender; mapping activation is a manual handoff gate, never an in-memory delivery cache. Native processes remain the local development entrypoints.

A Lambda root constructs only its selected dependencies during cold start and
reuses immutable configuration plus pool/client handles during warm invocations.
The API Lambda validates its staged Google ADC JSON without logging it, writes a
private `/tmp/aura-historia-google-adc/application_default_credentials.json` file,
sets `GOOGLE_APPLICATION_CREDENTIALS`, and clears the JSON environment value. The
Google/Vertex adapter and ADC provider still initialize only on an embedding request;
an OpenSearch reachability check remains confined to `/api/v1/ready`. It logs only its
component, Lambda request ID, remaining invocation budget, and cold-start duration.
The consent-evidence event below is the specific post-commit exception; no path logs
event bodies, credentials, raw provider payloads, or provider errors.

## Consent evidence log retention

The API and Cognito PostConfirmation Lambdas emit bounded `marketing_consent.evidence.v1`
JSON records only after their consent transaction commits. Their exact log groups are
`/aws/lambda/aura-historia-api-<stage>` and
`/aws/lambda/cognito-post-confirmation-<stage>`. CDK creates explicit log-group resources
with no `RetentionInDays` property and `Retain` removal/replacement policies in `dev` and
`prod`. The API and PostConfirmation functions receive `BACKEND_RELEASE_SHA`; this records
the backend artifact only and does not identify which frontend wording was deployed. The
global CreateLogGroup retention handler receives only those two stage-specific names as
exemptions. It continues to set 30 days for every other newly created log group. Its
existing `DescribeLogGroups` / `PutRetentionPolicy` permissions do not grant log reading
or deletion. Business Lambda roles receive no log read, query, or delete permission.

For a new stage, the explicit groups are created with the compute stack. For an existing
stage, the Lambda-created groups already exist outside CloudFormation; do not run the
normal Deploy first, because it would try to create colliding named groups. Under the
existing deployment/operator approval boundary, complete this one-time ownership and
retention migration without deleting or recreating either group:

1. Hold the normal deployment. Verify the account, region, compute stack, exact physical
   group names, current retention and resource ownership. Stop if either group has a
   different owner or unexpected configuration.
2. Synthesize the exact selected source SHA. Confirm the two `AWS::Logs::LogGroup`
   resources use the names above, have no retention property, and carry `Retain` for
   deletion and replacement. Prepare an import-only CloudFormation template/change set
   for those two logical IDs; preserve all existing stack resources and make no Lambda or
   unrelated changes in the import operation.
3. With explicit operator authorization, remove the existing 30-day retention policy
   from each verified group using CloudWatch `DeleteRetentionPolicy`. This clears expiry
   without deleting archived log events. Record the action and verify both groups still
   exist with retention unset.
4. Inspect the prepared change set and require exactly two `AWS::Logs::LogGroup` imports
   for the verified names, with no creates, updates, replacements or deletions. Execute
   only that reviewed import change set. Verify the groups are now stack-managed and
   retained.
5. Review and deploy the normal CDK template separately. Confirm the API and
   PostConfirmation `LoggingConfig` references the imported groups and the retention
   Lambda's exemption environment contains only those two exact names. Verify no group
   replacement/deletion is proposed and that unrelated log groups keep the existing
   30-day policy.

The infrastructure change does not perform a live import, retention change, log deletion,
or event export. For retained-log access, query fields, privacy review and approved
stream/group deletion steps, follow the [consent evidence operator procedure](../docs/durable-worker-runbook.md#marketing-consent-evidence-logs).

The API Lambda receives `STAGE`, `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH`,
`BACKEND_RELEASE_SHA`, PostgreSQL connection metadata plus `POSTGRES_SECRET_ARN`, OpenSearch endpoint/credentials,
Stripe billing settings, Loops newsletter settings, generated Cognito issuer/JWKS/client/pool settings,
Vertex project and location, and staged ADC credential JSON. Real-stage nonsecret and secret
configuration uses the existing SSM dynamic-reference paths:
`/opensearch/<stage>/endpoint-url`,
`/opensearch/<stage>/reader/{username,password}`, `/stripe/<stage>/api-key`,
`/vertex-ai/<stage>/{project-id,location}`, `/secrets/<stage>/google-application-credentials`, and
`/loops/<stage>/{api-key,newsletter-list-id}`. `LOOPS_API_KEY` uses a plain SSM `String` dynamic
reference; `LOOPS_API_BASE_URL` is the literal `https://app.loops.so/api`.
For AWS `dev`, `/opensearch/dev/endpoint-url` is
`https://opensearch.stage.aura-historia.com:9443`; the API uses the `reader`
pair and search workers use
`/opensearch/dev/{product-projector,filter-projector,percolator}/{username,password}`
as applicable. Shared `/opensearch/stage` configuration stays separate and
unchanged; the dev stack does not read it. See [Stage OpenSearch](../docs/opensearch-stage.md#dev-private-subnet-egress-and-release-gate-1850)
for the dev network and TLS gate.
The Lambda role has only `cognito-idp:ListUsers` and
`cognito-idp:AdminUserGlobalSignOut` on its own user pool, plus
`secretsmanager:GetSecretValue` on its exact runtime PostgreSQL secret. AWS SDK clients
use the execution-role credential chain. Every PostgreSQL Lambda reads that exact ARN at
`AWSCURRENT` before an invocation; same-version composed handlers reuse their warm pool,
and a changed version builds one new full composition while an active invocation keeps its
old pool lease. Real-stage templates never inject PostgreSQL username or password. Ephemeral
uses only fixture username/password and has no Secrets Manager dependency.

### Loops newsletter integration

For the development cut-off, configure the existing dev Loops workspace only; no production workspace setup or audience transfer is required. In that workspace, an operator must create the public mailing list **Aura Historia Newsletter**, record its actual ID, and create the exact string contact properties `language`, `currency`, and `auraUserId`. Set **Settings → Sending → Double opt-in** to **OFF**. Verify the account's sending domain, sender/reply-to, branding, company/contact details, and preference-center/unsubscribe footer before any separately authorized marketing send. Keep workflows paused during setup and controlled acceptance checks. CDK never creates lists/properties, validates credentials, changes account settings, or sends mail.

The API Lambda requires plain SSM `String` parameters for the list ID at `/loops/<stage>/newsletter-list-id` and the raw API key at `/loops/<stage>/api-key`. Provision `/loops/dev/newsletter-list-id` and `/loops/dev/api-key` for the intended dev workspace; the API key is sensitive even though its SSM value is plain text. Restrict parameter reads and do not put credentials in source, command transcripts, tests, or synthesized plaintext. The app's trusted endpoint base is `https://app.loops.so/api`; the adapter appends `/v1/contacts/update`. `ephemeral` uses `https://loops.test/api`, `ephemeral-newsletter-list`, and `ephemeral-loops-api-key`; these are isolated test values, not real credentials. The API Lambda alone receives Loops settings.

The application endpoint `PUT /api/v1/newsletter-subscriptions` remains the website's integration surface. Never send a Loops API key to frontend code or call the authenticated Loops API directly from the browser. This integration writes only contacts submitted through that endpoint; it does not auto-subscribe account creation, profile changes, paying customers, watchlist/search notification preferences, or other application users. It adds no confirmation email or second consent step, and it does not use Loops for transactional messages. Loops owns marketing list membership and opt-outs; a successful `204` means the provider accepted the write, not that a campaign will be sent or delivered. Ordinary repeats omit global subscription state and do not clear an opt-out. For campaigns, use the Loops dashboard, explicitly choose **Aura Historia Newsletter** as the audience, and use language segments rather than separate lists. Review contacts with missing/unsupported language rather than excluding them by default or sending them multiple variants; do not target the whole workspace audience by default. Account email/profile changes do not constitute renewed marketing consent. Process marketing-data deletion through Loops' supported deletion process; deleting an application account does not authorize recreating or resubscribing its marketing contact.

The preceding paragraph describes the existing direct API integration, not an activated consent worker. The [C03 target Loops consent adapter contract](../docs/events/flow.md#loops-consent-adapter-contract) covers identity, readback, provider blocks and failure semantics; it does not authorize Loops workspace changes or live sends.

The identity CloudFormation uses to resolve dynamic references (the deployment principal or configured execution role) needs `ssm:GetParameters` for both Loops parameters. The Lambda execution role does not need runtime SSM access for Loops. The API key is readable as plain text by principals with SSM parameter-read access and is resolved into the API Lambda environment, so principals able to read its Lambda configuration can also read it; tightly restrict parameter, function-configuration, and deployment access. This deploy-time reference is not runtime secret isolation.

Changing the SSM parameter value alone does not refresh a running Lambda environment. For key rotation, update `/loops/<stage>/api-key` and deploy an API Lambda version/configuration update that causes CloudFormation to re-resolve the dynamic reference; verify the new version with a controlled newsletter write, then revoke the old Loops key. For an immediate cut-off, an authorized Loops operator can disable or revoke the key; newsletter writes then fail until a replacement is deployed. This does not remove memberships or change opt-outs. Keep any future production marketing workspace/audience isolated from development contacts and workspace-level quota/state. There is no CDK-managed key rotation, provider fallback, or delivery control; campaigns and all remote-account actions require explicit operator authorization.

## Network foundation (F3)

Real stages have separate `/16` address space: `prod` uses `10.64.0.0/16` and `dev` uses `10.65.0.0/16`. Each has two public, two private-application, and two isolated private-database `/24` subnets across two availability zones. Exactly one managed NAT Gateway is placed in the first public subnet with one explicitly declared EIP. Both application subnet default routes use it; database route tables have no internet default route.

The application route tables use one S3 **gateway** endpoint. Its endpoint policy permits only `GetObject` and `ListBucket` on the existing artifact, mail-template, and CloudFormation-staging buckets. It does not grant Lambda IAM permissions. The data stack's DMS CDC construct declares one shared Secrets Manager **interface** endpoint: private DNS, `open: false`, HTTPS ingress only from `ApplicationSecurityGroup`, `DmsSecurityGroup`, and `MigrationSecurityGroup`, and an endpoint policy allowing `GetSecretValue` only for the exact admin, runtime, migrator, and replication PostgreSQL secrets plus `DescribeSecret` only for replication. F7 separately declares the Kinesis endpoint; no public database, crawler network, or crawler database access is declared here.

`ApplicationSecurityGroup`, `DatabaseSecurityGroup`, `DmsSecurityGroup`, `DmsEndpointSecurityGroup`, and `MigrationSecurityGroup` are exported for RDS/DMS/migration ownership. PostgreSQL ingress is TCP 5432 only from the application, DMS, and migration groups. The database group has no usable outbound rule. The CDK declaration permits application workloads to egress TCP 5432 to the database group, TCP 443 to `DmsEndpointSecurityGroup` for private runtime-secret retrieval, and TCP 443 to IPv4 destinations via NAT. Only in `dev`, `ApplicationSecurityGroup` additionally permits TCP 9443 to `148.251.91.20/32` (the recorded stage OpenSearch host A address) through the same NAT; prod and the other groups have no such rule. This destination address is recorded in `infra/src/config.ts`, not dynamically resolved by security groups. Do not broaden it to `0.0.0.0/0` or assume synthesis proves deployed connectivity. The private migration Lambda may egress only TCP 5432 to the database and TCP 443 to that endpoint. Security groups cannot express hostname allowlists: external HTTPS provider/AWS API host review stays with the workload and IAM/identity configuration; use the DNS-name HTTPS URL and verify the trusted TLS chain **and hostname** in the application, not a raw-IP URL or disabled verification.

The F7 DMS endpoint/security-group contract replaces the earlier open-ended endpoint wording: DMS egress is TCP 5432 to `DatabaseSecurityGroup` and TCP 443 to the interface-endpoint security group only. That endpoint group allows TCP 443 only from `DmsSecurityGroup`, `ApplicationSecurityGroup`, and `MigrationSecurityGroup`; its Secrets Manager endpoint is shared only for the exact DMS replication, application runtime, and migration role secrets. The endpoints have private DNS for `kinesis.eu-central-1.amazonaws.com` and `secretsmanager.eu-central-1.amazonaws.com`. DMS has no broad egress and does not use NAT.

No network stack or network export existed at the F1 baseline, so this change has no obsolete export to remove and makes no stateful-resource replacement. `data` imports the VPC for RDS and `compute` imports the VPC values required by PostgreSQL Lambdas. `NatGatewayEipAllocationId` and `NatGatewayEipPublicIp` are outputs. The host owner waived a standalone AWS-subnet/NAT connection check for dev/stage; **do not require NAT EIP allowlisting** at the Hetzner host. This is not evidence that a deployed AWS workload can reach OpenSearch. The checked-in `/32` is based on the runbook's observed A result, **not** proof of ownership approval, stable DNS, or deployed reachability. Before deploying #1850, the host/network owner must approve the destination CIDR, verify the current DNS A records and gateway routing, and update `infra/src/config.ts` and this runbook if the approved destination differs. If this cannot be established, hold the network deployment and search-dependent activation; never replace the `/32` with an unrestricted rule. Coordinate DNS, CIDR rule and TLS verification on host/IP changes. Under #1802/#1805 record the deployed rule/CIDR and private-workload search-dependent smoke (`/api/v1/ready`, reader query, projector/percolator as appropriate); explicitly report failed or omitted checks and hold dependent rollout instead of calling synth, SSM inspection or the waived standalone NAT test a pass. #1843 must carry this requirement into private Fargate networking where those tasks use OpenSearch; Lambda SG coverage is not Fargate coverage. [Stage OpenSearch](../docs/opensearch-stage.md#dev-private-subnet-egress-and-release-gate-1850) owns the detailed gate. One NAT is an accepted single-AZ egress dependency: its AZ failure stops application-subnet IPv4 egress, and application subnets in the other AZ can incur cross-AZ transfer charges. It is intentionally not highly available egress. RDS/DMS provisioning, external connection tests, pricing review, live change-set diff, and crawler coordination remain separate gates.

## RDS PostgreSQL foundation (F4)

Real stages create one private, encrypted, **Single-AZ** PostgreSQL RDS instance in the two isolated database subnets. It has no public endpoint, Aurora cluster, RDS Proxy, read replica, or crawler principal/access. F7 adds a separately owned CDC-only DMS path and never auto-creates a publication or replication slot. The data stack depends on network; compute depends on data. The existing database security group remains the only TCP 5432 boundary.

The selected engine is PostgreSQL `16.13`, the newest PostgreSQL 16 engine constant available in the pinned CDK library. AWS lists supported RDS PostgreSQL releases in its [release notes](https://docs.aws.amazon.com/AmazonRDS/latest/PostgreSQLReleaseNotes/postgresql-versions.html). Verify `16.13` remains available in the approved account and region before a change set or deploy; synthesis is not that verification.

| Stage | Instance | Initial / maximum gp3 storage | Automated backup retention | Removal policy |
| --- | --- | --- | --- | --- |
| `dev` | `db.t4g.small` | 30 / 60 GiB | 7 days | delete instance and automated backups |
| `prod` | `db.t4g.medium` | 50 / 100 GiB | 14 days | retain instance and automated backups; deletion protection |

Both stages use backup window `02:00-02:30 UTC`, maintenance window `sun:03:00-sun:03:30 UTC`, automatic minor upgrades, PostgreSQL log export, encrypted storage, and copy tags to snapshots. These values are a capacity/cost starting point, not live price, restore, or load-test evidence. Production retention also applies on replacement; retained data needs an explicit operator inventory before cleanup.

The PostgreSQL 16 parameter group requires TLS (`rds.force_ssl=1`) and enables logical replication (`rds.logical_replication=1`, five slots/senders, `max_slot_wal_keep_size=10240`). `rds.logical_replication` is static and requires a reboot before it takes effect. A stalled replication slot retains WAL; the 10 GiB per-slot cap can require consumer recovery or reload and does not make storage exhaustion impossible. Monitor `pg_replication_slots`, replication lag/WAL, and `FreeStorageSpace`. The F7 operator verifies the separately approved existing `test_decoding` slot; CDK and DMS never create or replace it. Full restore/recovery evidence belongs to #1805.

RDS enforces TLS. PostgreSQL Lambdas use the committed public RDS bundle layer at `POSTGRES_TLS_ROOT_CERT=/opt/aura-historia/rds-ca/global-bundle.pem`; migrated Rust roots require SQLx `VerifyFull`, which validates both the trusted CA and RDS hostname. Bundle rotation is an explicit reviewed asset-and-layer release; a missing, wrong, or stale bundle fails closed. Lambda pools are lazy with min zero and max one, never a global RDS connection cap. Local and isolated tests use a TLS-enabled PostgreSQL fixture with a separately packaged, generated public test CA at `/var/task/aura-historia/test-postgres-ca.pem`; plaintext is intentionally rejected. No automatic startup migration or SQL-running CloudFormation custom resource is included.

### SQLx pool and TLS validation (F5)

Migrated roots require a TLS-enabled PostgreSQL fixture. Run the code/config gates with:

```bash
cargo test -p platform-postgres --all-features
npm --prefix infra test
```

Before an approved isolated-RDS smoke, use an RDS endpoint and DNS name covered by its server certificate, inject the CA bundle path through `POSTGRES_TLS_ROOT_CERT`, and record only acquisition/reconnect durations. Prove one successful trusted connection and failure for wrong CA, wrong hostname, and plaintext; do not log credentials, connection strings, or raw provider data. Restart/credential-rotation tests require separate approval and the #1780 refresh handoff.

### Credentials and manual PostgreSQL migration

The data stack generates private Secrets Manager secrets for `aura_admin`, `aura_runtime`, `aura_migrator`, and `aura_replication`. It emits no secret ARN, value, password, username, or connection string as an output. Real application PostgreSQL Lambdas receive the runtime secret ARN only, with direct `GetSecretValue` permission on that one secret and through the dedicated application endpoint. They request `AWSCURRENT` at invocation start, key a full pool-and-handler/router composition cache by secret version ID, and do not close a leased old pool beneath active work. AWS secret rotation itself remains an operator-owned action; this code does not create or mutate a rotation schedule.

Real stages also include `database-migration-lambda-<stage>`. It has no public URL, API route, schedule, or event source. It runs in private application subnets with `MigrationSecurityGroup`, reads exactly the four role-secret ARNs through the private endpoint, uses the committed RDS CA with `VerifyFull`, holds a PostgreSQL advisory lock, bootstraps roles/extensions as `aura_admin`, then applies embedded root SQLx migrations as `aura_migrator`. It returns only safe success/failure categories. Only manual `Migrate (CD)` (`.github/workflows/migrate.yml`) invokes it synchronously. Deploy never runs migrations; `Initialize (CD)` invokes only FX. CloudFormation and business application startup never invoke migrations; crawler-local startup migrations are outside this AWS deployment contract. The CI deploy role needs exact `lambda:InvokeFunction` access to the stage migrator and `fxrate-lambda-<stage>` for their respective manual operations; it never reads database secrets. See [manual operations](#manual-operations) for deployed-SHA and result checks.

`sql/rds-bootstrap-roles.sql` remains a break-glass psql wrapper around the same static core SQL. Use approved secret injection, never command-line or shell-history passwords. The RDS administrator receives membership in `aura_migrator`, and `aura_migrator` receives database `CREATE`, before public-schema ownership changes. The core roles remain scoped: `aura_migrator` owns `public` and creates schema objects; `aura_runtime` has runtime DML and sequence privileges; `aura_replication` has source-table read privileges plus `rds_replication`, but no DDL. The initial migration uses standard RDS `pg_trgm` and `unaccent`; it does not require `pg_ttl_index`.

For recovery, select the retained snapshot or desired point-in-time restore timestamp within the automated-backup window, restore into an isolated replacement instance/subnet/security-group plan, validate engine/parameter/role/schema state, then plan endpoint and secret handoff before application traffic. Do not assume a restore preserves current role grants, application-password alignment, or logical slots/publications. #1805 owns the tested restore runbook and evidence.

## DMS CDC declaration and evidence (#1781)

For `dev` and `prod`, this CDK declaration creates private RDS PostgreSQL `16.13`, single-AZ DMS `3.6.1` on `dms.t3.small` (2 vCPU, 2 GiB), one provisioned Kinesis shard with seven-day retention, and Kinesis plus shared Secrets Manager interface endpoints. The replication secret path is `/aura-historia/<stage>/postgres/replication`; it is never an output or log value. `aura_replication` has table-scoped `SELECT` and `rds_replication` only.

The task is CDC-only and initially stopped. Its PostgreSQL endpoint explicitly selects `aura_historia`, uses DMS's `test-decoding` setting with the pre-existing named slot `aura_historia_dms_cdc_<stage>`, and reads only the generated replication secret through the regional DMS service principal; its distinct Kinesis target service-access role trusts `dms.amazonaws.com`. The empty-default compatibility parameter omits `CdcStartPosition` for greenfield task creation and preserves any existing approved first-start LSN during stack updates. The separately approved first start must obtain and validate the actual source slot/LSN, then call DMS `start-replication` with that approved LSN. Later recovery uses `resume-processing` and DMS's recovery checkpoint, never a replacement parameter value. Deploy may declare a new stopped task; no workflow starts/resets an existing task or creates/replaces a slot or checkpoint. Before data-stack deployment, Deploy bootstraps the shared account-level `dms-vpc-role` (DMS-only `sts:AssumeRole` trust and AWS-managed `service-role/AmazonDMSVPCManagementRole`). It creates a missing role, attaches a missing managed policy to a correctly trusted role, and refuses to replace an unexpected existing trust policy. This role is not owned or deleted by either stage's CDK stack. For manually deployed stacks, an operator must provision/verify it first. A lost or invalid slot requires a new fenced replay/rebuild plan. See [event flow](../docs/events/flow.md#cdc-routing) for table/operation routing and [worker operations](../docs/durable-worker-runbook.md#activation-and-legacy-handoff) for activation custody. An approved DMS operator plan must cover the actual slot/LSN, LOB/Kinesis bounds, fixture protocol and recovery before first start; the checked-in declaration alone does not establish those live facts.

From `infra/`, the existing configuration checks are:

```bash
npm test
npm run synth -- --context stage=dev
npm run synth -- --context stage=prod
```

They do not deploy or exercise AWS. The AWS fixture procedure is documented/manual; no new test script is implied. Real-stage declarations and synthesis are not live-resource or AWS-test proof. Look up DMS, Kinesis retention, and PrivateLink endpoint/data prices at change-set approval or execution; existing NAT has no DMS incremental charge.

## Target artifact boundary

On every trigger, `Deploy (CD)` builds/reuses the selected source's Rust Lambda
ZIP catalog referenced by CDK, including the ten scoped worker Lambdas and
`cdc-router-lambda`, compiled mail templates, and cataloged ECS images. Deployment
references stage-local S3 artifacts by `CommitSHA` and images by registry digest.
It never builds, uploads, configures, or deploys the legacy native
`aura-historia-worker` artifact or Sequin ingress. Native process deployment remains externally owned;
this change does not pause consumers or activate the DMS/Kinesis path.

## ProductListing command ingress (#1859)

The data stack declares a **separate FIFO** `product-listing-ingestion-queue-${stage}.fifo` and
`product-listing-ingestion-dlq-${stage}.fifo`. These are not Shopify's existing EventBridge
input/DLQ or any CDC Standard worker queue. Both require TLS, use SQS-managed encryption,
and disable content-based deduplication: the command publisher supplies explicit group and
deduplication IDs. Source retention is seven days, DLQ retention 14 days, five receives
before redrive, source visibility 270 seconds, and only the named source may redrive into
the DLQ. Real-stage source and DLQ retain on deletion **and replacement**; ephemeral
deletes. Data outputs expose both URLs. Never rename or purge queues to clear an alarm.

Compute imports the source by stage-local name and supplies
`PRODUCT_LISTING_INGESTION_QUEUE_URL` to **only** the API and Shopify producers. Each
gets `sqs:SendMessage` on this source ARN (including `SendMessageBatch` API calls;
`SendMessageBatch` is not an IAM action). Both keep their existing PostgreSQL and
network dependencies; real private application subnets reach SQS over the existing
HTTPS NAT egress, not an unconfigured endpoint policy. The ingestion Lambda has only
its own source consume actions, PostgreSQL runtime-secret access in real stages,
standard Lambda log/VPC permissions, and no DLQ message/replay, provider, search,
or queue-publish powers. The mapping targets a published version, is active after
protected initialization/migration, batches up to ten with **no batching window**,
returns `ReportBatchItemFailures`, and limits event concurrency to two against a
reserved concurrency of two. Its 512 MiB, 45-second invocation uses one PostgreSQL
connection per execution, so this consumer adds at most two database connections;
check aggregate account and database capacity before activation. Do not reserve a
fresh 45 seconds for each of ten messages; large message metadata can shrink a batch.

Runtime must validate *all* SQS message IDs before work and process the batch in
received order. On the first non-complete record, return that message ID **and all
unprocessed successor IDs**, even across groups; only a confirmed committed prefix
is omitted. A failed FIFO group blocks subsequent deliveries until repair/redrive;
submission ID is correlation only, while database command receipts deduplicate
completed commands. Pause the event source mapping to stop consumption, inspect
queue age/depth, Lambda errors/throttles, DLQ and persisted receipts using safe IDs,
then repair before small operator-authorized redrive. An older DLQ command may no
longer be valid after newer state: FIFO ordering and receipts do not make arbitrary
replay safe. Never log bodies, tokens, raw provider payloads, or secrets.

Prod sends Lambda errors/throttles, source oldest-age >=900s, visible backlog >=100,
and DLQ visible count >=1 to the existing alarm topic. Alarms use a five-minute period,
missing data not breaching. Three stage-bounded CloudWatch Logs metric filters count
one **JSON log entry per record** with `ingestion_outcome` equal to `completed`,
`failed`, or `unprocessed`, as `CompletedRecords`, `FailedRecords`, and
`UnprocessedRecords` in `AuraHistoria/ProductListingIngestion/prod`. Partial-response
failures need not increment Lambda `Errors`; the runtime must emit these safe
per-record outcomes (including the suffix) for the counters to populate. They are
not proofs of committed state or substitutes for queue and receipt inspection.

**Release gate:** build and upload the `product-listing-ingestion-lambda` artifact for
this stage/SHA before deploying compute. The protected Initialize/Deploy workflows
apply migrations before compute and deploy the API stack after compute; they do not
guarantee independently deployed producers wait for an active, verified consumer.
Stage data/queue deployment, receipt migration, consumer artifact and mapping
verification must precede enabling API/Shopify command publication. The Shopify
Lambda's FIFO URL and exact source-ARN `sqs:SendMessage` grant are ready for a separate
forwarding runtime cutover; the existing EventBridge → Shopify Standard SQS source/DLQ,
partial-response mapping, private PostgreSQL network/secret and source lookup stay in
place. When forwarding an eligible observation, only confirmed FIFO admission may
acknowledge the upstream Shopify SQS message; the FIFO consumer acknowledges only
confirmed committed application/receipt. Unconfirmed publication can duplicate on
retry and leaves the upstream message in Shopify's retry/DLQ custody; downstream
failures belong to the ingestion FIFO retry/DLQ. See the [flow](../docs/events/flow.md#shopify-queue-forwarding-boundaries-runtime-deployment-gated)
and [handoff runbook](../docs/durable-worker-runbook.md#shopify-forwarding-handoff).
The Shopify producer code now forwards mapped observations. This does not change the
old Shopify mapping or verify live AWS deployment. The separately declared async
HTTP verbs and WooCommerce forwarding also require staged producer cutovers; neither
synth nor CloudFormation resource ordering proves consumer readiness. Follow the
[ingestion rollout, bounded ephemeral smoke, and redrive gate](../docs/durable-worker-runbook.md#productlisting-ingestion-rollout-and-acceptance-gate)
before enabling each producer. Review a stage-specific CDK change set/diff for
unintended legacy queue replacements and establish custody of both source/DLQ
pairs and authoritative PostgreSQL state before any cutover.

## Worker queue contract

`src/worker-queue-config.ts` owns the typed catalog and shared settings. All ten
queue pairs are declared in `prod`, `dev`, and `ephemeral`. The
`product-listing-opensearch`, `product-listing-normalization`, `product-content-assessment`,
`product-embedding`, `product-translation`, `search-filter-projection`,
`search-filter-percolator`, `search-filter-match-notification`, `watchlist-notification`, and
`notification-delivery` queues have retained Lambda mappings in every compute stack.
First compute creation requires manual Deploy `scope=all` after verified migration,
initial FX and the other operator prerequisites; these mappings are active when created. This catalog remains separate from Shopify resources and wiring.

Each enabled scope owns one **Standard source queue** and one **Standard DLQ**:

- Source: `aura-worker-<scope>-<stage>`.
- DLQ: `aura-worker-<scope>-dlq-<stage>`.
- Stage is exactly CDK's `prod`, `dev`, or `ephemeral`, not a stack-name prefix or
  the frontend's `stage` label. Names are validated against SQS's 80-character
  limit; they are never truncated. Runtime `STAGE` must match the queue suffix.
- Source retention: **7 days** (604800s). DLQ retention: **14 days** (1209600s).
- Source `maxReceiveCount`: **5**. Long polling: **20s** on both queues.
- SQS-managed encryption on both; no customer KMS key or extra KMS grants.
- Both resource policies deny all SQS access over non-TLS transport. No public
  Allow or cross-account access is granted.
- DLQ `redrivePermission=byQueue` allows only its named source ARN. Source queues
  use `denyAll` so they cannot become another queue's DLQ. This is not permission
  for a runtime to perform operator replay/redrive.
- Prod queues retain on **deletion and replacement**. Dev/ephemeral queues delete.
  Retained old queues need explicit operator inventory/recovery; renaming a queue
  does not migrate its messages or consumers.

| Runtime scope | Output stem after `Worker` | Initial source visibility |
| --- | --- | ---: |
| `product-listing-opensearch` | `ProductListingOpensearch` | 300s |
| `search-filter-projection` | `SearchFilterProjection` | 300s |
| `search-filter-percolator` | `SearchFilterPercolator` | 300s |
| `search-filter-match-notification` | `SearchFilterMatchNotification` | 300s |
| `watchlist-notification` | `WatchlistNotification` | 300s |
| `product-content-assessment` | `ProductContentAssessment` | 270s |
| `product-embedding` | `ProductEmbedding` | 360s |
| `product-translation` | `ProductTranslation` | 300s |
| `product-listing-normalization` | `ProductListingNormalization` | 270s |
| `notification-delivery` | `NotificationDelivery` | 330s |

`product-listing-opensearch`, `product-content-assessment`, `product-translation`, `search-filter-projection`, `search-filter-percolator`, `search-filter-match-notification`, and `watchlist-notification` are 512 MiB, 45s Lambdas with retained SQS mappings targeting published function versions, batch size one, and `ReportBatchItemFailures`. Content assessment uses **270s** source visibility (`6 × 45s`); the other listed mappings use **300s**, exceeding six times the Lambda timeout. `product-embedding-lambda` is 1024 MiB with a 60s cap and **360s** source visibility (`6 × 60s`) for one bounded image/Vertex/persistence attempt. The percolator classifies enhanced saved-search candidates with Cloudflare Workers AI Clef Flash by default, using the user's original description, localized listing title and description, and at most one image. `product-translation-lambda` receives only PostgreSQL, Vertex project/location/model, Google ADC, and its source queue; it refreshes ADC after warm idle, performs inference outside its short guarded write transaction, and retries missing source, provider, persistence, timeout, panic, malformed, and unknown-commit work through SQS/DLQ. Mappings are active at compute creation; an operator-approved pause must preserve each resource, function version, queue pair and IAM role. Neither Lambda changes visibility or runs a receipt daemon; only completed service results are omitted from failures. The notification generators are PostgreSQL-only: they do not receive OpenSearch, Vertex, S3 template, or SES configuration or permissions. The saved-filter projection rereads authoritative PostgreSQL state and turns a source-missing upsert into its versioned persistent deletion fence before acknowledging.

For native-to-Lambda handoff, retain the same source queue and schema-2 job contract;
do not create, rename, or purge a replacement queue. Deploy compatible Lambda code
only after pausing the corresponding native consumer and settling in-flight work:
manual Deploy `scope=all` creates compute with the Lambda mappings already active. Never run both
consumers. Returning control to native work needs an explicitly reviewed mapping
change before resuming a compatible native consumer. Backlog remains durable in the retained source queue and
uses its ordinary retry/DLQ rules. Standard SQS may duplicate/reorder messages; handlers
must remain idempotent.

### Identity and outputs

The four bare-metal runtimes' AWS role/trust and process deployment are **not
defined in this CDK app**. No IAM user, access key, or invented deploy binding is
created. Each Lambda has its own CDK execution role with only the capabilities required by
its scope; the notification generators receive PostgreSQL secret access and consume
only their own source queue, not provider-delivery permissions. No Lambda has queue
purge, DLQ-message, or redrive power. The external identity owner attaches only needed unbound policies
to native workers; do not reuse the CI deploy role or a Lambda role as a worker role.

| Unbound policy | Exact source actions | Paired DLQ actions |
| --- | --- | --- |
| Publisher | `sqs:SendMessage`, `sqs:GetQueueAttributes` | `sqs:GetQueueAttributes` only |
| Consumer | `sqs:ReceiveMessage`, `sqs:DeleteMessage`, `sqs:ChangeMessageVisibility`, `sqs:GetQueueAttributes` | `sqs:GetQueueAttributes` only |

DLQ attribute reads are required by the runtime's startup validation. No runtime
DLQ message access, `GetQueueUrl`, batch pseudo-actions, wildcard resource grants,
purge, queue deletion, or operator redrive actions are included. A process that
both accepts CDC and polls its scoped queue needs **both** policies for that
scope. Existing S3 template-read and SES-send permissions remain separate and
unchanged; attaching queue policies is additive, not a replacement.

The data stack (or single ephemeral stack) outputs:

- `WorkerQueueAwsRegion` — effective CloudFormation region; set `AWS_REGION`
  explicitly to this value. `AWS_DEFAULT_REGION` alone is not this runtime's
  configuration contract. Queue URL, ARN, and SDK region must agree.
- `WorkerQueueStage` — set `STAGE` to this exact value.
- Per table stem: `Worker<Stem>QueueUrl`, `Worker<Stem>QueueArn`,
  `Worker<Stem>DeadLetterQueueUrl`, `Worker<Stem>DeadLetterQueueArn`,
  `Worker<Stem>PublisherPolicyArn`, `Worker<Stem>ConsumerPolicyArn`.
- Managed policy names: `aura-worker-<scope>-publisher-<stage>` and
  `aura-worker-<scope>-consumer-<stage>`.

These outputs and unbound policies remain for an externally managed native
consumer during a controlled handoff; they are not native process deployment,
credentials, or environment injection by CDK. Use the source queue URL, never
the DLQ, and keep stage and AWS region consistent with the outputs. For LocalStack,
`singleStack=true` still produces `...-ephemeral` names.

### Operations and rollout boundary

Prod adds two alarms per scope on the existing `cloudwatch-alarms-prod` SNS topic:

- Source `ApproximateAgeOfOldestMessage` **>= 900s**.
- DLQ `ApproximateNumberOfMessagesVisible` **>= 1**.

Both use **Maximum**, one **5-minute** evaluation period, and missing data as
**not breaching**. Lower stages have no alarms. Existing topic subscriptions and
Lambda/API alarms remain unchanged. These are backlog signals, not proof of
consumer health; idle queues can have missing metrics.

Investigate DLQ failures, correct the cause, then replay under separately owned
operator authorization. Never give runtime roles purge/redrive powers. Standard
queue retention keeps the original enqueue timestamp when a message moves to the
DLQ, so operators should not assume a fresh 14-day recovery window on arrival.

This retains the ProductListing Lambda code target, execution role, scoped
PostgreSQL/OpenSearch environment, generic Lambda error alarm, partner EventBridge
rules and Shopify mapping. Manual Deploy `scope=all`, after separate migration,
OpenSearch setup and initial FX, creates active ProductListing consumers and the
FX schedule; initial FX is a direct, idempotent Lambda invocation by Initialize,
not a CloudFormation custom resource. It does not change
Sequin subscriptions, publish a new production CDC path, start DMS, grant runtime
redrive/purge power, or prove live AWS behavior. Legacy native worker deployment remains
external. Synthesis alone does not establish durable delivery or AWS acceptance evidence.

## Deployment inputs

### Workflow and source contract

| Workflow / trigger | Inputs / source | Operation |
| --- | --- | --- |
| `Deploy (CD)` — `.github/workflows/deploy.yml`, push to `develop` | Push commit → `dev`; `scope=auto` | Source-pinned build/publish/CDK; no migration or initial FX invocation. |
| `Deploy (CD)`, push release tag | Valid UTC CalVer `YYYYMMDD-HHMM` → `prod`; `scope=auto` | Same path; tag must resolve to a commit reachable from `origin/develop`. No `prod` branch. |
| `Deploy (CD)`, manual dispatch from `develop` | Required `stage=dev/prod`; `ref` and `scope` below | Same path, including rollback; not a previous-template artifact update. |
| `Migrate (CD)` — `.github/workflows/migrate.yml`, manual PostgreSQL migration | Required `stage=dev/prod` and `commit_sha` | Invoke only the already-deployed private PostgreSQL migrator. |
| `Initialize (CD)` — `.github/workflows/initialize.yml`, manual FX bootstrap | Same required inputs as Migrate | Invoke only the already-deployed FX Lambda; no schema or infrastructure changes. |

For manual Deploy, dev `ref` is optional (defaults to `develop`) and accepts only
`develop` or a full 40-character commit SHA. Prod `ref` is required and must be an
**existing** valid CalVer tag, not a branch or raw SHA. The timestamp is UTC with
an actual calendar date and valid hour/minute, not just a digit-shaped name
(for example, `20260230-1200` is invalid). Every selected source, on every trigger,
must be an ancestor of fetched `origin/develop`. Resolve once to the full SHA and
pin all application jobs to it, including artifact publishing and stage CDK.
The dispatch branch controls which
workflow runs; it does not replace validation of the selected source. Legacy
refs lacking the current CDK layout or workflow helper tools are unsupported.

| Deploy `scope` | Stage-stack behavior after artifact preparation |
| --- | --- |
| `auto` (default on all triggers) | After preflight, deploy network, data and initialize. Stop at **foundation only** if compute is absent or migration sources differ from its deployed SHA; otherwise deploy application stacks. |
| `foundation` (manual) | Explicitly deploy only network, data and initialize, including migrator/FX code. Never updates application stacks or invokes either operation. |
| `all` (manual) | Foundation then compute, API and prod observability; permits first compute creation and bypasses the migration-source comparison. Explicitly acknowledges schema compatibility and all applicable operator prerequisites. |

After stack preflight, `auto` compares the current compute stack's `CommitSHA`
with the selected SHA using `git diff` over `migrations`, `infra/sql` and
`src/database-migration-lambda`. The CDK job checks out full Git history; SHA lookup
or diff errors fail before any stage-stack changes. A difference stops after
foundation even with existing compute. The summary directs **forward releases**
to Migrate at that SHA, then Deploy the same ref with `scope=all`; new stages also
need OpenSearch setup and initial FX. This is a **source-change safety gate**, not
database introspection or a readiness marker. Rerunning `auto` after migration
still stops until `all` advances compute's SHA; compatibility remains operator-owned.

CDK preserves previous activation parameters on updates, including
`CdcRouterEnabled` and `PeriodicMatcherEnabled`; both default to `false` on first
creation. DMS start-position compatibility parameters likewise retain prior
approved values. Scope is not an activation override. Compute creation activates
the ten worker mappings, partner integrations and FX/cleanup schedules together;
there is no separate release flag for them and **no built-in readiness marker**
proving migrations, FX, OpenSearch or handoff are complete. Neither stack existence
nor a successful foundation summary proves application readiness.

The per-stage compute stack also exposes `SearchFilterClassifierModel` and
`SearchFilterMatchShouldShowThresholdBps`. Their defaults come from the stage
configuration (`clef-flash` and `5000`, respectively); the model accepts `clef`
as an alternative and the threshold is an inclusive basis-point value from 0 to
10000. Both the percolator Lambda and periodic matcher use these same stack
parameters, so a stage can tune model selection and acceptance without editing
either runtime construct.

### First-time stage

Confirm [container repository setup](#container-repository-setup) before running
Deploy; even foundation-only releases prepare the selected artifact catalog.

1. Run Deploy `auto` (automatic or manual) or manual `foundation` for the selected
   ref. Record the resolved SHA and verify completed network/data/initialize stacks.
2. Run **Migrate Postgres** (`Migrate (CD)`) with that stage and exact deployed
   initialization `CommitSHA`; require confirmed success.
3. Have the OpenSearch host owner perform [first setup](../docs/opensearch-stage.md#reproducible-host-setup)
   if needed, or verify existing mappings, pipeline, scoped roles, TLS and network
   prerequisites under its [external manual contract](../docs/opensearch-stage.md#release-and-schema-change-contract).
4. Run **Initialize FX** (`Initialize (CD)`) with the same stage and SHA; verify
   successful persisted initial capture. It does not deploy application stacks.
5. Before manual Deploy **the same ref**, `scope=all`, verify matching migrations,
   initial FX, OpenSearch readiness, native-consumer handoff (old consumers stopped
   and in-flight/backlog work settled or preserved), and explicit SES/provider
   consent, recipients, quotas and credentials. Verify certificate/DNS and other
   required stage configuration. Hold `all` if any prerequisite is unverified.
   It creates active compute, not an inactive preview. Keep DMS/router activation
   separately approved; never run two consumers of one queue.

Use the pinned full SHA for dev continuation rather than a now-moving `develop`;
prod continues with the same immutable tag. After deployment record actual
function versions/aliases, mapping/rule states, `/api/v1/ready`/reader and relevant
projector/percolator smoke from private workloads, certificate/DNS state, queues
and DMS checkpoint/WAL/retention. Required pre-activation connectivity checks need
an approved private probe path; if unavailable, hold `all`, not a speculative
compute deployment. Host-local checks and the waived standalone NAT test are not
AWS workload evidence. Record failed/waived checks explicitly and hold dependent
traffic/cutover. See the [OpenSearch gate](../docs/opensearch-stage.md#dev-private-subnet-egress-and-release-gate-1850)
and [worker handoff](../docs/durable-worker-runbook.md#activation-and-legacy-handoff).

### Routine and schema-dependent releases

Routine `develop` pushes trigger dev Deploy; release maintainers promote a
reviewed merged source by pushing a valid UTC CalVer tag for prod. Both prepare
artifacts before the final CDK job's stage-environment policy, including any
configured required reviewers. Deploy **never invokes migration** or initial FX.
The source-change guard does not prove schema
compatibility: releases still require expand-and-contract compatibility with
running code and retained jobs. Foundation leaves existing consumers and schedules
running, including the FX function's stable unqualified ARN; updated FX code must
tolerate the pre-migration schema.

For a forward release with migration-source changes:

1. Merge with required CI into `develop` (dev), or push a release tag of the merged
   SHA (prod). Automatic Deploy builds/reuses artifacts, deploys foundation and
   stops with an **application not deployed** summary. Record the resolved SHA.
2. Run Migrate with the matching deployed initialization `CommitSHA`; require
   confirmed success. Complete any external OpenSearch changes with its host owner.
3. After compatibility/prerequisite review, manually Deploy **the same ref with
   `scope=all`**, even for existing compute. Use the pinned full SHA for dev or the
   same immutable prod tag. First-time stages also follow the FX/handoff gates above.

No temporary environment gate or cancellation is needed for this sequence; normal
stage approval policy still applies. Manual `foundation` remains available for
explicit preparation. Changes outside the compared paths (including OpenSearch-only
changes) are not covered by this guard. If another release requires manual gating,
**cancel blocked automatic runs before dispatching manual operations** and confirm
the stage concurrency slot is released: Deploy, Migrate and Initialize share it.
Do not leave a gated automatic run holding the slot while waiting for a manual run.

Do not run down migrations on rollback. Contract/removal changes wait until old
binaries, in-flight work, retained queues/replay and the rollback window no longer
need the old schema. Crawler startup migrations are [separately owned](../docs/crawler/operations.md#runtime-prerequisites),
not AWS business migrations.

### Manual operations

Migrate and Initialize accept a required full 40-character lowercase `commit_sha`:
this is an assertion of the **already-deployed**
`application-<stage>-initialize` stack's `CommitSHA`, not source selection or a
request to deploy it. Their shared helper checks a complete stack
(`CREATE_COMPLETE`, `UPDATE_COMPLETE` or `UPDATE_ROLLBACK_COMPLETE`) and exact SHA
match, waits for the function update, then invokes synchronously. It requires
invocation status 200 without `FunctionError` and strictly validates the result:
exactly `{"status":"ready"}` for migration, exactly JSON `null` for FX. CLI success
alone is not operation success. A missing/mismatched stack, malformed result or
Lambda failure fails closed; inspect restricted logs without publishing payloads
or secrets. An invocation timeout can leave completion unknown; inspect before
retrying. Migrate and Initialize keep `aws-<stage>` environment protection before
assuming credentials and invoking functions. Deploy and these operations share
stage serialization, not a distributed transaction or a lock across the entire
multi-run release procedure.

Initialize always uses `deployment:fxrate:initial:{stage}:v1`, including retries
at a later SHA. Retries may call the FX provider again but deduplicate the persisted
`fx_rates.source_event_id` row; this is not an exactly-once provider call. It is
first-time FX bootstrap, not recurring refresh. Neither manual workflow builds,
publishes, deploys CDK, imports resources, manages OpenSearch, or activates compute.

### Rollback and stack recovery

For prod rollback, **after compatibility review**, dispatch Deploy from `develop`
with `stage=prod`, `ref` set to an older existing supported CalVer tag, and prefer
`scope=all` to acknowledge readiness. Dev can select an older full merged SHA.
`auto` may stop at foundation when migration sources differ; the summary's Migrate
step is for forward releases only. **Never invoke an older migrator as rollback.**
The same source-pinned
build/reuse/publish/CDK path runs, so **rollback may change stage infrastructure**,
not just artifacts; [repository setup and ownership](#container-repository-setup)
remain outside the release/rollback path.
Review the selected source's CDK diff, replacements, API policy,
Lambda versions/aliases, current database compatibility, retained queues/jobs,
provider/template contracts and already committed effects before approval. Hold
for separate operator-controlled change-set inspection when needed; environment
approval is not inspection of an actual prepared change set. CDK execution is not
an atomic cross-stack rollback. It does not undo database writes, send cancellations,
email or other provider effects, and it never runs down migrations.

The previous-template artifact-only rollback path was removed to avoid mixing old
artifacts with unrelated newer infrastructure. Legacy tags without the required
layout/tools are unsupported; use a reviewed forward fix rather than retagging,
substituting `latest`, or bypassing validation. SQLx rejects missing/changed applied
migrations; the newer schema must remain compatible with the selected application.

In-progress, `UPDATE_ROLLBACK_FAILED`, failed `ROLLBACK_COMPLETE`, post-import
`IMPORT_COMPLETE`, or empty `REVIEW_IN_PROGRESS` shells require operator-owned
CloudFormation inspection and recovery; an archive-only import or empty shell is
not initialized compute. A usable
`UPDATE_ROLLBACK_COMPLETE` stack may be retried after inspection. Failed creation
cannot simply be updated. Inventory retained resources before any approved stack
cleanup; no workflow deletes or automatically imports them. A failed first
compute creation can retain both the CDC failure archive and named periodic-matcher
log groups; clearing only the bucket will not make a repeat creation safe. Follow
the [archive resource-import recovery](../docs/durable-worker-runbook.md#retained-cdc-archive-resource-import-recovery)
and separately reconcile other retained resources when applicable. After partial
application creation, inspect what is already active before resuming Deploy;
Initialize is not a stack-repair workflow. If only
post-deploy output resolution failed, inspect parameters and rerun the missing
verification where possible rather than blindly redeploying. Never purge queues
or replace RDS to get a release through. No workflow manages external OpenSearch
host/index/security or starts/resets DMS; first start and recovery remain separately
approved operations using the actual slot/LSN and later `resume-processing`.

### GitHub / IAM repository-admin checklist (#1549)

These are manual repository/identity administration tasks, **not actions performed
by the workflows or this documentation change**. Record completion and evidence
in issue #1549 before relying on the release model:

- [ ] Make `develop` the default and protected source branch; require the relevant
  successful `Integrate (CI)` checks before merging. Enable **squash merge only**;
  disable merge commits and rebase merging so release SHAs identify merged source.
- [ ] Migrate applicable protections from `prod` to `develop`, review references
  and confirm `prod` is unused, then manually delete the old `prod` branch. No
  workflow or documentation edit deletes it; production is released by tags.
- [ ] Create an active tag ruleset matching release tags with the GitHub **digit glob**
  `[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9]`.
  Enforce update and deletion restrictions with **no bypass**, including admins
  and automation. Restrict creation separately to release maintainers (a creation
  exception must not bypass immutability). The glob checks shape only; Deploy
  additionally validates the UTC calendar timestamp and `origin/develop` ancestry.
- [ ] Restrict `aws-dev` deployment sources to `develop`. Required reviewers are
  optional under the owner's dev policy; keep configured reviewers when required.
  In Deploy they gate only the final CDK job, not artifact preparation. Ordinary
  schema releases still use the source-change guard.
- [ ] Configure `aws-prod` with required reviewers and deployment rules allowing
  the release-tag glob **and `develop` for workflow dispatch**, not arbitrary
  branches. The dispatch input `ref` still must pass prod tag validation; an allowed
  workflow branch is not permission to deploy that branch as prod source.
- [ ] Keep `aws-<stage>` on Deploy's final `aws-cdk-deploy` job and on the separate
  manual Migrate/Initialize operations, not on image/ZIP/mail publishers. Only that
  final Deploy job changes stage stacks, after artifact preparation and environment
  approval where configured.
- [ ] Keep the existing `CI_DEPLOY_ROLE_ARN` secret and `AWS_REGION`,
  `S3_BINARY_ARTIFACTS_BUCKET_NAME` and `S3_MAIL_TEMPLATE_BUCKET_NAME` variables
  available at repository or organization scope for publishers without environments.
  No new mandatory role or secret is needed. Confirm account, region and bucket
  targets when reviewing configuration.
- [ ] Review OIDC trust for audience `sts.amazonaws.com` and these explicit subjects:
  - Publishers on develop pushes and manual dispatch from develop:
    `repo:aura-historia/backend:ref:refs/heads/develop` (exact match).
  - Publishers on release-tag pushes:
    `repo:aura-historia/backend:ref:refs/tags/????????-????` (`StringLike` pattern).
    IAM `?` matches any single character, not just digits; the protected GitHub tag
    ruleset and workflow UTC CalVer/ancestry checks enforce valid release sources.
    Do not copy GitHub's `[0-9]` glob into IAM or allow arbitrary `refs/tags/*`.
  - Final Deploy, Migrate and Initialize retain the exact environment subjects
    `repo:aura-historia/backend:environment:aws-dev` /
    `repo:aura-historia/backend:environment:aws-prod`. Environment jobs do not use
    branch/tag subjects; their environment restrictions enforce the triggering-ref
    policy. Checking out a selected SHA does not change a job's OIDC subject.
  Do not authorize arbitrary repositories/environments or use stored AWS keys.
- [ ] Review publisher IAM for ECR validation/read/pull/push: `ecr:DescribeRepositories`,
  `ecr:DescribeImages`, `ecr:BatchGetImage`, `ecr:GetDownloadUrlForLayer`,
  `ecr:BatchCheckLayerAvailability`, `ecr:InitiateLayerUpload`, `ecr:UploadLayerPart`,
  `ecr:CompleteLayerUpload` and `ecr:PutImage` on the catalog repositories, plus
  `ecr:GetAuthorizationToken` for login. S3 needs `s3:GetObject` for HEAD,
  `s3:ListBucket` to distinguish missing keys, and `s3:PutObject` / `s3:PutObjectTagging`
  for SHA-scoped conditional creation in the existing artifact/template buckets.
  Publication requires no repository/bucket creation, import, deletion or
  configuration administration; missing/invalid repositories require separate setup.

The existing `CI_DEPLOY_ROLE_ARN` is a shared CI role, **not an artifact-only role**:
its deployment/operation use also needs CloudFormation/CDK, staging-bucket/KMS
(where applicable), stack/function inspection and exact migrator/FX invocation
permissions. Environment approval is a **workflow checkpoint, not an isolated IAM
boundary** while publishers assume the same role. Separating least-privilege
publisher and deployment roles is an optional, separately reviewed out-of-band
hardening change, not a prerequisite introduced here. Deploy also requires
`iam:GetRole`, `iam:ListAttachedRolePolicies`, `iam:CreateRole` and
`iam:AttachRolePolicy` for the account-level `dms-vpc-role`, attaching only
`arn:aws:iam::aws:policy/service-role/AmazonDMSVPCManagementRole`. Unexpected role
trust or permission failure blocks deployment; repair trust out of band, never
let CI overwrite it. Review account, region and actual change sets/replacements
under the stage policy. No live GitHub rules, OIDC trust or IAM permissions are
verified by source review, tests or synthesis.

The Lambda artifact and mail-template buckets are fixed in `src/config.ts`:

- `aura-historia-binary-artifacts-eu-central-1`
- `aura-historia-mail-templates-eu-central-1`

LocalStack acceptance tests synthesize one ephemeral stack with CDK context
`singleStack=true` and pass the host-mapped edge port as `localStackMappedPort`.
These values are synth-time context, not CloudFormation parameters.

## Stage-specific SSM parameters

Real AWS stages resolve external integration settings via supported CloudFormation
SSM dynamic references. `google-application-credentials` is materialized only by
the API Lambda. Required paths are stage-specific for `prod` and `dev`:

```text
/opensearch/{stage}/endpoint-url
/opensearch/{stage}/reader/{username,password}
/opensearch/{stage}/product-projector/{username,password}
/opensearch/{stage}/filter-projector/{username,password}
/opensearch/{stage}/percolator/{username,password}
/eventbridge/{stage}/stripe-event-bus-name
/eventbridge/{stage}/shopify-event-bus-name
/stripe/{stage}/pro-product-id
/stripe/{stage}/ultimate-product-id
/stripe/{stage}/pro-monthly-price-id
/stripe/{stage}/pro-yearly-price-id
/stripe/{stage}/ultimate-monthly-price-id
/stripe/{stage}/ultimate-yearly-price-id
/certificates/{stage}/api-regional-certificate-arn
/certificates/{stage}/api-cloudfront-certificate-arn
/cognito/{stage}/identity-providers/google/{client-id,client-secret}
/vertex-ai/{stage}/project-id
/vertex-ai/{stage}/location
/vertex-ai/{stage}/model
/secrets/{stage}/google-application-credentials
/cloudflare/{stage}/account-id
/secrets/{stage}/cloudflare-workers-ai-api-token
/loops/{stage}/api-key
/loops/{stage}/newsletter-list-id
```

The Google Cognito identity provider resolves both its client ID and client secret from SSM `String` parameters. CloudFormation does not support `ssm-secure` in Cognito `ProviderDetails.client_secret`, so the client secret must be a plain `String`, not `SecureString`. Restrict SSM reads and CloudFormation/Cognito configuration access; never put the value in source, logs, or CLI arguments. Changing the SSM value alone does not update the deployed provider: deploy an identity-provider configuration change to re-resolve it before revoking an old Google client secret.

The API Lambda, `product-embedding-lambda`, and `product-translation-lambda` resolve their scoped Vertex and Google ADC settings through CloudFormation dynamic references. Each writes the JSON to its private `/tmp` ADC file during startup; the raw JSON is neither packaged nor logged. Neither needs runtime SSM permission. The embedding Lambda receives only Vertex project/location and ADC, not a Vertex model, OpenSearch, SES, notification-delivery, or template configuration. The translation Lambda receives only Vertex project/location/model and ADC, PostgreSQL, and its source queue. The percolator receives Cloudflare account ID, model (`clef-flash` by default or `clef`), the service acceptance threshold, and its OpenSearch endpoint, username, and password. Its environment contains the API token's `SecureString` parameter name; at startup it reads the decrypted value with `ssm:GetParameter` under permission scoped to that parameter. The token value is never placed in the Lambda environment or logs. Rotating the token does not replace the copy held by an already warm Lambda process; recycle the percolator Lambda after rotation. Use the AWS-managed SSM key for the token parameter, or grant the exact KMS decrypt permission to both runtime roles if a customer-managed key is selected. The periodic matcher receives the same Cloudflare model/account configuration and injects the token as an ECS task secret. `product-listing-opensearch-lambda` receives none of the Vertex, Cloudflare, or Google ADC configuration and has no Google or SSM permission. It resolves the listed OpenSearch endpoint, username, and password in real stages.
The initialization-stack `fxrate-lambda-<stage>` resolves `/fxratesapi/<stage>/api-token`.
Compute's recurring Scheduler target and invoke permission use its stable unqualified
function ARN, with no release-dependent FX version export/import. Initial FX is
synchronous in manual Initialize after a separate successful Migrate; subsequent
Deploys update FX code without running migrations while the recurring schedule
may still run. FX changes must tolerate the currently deployed schema. An
unqualified target follows deployed function code rather than pinning a separate
scheduler version. If live stacks
still import the prior FX version export or own an old compute FX function, plan
the resource-specific import/ownership transition separately before updating;
CloudFormation cannot remove an in-use export.
Environment-gated manual `Initialize (CD)` invokes only FX after database migration
and before a separate manual Deploy `scope=all` creates active compute, with stable source ID
`deployment:fxrate:initial:{stage}:v1`. This is not a CloudFormation custom resource;
later normal deployments do not recapture it. `ephemeral` has no real FX initialization
flow. The ephemeral stage uses local/mock values for third-party integrations where
possible.
