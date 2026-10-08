# Durable CDC and worker operations

**Target, not verified live state.** PostgreSQL is business truth; the intended path is DMS CDC → seven-day Kinesis → router Lambda → ten scoped Standard pairs plus one marketing-consent FIFO SQS source/DLQ pair → workers. `CdcRouterEnabled` is separate from compute creation (which activates worker mappings). [Architecture §12](arch.md#12-cdc-and-projection-architecture) owns invariants, [event flow](events/flow.md) owns routing, and [infrastructure](../infra/README.md#first-time-stage) owns deployment order. Applying the data stack carries declared DMS table-selection changes into task configuration, subject to DMS update constraints and task state; it does not start or restart DMS. Approval and coordination before that deployment, first-start/restart handling, slot/LSN and capture evidence, and external source decommissioning require an operator plan. Code and CDK do not prove live activation.

## Marketing consent evidence logs

The API and Cognito PostConfirmation evidence groups are `/aws/lambda/aura-historia-api-<stage>` and `/aws/lambda/cognito-post-confirmation-<stage>`. In real stages they have no automatic expiry and `Retain` removal/replacement policies. The retention Lambda skips only these exact stage names; its 30-day policy remains active for other new log groups. These logs are secondary evidence, not an audit database: commit and CloudWatch delivery are not atomic, so gaps and duplicates are possible, and CloudWatch records are mutable.

Use the existing account/stage privacy-operations access path for a documented support, security or data-subject request. The evidence groups contain pseudonymous recipient fingerprints; never query or export by raw email, export a complete group, or use the fingerprint as a metric dimension. Business Lambdas have no log read/delete permissions. The retention Lambda has only `logs:DescribeLogGroups` and `logs:PutRetentionPolicy`; this change adds no log administration to business functions. Where operator read/delete access is needed, keep it in the existing external operator boundary and scope it to the named evidence groups.

CloudWatch Logs Insights exposes the JSON event fields under `fields`. This query returns the defined evidence context without selecting the full message:

```text
fields @timestamp, fields.consent_effective_at_utc, fields.consent_recorded_at_utc,
  fields.consent_purpose, fields.consent_source, fields.consent_action,
  fields.subject_kind, fields.user_id, fields.recipient_fingerprint,
  fields.previous_consent, fields.current_consent, fields.consent_decision_id,
  fields.consent_revision, fields.consent_wording_reference,
  fields.consent_wording_locale,
  fields.request_id, fields.correlation_id
| filter fields.event = "marketing_consent.evidence.v1"
| sort fields.consent_recorded_at_utc desc
```

For a single approved subject lookup, add an exact `fields.user_id` filter when a registered User ID is available, or an exact `fields.recipient_fingerprint` filter for email-only evidence. Confirm stage, group, time window and identity before retrieval; retain only the minimum records required for the case and record the operator, purpose, query window and access time. The fingerprint is personal/pseudonymous data, not anonymous data.

Review access and retention regularly under the organization's privacy and security process. No-expiry configuration is not a claim that unlimited retention is legally required, nor does it remove data-minimization or erasure duties. CloudWatch deletion is at log-stream or log-group granularity, not a single event: after an authorized erasure decision, identify the containing stream, assess unrelated records that would be removed, and record any collateral impact. Use the approved scoped operator role for [`DeleteLogStream`](https://docs.aws.amazon.com/AmazonCloudWatchLogs/latest/APIReference/API_DeleteLogStream.html); whole-group deletion via [`DeleteLogGroup`](https://docs.aws.amazon.com/AmazonCloudWatchLogs/latest/APIReference/API_DeleteLogGroup.html) additionally requires coordination with the retained CloudFormation resource and future logging. Do not remove or recreate groups as routine retention management.

## Activation and legacy handoff

1. Record stage/account/release, approvals, identities and schema/job compatibility. Follow the [first-time stage path](../infra/README.md#first-time-stage): Deploy foundation → manual Migrate Postgres → operator OpenSearch setup if needed → FX-only Initialize → same-ref Deploy `scope=all`. Verify migrations, FX, OpenSearch, native-consumer handoff and SES/provider consent **before** `all` creates active mappings. There is no readiness marker; Migrate/Initialize neither deploy compute nor start DMS. Do not invent an LSN or restart with a new first-start point.
2. Inventory external Sequin subscriptions/processes, in-flight requests, legacy in-memory jobs/DLQs and work outside SQS; preserve evidence and settle or account for it. Under approval stop old delivery/consumers before replacement activation; never run two consumers of one queue or disable required fanout merely for a successful probe. Source removal does not drain work or decommission subscriptions, slots or secrets.
3. Confirm compatibility/backlog before compute creation; fence existing mappings when required. Separately approve DMS first start with the actual slot/LSN, capture real table/DELETE/control records, enable the router at `TRIM_HORIZON`, and verify checkpoints/archive delivery. Preserve only safe IDs, counts, lag and timestamps in evidence—not records, archive bodies, credentials, recipients or provider data.
4. Decommission old subscriptions, processes, slots and credentials only under an approved live plan proving ownership, stopped delivery, drained work, replay windows and rollback posture. Native-consumer rollback needs an approved compatible artifact, disabled Lambda mapping and settled work; removed source code is not a runnable fallback.

## Failure custody and controlled redrive

The router validates each whole-record route and confirms every required SQS send before checkpoint; on failure it returns the earliest unconfirmed sequence and stops. Partial/lost sends may duplicate jobs. After three retries or one hour the **original invocation** is sent to private retained S3 (90-day object expiry), **not** a worker DLQ, and is not replayed automatically. Check mapping-specific archive-delivered/dropped and destination-failure signals; failed delivery does not prove an object exists. Preserve source ARN, sequence and envelope. Repair the cause, use the fail-closed decoder, test isolated replay and obtain separate approval; never silently skip poison CDC.

Worker source queues retain seven days and paired DLQs 14; transfer preserves original enqueue age, so these windows are **not additive**. The consent FIFO pair moves messages after five receives; neither it nor Standard worker DLQs redrives itself indefinitely. Workers acknowledge only completed service outcomes; failures, malformed jobs, timeout, panic and uncertain effects retry with `ReportBatchItemFailures`. Normalization batches ten; other worker scopes batch one. Distinguish router S3, worker SQS DLQs, provider ingress DLQs and Lambda async DLQs. Monitor DMS lag/slot WAL, router iterator age/archive failures, queue oldest age/depth/DLQs and Lambda errors. For a small authorized redrive, inspect domain keys and PostgreSQL state, retention and duplicate effects; repair first and use operator access. Runtime roles cannot purge/replay/redrive; never purge to clear alarms.

## Marketing consent FIFO custody

Deploy the applied consent schema and selected-release `marketing-consent-sync-lambda`
before creating its active batch-one mapping. Its source and DLQ are retained in
both real stages; the router mapping stays disabled until separately approved.
The data-stack deployment delivers the declared DMS source-selection change to
the task configuration, subject to DMS update constraints and task state; it
does not start or restart DMS. Approve and coordinate that deployment, and keep
first start or any required restart as operator actions. The source retries five receives and then
stops in its DLQ; router failures can stop in the retained S3 archive after the
mapping retry/age limits. Neither location retries forever or replays itself.
Before a controlled replay, inspect the intent's PostgreSQL status, source/DLQ
age and Loops result, repair the cause and obtain operator approval. Preserve the
message and FIFO identifiers; do not infer provider acceptance from a timeout or
queue transfer. Keep API writer cutover for its later task.

### Consent workflow maintenance and diagnosis

The existing cleanup Lambda also runs one bounded batch per User-owned target:
unconfirmed DOI challenges after their 24-hour issuance window, confirmed DOI
challenges seven days after confirmation and after issuance expiry, completed
`APPLIED`/`SUPERSEDED` sync receipts after 120 days, and processed Loops webhook
receipts after at least 120 days. Each target commits separately, uses `SKIP
LOCKED`, and deletes at most the configured batch size (1–1000). A failed target
fails the invocation; earlier committed batches are safe to retry. Older webhook
receipts with a 35-day stored expiry remain until 120 days after processing.
Permanent minimal markers for committed positive Loops deliveries survive this
cleanup; never remove them to force a replay. They contain only delivery ID and
body digest, and protect ignored as well as applied positives.
These windows cover the declared 90-day router archive, seven-day Kinesis/source
queues and 14-day worker DLQ with recovery margin. They are operational
deduplication windows, not legal consent history. `PENDING`, `IN_PROGRESS`,
`BLOCKED` and `FAILED` intents, including provider-withdrawal race candidates and
unfinished `PROVIDER_RACE_REPAIR` revokes, are never housekeeping targets.

Use a read-only transaction and a fixed inspection time when sampling the
oldest work. These queries return opaque intent IDs and bounded rows; they
exclude email, source keys, provider IDs, payloads and lease tokens.
`transaction_timestamp()` fixes the inspection time for a transaction; keep
the limits. The first sample
shows pending work and leases, including stale ones:

```sql
SELECT intent_id, status, source, desired, changed_at, not_after,
       lease_expires_at, attempt_count
FROM marketing_email_consent_sync_intents
WHERE status IN ('PENDING', 'IN_PROGRESS')
ORDER BY intent_sequence
LIMIT 100;
```

Inspect blocked/failed work, provider-withdrawal race candidates, unfinished
repair revokes and expired grants awaiting settlement separately:

```sql
SELECT intent_id, status, source, desired, changed_at, not_after, attempt_count
FROM marketing_email_consent_sync_intents
WHERE status IN ('BLOCKED', 'FAILED')
ORDER BY status, intent_sequence
LIMIT 100;

SELECT intent_id, status, changed_at, attempt_count
FROM marketing_email_consent_sync_intents
WHERE status = 'BLOCKED'
  AND last_error_code = 'PROVIDER_WITHDRAWAL_RACE_CANDIDATE'
ORDER BY intent_sequence
LIMIT 100;

SELECT intent_id, status, changed_at, attempt_count
FROM marketing_email_consent_sync_intents
WHERE source = 'PROVIDER_RACE_REPAIR'
  AND status NOT IN ('APPLIED', 'SUPERSEDED')
ORDER BY intent_sequence
LIMIT 100;

SELECT intent_id, status, not_after, attempt_count
FROM marketing_email_consent_sync_intents
WHERE desired AND status IN ('PENDING', 'IN_PROGRESS', 'BLOCKED', 'FAILED')
  AND not_after <= transaction_timestamp()
ORDER BY not_after, intent_id
LIMIT 100;
```

For backlog, count only a bounded oldest sample and label it as a sample, not
the global total. Review source/DLQ age and visible count, router lag/archive
delivery, and cleanup outcome/count logs alongside it:

```sql
WITH oldest AS (
  SELECT status FROM marketing_email_consent_sync_intents
  WHERE status IN ('PENDING', 'IN_PROGRESS')
  ORDER BY intent_sequence LIMIT 1000
)
SELECT status, count(*) AS sampled_count FROM oldest GROUP BY status;
```

For webhooks, sample recent committed dispositions without exposing receipt
identifiers; failures before commit have no receipt and require a time-bounded
review of API 4xx/5xx, Loops pending/retrying/failed history, and endpoint
disablement. A zero receipt count does not prove healthy delivery:

```sql
SELECT disposition, count(*) AS sampled_count
FROM (
  SELECT disposition FROM loops_webhook_receipts
  WHERE processed_at >= transaction_timestamp() - interval '24 hours'
  ORDER BY processed_at DESC LIMIT 1000
) AS recent
GROUP BY disposition;
```

Before any small operator-authorized replay, reread the exact PostgreSQL intent,
current User decision/revision and mailbox ownership, provider state and repair
status. A missing intent cannot be manufactured from an archive/DLQ message;
an `APPLIED` receipt cannot be reset to force another send. Do not replay an
expired or revoked grant. A `BLOCKED` original grant can still need the guarded
repair-before-ACK path, so resolve abandoned work through the service state
machine before considering it settled. S3/Kinesis archives and SQS DLQs do not
retry indefinitely. Never use a periodic scan of
`users.marketing_email_consent=true` to resubscribe contacts. The User boolean is
only the local decision: provider opt-outs/suppression and send-time freshness
still gate delivery. This work does not authorize an independent SES marketing
sender; Loops remains the final delivery gate, and lost/disabled webhook
recovery needs review before any independent sender is enabled.

Raw-revision inserts only wake `product-listing-normalization-lambda`: it drains PostgreSQL stream heads and atomically persists terminal progress with zero or one canonical write/event. Unfinished/capped drains fail for retry. **No scheduled raw-backlog reconciliation exists.** Inspect pending heads, queue age/DLQ and CDC gaps; missing/expired wake-ups need an approved recovery plan, not fabricated completion. Crawler capture success is not normalization completion.

## ProductListing ingestion FIFO command recovery

The separate `product-listing-ingestion-queue-${stage}.fifo` (seven days) and `product-listing-ingestion-dlq-${stage}.fifo` (14 days) are command ingress, not CDC or Shopify upstream queues. The 45-second consumer receives up to ten without a batching window; it validates SQS IDs before work, then stops on the first incomplete record and returns that ID **and all unprocessed successors**, even across groups, via partial batch response. Only a confirmed completed prefix is acknowledged; matching durable `APPLIED` receipts protect a completed command if the response is lost. Malformed/unsupported commands, rejection, conflicts, setup failure, timeout, panic and uncertain commits retry (five receives); an incomplete group blocks successors. Partial failures need not increment Lambda `Errors`. See [execution contract](events/flow.md#v1-ingestion-command-execution-installed-consumer).

Before producers, apply the receipt migration, deploy source/DLQ and matching consumer artifact, and verify the published-version mapping. Before the USER/DELEGATED_USER fingerprint-scope correction, inspect source, DLQ and receipts for accepted v1 delegated-user commands; plan a compatibility cutover if any exist. Pause via the ingestion mapping **with a seven-day retention plan**, not by discarding messages. Watch source age/backlog, DLQ, throttles/errors and `CompletedRecords`/`FailedRecords`/`UnprocessedRecords`. Log only bounded outcome/error code, duration and log-safe SQS ID; decoded commands may include safe command/submission/source/index and ASCII trace IDs (letters/digits/hyphen/underscore, ≤128 bytes). Unsafe SQS IDs log as `invalid` but remain unchanged in batch responses; unsafe trace IDs log empty. Missing bodies/setup failures have no command correlation; unprocessed successors are not decoded. Interpret `RECEIPT_*`, verb-specific, `FINGERPRINT_CONFLICT` and `COMMIT_UNCONFIRMED` codes against receipts and current PostgreSQL state, never as proof of rollback or a safe skip. Preserve safe IDs and receipts; repair before small approved redrive. Never blindly redrive, acknowledge/delete failures, purge or infer exactly-once from FIFO. [Ingress settings](../infra/README.md#productlisting-command-ingress-1859).

## ProductListing ingestion rollout and acceptance gate

**Open until recorded:** tests, source, OpenAPI and CDK synth do not prove IAM, NAT-to-SQS, database access, mappings or DLQ transfer. Record approved stage/account, release SHA, redacted results and omissions; no production data in smoke.

1. **Prepare:** verify codec/publisher/service and transaction-free public ingress APIs; `product-listing-service::canonical_product_listing_write` is still a public cross-crate transaction-aware export for normalization—resolve or explicitly approve this exception before declaring private mechanics complete. Run and record route/provider/consumer/PostgreSQL/infra tests. Apply receipt migration before consumer writes; retain Shopify Standard input/DLQ, raw-normalization and crawler paths. There is no submission table or polling endpoint.
2. **Stage before publication:** review retained queue identity and migration order; deploy FIFO source/DLQ, stage network/IAM/alarms and matching consumer version/mapping. Verify batch ten, partial failures, bounded concurrency/DB connections and stage-local source-only grants. Prove controlled consumption before publishing; enable async HTTP routes only afterward, then cut over Shopify and WooCommerce one at a time, fencing immediate capture for the same observation. Preserve Shopify upstream mapping/backlog; coordinate pauses with source retention. Stack order alone is not runtime health.
3. **Deterministic acceptance:** with captured publication, real codec/processor and test PostgreSQL, cover all canonical verbs, both providers and trusted internal ingress; valid REST siblings must have no side effects before manual consumption. Check patch null/omitted/value, original index holes, replay/receipt and fingerprint conflicts, old completed updates, no-send whole-body/auth failures, unchanged synchronous parsers, uncertain publication, group blocking, committed prefix plus failed/unprocessed suffix (including other groups), and setup/wire/timeout/panic/unknown-commit failures. Local tests are not AWS evidence.
4. **Approved ephemeral AWS smoke (unexecuted until recorded):** use disposable fixtures, least privilege, bounded windows and no production data. Through the actual front door check four Partner routes, auth denials, JSON/body/key CORS, partial admission and zero sends on whole-request errors. Check Shopify upstream partial failure; WooCommerce exact-byte signature and bodyless `204` only for confirmed mapped admission or authorized ignored no-op; private FIFO send, PostgreSQL receipt/raw evidence, normalization, real prefix/suffix retry and repairable retry/DLQ transfer. Inspect both Shopify and FIFO DLQs; clean up under approval and record redacted IDs/counts, never payloads/tokens/signatures. Missing credentials, environment or isolated failure fixture means **unexecuted**, not passed.
5. **Budget/decision:** record burst sample/window, admission latency, completed/failed/unprocessed throughput, queue age/backlog and DB connections against approved budgets. Check partial-failure alarms (not necessarily Lambda `Errors`), retention, network/DB capacity and cost. Do not raise concurrency above two or infer an SLA from a sample. Fence the affected producer if a gate fails; rollback requires compatible consumer artifact plus mapping/retention coordination, not reversal of committed effects. Keep remaining gaps in the parent issue checklist.

### HTTP admission and FIFO execution are different partial boundaries

REST reports confirmed **queue custody per original index**, not completion; an unconfirmed send may already be queued. Whole-body/auth/key failures send nothing; evaluated reports, size/status precedence and examples are in [OpenAPI](swagger.yaml) and [HTTP front door](http-api-front-door.md#async-partner-product-listing-routes-18601863). Producers budget SQS sends within the remaining invocation time (publisher capped at ten seconds), reserving response headroom; expired/unconfirmed sends remain uncertain. WooCommerce must not return `204` for an unresolved forward; Shopify retains its upstream record. There is no `Location`, persisted submission or status polling.

Supply a valid `Idempotency-Key` up front for recoverable REST retries; folded duplicate/comma keys are rejected, and a generated key lost with a reply is unrecoverable. `submissionId` is correlation, not batch deduplication. After a lost/uncertain reply retry the **entire unchanged ordered array with the same actor, source and key**, retaining invalid entries, accepted siblings and original indices; fix definite failures separately under a new key. An unresolved same-group send blocks eligible successors; other groups may advance. FIFO consumption instead acknowledges only completed prefix and fails the first incomplete command plus **every** successor across groups. Receipts prevent repeated committed effects, not duplicate enqueue or execution of old unfinished work.

### Controlled command redrive decision

First locate custody: uncertain HTTP admission, Shopify upstream Standard DLQ, FIFO source/DLQ or downstream normalization Standard queue. Preserve safe message/command IDs, group/order/index and receipt; inspect authoritative listing, raw/provider and command receipts plus grants. Repair cause under owner approval. A matching `APPLIED` receipt makes a completed old update replay a no-op; an old **never-completed** update can apply against newer state or fail guards. A mismatched fingerprint fails closed. FIFO order cannot fence synchronous writes, normalization, other groups or manually reintroduced work. Check source-order barriers (especially Shopify timestamp-free `UNKNOWN_DELETE`) and current state before a small ordered isolated redrive within retention; observe completion and normalization separately, then queue/DB alarms. Unknown commits and lost replies may already have effects: never invent success, clear barriers, blindly skip/purge, compact a same-key retry or assume a DLQ record means no other copy ran. Runtime roles have no redrive/purge grant.

## Shopify forwarding handoff

EventBridge → retained Standard `shopify-lambda-queue-${stage}`/`shopify-lambda-dlq-${stage}` → Shopify Lambda remains the provider ingress boundary; it looks up PostgreSQL source/credentials and forwards mapped commands using a stage-local FIFO URL and exact `sqs:SendMessage` grant. Under approval deploy FIFO/receipt migration and matching active consumer first; verify IAM, DB, mapping, age/DLQ alarms and outcomes before enabling Shopify. Review change sets for replacement of **either** retained upstream or FIFO resources; confirm rule/mapping still target upstream. Deployment order/CDK is not live proof; hold cutover and manage upstream backlog/retention if any gate fails.

Track two acknowledgments: EventBridge only delivers to Shopify Standard SQS. Shopify acknowledges eligible raw observations only after confirmed FIFO `Accepted`; rejected/uncertain/not-attempted sends retry upstream (five receives, then Shopify DLQ; four-day source retention). A lost confirmation may already enqueue, so retain identity on retry. FIFO later acknowledges only committed `APPLIED`/matching receipt; failure/uncertainty retries to ingestion DLQ (seven-day source, 14-day DLQ). Ignored provider inputs are separate; neither acknowledgment proves normalization or search visibility. Inspect **both** queue ages/DLQs and PostgreSQL raw/command receipts before small authorized recovery; never blindly move/purge messages or infer rejected delivery. To stop forwarding fence/roll back only Shopify producer under approval; pause shared consumer only with a FIFO retention plan. Never concurrently run synchronous capture and forwarding for the same observation without a compatibility plan. No provider bodies, credentials or raw commands in logs/evidence. [Event flow](events/flow.md#shopify-queue-forwarding-boundaries-runtime-deployment-gated).

## WooCommerce webhook queue handoff

Before forwarding, verify shared FIFO/receipt schema, compatible active consumer, API queue URL and source-ARN-only send grant. Smoke a signed webhook with exact bytes and valid bearer/capability/source grant (consumer rechecks current grant): authorized ignored status returns bodyless `204` without a message or provider receipt; mapped observation returns bodyless `204` **only on confirmed FIFO acceptance**. Unconfirmed/unavailable or oversized sends must not return `204`. Contract/synth is not deployment proof.

After mapped `204`, consumer-side provider receipt, order or raw-capture conflict can still enter the FIFO DLQ, not immediate capture `409`; the provider may not retry. Inspect command IDs, FIFO age/DLQ and PostgreSQL raw/provider/command receipts before controlled recovery. For `503`/`500`/timeout/lost reply, retry with the same signed bytes, source, topic, caller and delivery ID if present: an uncertain send may already be queued. Without delivery ID, a lost reply cannot recover the original command identity or guarantee provider-receipt retry deduplication. Fence only the WooCommerce producer for rollback; do not disable shared consumption or run concurrent synchronous capture for the same observation. Keep payloads, signatures and secrets out of logs/evidence.

## Retained CDC archive resource import recovery

A failed/deleted compute stack may leave replayable invocations in `aura-historia-cdc-router-failures-<stage>`. Ownership repair requires **explicit operator CloudFormation resource import**, not Deploy/Migrate/Initialize. Old `S3_CDC_ROUTER_FAILURE_ARCHIVE_BUCKET_NAME_DEV`/`_PROD` workflow variables are unused. Never add `--import-existing-resources` to normal deploy: broad auto-import risks adopting unrelated resources.

1. Hold deployments; approve and record operator, UTC time, stage/account/region, target `application-<stage>-compute`, supported source SHA and bucket. Inventory stack, pending change sets and activation. A missing stack or empty `REVIEW_IN_PROGRESS` shell is not ready compute; resolve failed CREATE change sets under approval. Never delete, empty or rename the bucket to make deployment pass.
2. Confirm expected account and `eu-central-1`, **no other CloudFormation owner** (inventory and expected-owner checks, not name alone), all public-access blocks, SSE-S3 AES-256, HTTPS-only policy and enabled 90-day expiry. Preserve contents/retention; stop on mismatch or missing evidence. Another owner requires a separate plan, not a stolen import.
3. Synthesize/review the selected compute template without deploying. Verify `AWS::S3::Bucket` logical ID `EventingCdcRouterFailureArchive599BCB3E`, physical name `aura-historia-cdc-router-failures-dev` or `-prod`, properties, `DeletionPolicy: Retain` and `UpdateReplacePolicy: Retain`; stop on mismatch.
4. Prepare an **IMPORT** change set with `ResourcesToImport` for **only** that logical ID/type and `ResourceIdentifier.BucketName` set to the verified bucket. Preserve other resources in an existing stack; for a missing stack use a reviewed import-only template/stack at the exact logical ID, then separately reconcile full compute. No unrelated creations, updates, deletions or automatic adoption.
5. Manually inspect the prepared change set: require exactly one `Action=Import` for the expected ID/type/physical bucket. Reject other imports/additions/modifications/replacements/deletions; review parameters and bucket ownership, policy, encryption, access block and expiry separately (CloudFormation does not prove parity). Record ARN/approval and execute **only** that set.
6. Verify completion, exact ownership, settings and contents with metadata-only checks; inspect drift before normal deploy. Inventory **all** `DELETE_SKIPPED` resources: retained `/aura-historia/<stage>/periodic-matcher` and `.../periodic-matcher-lifecycle` log groups can collide. Recover those separately under a narrow plan; never delete history as a shortcut. Archive-only import is **not readiness**: recheck migration/FX/OpenSearch/native-handoff/SES gates, review full CDK changes separately and complete the operator-controlled full-template **UPDATE** to `UPDATE_COMPLETE` before normal Deploy (`IMPORT_COMPLETE`/archive-only is rejected). Remove obsolete GitHub variables through repo administration. Import neither replays nor purges archived events; replay requires separate approval.

## Projection fences and rebuild

PostgreSQL events/revisions are authoritative; OpenSearch is rebuildable. ProductListing withdrawal and search-filter deletion use content-free, externally versioned `projectionDeleted: true` tombstones: deploy mappings/**all** readers before writers, exclude tombstones from search/ranking/percolation and fence old physical-DELETE writers (including in-flight requests). Never TTL/physically delete tombstones: remote delete-version memory expires after `index.gc_deletes`. Rebuild in an isolated index generation: fence old writers, replay authoritative facts, catch up commits, verify IDs/source versions/deletion visibility, **then** switch alias. Withdrawn listings retain source state; hard-deleted filters require retained deletion facts or a fenced fresh-generation rebuild. Index reset, expired SQS jobs and absent history cannot restore facts.

## Notification recovery limits

`notification_deliveries` owns intent/lease state. Active claims defer until persisted expiry +5s; claim/status races defer 1s, never complete. EMAIL commits the claim before SES; retry finalization with the **same lease token, completion time and provider receipt/error tuple**. Only confirmed terminal finalization acknowledges SQS. SES acceptance with lost finalization can duplicate email after the five-minute lease expires. Preserve safe delivery/attempt IDs and PostgreSQL state; inspect provider evidence under approved access before controlled replay. Never automatically resend historic customer email, log rendered content/recipients or treat provider timeout as definitely unsent.
