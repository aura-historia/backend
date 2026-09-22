# Migration F7/R5 — DMS to Kinesis CDC activation

This is the operational contract for the private PostgreSQL → AWS DMS → Kinesis → router-Lambda path. It is a controlled operational procedure, not a shell verification wrapper and not proof that any environment has been activated.

R5 activates **only** committed `public.product_listing_events` `INSERT` records. The checked-in router retains the complete future table catalog, but no other table is selected by the DMS task until R6 approval and evidence. A discovered ProductListing event still fans out to all five required destinations: ProductListing OpenSearch, search-filter percolation, content assessment, embedding, and translation. Do not suppress destinations because a downstream consumer is not deployed.

## Stable ownership and boundaries

| Resource | Stable identity / owner | Rule |
| --- | --- | --- |
| PostgreSQL source | `aura_historia` on private RDS | PostgreSQL remains authoritative business state. |
| Logical slot | `aura_historia_dms_cdc_<stage>` / DMS owner after approved creation | Uses `test_decoding`; never delete, recreate, or advance it as routine recovery. |
| DMS task | `aura-historia-cdc-<stage>` | CDC-only, initially stopped; no full load or target reload. |
| DMS instance | `aura-historia-dms-cdc-<stage>` | Single-AZ `dms.t3.small`; a capacity alert requires investigation, not automatic replacement. |
| Kinesis stream | `aura-historia-cdc-<stage>` | One provisioned shard, seven-day retention. |
| Router mapping | `CdcRouterEnabled` in compute | Default off and independent of `ProductListingOpenSearchConsumerEnabled`. |
| Router failure archive | `aura-historia-cdc-router-failures-<stage>` | Private, retained, 90-day S3 recovery input; it is not a worker DLQ. |

Normal `Deploy (CD)` and `Initialize (CD)` must not call DMS start/stop/reset APIs, create/drop slots, or set a first-start LSN. `Initialize (CD)` explicitly keeps `CdcRouterEnabled=false`. Routine compute artifact promotion preserves the approved CloudFormation parameter values and must not replace DMS, Kinesis, RDS, source queues, or the journal.

## Preconditions and approval record

Before an approved initial start, record the release SHA, stage, account, region, operator, UTC time, DMS instance/task/endpoint ARNs, stream ARN, router Lambda version, and the selected table mapping. Record safe identifiers and timestamps only—never credentials, source rows, raw DMS/Kinesis payloads, S3 archive bodies, or signed URLs.

Confirm all of the following:

1. The data, initialization, and compatible compute/router artifacts are deployed and `CdcRouterEnabled=false`.
2. The task maps only `public.product_listing_events`; its source endpoint names database `aura_historia`, slot `aura_historia_dms_cdc_<stage>`, plugin `test-decoding`, `sslMode=require`, and fails on LOB truncation. The 512 KiB limited-LOB policy must be accepted for the approved fixture.
3. The RDS parameter group has effective logical replication settings after any required reboot. Confirm replication slots/senders, retained WAL capacity, current RDS free storage, and the actual `pg_replication_slots` state.
4. The source role has an explicit approved disposition for its effective SQL privileges. The desired narrowed contract is the selected-table read scope plus `rds_replication`; do not silently broaden it during activation. Record any accepted broader read scope, approver, and expiry/review date.
5. The DMS source TLS contract is separately verified: DMS uses PostgreSQL `sslMode=require`; it is not equivalent to the application Lambdas' `VerifyFull` RDS-CA path. Record the endpoint/certificate/trust compatibility result without recording certificate or secret material.
6. DMS can use the existing account-level `dms-vpc-role`, its scoped source-secret role, Kinesis target role, private Kinesis endpoint, and private Secrets Manager endpoint. Perform an approved DMS endpoint connection test and record only outcome, resource identities, and timestamps.
7. The ten router destination queue pairs exist. The ProductListing OpenSearch worker is compatible and its native/Lambda handoff is explicitly planned; never run incompatible consumers concurrently.
8. The production CloudWatch alarms and DMS state-change notification are present before sustained capture.

## First start: slot and native PostgreSQL position

Only an approved operator with a private, verified-TLS PostgreSQL path may run these statements. Replace `<stage>` with `dev` or `prod`; do not put passwords in a command line or shell history.

First inspect the named slot:

```sql
SELECT slot_name, plugin, database, active, restart_lsn, confirmed_flush_lsn, wal_status
FROM pg_replication_slots
WHERE slot_name = 'aura_historia_dms_cdc_<stage>';
```

If it is absent, creation requires explicit slot-ownership approval. Create exactly the named `test_decoding` slot once and record the returned `lsn` as the initial native PostgreSQL start point:

```sql
SELECT slot_name, lsn AS consistent_point
FROM pg_create_logical_replication_slot('aura_historia_dms_cdc_<stage>', 'test_decoding');
```

If it already exists, do **not** recreate it. Obtain and record an approved current native position and the slot's retained position, then select the approved first-start LSN under the recorded slot-ownership decision:

```sql
SELECT pg_current_wal_lsn() AS observed_lsn;
SELECT slot_name, restart_lsn, confirmed_flush_lsn
FROM pg_replication_slots
WHERE slot_name = 'aura_historia_dms_cdc_<stage>';
```

The first-start LSN must be an actual uppercase PostgreSQL `X/Y` LSN from this source. The CDK compatibility parameter is not a routine recovery control; leave it empty for greenfield creation unless an approved existing stack value must be retained. Do not use a timestamp, a synthetic value, the Kinesis sequence number, or a later recomputed LSN as a substitute.

After a successful endpoint connection test and a recorded LSN approval, start the existing task once:

```sh
aws dms start-replication-task \
  --replication-task-arn '<recorded-task-arn>' \
  --start-replication-task-type start-replication \
  --cdc-start-position '<approved-native-postgresql-lsn>' \
  --region eu-central-1
```

Wait for DMS to report `running` and record the task status/time. A running task is not delivery proof. Do not use `reload-target`, modify table mappings, enable a full load, reset the task, or automatically delete a stalled slot.

## Controlled router handoff and journal fixture

Keep `CdcRouterEnabled=false` until the initial DMS start is healthy and the stream has the expected source records. Then deploy the already-built compute artifact through the protected CloudFormation change-set path with only this explicit mapping change:

```text
application-<stage>-compute:CdcRouterEnabled=true
```

This change enables the existing mapping at `TRIM_HORIZON`; it does not start DMS. Preserve the current ProductListing consumer parameter value independently.

Use an approved, non-sensitive ProductListing write that commits one supported v1 `product_listing_events` row. Record the journal event ID, listing ID, transaction commit time, capture time, stream arrival time, router invocation time, each expected queue receipt time, and projector completion time. Inspect metadata and bounded structural properties only:

- native DMS `{ data, metadata }` representation;
- the expected DMS `insert` operation and `public.product_listing_events` identity;
- JSON/LOB boundary behavior against the 512 KiB policy;
- expected DMS control records; and
- five compact ProductListing jobs with domain IDs, not source payload content.

The committed discovery fixture must reach the five destinations listed above. A rolled-back write must produce no successful downstream work. Before final source handoff, explicitly pause/withdraw the old journal subscription, let in-flight work settle, and preserve source queues and old consumers' durable backlog. At-least-once duplicates are expected during a controlled overlap only when consumers are compatible and idempotency is proven; do not manufacture a one-queue success by disabling required fanout.

Run the existing checks before any approved AWS fixture:

```sh
npm --prefix infra run build
npm --prefix infra test
npm --prefix infra run synth:all
cargo test -p aura-historia-worker kinesis::tests --lib --all-features
```

These are source/template checks only. The fixture record is live evidence only when tied to the deployed identities above.

## Checkpoints, poison handling, and recovery

The router validates all routes before its first SQS send, returns the earliest failed Kinesis sequence number with `ReportBatchItemFailures`, and stops processing later records. A partial fanout/lost response therefore retries and may duplicate already-confirmed jobs; downstream idempotency owns that case. After three retries or one hour, Lambda sends the original failed invocation to the private S3 archive. Correct the cause before an operator-controlled, fail-closed replay; do not give the Lambda or workers archive read, purge, or redrive permissions.

Exercise an approved poison record/failure test and record the stream sequence number, retry count/age, archive object identity, alarm time, and replay outcome. Never copy the payload into tickets or logs. A target outage must leave the affected record uncheckpointed until every required publication is confirmed.

For a DMS or source restart after the initial start, inspect task state and resume its existing recovery checkpoint—do not pass `--cdc-start-position` again:

```sh
aws dms start-replication-task \
  --replication-task-arn '<recorded-task-arn>' \
  --start-replication-task-type resume-processing \
  --region eu-central-1
```

Record pre-stop and post-resume safe sequence/job IDs, duplicate handling, and measured source/target catch-up lag. A missing/invalid slot, lost checkpoint, expired stream data, or required replay beyond retained data is a fenced rebuild/recovery decision; stop and obtain a new approved plan rather than recreating the slot or task.

## Signals, retention, and response

Production monitoring is intentionally conservative:

| Signal | Threshold | Response |
| --- | --- | --- |
| DMS source or target CDC latency | ≥ 300 seconds for 5 minutes | Check task state, endpoint connectivity, stream capacity, slot/WAL state, and catch-up rate. |
| DMS instance CPU | ≥ 80% for 15 minutes | Reduce test load or approve capacity remediation; never auto-replace the instance. |
| Kinesis write provisioned throughput exceeded | ≥ 1 in 5 minutes | Investigate DMS target throttling and approve shard/capacity action. |
| RDS free storage (WAL pressure proxy) | ≤ 10 GiB for 5 minutes | Inspect `pg_replication_slots` and retained WAL; resume/fix capture before storage exhaustion. This is not a substitute for slot-level inspection. |
| Router iterator age | ≥ 15 minutes | Inspect mapping, errors, stream retention, and downstream availability. |
| Router errors/destination failures or archive delivery | ≥ 1 in 5 minutes | Preserve source/archive evidence, repair the cause, and replay only under approval. |
| DMS task failed/stopped event | Immediate SNS notification | Treat unexpected stops as potential WAL accumulation; do not delete the slot. |

Review actual DMS instance-hours, provisioned Kinesis shard-hours and PUT payload units, Kinesis retention, interface endpoint hours/data, CloudWatch/S3 archive storage, and regional transfer pricing at approval time. DMS uses private endpoints and does not rely on NAT; those prices and endpoint-AZ compatibility remain live preflight checks.

## Evidence status

This repository supplies declarations, unit tests, and this operator contract. It does not contain credentials or a claim that AWS DMS has connected, captured, resumed, delivered to SQS, written an archive object, or completed a projection. Store approved live evidence separately with the deployment identities and safe timestamps/IDs described above. R6 owns activation and proof for raw revisions, saved filters, matches, and notification deliveries.
