# Aura Historia infrastructure

This directory contains the AWS CDK app for the Aura Historia backend.

The application is split into small CDK/CloudFormation stacks with typed
configuration objects instead of hand-written templates. The same stack code is
synthesized for:

- `prod` — real AWS production resources, production alarms enabled
- `dev` — real AWS development resources, production alarms disabled
- `ephemeral` — LocalStack resources, including a local OpenSearch domain

## Migration baseline

The [architecture contract](../docs/arch.md#12-cdc-and-projection-architecture) records the target worker ownership and cutover boundaries. [Migration F7](../docs/migration-f7-dms.md) owns the #1781 DMS-to-Kinesis CDC contract. Neither document, this README, nor CDK synthesis proves deployed state.

## Structure

```text
bin/app.ts                 # CDK entrypoint and stage selection
src/application-stack.ts   # data, compute, API, observability stack composition
src/config.ts              # stage configuration, fixed buckets, RDS shape, SSM dynamic refs
src/worker-queue-config.ts # typed worker queue scopes, timing, retention, alarms
src/parameters.ts          # deployment artifact version input
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

It composes in the data stack after network and storage. It does not exist for `ephemeral`, full-load tables, an outbox, a custom CDC target, or a Sequin redesign. The detailed start, slot, mapping, LOB, test, and cost contract is in [Migration F7](../docs/migration-f7-dms.md).

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

Synth creates these stacks per stage:

- `application-{stage}-network` — real-stage two-AZ VPC, one NAT/EIP, S3 gateway endpoint, and database/workload security groups
- `application-{stage}-data` — private RDS PostgreSQL and DMS/Kinesis CDC in real stages, Shopify/worker SQS, unbound worker IAM policies, and LocalStack OpenSearch
- `application-{stage}-compute` — Lambdas, Cognito, eventing, schedules, and (real stages only) the periodic matcher task
- `application-{stage}-api` — HTTP API Gateway routes, domain, CloudFront, integrations, authorizer
- `application-prod-observability` — prod-only alarms and alarm topic
- `aura-historia-container-artifacts` — stage-neutral retained ECR owner for the image catalog, synthesized separately by `bin/artifacts.ts`

The network stack is absent for `ephemeral`: LocalStack synthesis does not declare a VPC, NAT, EIP, gateway endpoint, or workload security groups. Real-stage stacks use `eu-central-1`; synth may omit an account only for template validation. A deployment must select the approved account explicitly:

```bash
npm run cdk -- deploy application-prod-network -c stage=prod -c account=123456789012 -c region=eu-central-1
```

The app rejects a non-12-digit account context and an explicitly selected real-stage region other than `eu-central-1`. Template-only synth without an account ignores an ambient CI runner region and uses `eu-central-1`; a deployment must select the approved account explicitly. Deploy network, then data, then compute; data imports the VPC for RDS and compute imports the RDS endpoint plus runtime credential dynamic references. The `cloudwatch-log-retention-lambda` remains outside the VPC. This F3/F4 declaration started from `develop` SHA `dc1ae85af84ee53cf1e8c678e7453017da1ccd56`; it is not live-provisioning evidence.

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

Forward deployment uses CDK CloudFormation change sets (`--method change-set`),
never hotswap. Manual artifact rollback uses the existing CloudFormation templates,
not historical CDK source or infrastructure templates.

Deployments do not require a full CDK bootstrap stack in the target account/region.
Each stack uses `CliCredentialsStackSynthesizer` with the existing staging bucket
`aura-historia-cfn-artifcats-eu-central-1`. CDK uploads large CloudFormation
templates and any future file assets under the stage prefix (`${stage}/`). Lambda ZIPs are referenced as prebuilt S3 artifacts keyed by `CommitSHA`, not as
CDK-managed assets. The periodic matcher is a separate immutable ECR artifact,
referenced by digest in each real-stage compute stack; it is not a Lambda ZIP or a
CDK Docker asset. The stage-neutral ECR owner is independently synthesizable without
compute parameters, Docker, AWS lookups, or secret values.

A push builds and uploads 20 Lambda ZIPs as `<binary>-<stage>-<commit-sha>.zip`
and the tracked MJML templates as `<stage>/<commit-sha>/mjml/<template-path>.html`
(for example, `dev/<sha>/mjml/watchlist/product-update/price/en.html`).
`CommitSHA` is the full Git commit SHA, not the PR head SHA. The compiler is installed
with `npm --prefix mjml ci` from the checked-in lockfile. These artifacts remain
stage-local; manual Deploy selects existing keys and never builds or copies across
stages. Ordinary S3 ZIP/template uploads can overwrite an existing SHA key and do
not guarantee byte immutability.

ECS images are described by the non-secret `ci/container-images.json` catalog. CDK
validation exposes that catalog as the publishing matrix. Every Deploy (CD) run
reconciles the independent retained ECR artifact stack after infrastructure tests,
before publishing images (on push) or deploying the application. Manual
`workflow_dispatch` updates and rollbacks skip image publication: they resolve
already-published immutable SHA tags in the reconciled repositories and preflight
stage mail artifacts. Stage-specific Lambda ZIPs must also have been uploaded for
the selected SHA; the manual path does not build or upload them. A failed artifact
reconciliation or missing registry image blocks deployment before compute changes.
Each push publishes every catalog entry through the same generic job; immutable
image publication itself does not select a protected deployment environment. For a
new SHA tag, that job builds one local image, runs the image-owned `ci/container-images/<id>/smoke.sh`, and only then
adds and pushes the immutable `git-<full-sha>` release tag. Existing tags are reused
without rebuilding. The registry digest is resolved from ECR; it is not inferred
from a local image ID. Deploy, Initialize, and manual artifact rollback use the
shared resolver to query every required repository/SHA tag and pass a complete
parameter-to-digest map. Matrix jobs do not publish a colliding shared digest job
output. The container PR workflow builds and tests images without AWS credentials.
All required artifacts must resolve before migration or compute/API admission.

For a compatible artifact rollback, manually dispatch Deploy with `stage` and an
older, already uploaded `commit_sha`. The current workflow definition runs the
operation; the supplied SHA selects artifacts, not historical CDK code or workflow
logic. It requires complete application stacks and uses their *previous
CloudFormation templates*, updating initialization `CommitSHA` and compute
`CommitSHA` plus every digest parameter in the selected image catalog, while
preserving every other compute parameter, including `PeriodicMatcherEnabled` and
`CdcRouterEnabled`. The previous compute template must already support every catalog
digest parameter and the stable task-definition output; otherwise Deploy stops with an
infrastructure-upgrade instruction before changing either stack.
The no-op check compares the source SHA and the complete digest map. Rollback does not
execute the older embedded migrator or initial FX, alter schema history, or roll back
network, data, domain or API policy. SQLx 0.9.0 rejects migrations absent from or
changed in the embedded migration set; normal forward migrations retain this
validation. Choose artifacts compatible with the current templates, newer database
schema, retained schema-2 jobs and provider/template contracts; arbitrary historical
infrastructure rollback or old releases missing required binaries are not supported.
Adding a catalog image changes the compute release contract. Historical commits
without a compatible retained image for every current catalog entry are not
automatically rollback-compatible. Use a separately reviewed infrastructure/template
compatibility path; never substitute `latest` or rebuild a missing historical artifact.
This two-stack update is not atomic; already committed writes, email and provider
effects are not undone.

## Periodic saved-filter matcher (#1843)

The real-stage compute stack declares a standalone `aura-historia-periodic-matcher-{dev,prod}` Fargate task and a disabled-by-default EventBridge Scheduler target. The image catalog maps the `periodic-matcher` source crate/binary to the stable stage-neutral ECR repository `aura-historia-periodic-matcher`; the shared artifact stack owns that retained, immutable repository. Images use `git-<full-source-sha>` tags, while compute task definitions use `repositoryUri@sha256:<registry-digest>`, never a mutable tag. Dev-to-prod promotion reuses the same retained digest. The independently synthesized artifact stack declares all catalog repositories but does not build or select images. The undeployed artifact stack uses the generic CloudFormation name `aura-historia-container-artifacts`. The matcher repository's physical name, construct identity, retention, and export name remain unchanged.

The compute stack owns one named scheduled ECS cluster per real stage and injects it into each `ScheduledEcsJob`. The cluster's logical ID `PeriodicMatcherCluster207C1F86` is frozen to preserve the CloudFormation resource during the move from matcher-owned to shared cluster; removing the override requires a CloudFormation migration. This is a migration-specific exception, not a pattern for future scheduled ECS jobs. `infra/src/constructs/scheduled-ecs-job.ts` owns each job's private Fargate task, Scheduler, IAM, DLQ, lifecycle-log and output resources. Jobs select CPU/memory and the catalog image platform (amd64 or arm64), and may add task-role grants; the matcher retains its 1024 CPU / 2048 MiB amd64 task and empty task role. The `periodic-matcher.ts` wrapper supplies the matcher image, credentials, environment, schedule and stable resource names; future jobs should use their own wrappers, not extend `aura-historia-cron`.

The matcher schedule is `cron(0 15 * * ? *)`, timezone `UTC`, flexible window `OFF`, task count one, Fargate platform `1.4.0`. The `PeriodicMatcherEnabled` compute parameter defaults to `false` on first creation. New-template push deploys and Initialize may introduce newly added digest parameters on an older valid compute stack; existing activation values are preserved, while a newly introduced matcher enablement parameter uses its `false` default. Manual artifact rollback changes `CommitSHA` and every catalog digest together while retaining all other parameters and using the deployed CloudFormation template. A pre-feature compute template that lacks a required digest parameter cannot use artifact-only rollback; it must first receive an infrastructure-bearing release. Push publication resolves/reuses an immutable artifact or builds and smoke-tests the selected source once before publishing its final tag. Initialize and manual Deploy only resolve existing registry artifacts and never rebuild, pull, or repeat application-specific image smoke tests. The shared resolver fails on missing or malformed artifacts before migration/admission; it never turns manual or initialization paths into a build.

The image is a normal Linux/amd64 executable, not a Lambda runtime. ECS starts `search-filter-periodic-match` with no arguments; it matches once and exits. Do not set an ECS command override or use an automatic restart loop. The task starts at CPU 1024 / 2048 MiB, uses the existing private application subnets and exactly the existing `ApplicationSecurityGroup`, has no public IP, and has a separate empty task role. Its execution role retrieves only the staged runtime PostgreSQL secret and OpenSearch/Google parameters, pulls this ECR repository, and writes the named application log group. The application receives PostgreSQL JSON `username`/`password` from `/aura-historia/<stage>/postgres/runtime`; OpenSearch reader credentials and ADC are injected through ECS secret references, not plaintext CloudFormation environment values. It uses the committed public RDS CA at `/opt/aura-historia/rds-ca/global-bundle.pem`, read-only root filesystem, non-root UID/GID `10001`, and a task-local writable `/tmp` volume for private ADC materialization.

The matcher preserves the existing bounded service policy and per-filter durable progress. A separate PostgreSQL advisory-lock connection excludes overlapping runs; task cancellation/timeout fails the process and does not reset committed matches or progress. PostgreSQL remains authoritative. Only committed `search_filter_matches` facts enter the existing CDC path; the matcher has no SQS, notification, SES, or schema-migration permission. The daily run is at-least-once scheduling with idempotent business writes, not an exactly-once trigger or exhaustive historical backfill.

There are two independent failure channels:

- Scheduler target-delivery failures go to `aura-historia-periodic-matcher-delivery-<stage>`, a dedicated encrypted Standard SQS DLQ with 14-day retention. Production alarms on visible messages through the existing `cloudwatch-alarms-prod` topic. The queue has no automatic consumer or replay.
- ECS STOPPED events for the task-family prefix are written to `/aura-historia/<stage>/periodic-matcher-lifecycle`; production sends whitelisted nonzero-container and startup/interruption notifications through the existing alarm topic. Each EventBridge Logs target emits a JSON envelope with exactly a `timestamp` and a string `message`, for example `classification=interruption taskArn=... taskDefinitionArn=... clusterArn=... status=STOPPED stopCode=TaskFailedToStart`. The stopped and nonzero-exit rules omit `stopCode`; only the interruption rule includes it. The application log group is `/aura-historia/<stage>/periodic-matcher`. Retention is 30 days in dev and 90 days in prod. For lifecycle evidence use Logs Insights `fields @timestamp, @message | sort @timestamp desc`; filter with `@message like /classification=interruption/` or `@message like /classification=application-container-nonzero/`. Application logs are JSON; query `job = "search-filter-periodic-match"`, `stage`, `revision`, `outcome`, `duration_ms`, `window_end`, `filters_selected`, `filters_completed`, and `filters_failed`, and inspect the `message` field values `cron.matching.started`, `cron.matching.report`, `cron.matching.finished`, and `cron.matching.terminal`. A report with failed filters is incomplete and exits nonzero. `SkippedAlreadyRunning` exits successfully as an explicit exclusion, not a matched run.

Neither a clear Scheduler DLQ nor these logs prove that the expected daily task completed; there is no missing-heartbeat detector or durable completion journal. Investigate a missing expected occurrence and inspect the named `periodic-matcher` container's exit/startup reason and task-definition revision. The compute stack exposes the stable `PeriodicMatcherTaskDefinitionArn` output consumed by Deploy, Initialize, and rollback summaries, alongside the matcher cluster/task/schedule/log details. Do not depend on generated nested-construct output keys for operator automation.

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
binary with:

```bash
cargo lambda build --locked --release \
  --target x86_64-unknown-linux-musl \
  --output-format zip
```

`cargo-lambda` produces `target/lambda/<binary>/bootstrap.zip`; CI copies it to
`<binary>-<stage>-<commit-sha>.zip`, which exactly matches
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
an OpenSearch reachability check remains confined to `/ready`. It logs only its
component, Lambda request ID, remaining invocation budget, and cold-start duration;
it never logs event bodies, credentials, or provider errors.

The API Lambda receives `STAGE`, `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH`,
PostgreSQL connection metadata plus `POSTGRES_SECRET_ARN`, OpenSearch endpoint/credentials,
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

The identity CloudFormation uses to resolve dynamic references (the deployment principal or configured execution role) needs `ssm:GetParameters` for both Loops parameters. The Lambda execution role does not need runtime SSM access for Loops. The API key is readable as plain text by principals with SSM parameter-read access and is resolved into the API Lambda environment, so principals able to read its Lambda configuration can also read it; tightly restrict parameter, function-configuration, and deployment access. This deploy-time reference is not runtime secret isolation.

Changing the SSM parameter value alone does not refresh a running Lambda environment. For key rotation, update `/loops/<stage>/api-key` and deploy an API Lambda version/configuration update that causes CloudFormation to re-resolve the dynamic reference; verify the new version with a controlled newsletter write, then revoke the old Loops key. For an immediate cut-off, an authorized Loops operator can disable or revoke the key; newsletter writes then fail until a replacement is deployed. This does not remove memberships or change opt-outs. Keep any future production marketing workspace/audience isolated from development contacts and workspace-level quota/state. There is no CDK-managed key rotation, provider fallback, or delivery control; campaigns and all remote-account actions require explicit operator authorization.

## Network foundation (F3)

Real stages have separate `/16` address space: `prod` uses `10.64.0.0/16` and `dev` uses `10.65.0.0/16`. Each has two public, two private-application, and two isolated private-database `/24` subnets across two availability zones. Exactly one managed NAT Gateway is placed in the first public subnet with one explicitly declared EIP. Both application subnet default routes use it; database route tables have no internet default route.

The application route tables use one S3 **gateway** endpoint. Its endpoint policy permits only `GetObject` and `ListBucket` on the existing artifact, mail-template, and CloudFormation-staging buckets. It does not grant Lambda IAM permissions. The data stack's DMS CDC construct declares one shared Secrets Manager **interface** endpoint: private DNS, `open: false`, HTTPS ingress only from `ApplicationSecurityGroup`, `DmsSecurityGroup`, and `MigrationSecurityGroup`, and an endpoint policy allowing `GetSecretValue` only for the exact admin, runtime, migrator, and replication PostgreSQL secrets plus `DescribeSecret` only for replication. F7 separately declares the Kinesis endpoint; no public database, crawler network, or crawler database access is declared here.

`ApplicationSecurityGroup`, `DatabaseSecurityGroup`, `DmsSecurityGroup`, `DmsEndpointSecurityGroup`, and `MigrationSecurityGroup` are exported for RDS/DMS/migration ownership. PostgreSQL ingress is TCP 5432 only from the application, DMS, and migration groups. The database group has no usable outbound rule. The CDK declaration permits application workloads to egress TCP 5432 to the database group, TCP 443 to `DmsEndpointSecurityGroup` for private runtime-secret retrieval, and TCP 443 to IPv4 destinations via NAT. Only in `dev`, `ApplicationSecurityGroup` additionally permits TCP 9443 to `148.251.91.20/32` (the recorded stage OpenSearch host A address) through the same NAT; prod and the other groups have no such rule. This destination address is recorded in `infra/src/config.ts`, not dynamically resolved by security groups. Do not broaden it to `0.0.0.0/0` or assume synthesis proves deployed connectivity. The private migration Lambda may egress only TCP 5432 to the database and TCP 443 to that endpoint. Security groups cannot express hostname allowlists: external HTTPS provider/AWS API host review stays with the workload and IAM/identity configuration; use the DNS-name HTTPS URL and verify the trusted TLS chain **and hostname** in the application, not a raw-IP URL or disabled verification.

The F7 DMS endpoint/security-group contract replaces the earlier open-ended endpoint wording: DMS egress is TCP 5432 to `DatabaseSecurityGroup` and TCP 443 to the interface-endpoint security group only. That endpoint group allows TCP 443 only from `DmsSecurityGroup`, `ApplicationSecurityGroup`, and `MigrationSecurityGroup`; its Secrets Manager endpoint is shared only for the exact DMS replication, application runtime, and migration role secrets. The endpoints have private DNS for `kinesis.eu-central-1.amazonaws.com` and `secretsmanager.eu-central-1.amazonaws.com`. DMS has no broad egress and does not use NAT.

No network stack or network export existed at the F1 baseline, so this change has no obsolete export to remove and makes no stateful-resource replacement. `data` imports the VPC for RDS and `compute` imports the VPC values required by PostgreSQL Lambdas. `NatGatewayEipAllocationId` and `NatGatewayEipPublicIp` are outputs. The host owner waived a standalone AWS-subnet/NAT connection check for dev/stage; **do not require NAT EIP allowlisting** at the Hetzner host. This is not evidence that a deployed AWS workload can reach OpenSearch. The checked-in `/32` is based on the runbook's observed A result, **not** proof of ownership approval, stable DNS, or deployed reachability. Before deploying #1850, the host/network owner must approve the destination CIDR, verify the current DNS A records and gateway routing, and update `infra/src/config.ts` and this runbook if the approved destination differs. If this cannot be established, hold the network deployment and search-dependent activation; never replace the `/32` with an unrestricted rule. Coordinate DNS, CIDR rule and TLS verification on host/IP changes. Under #1802/#1805 record the deployed rule/CIDR and private-workload search-dependent smoke (`/ready`, reader query, projector/percolator as appropriate); explicitly report failed or omitted checks and hold dependent rollout instead of calling synth, SSM inspection or the waived standalone NAT test a pass. #1843 must carry this requirement into private Fargate networking where those tasks use OpenSearch; Lambda SG coverage is not Fargate coverage. [Stage OpenSearch](../docs/opensearch-stage.md#dev-private-subnet-egress-and-release-gate-1850) owns the detailed gate. One NAT is an accepted single-AZ egress dependency: its AZ failure stops application-subnet IPv4 egress, and application subnets in the other AZ can incur cross-AZ transfer charges. It is intentionally not highly available egress. RDS/DMS provisioning, external connection tests, pricing review, live change-set diff, and crawler coordination remain separate gates.

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

### Credentials and protected initialization

The data stack generates private Secrets Manager secrets for `aura_admin`, `aura_runtime`, `aura_migrator`, and `aura_replication`. It emits no secret ARN, value, password, username, or connection string as an output. Real application PostgreSQL Lambdas receive the runtime secret ARN only, with direct `GetSecretValue` permission on that one secret and through the dedicated application endpoint. They request `AWSCURRENT` at invocation start, key a full pool-and-handler/router composition cache by secret version ID, and do not close a leased old pool beneath active work. AWS secret rotation itself remains an operator-owned action; this code does not create or mutate a rotation schedule.

Real stages also include `database-migration-lambda-<stage>`. It has no public URL, API route, schedule, or event source. It runs in private application subnets with `MigrationSecurityGroup`, reads exactly the four role-secret ARNs through the private endpoint, uses the committed RDS CA with `VerifyFull`, holds a PostgreSQL advisory lock, bootstraps roles/extensions as `aura_admin`, then applies embedded root SQLx migrations as `aura_migrator`. It returns only safe success/failure categories. Protected `Initialize (CD)` and later approved push Deploys invoke it synchronously before new application admission; manual artifact-only Deploy never invokes it. CloudFormation and runtime never invoke migrations. The CI deploy role needs exact `lambda:InvokeFunction` access to it and `fxrate-lambda-<stage>` (FX for Initialize only); it never reads database secrets.

`sql/rds-bootstrap-roles.sql` remains a break-glass psql wrapper around the same static core SQL. Use approved secret injection, never command-line or shell-history passwords. The RDS administrator receives membership in `aura_migrator`, and `aura_migrator` receives database `CREATE`, before public-schema ownership changes. The core roles remain scoped: `aura_migrator` owns `public` and creates schema objects; `aura_runtime` has runtime DML and sequence privileges; `aura_replication` has source-table read privileges plus `rds_replication`, but no DDL. The initial migration uses standard RDS `pg_trgm` and `unaccent`; it does not require `pg_ttl_index`.

For recovery, select the retained snapshot or desired point-in-time restore timestamp within the automated-backup window, restore into an isolated replacement instance/subnet/security-group plan, validate engine/parameter/role/schema state, then plan endpoint and secret handoff before application traffic. Do not assume a restore preserves current role grants, application-password alignment, or logical slots/publications. #1805 owns the tested restore runbook and evidence.

## DMS CDC declaration and evidence (#1781)

For `dev` and `prod`, this CDK declaration creates private RDS PostgreSQL `16.13`, single-AZ DMS `3.6.1` on `dms.t3.small` (2 vCPU, 2 GiB), one provisioned Kinesis shard with seven-day retention, and Kinesis plus shared Secrets Manager interface endpoints. The replication secret path is `/aura-historia/<stage>/postgres/replication`; it is never an output or log value. `aura_replication` has table-scoped `SELECT` and `rds_replication` only.

The task is CDC-only and initially stopped. Its PostgreSQL endpoint explicitly selects `aura_historia`, uses DMS's `test-decoding` setting with the pre-existing named slot `aura_historia_dms_cdc_<stage>`, and reads only the generated replication secret through the regional DMS service principal; its distinct Kinesis target service-access role trusts `dms.amazonaws.com`. The empty-default compatibility parameter omits `CdcStartPosition` for greenfield task creation and preserves any existing approved first-start LSN during stack updates. The separately approved first start must obtain and validate the actual source slot/LSN, then call DMS `start-replication` with that approved LSN. Later recovery uses `resume-processing` and DMS's recovery checkpoint, never a replacement parameter value. Neither deployment workflow starts, resets, creates, or recreates a task, slot, or checkpoint. Before push deployment of the data stack, the protected Deploy workflow bootstraps the shared account-level `dms-vpc-role` (DMS-only `sts:AssumeRole` trust and AWS-managed `service-role/AmazonDMSVPCManagementRole`). It creates a missing role, attaches a missing managed policy to a correctly trusted role, and refuses to replace an unexpected existing trust policy. This role is not owned or deleted by either stage's CDK stack. For manually deployed stacks, an operator must provision/verify it first. A lost or invalid slot requires a new fenced replay/rebuild plan. See [Migration F7](../docs/migration-f7-dms.md) for the lifecycle, availability command, table/operation mapping, decimal-string versions, LOB/Kinesis bounds, and committed-versus-rolled-back fixture protocol.

From `infra/`, the existing configuration checks are:

```bash
npm test
npm run synth -- --context stage=dev
npm run synth -- --context stage=prod
```

They do not deploy or exercise AWS. The AWS fixture procedure is documented/manual; no new test script is implied. Real-stage declarations and synthesis are not live-resource or AWS-test proof. Look up DMS, Kinesis retention, and PrivateLink endpoint/data prices at change-set approval or execution; existing NAT has no DMS incremental charge. See [Migration F7](../docs/migration-f7-dms.md#cost-delta).

## Target artifact boundary

On pushes, `Deploy (CD)` builds and publishes only the Rust Lambda ZIP catalog
referenced by CDK, including the ten scoped worker Lambdas and `cdc-router-lambda`,
and the compiled mail templates. The approved deploy job references the stage-local
uploaded artifacts by `CommitSHA`. Neither path
builds, uploads, configures, or deploys the legacy native `aura-historia-worker`
artifact or Sequin ingress. Native process deployment remains externally owned;
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
The Shopify producer code now forwards mapped observations. This change does not
add HTTP endpoints, change the old Shopify mapping, or verify live AWS deployment. Review a stage-specific CDK change
set/diff for unintended legacy queue replacements and establish custody of both
source/DLQ pairs and authoritative PostgreSQL state before any producer cutover.

## Worker queue contract

`src/worker-queue-config.ts` owns the typed catalog and shared settings. All ten
queue pairs are declared in `prod`, `dev`, and `ephemeral`. The
`product-listing-opensearch`, `product-listing-normalization`, `product-content-assessment`,
`product-embedding`, `product-translation`, `search-filter-projection`,
`search-filter-percolator`, `search-filter-match-notification`, `watchlist-notification`, and
`notification-delivery` queues have retained Lambda mappings in every compute stack.
Compute is created only by Initialize after migration and initial FX; these mappings
are active when created. This catalog remains separate from Shopify resources and wiring.

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

`product-listing-opensearch`, `product-content-assessment`, `product-translation`, `search-filter-projection`, `search-filter-percolator`, `search-filter-match-notification`, and `watchlist-notification` are 512 MiB, 45s Lambdas with retained SQS mappings targeting published function versions, batch size one, and `ReportBatchItemFailures`. Content assessment uses **270s** source visibility (`6 × 45s`); the other listed mappings use **300s**, exceeding six times the Lambda timeout. `product-embedding-lambda` is 1024 MiB with a 60s cap and **360s** source visibility (`6 × 60s`) for one bounded image/Vertex/persistence attempt. `product-translation-lambda` receives only PostgreSQL, Vertex project/location/model, Google ADC, and its source queue; it refreshes ADC after warm idle, performs inference outside its short guarded write transaction, and retries missing source, provider, persistence, timeout, panic, malformed, and unknown-commit work through SQS/DLQ. Mappings default to disabled; each resource, function version, queue pair, and IAM role stay present while off. Neither Lambda changes visibility or runs a receipt daemon; only completed service results are omitted from failures. The notification generators are PostgreSQL-only: they do not receive OpenSearch, Vertex, S3 template, or SES configuration or permissions. The saved-filter projection rereads authoritative PostgreSQL state and turns a source-missing upsert into its versioned persistent deletion fence before acknowledging.

For native-to-Lambda handoff, retain the same source queue and schema-2 job contract;
do not create, rename, or purge a replacement queue. Deploy compatible Lambda code
only after pausing the corresponding native consumer and settling in-flight work:
Initialize creates compute with the Lambda mappings already active. Never run both
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
rules and Shopify mapping. Compute creation after protected initialization creates
active ProductListing consumers and the FX schedule; initial FX is a direct,
idempotent Lambda invocation, not a CloudFormation custom resource. It does not change
Sequin subscriptions, publish a new production CDC path, start DMS, grant runtime
redrive/purge power, or prove live AWS behavior. Legacy native worker deployment remains
external. Synthesis alone does not establish durable delivery or AWS acceptance evidence.

## Deployment inputs

The compute stack exposes `CommitSHA` and the one independent default-false
`CdcRouterEnabled` parameter. Initialize creates compute only after migrations and
initial FX; its ten SQS consumers, partner rules and maintenance schedules then
become active without a separate release flag. Before running Initialize, approve
any external-provider/SES exposure and verify private dev OpenSearch connectivity
(#1850), quotas and native-consumer handoff. The router remains off until the
separately approved DMS slot/task-start and delivery gates. A router parameter
change does not recreate queues, the DMS task, slot, stream or checkpoints.

`Deploy (CD)` builds and publishes artifacts on pushes to `develop`/`prod`, then
waits for the protected `aws-dev`/`aws-prod` GitHub environment approval before
upserting the stage stacks. A manual dispatch with `stage` and a previously
published full `CommitSHA` remains available for a reviewed rollback. Configure
required environment reviewers and branch restrictions before running. The
workflow executes CDK change sets after environment approval; it does **not**
provide a separate pre-execution inspection of actual change sets. For a change
requiring that level of review, hold the run and use an operator-controlled
CloudFormation review instead of treating approval alone as that evidence.
Review the account, region, potential replacements, Lambda version/alias targets,
queues, CDC router value, secret/endpoint references and DMS capture parameters.
Configure existing OIDC `CI_DEPLOY_ROLE_ARN` for uploads and the protected
Deploy/Initialize jobs, with required stage-bucket upload/read, CloudFormation/CDK
and exact stage migration/FX invoke permissions. Push Deploy additionally needs
`iam:GetRole`, `iam:ListAttachedRolePolicies`, `iam:CreateRole` and
`iam:AttachRolePolicy` for the account-level `dms-vpc-role` (attach only
`arn:aws:iam::aws:policy/service-role/AmazonDMSVPCManagementRole`). A permission
failure or unexpected role trust blocks data deployment; an operator must repair
trust out of band rather than let CI overwrite it. The protected jobs select
`aws-dev` or `aws-prod` with required reviewers; configure the existing GitHub
secrets and variables so the push upload jobs can assume the role as well. Scope
OIDC trust and IAM to approved repository/workflows/stages, including staging-bucket
and KMS permissions where applicable. No new upload role or stored AWS keys are
required; live IAM permissions have not been verified here.

On the first approved push, successful infrastructure tests and both uploads
precede environment approval. Deploy creates only network, data and initialization
stacks, including the private migration and FX Lambdas. Its success reports
**foundation only**, not a running backend. Run `Initialize (CD)` manually with
the same `stage` and SHA: it checks foundation/source identity, migrates, captures
initial FX and only then creates compute, API and prod observability. Compute
creation activates worker consumers, partner integrations and maintenance schedules;
it is not an inactive preview. Migration or FX failure blocks application creation.

Later approved push Deploys recognize complete compute/API stacks (plus observability in
prod), update the migration Lambda to the selected SHA, migrate before updating
compute/API, and preserve the approved `CdcRouterEnabled` value. After partial creation (for example compute succeeded but API failed), a corrected
push may advance stable foundation to its new SHA without migration or application
update. Then Initialize with that SHA performs migration/initial-FX checks and
completes creation. A stack in `UPDATE_ROLLBACK_COMPLETE` can be retried; in-progress,
`UPDATE_ROLLBACK_FAILED` or failed `ROLLBACK_COMPLETE` stacks require operator-owned
CloudFormation recovery (failed creation cannot simply be updated). No workflow
automatically deletes retained resources. After a failed initial data-stack creation,
inspect retained resources and have an operator delete the `ROLLBACK_COMPLETE` stack
before retrying Deploy; CDK cannot update it. If CloudFormation deployment succeeds
but post-deploy output resolution fails, do not rerun deployment blindly: inspect
current stack parameters first and rerun only the missing output/verification step
if possible. There is no SSM readiness flag or separate
worker/partner/maintenance activation flag. Migration
failure does not undo work already in flight on the previous release; only approve
schema changes compatible with running work and retained schema-2 jobs. Before
first Initialize, verify native-consumer handoff, SES/provider consent and quotas,
and the dev OpenSearch TCP 9443 private-workload/TLS gate; otherwise hold that run.
Neither workflow starts/resets DMS, replaces RDS, purges queues or manages external
OpenSearch host/index/security. #1843 owns periodic Fargate matching; a first CDC
start needs the approved slot/LSN and later recovery uses `resume-processing` per
[Migration F7](../docs/migration-f7-dms.md).

For a dev handoff, record the stage-local artifact SHA, workflow runs, operator and
UTC approval, the reviewed changes and deployed function versions/alias targets,
`api.stage.aura-historia.com` certificate/DNS state, four `/opensearch/dev/`
role references, private-workload `/ready`/reader and relevant projector smoke,
initial FX, mapping/rule states, queues and DMS checkpoint/WAL/retention. Record
failed or waived checks explicitly; do not call synth, the host-only connectivity
check, or a waived standalone NAT test live proof. On failure hold further
activation, account for retained state and reconcile side effects before an
approved compatible SHA rollback. Provider/SES smoke and live AWS spend require
separate authorization.

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
/vertex-ai/{stage}/project-id
/vertex-ai/{stage}/location
/vertex-ai/{stage}/model
/secrets/{stage}/google-application-credentials
/loops/{stage}/api-key
/loops/{stage}/newsletter-list-id
```

The API Lambda, `search-filter-percolator-lambda`, `product-embedding-lambda`, and `product-translation-lambda` resolve their scoped Vertex and Google ADC settings through CloudFormation dynamic references. Each writes the JSON to its private `/tmp` ADC file during startup; the raw JSON is neither packaged nor logged. Neither needs runtime SSM permission. The embedding Lambda receives only Vertex project/location and ADC, not a Vertex model, OpenSearch, SES, notification-delivery, or template configuration. The translation Lambda receives only Vertex project/location/model and ADC, PostgreSQL, and its source queue. The percolator additionally resolves only its model and OpenSearch endpoint, username, and password. `product-listing-opensearch-lambda` receives none of the Vertex or Google ADC configuration and has no Google or SSM permission. It resolves the listed OpenSearch endpoint, username, and password in real stages.
The initialization-stack `fxrate-lambda-<stage>` resolves `/fxratesapi/<stage>/api-token`.
Compute's recurring Scheduler target and invoke permission use its stable unqualified
function ARN, with no release-dependent FX version export/import. Initial FX is
synchronous after migration; subsequent pushes update FX code before migrations
while the recurring schedule may still run. FX changes must tolerate the
pre-migration schema in that interval. An unqualified target follows deployed
function code rather than pinning a separate scheduler version. If live stacks
still import the prior FX version export or own an old compute FX function, plan
the resource-specific import/ownership transition separately before updating;
CloudFormation cannot remove an in-use export.
Protected manual `Initialize (CD)` invokes it after database migration and before
creating active compute, with stable source ID
`deployment:fxrate:initial:{stage}:v1`. This is not a CloudFormation custom resource;
later normal deployments do not recapture it. `ephemeral` has no real FX initialization
flow. The ephemeral stage uses local/mock values for third-party integrations where
possible.
