# Durable CDC and worker operations

Use this runbook for activation, custody and recovery—not as evidence that a stage is
running. [Event flow](events/flow.md) owns routes/completion;
[infrastructure](infra.md) owns releases and DMS operations; [marketing consent](marketing-consent.md)
owns consent-specific recovery. Record the approved stage/account, release SHA, operator,
checks and omissions. Keep payloads, credentials and personal content out of evidence.

## Activation and legacy handoff

1. Follow the [release sequence](infra.md#first-time-stage). Verify schema/job
   compatibility, FX, OpenSearch, provider/SES prerequisites and connection capacity
   **before** `scope=all` creates active mappings and integrations. Deployment is not
   a readiness probe; if required checks are unavailable, hold activation.
2. Inventory old subscriptions/processes, queue/DLQ backlog and in-flight or process-local
   work. Stop old delivery/consumers under approval before replacements take ownership.
   Source removal does not drain jobs, decommission external Sequin subscriptions or
   revoke identities. Never run competing old/new consumers or rename/purge queues for handoff.
3. Coordinate selected-source changes with DMS task state. Separately approve its actual
   slot/start position and router activation; do not invent an LSN or restart from a new
   first-start point. Verify real CDC operation/control shapes, checkpoints and failure
   archive delivery. Router and periodic-matcher enablement are independent release gates.
4. Verify private workload connectivity and scoped access, completed outcomes, lag and
   retained failure custody. Retire old subscriptions, slots and credentials only after
   ownership, drained/preserved work and rollback posture are proven. Rollback needs a
   compatible artifact and a coordinated mapping/backlog plan, not removed legacy source.

## First CDC start on a new stage

CDK configures the source endpoint's slot name and a stopped CDC-only task. Neither
Deploy nor Migrate creates the PostgreSQL slot or starts capture. Do this once under the
approved activation plan, not on every deployment.

1. Verify account/stage, deployed release, endpoint connection tests, selected tables,
   router mapping and retained failure destinations. Confirm the task has never started
   and has no recovery checkpoint. The source login must inherit the `rds_replication`
   capability: verify `pg_has_role(current_user, 'rds_replication', 'USAGE')` through
   its connection. Membership alone is insufficient when its grant is non-inherited.
   Inventory existing selected-source rows and old consumers; CDC-only capture does not backfill those rows.
2. Through an authenticated private PostgreSQL connection, inspect the configured slot:

   ```sql
   SELECT slot_name, slot_type, plugin, database, active, restart_lsn, confirmed_flush_lsn, wal_status
   FROM pg_replication_slots
   WHERE slot_name = '<approved-slot-name>';
   ```

   An absent slot on an approved new stage needs explicit creation and reconciliation
   of preexisting state. An unexpectedly missing slot on a previously running stage
   is recovery: stop and use a fenced replay/rebuild plan. Never automatically recreate
   a lost slot or treat a configured endpoint slot name as proof that the slot exists.
3. For approved new capture only, use a database administrator or an identity with
   the required slot-creation privileges. Create the configured slot with the configured
   plugin; the current DMS endpoint uses PostgreSQL `test_decoding`:

   ```sql
   SELECT * FROM pg_create_logical_replication_slot('<approved-slot-name>', 'test_decoding');
   ```

   Requery the slot. Require the correct database/plugin, an inactive healthy logical
   slot, and non-null restart/confirmed positions. Record its actual `confirmed_flush_lsn`;
   do not substitute the current WAL position or a timestamp. Start promptly and monitor
   retained WAL while capture is stopped.
4. Start the existing task from the recorded position:

   ```sh
   aws dms start-replication-task \
     --region <stage-region> \
     --replication-task-arn <existing-task-arn> \
     --start-replication-task-type start-replication \
     --cdc-start-position <confirmed-flush-lsn>
   ```

   Confirm running state, source-slot activity and checkpoint progress. Subsequent
   recovery uses `resume-processing` and the task checkpoint, not a fresh first start.
5. Reconcile approved preexisting state through its owning consumer contract. A guarded
   no-op update can wake a current saved-filter projection without changing its values,
   version or timestamps; verify row identity/version and unchanged business state.
   This is not a general backfill method for immutable history or notification intents.
   Recheck source inventory at the capture boundary; additional preexisting work needs
   its own reconciliation plan.
6. Trace a real selected-source change through DMS, Kinesis, confirmed router publication,
   SQS and the owning worker's completed outcome. Connection tests, control records,
   an enabled mapping or a successful invocation alone do not prove completion. Check
   lag, retained WAL, queue/DLQ custody and router archive failures without exporting
   payloads or consuming live messages for inspection. Record safe evidence and remove
   temporary query infrastructure and its credentials access.

## Failure custody and controlled redrive

| Failure location | What to inspect |
| --- | --- |
| DMS / Kinesis | Task/slot health, retained WAL, capture lag, iterator age, checkpoint and transport retention |
| Router | Earliest failed sequence, publication outcome and private S3 invocation archive; not a worker DLQ |
| Worker source / DLQ | Oldest age, depth, receive/retry history, domain identity and PostgreSQL completion state |
| Provider ingress | Upstream delivery history and its queue/DLQ as well as downstream command custody |
| Scheduled work | Target-delivery DLQ **and** actual invocation/task exit/report; successful launch is not completed work |

Current recovery windows are defined in the constructs: Kinesis and CDC/command
source queues retain seven days, worker DLQs 14 days, and router failure objects 90 days.
Shopify upstream source retention is four days. Verify effective attributes before recovery.
Standard SQS DLQ transfer preserves original enqueue time, so source/DLQ windows are not
additive; FIFO transfer resets DLQ enqueue time but still does not guarantee recoverability.
Router exhaustion archives the failed invocation; neither archives nor DLQs replay themselves.
Archive-destination failure also needs investigation—an error does not prove an object exists.

Workers use partial batch responses. Only confirmed complete records acknowledge; setup,
wire, timeout/panic, dependency, active-claim and uncertain-effect failures retain custody.
FIFO processing stops at the first incomplete record and reports every unprocessed
successor, even across groups. Partial failures need not increment Lambda `Errors`;
monitor outcome counts, age/backlog, DLQs and provider failures too.

For recovery:

1. Locate **all** copies and custody boundaries; preserve original domain IDs, source
   sequence, command fingerprint and FIFO grouping/order. A DLQ copy does not prove no
   other copy completed. Expired transport cannot recreate absent authoritative history.
2. Inspect PostgreSQL receipts, revisions, leases and current eligibility with bounded,
   read-only queries. Use metadata/safe IDs, not exported raw records. A lost commit or
   provider reply is not proof of rollback or non-delivery.
3. Repair the cause and test isolated replay using the real fail-closed decoder. Obtain
   separate operator approval for a small ordered redrive within retention; observe
   completion and downstream effects before expanding it.
4. Preserve receipts and fences. Never fabricate success, reset an applied receipt,
   skip poison input, blindly resend historic mail, or purge to clear alarms. Runtime
   roles do not have operator replay/redrive/purge authority.

## ProductListing ingestion FIFO command recovery

Admission confirms custody, not execution. [Retry identity](product-listing.md#retry-identity)
requires the unchanged original ordered batch, actor, source and effective key; correct
known-invalid items separately with a new key. Consumer success requires atomic application
and its matching `APPLIED` receipt. Those receipts survive business-row deletion.

A completed old command replays as a no-op; an old **unfinished** command can still apply
against newer state or fail guards. Before redrive, inspect source-order barriers, current
partnership/source grants and raw/provider/command receipts. FIFO does not order synchronous
writes, normalization or manually reintroduced work. Pause producers/consumption only with
an explicit backlog and retention plan; do not discard messages to stop traffic.

Deploy the receipt schema, retained queue pair and compatible consumer before enabling
producers. Verify actual IAM/private SQS and database access, published mapping, partial
failure behavior, retry/DLQ transfer and bounded concurrency. Keep acceptance fixtures
isolated and record admissions, committed receipts and normalization separately.

## Shopify forwarding handoff

Keep the EventBridge → Shopify Standard queue → Shopify Lambda boundary. The Lambda
acknowledges a mapped observation only after confirmed command-FIFO admission. A lost send
reply may already have queued it; retry with the same identity. The FIFO acknowledges
application independently, and normalization/projection complete later.

Before cutover, verify both retained queue pairs, source grants, receipt schema and active
compatible consumer. Fence immediate capture for the same observations. Inspect **both**
upstream and ingestion queue ages/DLQs on failure. Rollback fences the Shopify producer;
pausing shared consumption requires a separate retention plan.

## WooCommerce webhook queue handoff

Verify the API's stage-local FIFO URL/send grant and compatible consumer before forwarding.
Use isolated exact-byte signed probes with valid partner/source authorization. Authorized
ignored inputs produce no command/receipt; a mapped `204` means confirmed admission only.
Later receipt/order/capture conflicts can reach the FIFO DLQ without a provider retry.

After an uncertain HTTP reply, preserve signed bytes, source, topic, caller and delivery ID.
Without a provider delivery ID, the original command identity may not be recoverable. Inspect
FIFO and database state before redrive. Fence only this producer for rollback, not shared
consumption, and do not run concurrent immediate capture of the same observations.

## Retained CDC archive resource import recovery

Retained archives/logs/queues can outlive a failed or deleted stack. Do not delete history,
rename resources or enable broad auto-import to make deployment pass.

1. Hold deployment; inventory physical resources, account/region, exact ownership and
   failed/pending CloudFormation state. Verify policy, encryption, public-access blocks,
   retention and contents without exporting events.
2. Review the selected template and exact logical/physical resource identity. Under
   separate approval prepare an **import-only** change set for the verified ownerless
   resource; reject unrelated imports, creations, changes, replacement or deletion.
3. Verify completion, ownership, settings and drift. Review full-template reconciliation
   separately and reach an acceptable normal stack state before Deploy. Import-only
   completion is not application readiness and does not replay archived work.

Apply the same narrow ownership procedure to other retained resources; archive adoption
alone does not settle log-group or queue collisions. [Stack recovery](infra.md#rollback-and-stack-recovery)
also requires checking partial activation and release prerequisites.

## Projection fences and rebuild

ProductListing withdrawals and saved-filter deletions use content-free, externally
versioned `projectionDeleted: true` tombstones. Deploy mappings and **all** excluding
readers before writers; fence old physical-DELETE writers, including in-flight requests.
Never TTL or physically delete tombstones: remote physical-delete version memory expires.

Rebuild into an isolated index generation: fence old writers, reconstruct authoritative
facts, catch up commits, verify IDs/source versions/deletion visibility, then switch the
approved reader/alias target. Withdrawn listings retain source state; hard-deleted filters
need retained deletion facts or a fenced fresh-generation rebuild. Index reset, expired
queues and absent history cannot restore those facts. [OpenSearch operations](opensearch-stage.md)
own host/security and asset changes.

## Notification recovery limits

Delivery intent/lease state lives in PostgreSQL. Commit the claim before SES; retry
finalization with the **same lease and result**, not another send. Active claims and lost
leases are not completion. A send/finalize crash can duplicate mail after lease expiry.
Inspect safe delivery IDs, database completion and restricted provider evidence before
approved recovery. Never interpret timeout as definitely unsent or automatically resend
historic customer mail.

## Geographic resolution recovery

[Geography](geography.md#durable-resolution-and-party-integration-2002) owns scope,
acceptance and retention eligibility. Apply its migration before the new Party and worker
runtimes. Stage `geoResolutionEnabled` and PostgreSQL provider `enabled` and
`storage_permitted` default false. Confirm the reviewed provider profile, applicable
storage/private-input rights, attribution, database capacity and daily/concurrency caps
before enabling paid work. Disabling paid calls preserves permitted cached evidence;
revoking rights requires `storage_permitted=false` and verification of physical purging.

Only compact resolution IDs/generations cross CDC and SQS. Private input, provider
evidence and scope owners must stay out of logs, jobs and operator evidence. Monitor
`geo_resolution_input` reuse, `geo_resolution_outcome` method/policy/outcome/failure and
attempt counts, `geo_resolution_oldest_pending` age and `geo_resolution_drain` counts,
plus queue age/depth/DLQ and
scheduled drain failures. These structured log fields are aggregate observations, not
provisioned CloudWatch metric filters. Investigate BLOCKED/FAILED database state as well
as transport custody; partial failures need not increment Lambda Errors.

The one-minute scheduled drain processes at most two pending inputs per invocation and
bounded retention cleanup, independently of wake-up delivery. It runs with paid work off
to purge expired, revoked and orphaned provider evidence and review selections. Readers
hide ineligible evidence immediately. Verify the INVALIDATED journal entry and changed
generation after cleanup; the journal contains no provider derivatives. Listing
projection consumption is a separate roadmap integration and must use current state.

For recovery, use bounded read-only metadata queries to inspect current binding,
generation, lease, attempt count, last failure, deadline and provider/scope budgets. An
active claim is incomplete. A paid-call/commit crash can repeat charging after lease
expiry; five durable attempts and one SDK attempt per reservation bound ordinary retries.
Budget reservations are not refunded after uncertain calls. The scheduled drain resumes
expired leases; exhausted work requires an explicit protected Retry after correcting the
cause. Authentication/configuration/retention failures are not geographic no-match.

Request establishes work for a preexisting eligible site; CDC-only capture does not
backfill PartyLocations. Protected Retry and candidate selection require current location
CAS and resolution generation. An unchanged completed retry is free; forced resolution
invalidates prior derivatives and selections, and cannot bypass budget caps. Preserve
IDs/generations on a small approved DLQ redrive: old generations acknowledge as stale,
completed current generations reuse evidence, active claims defer, and malformed jobs
retain custody. Never fabricate a new generation by editing transport payloads or copy a
private input into a diagnostic job. Confirm completion and current eligibility before
expanding redrive.
