# ProductListing raw-normalization runbook

## Ownership and safety boundary

`product-listing-normalization-lambda` consumes only the `product-listing-normalization` Standard SQS queue. The compact schema-2 wake-up contains `prs_` stream ID, `prr_` revision ID, and a positive revision number; raw source evidence remains in PostgreSQL and must not be copied into logs, queues, or incident tickets.

PostgreSQL is authoritative. `NormalizeProductListingRawRevisionUseCase` locks the durable stream head and processes only its immediate next revision. In one transaction it writes zero or one canonical ProductListing state/event, one terminal raw-normalization row, and head progress. SQS delivery order, Lambda execution order, and message IDs are not ordering or idempotency authorities.

A clean stream drain is acknowledged. A terminal candidate-data rejection (`REJECTED`) is a clean drain because its progress is durable. A capped drain, active/competing ownership, persistence/configuration/dependency failure, timeout, panic, cancellation, malformed job, or unknown commit outcome is returned as `batchItemFailures` and remains eligible for SQS retry and redrive. Never purge the source queue or DLQ to clear a backlog.

## Runtime configuration

| Setting | Value | Rationale |
| --- | --- | --- |
| Lambda timeout | 45 seconds | Bounded one-record attempt, including credential refresh and PostgreSQL composition. |
| Invocation response headroom | 5 seconds | The handler stops work before Lambda's deadline and can truthfully return partial-batch failures. |
| Source queue visibility | 270 seconds | Six times the 45-second bounded Lambda timeout; this replaces the native daemon's 300-second drain/heartbeat setting. |
| Batch size | 10 | Records are processed sequentially under the shared invocation budget. `ReportBatchItemFailures` acknowledges only fully drained records and retains each capped or failed record by message ID. |
| Source retention / DLQ retention | 7 / 14 days | Standard worker catalog contract. DLQ transfer preserves original source-message age. |
| Redrive | 5 receives | Retryable records remain visible to operators without grant to purge or mutate the DLQ. |

The Lambda has private PostgreSQL access and only the runtime PostgreSQL credential secret plus source-queue consumer actions. It has no OpenSearch, provider, worker-daemon, receipt-heartbeat, reserved concurrency, provisioned concurrency, provisioned polling, or event-source `MaximumConcurrency` configuration.

## Activation and rollback

1. Keep `ProductListingNormalizationConsumerEnabled=false` until the native normalizer is paused, active work has settled, the source queue is confirmed to contain compatible schema-2 jobs, and the scheduled authoritative reconciliation rollout gate is complete.
2. Deploy the dedicated function/version and mapping. Enable only the mapping with an approved CloudFormation change.
3. Do **not** run the native normalizer and Lambda mapping simultaneously in production. Competing Lambda invocations are safe because PostgreSQL owns stream locking; a native/Lambda overlap is not an approved handoff strategy.
4. To return to native consumption, first disable the Lambda mapping, wait for active invocations and visibility leases to settle, then start the compatible native consumer. Do not rename, replace, purge, or recreate either queue.

Scheduler wiring is a separate rollout task. Before the permanent native reconciliation timer is retired, Scheduler must invoke the same bounded `Reconcile` / `ReconcileFromCursor` service modes against PostgreSQL. The cursor is reconstructible; no Lambda warm-process cursor or FIFO is required for correctness.

## Investigation

Start with the source-age, DLQ-visible, and Lambda-errors alarms. Inspect structured logs by `request_id`, stable outcome category, message ID, stream ID where safely available, and counts—never raw values or provider payloads.

Useful PostgreSQL checks in an approved, read-only operational session:

```sql
-- Oldest pending stream age and count.
SELECT min(revision.captured_at) AS oldest_pending_at, count(DISTINCT revision.product_listing_raw_stream_id) AS pending_streams
FROM product_listing_raw_revisions AS revision
LEFT JOIN product_listing_raw_normalization_heads AS head
  ON head.product_listing_raw_stream_id = revision.product_listing_raw_stream_id
WHERE revision.revision > coalesce(head.last_processed_revision, 0);

-- Terminal outcome distribution; REJECTED is durable candidate-data terminal work.
SELECT outcome, count(*)
FROM product_listing_raw_normalizations
GROUP BY outcome
ORDER BY outcome;
```

For a repeated candidate rejection, inspect the bounded `error_code` and correct source mapping/input only after preserving the immutable evidence in PostgreSQL. For configuration, schema, database, or credential failure, correct the dependency first; progress must remain pending. After correction, allow normal source retry or perform an approved, scoped redrive. Do not infer success from one attempted revision: verify the stream head advanced and the expected zero-or-one canonical event/state mutation is committed.

## Required evidence before broad activation

- Capture a raw revision, observe its compact CDC wake-up, then verify its terminal progress and expected canonical state/event.
- Demonstrate duplicate and reversed wake-ups, and competing invocations, preserve head revision order and do not duplicate canonical mutations.
- Demonstrate a capped stream retries in a cold invocation and completes without process-local state.
- Exercise pre-commit failure and post-commit/before-ack redelivery with PostgreSQL barriers.
- Deliberately omit a wake-up and prove the bounded authoritative reconciliation entrypoint finds it after scheduled wiring is available.
- Measure invocation duration, queue age, retry/redrive counts, database connections/locks, and any normalization-provider cost before increasing activation scope.
