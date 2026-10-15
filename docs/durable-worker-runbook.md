# Durable CDC and worker operations

Use this runbook for activation, custody and recovery—not as evidence that a stage is
running. [Event flow](events/flow.md) owns routes/completion;
[infrastructure](infra.md) owns releases and DMS operations; [marketing consent](marketing-consent.md)
owns consent-specific recovery. Record the approved stage/account, release SHA, operator,
checks and omissions. Keep payloads, credentials and personal content out of evidence.

## Shipping evidence recovery

The [shipping sync contract](shipping.md#evidence-and-connector-synchronization) owns
limits and authority. Finalization runs through the API; there is no shipping queue or
background normalizer to drain. Producers/operators must retain their exact fetch key,
session ID, ordered chunk digests and manifest until confirmed `APPLIED`. Existing
Shopify and WooCommerce catalog backfill admission remains independent: catalog queue
admission does not prove shipping application or justify dropping a pending session.

- After a response timeout, read the session receipt or repeat the identical operation.
  After uncertain installation rotation, resolve installation status before rotating again.
- Page the current installation's `pending` directory (at most 32 IDs, UUID keyset) and
  inspect each receipt, including completed applications with dirty current dependencies.
  Repair `LISTING_MISSING`/`PROFILE_MISSING` through the normal
  authorized catalog/configuration path, then retry the stored manifest before its
  24-hour deadline. Configuration repair can fence an earlier membership fetch; acquire a
  new snapshot when its receipt then requires reconciliation. Review retained manager
  assertions through typed management commands for `INTEGRATION_CONFLICT`, rather than
  migrating their class/variant meaning across installations. Shipping intake must never
  create or restore a dependency.
- `RECONCILE_REQUIRED` marks a durably dirty scope. Resolve current fences and acquire
  a new full snapshot; replaying the old receipt cannot clear the race. Failed extraction
  or lost provider permissions preserves prior assertion age. Renew permissions and fetch
  anew rather than finalizing partial pages. `EXPIRED` requires a new acquisition.
- Disconnect/replace through installation management; provider configuration replacement
  also revokes old work. Never copy an old epoch into a new installation. Current source
  authorization is checked even for matching successful replays.
- Invoke source `shipping-sync/cleanup` with a limit of 1–32 to erase captured content
  older than 30 days. Repeat bounded calls during authorized maintenance. It also expires
  unfinished retained sessions. Digests, manifests, receipt/generation/epoch identities and
  canonical ownership fences survive without TTL; erasure cannot make a retry key reusable.
  This route is callable maintenance, not an activated retention schedule. Protected profile
  history and current ownership state follow the separate shipping management retention
  and authorized-erasure requirements. Fence the installation before exceptional content
  erasure; retain address-free IDs/revisions rather than restoring erased captured content.

For a deleted source, source HTTP authorization is unavailable. Use the approved private
PostgreSQL maintenance role with the retained native source UUID: select at most 32 sessions
older than 30 days under row locks, expire unfinished sessions, then set their chunk payloads
to null with an erasure timestamp in the same transaction. The retention trigger permits
that content-only update; preserve digests, counts, application signals and identity fences.
Do not disable immutability triggers or restore erased chunks to repair later ingestion.

Record only source/session/epoch IDs, state/failure codes and counts. Raw candidates,
policy addresses, evidence references and request bodies must stay out of logs and
ordinary jobs. Check current listing responses for canonical effects; `projection: UNKNOWN`
does not confirm search visibility. [Executable PostgreSQL recovery tests](../src/shipping-sync-postgres/tests/synchronization.rs)
and [shared fixtures](contracts/shipping-sync-v1/fixtures) exercise the protocol.

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
   Complete source/destination retirement **before** a data-stack update removes queues
   or access policies: exclude retiring sources from effective capture under the approved
   checkpoint plan, then drain previously captured records through the compatible router.
   Verify per-shard router checkpoints passed those records, pause the old router so it cannot
   publish to retiring destinations, and settle queue/DLQ/in-flight and retained archive custody
   before resource removal. A new router rejects retired-source records and stops later records
   in that batch; selection exclusion alone is not a safe cutover. Preserve unrelated records
   and never purge a shared stream. Resume routing only with the compatible destination catalog.
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
5. Reconcile approved preexisting state through its owning consumer contract. CDC-only
   capture does not backfill immutable history or notification intents; do not mutate
   business rows merely to manufacture delivery. Recheck source inventory at the capture
   boundary; additional preexisting work needs its own reconciliation plan.
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

ProductListing withdrawals use content-free, externally versioned
`projectionDeleted: true` tombstones. Deploy mappings and **all** excluding
readers before writers; fence old physical-DELETE writers, including in-flight requests.
Never TTL or physically delete tombstones: remote physical-delete version memory expires.

Rebuild into an isolated index generation: fence old writers, reconstruct authoritative
facts, catch up commits, verify IDs/source versions/deletion visibility, then switch the
approved reader/alias target. Withdrawn listings retain authoritative source state.
Index reset, expired queues and absent history cannot restore missing authoritative facts.
[OpenSearch operations](opensearch-stage.md) own host/security and asset changes.

## Periodic matching recovery

[Event flow](events/flow.md#periodic-saved-filter-matching) owns matching semantics;
[infrastructure](infra.md#cdc-and-scheduled-work) owns schedule activation. Before enabling
runs, verify private PostgreSQL access, ProductListing search with reader credentials,
complete persisted FX and classifier configuration. Filter candidates and progress come
from PostgreSQL. Apply the matching-evidence migration before expanded runs; it resolves
existing policies and plan/quota states and fences old snapshots. Older saved-filter writers
cannot populate the required policy or reconcile it on tier changes. Quiesce saved-filter
and subscription writes and pause/drain matcher runs for migration and deployment of the
compatible API, billing and matcher runtimes; resume only when every writer maintains the
new invariant. Query-only filters need no embedding for lexical retrieval. Enhanced filters
eligible for activation require the existing write-time embedding; migration admission
aborts if one is missing. Repair missing embeddings with the deployed
saved-filter write path before migration admission (an effective intent change generates
the embedding; restore the intended query afterward). The update use case repairs missing
semantic embeddings outside its transaction and rechecks current intent and tier under
lock. Preserve authored intent and do not generate embeddings during matching.

With concurrency eight, a 50-candidate filter may require seven request waves per attempt;
image fetching adds bounded preparation time. The two-hour run deadline also bounds expanded
Free/Pro coverage: monitor selected/attempted counts, failed filters and timeout outcomes for
saturation. Candidate/scan caps are successful bounded discovery, not exhaustive coverage.

Inspect Scheduler delivery failures separately from actual ECS task exit and application
completion reports. Failed filters, cancellation or timeout are incomplete work even if
other matches committed; overlapping-run exclusion is not evidence that a run advanced
progress. Verify per-filter progress and persisted matches with bounded read-only checks.
Repair the dependency and use the next scheduled run or an approved bounded invocation;
preserve checkpoints and match identities instead of resetting progress or deleting
matches to force a replay. The replay overlap tolerates bounded projection lag, not an
unlimited outage: review uncovered windows after prolonged search/projection failures.

Match INSERT delivery still requires healthy CDC/router/notification workers. Verify
that path independently; rerunning matching is not a substitute for recovering a retained
notification job and must not blindly resend historical mail.

## Notification recovery limits

Delivery intent/lease state lives in PostgreSQL. Commit the claim before SES; retry
finalization with the **same lease and result**, not another send. Active claims and lost
leases are not completion. A send/finalize crash can duplicate mail after lease expiry.
Inspect safe delivery IDs, database completion and restricted provider evidence before
approved recovery. Never interpret timeout as definitely unsent or automatically resend
historic customer mail.

## Geographic resolution recovery

[Geography](geography.md#durable-resolution-and-party-integration-2002) owns acceptance,
privacy and retention. Resolution workers always process eligible work; provider controls
govern provider availability, storage permission and budgets. New provider profiles default
off. Before enabling a profile, confirm applicable provider/private-input
rights, attribution, database capacity and call/concurrency budgets. Turning off paid calls
preserves permitted cache entries; rights withdrawal requires revocation and physical purging.

Apply the consolidated geography migration with compatible API, router, workers and cleanup
binaries through the [schema-dependent release procedure](infra.md#routine-and-schema-dependent-releases).
Coordinate DMS capture of compact resolution and listing-geography wake-ups with the existing
checkpoint. Keep private inputs, bindings, work checkpoints and evidence outside CDC. Protected
Request establishes work for preexisting eligible PartyLocations; CDC-only capture does not
backfill them. Deploy compatible ProductListing geographic mappings and verify refreshed
product search visibility before enabling new geographic filters.

The hourly backend cleanup purges ineligible evidence and requeues bounded runnable work,
even with paid calls disabled. Fresh wake-ups require healthy CDC/SQS. Recovery preserves
attempts, pending age, backoff and spent budgets; live leases retain their reservations.
Additional bounded cleanup invocations follow ordinary operational approval when backlog or
rights withdrawal requires faster erasure. Readers suppress ineligible evidence immediately;
listing search accepts projection lag while removal propagates.

Inspect only safe metadata: binding identity, generation, lease, attempts, failure category,
deadline and budgets. Monitor resolution reuse/outcomes, oldest pending age, maintenance
counts, queue age/depth/DLQ and cleanup failures. Structured log fields are observations, not
provisioned metric filters; BLOCKED/FAILED work also needs inspection. Never include private
input or provider payloads in logs, operator evidence or replay messages.

An active claim is incomplete. A paid-call/commit crash may repeat charging after lease
expiry; reservations are not refunded. Business Retry/selection requires current location
CAS and resolution generation. Unchanged completed retries reuse evidence; forced resolution
invalidates derivatives/selections and cannot bypass budgets. Private addresses expose
ordinary replacement/read APIs; corrected configuration gates allow bounded background
recovery, while exhausted or invalid inputs require cause correction and changed input or
approved operational recovery.

Redrive a small approved DLQ sample with original identities/generations: superseded work
completes as stale, completed work reuses evidence, active leases defer and malformed jobs
retain custody. Confirm completion and eligibility before expanding redrive. Never invent a
new generation in a transport payload. Runbook checks do not establish deployed readiness.

## Listing geography recovery

The worker consumes compact work ID/generation wake-ups and commits bounded pages plus
checkpoints. Continuations retain durable custody; newer invalidation fences claims and
restarts traversal. The hourly cleanup also drains a page to recover missed wake-ups. Neither
path calls a geocoder or writes OpenSearch directly. Initial migration enqueues source-level
rebuild work without rewriting every listing synchronously.

Apply the cleanup of obsolete acknowledgment storage only after geography and product projector
binaries and rollback sources no longer reference it, under the
[schema-dependent release rules](infra.md#routine-and-schema-dependent-releases).

For rebuilding, fence old projection writers and follow the existing isolated OpenSearch
generation procedure. Enqueue source UUIDs in bounded keyset pages with
`SELECT enqueue_listing_geography('SOURCE', $1)` in the operational transaction. Workers reread
current permitted authority. After draining derived state, replay the product projection
journal and validate the replacement index before activation: source enqueue alone cannot
repopulate a missing index when derived values are unchanged. Preserve checkpoints,
projection versions and deletion tombstones. Repeat source traversal if source creation
overlaps it; listing journals independently cover concurrent listings.

Monitor oldest pending work, expired claims, retention deadlines, projection queue lag and
backlog age, failures/DLQ, version/fence correctness and rebuildability. Product projector
acknowledgment means OpenSearch accepted the write or external versioning fenced it as stale.
Search visibility follows the normal refresh cycle; no persisted acknowledgment claims
search visibility. Queries continue during catch-up; repair/replay stalled geography and product
projectors to propagate retirement, applicability and evidence removal.
