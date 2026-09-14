# R5 — DB Lambdas through fixed NAT egress

Status: offline integration tested; **no live connectivity or rollout acceptance**.
Uses existing CDK stacks, catalog, NAT construct and asset bucket. No new deployment framework.

## What runs

Opt-in `bin/app.ts` context:

```text
--context stage=dev --context postgresLambdaConfig=/absolute/operator.json
```

The path is operator-supplied, not an example file to deploy unchanged. No environment fallback.
Missing context preserves legacy templates; malformed supplied context fails, never falls back.
Only `dev`/`prod` accept opt-in. The JSON account/region explicitly bind every stack.
No AWS lookups. The fixed existing artifact/staging buckets still need verification in the target account/region.

Data declares private/public subnets, NAT/EIPs and a dedicated SG. Compute attaches only
`postgres:true` entries in `constructs/lambdas.ts`: post-confirmation, Shopify, Stripe and FX sync.
No VPC/layer on log retention, FX initialization provider or WAF provider. Existing function,
role, queue, custom-resource and invocation identities remain stable. Native workers are unchanged.

DB traffic: private subnet → NAT → exact public PostgreSQL `/32`/port. No IPv6 route.
HTTPS policy is explicit: `PUBLIC_IPV4` allows **any IPv4 TCP443**, not hostname filtering;
`CIDR_ALLOWLIST` requires maintained AWS/provider destinations. FX uses `api.fxratesapi.com`.
VPC DNS traffic has AWS SG-filtering exceptions. NAT is not a general egress firewall.

Lambda's VPC execution policy permits service-managed ENIs. An exact unqualified
`lambda:SourceFunctionArn` conditional deny prevents the function code using those ENI actions.
No new runtime credential provider or extra Lambda database sessions introduced.

## Required operator JSON

Unknown/missing fields reject. No secrets belong here. `network` uses the existing
[`LambdaEgressProps` contract](lambda-egress-09a.md#required-operator-inputs), without its stage/environment fields.

| Field | Required value |
|---|---|
| `stage` | `dev` or `prod`, matching CDK context |
| `environment.account` | Actual nonzero 12-digit target account; credentials are not authorization |
| `environment.region` | Actual target region |
| `network.ipProtocol` | `IPV4` |
| `network.vpcCidr` | Approved nonconflicting RFC1918 CIDR |
| `network.availabilityZones` | 1–3 actual AZs with explicit public/private subnet CIDRs |
| `network.natTopology` | `SINGLE` plus `availabilityZone`, or `PER_AZ` |
| `network.database.destinationCidr` | Actual owned, routable PostgreSQL public IPv4 `/32` |
| `network.database.port` | Actual TLS PostgreSQL port, not443 |
| `network.httpsPolicy` | `PUBLIC_IPV4`, or `CIDR_ALLOWLIST` plus `destinationCidrs` |
| `databaseHostname` | DNS certificate identity, not URL/userinfo/raw IP; DNS must resolve to the approved `/32` |
| `caAssetDirectory` | Absolute immutable directory with the exact public-only tree below |
| `reservedConcurrency` | Exact keys `postConfirmation`, `shopify`, `stripe`, `fxRateSync`; integers1–1000, Shopify at least2 |
| `lambdaConnectionBudget` | Approved minimum estimate, integer at least `2 × sum(reservedConcurrency)` |

This is syntax/cross-field validation, not AZ/quota/certificate provenance/DNS ownership validation.
Select reservations from measured workload and account quota. Shopify's SQS mapping maximum
matches its reservation; source/DLQ identity, retention and message semantics do not change.

Each Lambda pool has maximum2. Reservations bound concurrent invocations, **not retained sessions**
in warm/frozen/replaced environments. Budget native clients, both API slots, crawler's two DBs,
cron's dedicated advisory-lock session, Sequin, bootstrap/backup work and operational reserve too.
Do not treat `lambdaConnectionBudget` as server-side enforcement.

## CA delivery and credentials

```text
<caAssetDirectory>/             0755 or0555
  postgres-ca/                 0755 or0555
    root.pem                   0644 or0444
```

Only currently valid public CA PEM blocks, at most1MiB. No private keys, leaf certificates,
extra files or symlinks. Keep source and synth output trusted/immutable through publication.
Use `umask 022`; staged directories must remain readable and not group/world-writable.

Ordinary CDK `Code.fromAsset` publishes a content-addressed layer through the existing
`CliCredentialsStackSynthesizer` bucket/stage prefix. Lambda mounts the actual file at
`/opt/postgres-ca/root.pem`; all four DB environments point there and require `verify-full`.
No runtime fetch/secret materializer. Local synth proves staged bytes, not an AWS-mounted layer
or successful TLS. SQLx uses supplied CAs plus WebPKI roots; no exclusive CA pinning or mTLS.

Database name/username/password retain existing SSM dynamic references:
`/postgres/{stage}/database`, `/postgres/{stage}/username`, `/secrets/{stage}/postgres-password`.
The existing helper uses ordinary `ssm` references; a `/secrets/` name does **not** prove SecureString
or an approved secret-storage policy. Verify that policy before activation; no secret values
were retrieved here. Lambda env is not protection from authorized Lambda administrators.
Configured hostname/port override the legacy host/port references for DB Lambdas; legacy data-stack
`PostgresHost`/`PostgresPort` outputs are not authoritative for this enabled override.

Use a restricted Lambda SQL role distinct from schema/bootstrap, replication and native roles.
Existing refs do not create SQL roles or prove separation. Runtime must not own schema or be superuser.

Rotation: publish overlapping old+new trust, roll clients, rotate server certificate, test fresh
connections, then remove old trust. Changed bundle changes layer content identity; versions are
retained on replacement/deletion. Keep necessary assets/references and explicitly retire unused
versions later. Existing pools/Lambda environments do not refresh automatically. Changing an SSM
value alone does not force CloudFormation to refresh an unchanged dynamic reference; use an
explicit reviewed Lambda configuration update and verify it. Never roll credentials back with code.

## First activation — operator-controlled pause

No command in this change deploys resources. Run live steps only with explicit target/operation
approval and a scoped assumed role, **not root credentials**. Review change sets before execution.
NAT/EIP, transfer and IPv4 charges apply; review quotas, CIDR capacity and account AZ access first.

**Current target gate:** legacy AWS dev teardown is partially executed. API stack deletion is blocked
by external root API mapping `o7vx2a` on `api.aura-historia.com` pointing at the legacy dev stage.
CloudFront/WAF removal completed after owner cancelled its plan; compute/data remain intact.
Mapping removal affects a production-facing hostname and requires separate explicit authority.
Preserve domain/DNS/certificate; do not start new deployment or retry deletion blindly. Exact operation/state and authorized scope are in
`../docs/deployment/implementation-status.md`.

1. Approve target account/region, data-stack changes, CA provenance, endpoints, roles and budget.
   Verify existing stack/resource identities and artifact/staging bucket ownership/location.
   Freeze the reviewed config/assembly and immutable Lambda artifact selection.
2. If updating existing compute, arrange an explicit invocation pause and let in-flight work finish:
   Shopify event-source mapping, FX schedule, Stripe partner delivery and Cognito sign-up trigger
   callers need individual handling. Use a hold effective throughout the reviewed update **and rollback**.
   Do not assume an out-of-band disable of a CloudFormation-managed rule/mapping survives deployment:
   declared enabled states, updates or replacements can restore delivery. Fresh sources may start enabled
   before post-deployment verification. Inspect re-enablement/replacement, queued/retried delivery, and
   approve source-specific controls before activation. Do not drop/purge events or infer an unreachable
   function stopped.
3. Deploy **data stack only** using normal CloudFormation change sets. Obtain outputs named
   `LambdaNatIpv4<availability-zone>`; those actual addresses, not template tokens, are the allowlist.
   This stack also owns existing queues/policies; inspect its complete diff. Do not deploy `--all`.
4. With separate host/provider firewall authority, allow only exact NAT `/32`s plus approved private/admin
   sources. Require TLS-only HBA, SCRAM, certificate SAN matching `databaseHostname`, and verified DNS.
   Test external IPv4 **and IPv6**, including Docker forwarding; UFW alone is insufficient evidence.
5. For an explicitly fresh database only: install required extensions/current business schema and
   restricted roles. Initialize crawler separately for native deployment. Existing data is not disposable.
   No adoption/backfill/down-migration added here. Check FX function's schema/provider prerequisites.
6. Authorize compute creation/update **including business effects**. First creation of
   `InitialFxRateSnapshot` synchronously invokes the real FX function; it may call its provider and write
   PostgreSQL before stack completion. The provider's Update/Delete do not initialize again.
   Compute depends on data; the new FX schedule depends on initialization. Neither dependency proves
   external firewall/schema readiness, nor pauses an already-enabled trigger during an update.
7. Deploy compute only after those gates. Check layer/config/reservations and invoke an approved DB
   function with an explicitly safe test case. No existing DB Lambda is advertised as a read-only health
   probe. Confirm verified TLS and actual source EIP from protected host observations; never log URLs,
   passwords, request bodies or provider error bodies.
8. Prove an unapproved external source cannot reach PostgreSQL. Resume each paused source deliberately,
   verify event handling and connection headroom. API stack/native startup can follow required Cognito
   outputs; do not make compute depend on a fully initialized native API.

An already-created FX resource will not rerun on Update. If a fresh snapshot is missing, use a
separately approved explicit invocation, not a fake-success Create/update workaround.

## Failure and removal

CloudFormation rollback does not undo FX effects, external firewall changes, schema initialization
or consumed events. Inspect actual stack/function/network/source states; keep invocation sources
paused when outcomes are uncertain. Do not blindly retry or call the environment rolled back.

Before live use, removing this opt-in slice is a source-only change. After activation, **omitting
config is not a safe rollback**: it detaches Lambdas, removes network resources and stops delivering
the CA. Retain network/trust settings when selecting a previous `CommitSHA`. Any detachment or
NAT replacement needs a reviewed change set, invocation pause and coordinated firewall update.
No automatic infrastructure/schema recovery or workflows are introduced.

## Offline evidence

Pinned Node26.8.2: build, all489 infra tests, six installed-CLI synth cases pass
(default dev/prod/ephemeral, single-stack ephemeral, opt-in dev/prod).
CA and config tests use generated public test certificates and synthetic inputs, no cloud credentials.
Independent review found all11 synthesized default template objects deeply equal to the R4 baseline and checked
acyclic dependency/IAM wiring. These checks do not establish live EIP/TLS/firewall/secret correctness.
Exact commit, reviewers and outstanding gates: `../docs/deployment/implementation-status.md`.
