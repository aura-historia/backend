# Migration F7/R5/R6 — DMS to Kinesis CDC activation

This is the operational contract for the private PostgreSQL → AWS DMS → Kinesis → router-Lambda path, replacing the historical Sequin/native worker source. It is a controlled operational procedure, not a shell verification wrapper or proof that any environment has been activated **or that external legacy delivery has been decommissioned**. Removing the legacy source code does not stop Sequin, release its slot, revoke credentials, settle native in-flight work, start DMS, or enable router/worker mappings.

R6 selects committed rows from `public.product_listing_events`, `product_listing_raw_revisions`, `search_filters`, `search_filter_matches`, and `notification_deliveries`. Application jobs remain deliberately narrower: journal/raw/match/delivery tables trigger only on `INSERT`; saved filters trigger on `INSERT`, `UPDATE`, and `DELETE`. DMS may emit valid non-trigger updates or deletes for an INSERT-only table; the router validates their minimum envelope and selected-table identity, acknowledges them with no job, and retains malformed, unknown, or incompatible records for recovery. No selected table is a generic subscription.

## Stable ownership and boundaries

| Resource | Stable identity / owner | Rule |
| --- | --- | --- |
| PostgreSQL source | `aura_historia` on private RDS | PostgreSQL remains authoritative business state. |
| Logical slot | `aura_historia_dms_cdc_<stage>` / DMS owner after approved creation | Uses `test_decoding`; never delete, recreate, or advance it as routine recovery. |
| DMS task | `aura-historia-cdc-<stage>` | CDC-only, initially stopped; no full load or target reload. |
| DMS instance | `aura-historia-dms-cdc-<stage>` | Single-AZ `dms.t3.small`; a capacity alert requires investigation, not automatic replacement. |
| Kinesis stream | `aura-historia-cdc-<stage>` | One provisioned shard, seven-day retention. |
| Router mapping | `CdcRouterEnabled` in compute | Default off; independent of the active-on-initialization SQS consumers. |
| Router failure archive | `aura-historia-cdc-router-failures-<stage>` | Private, retained, 90-day S3 recovery input; it is not a worker DLQ. |

Normal `Deploy (CD)` and `Initialize (CD)` must not call DMS start/stop/reset APIs, create/drop slots, or set a first-start LSN. Initial compute creation defaults `CdcRouterEnabled=false`; routine pushes and artifact-only manual Deploy preserve its approved parameter value. The ten active SQS consumers are created during Initialize only after migrations and FX succeed. Compute creation is not an inactive preview; pause old consumers and approve producer/provider handoff before Initialize. Routine releases must not replace DMS, Kinesis, RDS, source queues, or the journal.

## Preconditions and approval record

Before an approved initial start, record the release SHA, stage, account, region, operator, UTC time, DMS instance/task/endpoint ARNs, stream ARN, router Lambda version, and the selected table mapping. Record safe identifiers and timestamps only—never credentials, source rows, raw DMS/Kinesis payloads, S3 archive bodies, or signed URLs.

Confirm all of the following:

1. The data, initialization, and compatible compute/router artifacts are deployed and `CdcRouterEnabled=false`.
2. The task maps exactly the five R6 tables above; its source endpoint names database `aura_historia`, slot `aura_historia_dms_cdc_<stage>`, plugin `test-decoding`, `sslMode=require`, and fails on LOB truncation. It retains the journal routing fields, keeps only job identifiers for the four R6 tables, strips raw/sensitive source columns, and sends raw `revision` and saved-filter `version` as exact decimal strings. The 512 KiB limited-LOB policy must be accepted for the approved fixture.
3. The RDS parameter group has effective logical replication settings after any required reboot. Confirm replication slots/senders, retained WAL capacity, current RDS free storage, and the actual `pg_replication_slots` state.
4. The source role has an explicit approved disposition for its effective SQL privileges. The desired narrowed contract is the selected-table read scope plus `rds_replication`; do not silently broaden it during activation. Record any accepted broader read scope, approver, and expiry/review date.
5. The DMS source TLS contract is separately verified: DMS uses PostgreSQL `sslMode=require`; it is not equivalent to the application Lambdas' `VerifyFull` RDS-CA path. Record the endpoint/certificate/trust compatibility result without recording certificate or secret material.
6. DMS can use the existing account-level `dms-vpc-role`, its scoped source-secret role, Kinesis target role, private Kinesis endpoint, and private Secrets Manager endpoint. Perform an approved DMS endpoint connection test and record only outcome, resource identities, and timestamps.
7. The ten router destination queue pairs exist. The scoped Lambda worker artifacts are compatible with retained schema-2 jobs. The worker mappings become active when Initialize creates compute; confirm any historical native consumers are stopped and in-flight work settled **before Initialize**. Never run concurrent competing consumers of one queue.
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

## Controlled router handoff and R6 capture evidence

Keep `CdcRouterEnabled=false` until the initial DMS start is healthy and the stream has the expected source records. Then deploy the already-built compute artifact through the protected CloudFormation change-set path with only this explicit mapping change:

```text
application-<stage>-compute:CdcRouterEnabled=true
```

This change enables the existing mapping at `TRIM_HORIZON`; it does not start DMS. ProductListing and the other SQS mappings are not tied to the router parameter.

Before changing a running R5 task mapping, record its task checkpoint, slot `restart_lsn`/`confirmed_flush_lsn`, stream retention window, queued source-job age, mapping digest, and safe IDs/timestamps. Stop only under approval, apply the reviewed mapping without full load or target reload, then resume the existing task with `resume-processing`—never supply a new CDC start position, reset the task, recreate the slot, purge queues, or replay historical notifications. Confirm WAL and retained Kinesis/SQS windows cover the stop interval before proceeding.

The approved live exercise must record safe identifiers, timestamps, counts, deployed resource identities, and bounded structural observations only—never source rows or DMS/Kinesis bodies:

| Committed operation | Required completion signal |
| --- | --- |
| Journal `INSERT` | The existing five-job routing union; rollback produces no jobs. |
| Raw revision `INSERT` | One normalization job with stream ID, revision ID, and exact positive revision only; no raw JSON in the job. |
| Saved-filter `INSERT`/`UPDATE` | One projection invalidation with canonical ID/version; a source-missing upsert races safely to its external-version tombstone. |
| Saved-filter `DELETE` | A real DMS `delete` record retains OLD `user_search_filter_id`, `user_id`, and an exact positive decimal `version` after the row is absent; delayed old upserts, duplicate deletes, and newer state preserve the projection fence. |
| Match `INSERT` | One notification job with the exact historical `(user_id, user_search_filter_id, product_listing_id, origin_event_id)` identity. |
| Delivery `INSERT` | One delivery-intent job; the consumer's authoritative reread validates initial `EMAIL`/`PENDING` state. |

Also prove valid match feedback changes plus delivery claim/finalization updates or deletes are consumable no-ops, expected table-description controls are consumable no-ops, and malformed metadata/required trigger values remain unacknowledged. Control classification, operation, and table identity come from `metadata`; the `control` object contains table details. A table-scoped `create-table` for a selected table is informational only with a valid table definition. Missing table identity, unknown or schema-altering controls, and contradictory payload metadata remain unacknowledged. The structural control fixture is adapted from the AWS streaming specification (Kafka example), not a capture from this DMS-to-Kinesis deployment; inspect a sanitized live control record before activation. The renamed synthetic DELETE vector is parser specification evidence only; it is never proof of actual DMS output. Before final source handoff, explicitly pause/withdraw externally owned Sequin subscriptions and old process deployment, let in-flight work settle, and inventory/preserve legacy in-memory and durable backlogs. Stop any duplicate publishers before normal operation; no source-code deletion or one-queue success substitutes for confirmed ten-scope fanout. Retire old replication slots, infrastructure, and credentials only under separately approved, evidence-backed decommissioning—never drop or advance an active/unknown slot as a shortcut. The old native raw-normalizer scan cannot be relied on after retirement: target raw recovery uses CDC wake-ups, persisted stream progress, bounded Lambda draining, and SQS retries/DLQ; there is no scheduled raw reconciliation. Preserve R17 crawler raw-capture and local-state contracts during this handoff.

Run the existing checks before any approved AWS fixture:

```sh
npm --prefix infra run build
npm --prefix infra test
npm --prefix infra run synth:all
cargo test -p cdc-router-lambda --lib --all-features
cargo test -p product-listing-normalization-lambda --lib --all-features
```

These are source/template checks only. Synthetic fixtures are not evidence of a DMS-to-Kinesis capture or SQS integration. The standalone `cdc-router-lambda` alone owns Kinesis processing. Historically the native `/cdc/sequin` parser also accepted DMS-shaped JSON as a compatibility behavior, **not** an alternative DMS/Kinesis router; removing that ingress does not prove legacy delivery is disabled in AWS. Only a record captured in the approved environment and tied to the deployed identities above constitutes live evidence.

## Checkpoints, poison handling, and recovery

The router validates all routes before its first SQS send, returns the earliest failed Kinesis sequence number with `ReportBatchItemFailures`, and stops processing later records. A partial fanout/lost response therefore retries and may duplicate already-confirmed jobs; downstream idempotency owns that case. After three retries or one hour, Lambda sends the original failed invocation to the private S3 archive. Lambda's documented archive object is an outer invocation record whose required `payload` field is an escaped JSON string containing the original Kinesis invocation; the fail-closed replay decoder rejects missing/non-string/invalid payloads, empty batches, unusable sequences, and oversized archives. DMS data records are `{ data, metadata }`; expected controls are `{ control, metadata }`, and schema-altering or unknown controls remain retained failures. Correct the cause before an operator-controlled, fail-closed replay; do not give the Lambda or workers archive read, purge, or redrive permissions.

Exercise an approved poison record/failure test that proves both successful S3 archival and notification via the router mapping's `OnFailureDestinationDeliveredEventCount` alarm; record the stream sequence number, retry count/age, archive object identity, alarm time, and replay outcome. The mapping's `DroppedEventCount` alarm reports discarded records separately; `DestinationDeliveryFailures` reports unsuccessful archive delivery. Native tests and synthesis do not prove CloudWatch delivery. Never copy the payload into tickets or logs. A target outage must leave the affected record uncheckpointed until every required publication is confirmed.

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
| Router errors, successful failure-archive delivery, dropped records, or archive delivery failures | ≥ 1 in 5 minutes | Inspect the mapping-specific event-count and destination signals; preserve source/archive evidence, repair the cause, and replay only under approval. |
| DMS task failed/stopped event | Immediate SNS notification | Treat unexpected stops as potential WAL accumulation; do not delete the slot. |

Review actual DMS instance-hours, provisioned Kinesis shard-hours and PUT payload units, Kinesis retention, interface endpoint hours/data, CloudWatch/S3 archive storage, and regional transfer pricing at approval time. DMS uses private endpoints and does not rely on NAT; those prices and endpoint-AZ compatibility remain live preflight checks.

## Evidence status

This repository supplies declarations, unit tests, and this operator contract. It does not contain credentials or a claim that AWS DMS has connected, captured, resumed, delivered to SQS, written an archive object, or completed a projection. Nor does source removal prove external Sequin subscriptions, processes, slots, or credentials were decommissioned. Store approved live activation **and decommission** evidence separately with deployment identities and safe timestamps/IDs. Synthetic R6 specification coverage is not live capture proof. For SQS worker recovery, source/DLQ retention, notification ambiguity, and projection fencing, use the [durable-worker runbook](durable-worker-runbook.md).
