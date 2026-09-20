# Aura Historia infrastructure

This directory contains the AWS CDK app for the Aura Historia backend.

The application is split into small CDK/CloudFormation stacks with typed
configuration objects instead of hand-written templates. The same stack code is
synthesized for:

- `prod` — real AWS production resources, production alarms enabled
- `dev` — real AWS development resources, production alarms disabled
- `ephemeral` — LocalStack resources, including a local OpenSearch domain

## Migration baseline

The checked-in [Migration F1 inventory](../docs/migration-f1-inventory.md) records the approved migration target, current CDK declarations, unverified live resources, ownership, and cutover gates. It is the starting reference for replacement work; this README and CDK synthesis do not prove deployed state.

## Structure

```text
bin/app.ts                 # CDK entrypoint and stage selection
src/application-stack.ts   # data, compute, API, observability stack composition
src/config.ts              # stage configuration, fixed buckets, RDS shape, SSM dynamic refs
src/worker-queue-config.ts # typed native worker scopes, timing, retention, alarms
src/parameters.ts          # deployment artifact version input
src/resources/             # synth-time resources, e.g. Cognito email HTML and inline JS
src/constructs/            # focused infrastructure modules
  api.ts                   # HTTP API Gateway routes, domain, CloudFront, WAF, CORS, JWT authorizer
  cognito.ts               # Cognito user pool, public client, IdPs, hosted UI domain
  eventing.ts              # EventBridge buses/rules, SQS mappings, Pipes
  lambdas.ts               # Lambda definitions, env vars, IAM grants
  network.ts               # two-AZ VPC, one NAT/EIP, S3 endpoint, workload security groups
  observability.ts         # prod-only alarms and alarm topic
  opensearch.ts            # external dev/prod endpoint or LocalStack domain
  queues.ts                # existing Shopify Lambda queue and DLQ
  worker-queues.ts          # separate native worker queues, scoped IAM, handoff outputs
  storage.ts               # private RDS PostgreSQL, generated role secrets, connection settings
sql/
  rds-bootstrap-roles.sql  # manual post-provision database-role bootstrap

```

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
```

These commands build, test, and synthesize only; they do not deploy.

Synth creates these stacks per stage:

- `application-{stage}-network` — real-stage two-AZ VPC, one NAT/EIP, S3 gateway endpoint, and database/workload security groups
- `application-{stage}-data` — private RDS PostgreSQL in real stages, Shopify/worker SQS, unbound worker IAM policies, and LocalStack OpenSearch
- `application-{stage}-compute` — Lambdas, Cognito, eventing, schedules
- `application-{stage}-api` — HTTP API Gateway routes, domain, CloudFront, integrations, authorizer
- `application-prod-observability` — prod-only alarms and alarm topic

The network stack is absent for `ephemeral`: LocalStack synthesis does not declare a VPC, NAT, EIP, gateway endpoint, or workload security groups. Real-stage stacks use `eu-central-1`; synth may omit an account only for template validation. A deployment must select the approved account explicitly:

```bash
npm run cdk -- deploy application-prod-network -c stage=prod -c account=123456789012 -c region=eu-central-1
```

The app rejects a non-12-digit account context and an explicitly selected real-stage region other than `eu-central-1`. Template-only synth without an account ignores an ambient CI runner region and uses `eu-central-1`; a deployment must select the approved account explicitly. Deploy network, then data, then compute; data imports the VPC for RDS and compute imports the RDS endpoint plus runtime credential dynamic references. The `cloudwatch-log-retention-lambda` remains outside the VPC. This F3/F4 declaration started from `develop` SHA `dc1ae85af84ee53cf1e8c678e7453017da1ccd56`; it is not live-provisioning evidence.

Dev CloudFront owns the wildcard alias `*.dev.aura-historia.com`; the API URL stays
`api.dev.aura-historia.com`. This avoids stale exact DNS targets blocking distribution
creation. Prod uses the exact alias `api.aura-historia.com`.

Deployments should use `cdk deploy --all` without hotswap. CI uses
CloudFormation change sets (`--method change-set`) so stack updates keep
CloudFormation's normal rollback semantics.

Deployments do not require a full CDK bootstrap stack in the target account/region.
Each stack uses `CliCredentialsStackSynthesizer` with the existing staging bucket
`aura-historia-cfn-artifcats-eu-central-1`. CDK uploads large CloudFormation
templates and any future file assets under the stage prefix (`${stage}/`). Lambda
ZIPs and scheduled Fargate images are still referenced as prebuilt S3/ECR
artifacts keyed by `CommitSHA`, not as CDK-managed assets. The retired periodic
matcher ECS image is no longer built or referenced by CDK.

Rollback is performed by redeploying a previous `CommitSHA` parameter value to the
compute stack. Lambda ZIP keys and mail-template prefixes include that SHA, so CDK
points compute resources back to the previously uploaded artifacts.

## Rust Lambda artifact contract

The current Lambda catalog uses `provided.al2023`, `x86_64`, and one executable
`bootstrap` in each ZIP. This matches the deployed CDK architecture; do not change
the target without changing the CDK definition and package smoke evidence together.
The Amazon Linux 2023 TLS trust bundle is supplied to PostgreSQL Lambdas through
`POSTGRES_TLS_ROOT_CERT`; no certificate, database password, provider token, or
signed event body is packaged or logged.

CI installs `cargo-lambda 1.9.0` with `--locked` and builds each catalog
binary with:

```bash
cargo lambda build --locked --release \
  --target x86_64-unknown-linux-musl \
  --output-format zip
```

`cargo-lambda` produces `target/lambda/<binary>/bootstrap.zip`; CI copies it to
`<binary>-<stage>-<commit-sha>.zip`, which exactly matches
`src/constructs/lambdas.ts`. Native processes remain the local development
entrypoints; no API Lambda HTTP adapter, worker polling loop, cron scheduler, or
health daemon is included in an artifact invocation.

A Lambda root constructs only its selected dependencies during cold start and
reuses immutable configuration plus pool/client handles during warm invocations.
It logs only its component, Lambda request ID, remaining invocation budget, and
cold-start duration; it never logs event bodies, credentials, or provider errors.
AWS SDK clients use the execution-role credential chain. PostgreSQL credentials
remain deploy-time dynamic references and require a redeploy after rotation until
the separately owned F5 refresh interface is available; the runtime has no
secret-read IAM or background refresh timer.

## Network foundation (F3)

Real stages have separate `/16` address space: `prod` uses `10.64.0.0/16` and `dev` uses `10.65.0.0/16`. Each has two public, two private-application, and two isolated private-database `/24` subnets across two availability zones. Exactly one managed NAT Gateway is placed in the first public subnet with one explicitly declared EIP. Both application subnet default routes use it; database route tables have no internet default route.

The application route tables use one S3 **gateway** endpoint. Its endpoint policy permits only `GetObject` and `ListBucket` on the existing artifact, mail-template, and CloudFormation-staging buckets. It does not grant Lambda IAM permissions. No paid interface endpoint, NAT instance, proxy, mandatory IPv6, public database, crawler network, or crawler database access is declared here.

`ApplicationSecurityGroup`, `DatabaseSecurityGroup`, `DmsSecurityGroup`, and `MigrationSecurityGroup` are exported for later RDS/DMS/migration ownership. PostgreSQL ingress is TCP 5432 only from those three explicit source groups. The database group has no usable outbound rule. Application workloads may egress TCP 5432 only to the database group and TCP 443 to IPv4 destinations via NAT. Security groups cannot express hostname allowlists: external HTTPS provider/AWS API host review stays with the workload and IAM/identity configuration; TLS certificate validation remains an application requirement, not a security-group setting. The DMS task owns any Kinesis or Secrets Manager endpoint decision.

No network stack or network export existed at the F1 baseline, so this change has no obsolete export to remove and makes no stateful-resource replacement. `data` imports the VPC for RDS and `compute` imports the VPC values required by PostgreSQL Lambdas. `NatGatewayEipAllocationId` and `NatGatewayEipPublicIp` are outputs. Give the public IP to the Hetzner OpenSearch owner for allowlisting before a workload uses that path. One NAT is an accepted single-AZ egress dependency: its AZ failure stops application-subnet IPv4 egress, and application subnets in the other AZ can incur cross-AZ transfer charges. It is intentionally not highly available egress. RDS/DMS provisioning, external connection tests, pricing review, live change-set diff, and crawler coordination remain separate gates.

## RDS PostgreSQL foundation (F4)

Real stages create one private, encrypted, **Single-AZ** PostgreSQL RDS instance in the two isolated database subnets. It has no public endpoint, no Aurora cluster, RDS Proxy, read replica, crawler principal/access, DMS resource, publication, or replication slot. The data stack depends on network; compute depends on data. The existing database security group remains the only TCP 5432 boundary.

The selected engine is PostgreSQL `16.13`, the newest PostgreSQL 16 engine constant available in the pinned CDK library. AWS lists supported RDS PostgreSQL releases in its [release notes](https://docs.aws.amazon.com/AmazonRDS/latest/PostgreSQLReleaseNotes/postgresql-versions.html). Verify `16.13` remains available in the approved account and region before a change set or deploy; synthesis is not that verification.

| Stage | Instance | Initial / maximum gp3 storage | Automated backup retention | Removal policy |
| --- | --- | --- | --- | --- |
| `dev` | `db.t4g.small` | 30 / 60 GiB | 7 days | delete instance and automated backups |
| `prod` | `db.t4g.medium` | 50 / 100 GiB | 14 days | retain instance and automated backups; deletion protection |

Both stages use backup window `02:00-02:30 UTC`, maintenance window `sun:03:00-sun:03:30 UTC`, automatic minor upgrades, PostgreSQL log export, encrypted storage, and copy tags to snapshots. These values are a capacity/cost starting point, not live price, restore, or load-test evidence. Production retention also applies on replacement; retained data needs an explicit operator inventory before cleanup.

The PostgreSQL 16 parameter group requires TLS (`rds.force_ssl=1`) and enables logical replication (`rds.logical_replication=1`, five slots/senders, `max_slot_wal_keep_size=10240`). `rds.logical_replication` is static and requires a reboot before it takes effect. A stalled replication slot retains WAL; the 10 GiB per-slot cap can require consumer recovery or reload and does not make storage exhaustion impossible. Monitor `pg_replication_slots`, replication lag/WAL, and `FreeStorageSpace`. The DMS source task later owns publication and slot creation. Full restore/recovery evidence belongs to #1805.

RDS enforces TLS. #1779 injects `POSTGRES_TLS_ROOT_CERT=/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem` into PostgreSQL Lambdas; migrated Rust roots require it and use SQLx `VerifyFull`, which validates both the trusted CA and RDS hostname. The configured path is the AL2023 system trust bundle, so CA rotation ships through the runtime image/config release; a missing, wrong, or stale bundle fails closed. Lambda pools are lazy with min zero and max one, never a global RDS connection cap. Local and isolated tests need a TLS-enabled PostgreSQL fixture with a separately supplied test CA; plaintext is intentionally rejected. No automatic startup migration or SQL-running CloudFormation custom resource is included.

### SQLx pool and TLS validation (F5)

Migrated roots require a TLS-enabled PostgreSQL fixture. Run the code/config gates with:

```bash
cargo test -p platform-postgres --all-features
cargo test -p aura-historia-worker --lib --all-features
npm --prefix infra test
```

Before an approved isolated-RDS smoke, use an RDS endpoint and DNS name covered by its server certificate, inject the CA bundle path through `POSTGRES_TLS_ROOT_CERT`, and record only acquisition/reconnect durations. Prove one successful trusted connection and failure for wrong CA, wrong hostname, and plaintext; do not log credentials, connection strings, or raw provider data. Restart/credential-rotation tests require separate approval and the #1780 refresh handoff.

### Credentials and manual first initialization

The data stack generates private Secrets Manager secrets for `aura_admin`, `aura_runtime`, `aura_migrator`, and `aura_replication`. It emits no secret ARN, value, password, username, or connection string as an output. PostgreSQL Lambdas receive only the runtime username/password through deploy-time Secrets Manager dynamic references; they do not have runtime secret-read IAM. Rotation therefore needs an approved credential handoff and compute redeploy; automatic application-secret rotation is not configured.

After RDS is ready, an approved migration workload in the migration security group must retrieve the generated credentials through approved operator access and run [`sql/rds-bootstrap-roles.sql`](sql/rds-bootstrap-roles.sql) as `aura_admin` against `aura_historia`:

```bash
psql "host=<private-rds-endpoint> dbname=aura_historia user=aura_admin sslmode=verify-full sslrootcert=<trusted-rds-ca.pem>" \\
  -v runtime_password='<runtime secret password>' \
  -v migrator_password='<migrator secret password>' \
  -v replication_password='<replication secret password>' \
  -f sql/rds-bootstrap-roles.sql
```

The script creates or updates scoped login roles without embedding passwords. `aura_migrator` owns `public` and creates schema objects; `aura_runtime` has runtime DML and sequence privileges; `aura_replication` has source-table read privileges plus `rds_replication`, but no DDL. It also revokes `PUBLIC` schema creation and establishes migrator-owned default grants. Run business migrations as `aura_migrator` **after** this bootstrap, then run any separately owned initial capture. Never put secrets on the command line or in a shell history in a real operation; use an approved secret-injection mechanism instead.

The initial business migration needs standard RDS extensions `pg_trgm` and `unaccent`. It also still fails deliberately when `pg_ttl_index` is absent. RDS PostgreSQL does not supply that dependency; #1776 must remove or replace it before clean fresh RDS schema initialization is possible. This foundation does not alter that migration.

For recovery, select the retained snapshot or desired point-in-time restore timestamp within the automated-backup window, restore into an isolated replacement instance/subnet/security-group plan, validate engine/parameter/role/schema state, then plan endpoint and secret handoff before application traffic. Do not assume a restore preserves current role grants, application-password alignment, or logical slots/publications. #1805 owns the tested restore runbook and evidence.

## Native processes

Production native processes are:

- `aura-historia-api`
- `aura-historia-worker`
- `aura-historia-cron`

## Native worker queue contract (#1558)

`src/worker-queue-config.ts` owns the typed catalog and shared settings. All ten
scopes are enabled in `prod`, `dev`, and `ephemeral`. This catalog is separate from
the Shopify Lambda catalog: no tier dimension, new Lambda, event-source mapping,
or change to existing Shopify resources/wiring.

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
| `product-listing-opensearch` | `ProductListingOpensearch` | 60s |
| `search-filter-projection` | `SearchFilterProjection` | 60s |
| `search-filter-percolator` | `SearchFilterPercolator` | 300s |
| `search-filter-match-notification` | `SearchFilterMatchNotification` | 60s |
| `watchlist-notification` | `WatchlistNotification` | 60s |
| `product-content-assessment` | `ProductContentAssessment` | 60s |
| `product-embedding` | `ProductEmbedding` | 300s |
| `product-translation` | `ProductTranslation` | 300s |
| `product-listing-normalization` | `ProductListingNormalization` | 300s |
| `notification-delivery` | `NotificationDelivery` | 360s |

These are **polling Rust processes**, not Lambda SQS event sources. The infra
six-times-Lambda-timeout guidance does **not** apply. Values match the worker's
45s short / 240s slow execution budgets; notification's 360s visibility leaves
headroom around its five-minute service-owned lease. Workers own bounded
execution, visibility heartbeats, retry, and deletion after successful handling.
Standard SQS may duplicate/reorder messages; handlers must remain idempotent.

### Identity and outputs

The bare-metal runtime's AWS role/trust and process deployment are **not defined
in this CDK app**. No IAM user, access key, new runtime role, or invented deploy
binding is created. Reuse the existing AWS credential/assumed-role arrangement.
The external identity owner attaches only the needed per-scope managed policies;
do not reuse the CI deploy role or an unrelated Lambda role as the worker role.

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

Set `AURA_HISTORIA_WORKER_SCOPE` to the exact runtime scope and
`AURA_HISTORIA_WORKER_QUEUE_URL` to its **source** `QueueUrl`, never its DLQ.
See [`examples/worker.env.example`](examples/worker.env.example). Preserve existing
`POSTGRES_*` and scope-specific OpenSearch/Vertex settings. EMAIL delivery still
requires `S3_BUCKET_NAME_TEMPLATES`, `NOTIFICATION_EMAIL_FROM`,
`NOTIFICATION_EMAIL_REPLY_TO`, `COMMIT_SHA`, and `STAGE`, with existing S3/SES grants.
No secrets or credentials belong in the example or stack outputs.

For LocalStack, `singleStack=true` still produces `...-ephemeral` names. Use
`STAGE=ephemeral`; substituting `local` or `test` implies different queue names.
`AWS_ENDPOINT_URL_SQS` is allowed only in `ephemeral`, `local`, or `test`, with
exactly the same origin as the queue URL. Real AWS stages must not set endpoint
overrides; the runtime rejects global `AWS_ENDPOINT_URL`.

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

This provisions the infra side only. It neither deploys a worker nor changes
Sequin subscriptions, external IAM trust, credentials, or S3/SES configuration.
Before runtime cutover, the external owner must attach the exported policies,
apply the matching environment, verify startup attribute checks/readiness, and
verify real publish/consume/retry/DLQ behavior. Synthesis alone does not establish
an end-to-end durable-delivery guarantee or change the documented MVP guarantee.

## Deployment inputs

Only the compute stack exposes a CloudFormation parameter:

- `CommitSHA` — artifact version to deploy or roll back to

The Lambda artifact and mail-template buckets are fixed in `src/config.ts`:

- `aura-historia-binary-artifacts-eu-central-1`
- `aura-historia-mail-templates-eu-central-1`

LocalStack acceptance tests synthesize one ephemeral stack with CDK context
`singleStack=true` and pass the host-mapped edge port as `localStackMappedPort`.
These values are synth-time context, not CloudFormation parameters.

## Stage-specific SSM parameters

Real AWS stages resolve external integration settings via CloudFormation dynamic
references to SSM Parameter Store. Required paths are stage-specific for `prod`
and `dev`:

```text
/opensearch/{stage}/endpoint-url
/opensearch/{stage}/username
/opensearch/{stage}/password
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
/secrets/{stage}/gemini-api-key
/secrets/{stage}/google-application-credentials
/secrets/{stage}/google-geocoding-api-key

/secrets/{stage}/zoho-accounts-url
/secrets/{stage}/zoho-campaigns-url
/secrets/{stage}/zoho-client-id
/secrets/{stage}/zoho-client-secret
/secrets/{stage}/zoho-list-key
/secrets/{stage}/zoho-refresh-token
```

`fxrate-lambda` currently reads `/fxratesapi/prod/api-token` for the scheduled
sync. On first real-stage compute-stack creation, a custom resource synchronously
invokes this same Lambda with stable deployment source ID `deployment:fxrate:initial:{stage}:v1`.
Deployment fails when this initial capture fails; it must run after PostgreSQL
business migrations. Updates, deletes, and `ephemeral` do not invoke it. The
`ephemeral` stage uses local/mock values for third-party integrations where possible.
