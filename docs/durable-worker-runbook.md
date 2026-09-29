# Durable CDC and worker operations

This runbook describes the **post-legacy-source target**, not verified live state. PostgreSQL is business truth; the approved path is DMS CDC → seven-day Kinesis stream → versioned router Lambda → ten scoped Standard SQS source/DLQ pairs → worker Lambdas. `CdcRouterEnabled` is independent of compute creation, which activates the worker mappings; worker handoff prerequisites are operator gates, not per-worker deployment flags. [Migration F7](migration-f7-dms.md) owns DMS slot/LSN, first start, capture, archive evidence, and external source decommissioning; [event flow](events/flow.md) owns per-table routing. Neither a CDK declaration nor removal of native/Sequin source proves that any mapping is enabled, DMS is running, or old external subscriptions have stopped.

## Activation and legacy handoff

1. Obtain stage/account/release and operator approvals, check deployment identities, schema/job compatibility and the [F7 preconditions](migration-f7-dms.md#preconditions-and-approval-record). Follow the [first-time stage path](../infra/README.md#first-time-stage): Deploy foundation, manual Migrate Postgres, operator OpenSearch setup if needed, FX-only Initialize, then Deploy the same ref with `scope=all`. Verify migrations, FX, OpenSearch, native-consumer handoff and SES/provider consent before `all`, which creates active consumers. There is no built-in readiness marker; neither manual operation deploys compute or starts DMS. Never invent an LSN or restart a task with a new first-start point.
2. Inventory old externally operated Sequin subscriptions, process deployments, in-flight requests and any legacy in-memory jobs/DLQs; preserve evidence and settle or explicitly account for work not held by SQS. Code removal cannot recover lost work or decommission Sequin, replication slots, or secrets. Stop old delivery/consumers under approval before activating the replacement; never run two consumers of one queue. Do not turn off required fanout merely to get one successful queue probe.
3. Compute creation enables worker mappings, so confirm source compatibility and retained backlog before that step; separately fence existing mappings when an approved handoff requires it. Follow F7 for the separately approved DMS first start, real table/DELETE/control capture, router enabling at `TRIM_HORIZON` and checkpoint/archival checks. Ordinary Deploy preserves prior router/matcher activation values and never migrates. Record safe identifiers, counts, lag, and timestamps only. Do not log or paste source records, archive bodies, credentials, recipients, or raw provider data.
4. Decommission external subscriptions, old processes, slots and credentials only under a separately approved live plan with proof of ownership, stopped delivery, drained work, retained replay windows and rollback posture. Source-code removal is not evidence that any of these steps happened. A rollback to a native consumer requires a separately approved compatible artifact, disabled Lambda mapping and settled work; do not assume the removed native runtime exists to resume.

## Failure custody and controlled redrive

The Kinesis router prevalidates the whole per-record route and confirms every required SQS send before checkpointing. It returns the earliest unconfirmed sequence and stops; partial sends can duplicate jobs on retry. After three retries or one hour, the **original invocation** goes to the private retained S3 failure archive (90-day object expiry). This is not a worker queue DLQ. Check mapping-specific archive-delivered/dropped and destination-failure signals; an archive delivery failure is not proof of a retained object. Preserve original source ARN, sequence and envelope; repair the cause, use the fail-closed decoder, test replay in isolation and obtain approval before any live replay. Never silently skip poison records.

Scoped SQS source queues retain jobs seven days; paired DLQs retain them 14 days. Standard source-to-DLQ transfer retains the original enqueue age—there is no guaranteed 21-day total window. Each worker acknowledges only a completed service outcome; `ReportBatchItemFailures`, timeout, panic, malformed job, dependency failure and unknown completion leave records for ordinary SQS retry/redrive (five receives). The raw normalization mapping batches ten; other worker scopes batch one. Do not confuse Lambda asynchronous-invocation DLQs, provider ingress DLQs, the router S3 archive, and these worker SQS DLQs. Monitor queue oldest age/depth, DLQ count, router iterator age, DMS lag/slot WAL and Lambda errors. Before a small controlled redrive, identify the failed domain keys and authoritative state, repair root cause, assess duplicate effects and remaining retention; use separately authorized operator access. Runtime roles cannot purge, replay or redrive. Never purge a queue to clear alarms.

Raw revision inserts only wake `product-listing-normalization-lambda`. It reads PostgreSQL stream heads, atomically records terminal normalization progress with zero or one canonical write/event, and returns unfinished/capped drains as SQS failures for retry. It has **no scheduled raw-backlog reconciliation**: inspect persisted pending heads, queue age/DLQ and CDC gaps; a missing/expired wake-up requires an approved recovery plan, not a fabricated successful completion. The pre-removal native normalizer's 30-second reconciliation loop was legacy behavior and is not an activation gate. Crawler R17 local scheduling, review, raw-capture completion and provenance remain separate; a crawler capture success does not mean canonical normalization completed.

## Retained CDC archive resource import recovery

A failed/deleted compute stack can leave
`aura-historia-cdc-router-failures-<stage>` with replayable invocations. Ownership
recovery is an **explicit operator CloudFormation resource import**, not Deploy,
Migrate or Initialize. The old `S3_CDC_ROUTER_FAILURE_ARCHIVE_BUCKET_NAME_DEV` /
`S3_CDC_ROUTER_FAILURE_ARCHIVE_BUCKET_NAME_PROD` workflow variables are no longer
used. Automatic retained-bucket adoption was removed: broad CDK auto-import can
adopt unrelated named resources, and routine release approval does not authorize
ownership repair. Do not add `--import-existing-resources` to normal deployment.

1. Hold stage deployments and obtain a recovery approval recording operator, UTC
   time, stage, account, region, target `application-<stage>-compute`, selected
   supported source SHA and retained bucket. Inventory stack resources, pending
   change sets and activation state. A missing stack or empty `REVIEW_IN_PROGRESS`
   shell is not initialized compute; inspect/resolve the failed CREATE change set
   under approval. Never delete the retained bucket, empty it or rename it to make
   deployment pass.
2. Verify the bucket belongs to the approved deployment account and `eu-central-1`,
   and is **not owned by another CloudFormation stack**. Use expected-owner checks
   and CloudFormation ownership/resource inventory, not name alone. Confirm all
   public-access blocks, SSE-S3 **AES-256** encryption, HTTPS-only access and the
   enabled **90-day object-expiry** lifecycle rule; preserve retention and contents.
   Stop on missing evidence or mismatch. If another stack owns it, resolve that
   ownership through a separate plan, never steal it with an import.
3. Synthesize/review the selected source's compute template without deploying it.
   The expected archive is exactly `AWS::S3::Bucket` logical ID
   **`EventingCdcRouterFailureArchive599BCB3E`**, with physical name
   `aura-historia-cdc-router-failures-dev` or
   `aura-historia-cdc-router-failures-prod` for the selected stage. Verify the
   logical ID and properties against the template; stop if they differ. Preserve
   `DeletionPolicy: Retain` and `UpdateReplacePolicy: Retain`.
4. Prepare a CloudFormation **IMPORT** change set with an explicit
   `ResourcesToImport` entry for only that logical ID/type and
   `ResourceIdentifier.BucketName` equal to the verified stage bucket. For an
   existing stack, retain its other resources unchanged. For a missing stack,
   use a reviewed import-only template/stack for the archive at the exact logical
   ID, then separately reconcile the full compute template after prerequisites.
   Do not combine unrelated resource creation, update or deletion with import.
   Do not use a broad automatic import to discover/adopt all named resources.
5. **Inspect the prepared change set manually before execution.** Require exactly
   one resource change: `Action=Import`, the expected logical ID,
   `ResourceType=AWS::S3::Bucket` and the exact stage bucket physical identity.
   Reject every additional import, addition, modification, replacement or deletion.
   Review parameters, ownership, policy, encryption, public-access block and expiry
   alongside the change set; do not assume CloudFormation proves property parity.
   Record the change-set ARN and approval, then explicitly execute only that set.
6. Verify import completion, exact stack ownership, unchanged bucket safety
   settings and retained contents using metadata-only checks; run drift inspection
   and resolve discrepancies before normal deployment resumes. Inspect **all**
   `DELETE_SKIPPED` resources from failed compute creations before the full-template
   update: the periodic matcher's retained application and lifecycle log groups
   (`/aura-historia/<stage>/periodic-matcher` and
   `/aura-historia/<stage>/periodic-matcher-lifecycle`) can also collide on recreation.
   Recover ownership of any such resources through a separate, narrowly reviewed
   operator plan; the archive-only import neither adopts log groups nor makes
   deleting their history safe. An archive-only stack is **not application
   readiness**. Recheck the full migration/FX/OpenSearch/native-handoff/SES
   prerequisites and review the subsequent full CDK changes separately before
   creating active compute. Complete that full-template **UPDATE** through the
   operator-controlled recovery path and require `UPDATE_COMPLETE` before returning
   to normal Deploy. Deploy deliberately rejects `IMPORT_COMPLETE` and archive-only
   stacks; importing the bucket alone does not complete recovery. Remove obsolete
   GitHub variables through repo administration. Import neither replays nor purges
   archived events; controlled replay remains a separately approved recovery
   operation.

## Projection fences and rebuild

PostgreSQL and its event/revision facts are authoritative; OpenSearch is rebuildable. ProductListing withdrawal and search-filter deletion use content-free, externally versioned `projectionDeleted: true` tombstones. Deploy mappings and **all** readers before writers, exclude tombstones before search/ranking/percolation, and fence old physical-DELETE writers including in-flight requests. Never TTL or physically delete tombstones: remote delete-version memory expires after `index.gc_deletes`. For a rebuild, isolate a new index generation, fence old writers, replay/rebuild from authoritative facts, catch up committed changes, verify IDs/source versions/deletion visibility and only then switch the alias. Withdrawn ProductListings retain state for backfill; hard-deleted search filters need retained deletion facts or an externally fenced fresh-generation rebuild. An index reset, expired SQS job, or missing source history is not a recovery mechanism.

## Notification recovery limits

`notification_deliveries` is authoritative for intent and lease state. Active claims defer until persisted expiry +5s; claim/status races defer 1s, never complete. EMAIL uses SES after committing the claim; finalization retries must reuse the same lease token, completion time and provider receipt/error tuple. Only confirmed terminal finalization acknowledges the SQS job. SES acceptance with a lost finalization response can cause a duplicate after the five-minute lease expires. Preserve safe delivery/attempt IDs and PostgreSQL state; check provider evidence under approved access before any controlled replay. Do **not** automatically resend historic customer email, log rendered content or recipient details, or treat provider timeout as definitely unsent.
