# Durable worker runbook

## ProductListing OpenSearch Lambda (#1783)

PostgreSQL is truth. `product-listings` OpenSearch is rebuildable. The source queue is `aura-worker-product-listing-opensearch-<stage>` and its paired DLQ is `aura-worker-product-listing-opensearch-dlq-<stage>`.

The dedicated `product-listing-opensearch-lambda` handles schema-2 compact jobs only. It has a 45s timeout, batch size one, no batching window, and source visibility of 270s (`6 × 45s + 0s`). It has no receipt loop or visibility adjustment. `ReportBatchItemFailures` includes every retry, poison, timeout, panic, cancellation, or acceptance-unknown record by SQS `messageId`; only applied, deleted, and stale outcomes omit an ID. Future claim/defer work remains a failure until a separately reviewed, bounded SQS timing policy exists.

Schema 1, malformed, oversized, wrong-scope/type, noncanonical-ID, forged-key, or invalid-version jobs are poison and stay on the source queue. The source redrives after five receives. Standard SQS preserves the original enqueue timestamp when transferring to its DLQ: source7d plus DLQ14d is not a fresh 21-day recovery window.

The Lambda's source DLQ is distinct from a Lambda asynchronous-invocation DLQ. This Lambda has no async-invocation DLQ because SQS polling owns retry and redrive. Runtime roles have no purge, receive-from-DLQ, or redrive permissions.

## Mapping pause and recovery

1. Pause the ProductListing OpenSearch **event-source mapping** before a deployment or fenced rebuild; do not rename source or DLQ constructs.
2. Record source/DLQ approximate age and count, mapping UUID/state, Lambda version, deployed `CommitSHA`, and projection source versions. Do not treat CDK synth as live evidence.
3. Drain or retain observed queue/DLQ evidence under approved operator ownership. Never purge.
4. Deploy mappings and all readers before tombstone writers. Fence old writers, including in-flight remote writes.
5. Re-enable only the named mapping. Confirm the Lambda points at the intended function version and source queue.
6. Repair the cause before an operator with separate redrive rights invokes source-DLQ redrive. Recheck retention before moving any message.

`npm --prefix infra run prove:product-listing-opensearch-lambda` is an opt-in isolated-AWS smoke helper. It rejects production, requires an explicitly matched account and empty named source/DLQ pair, verifies the pair's redrive policy, sends supplied compact fixture jobs, waits for the expected secured OpenSearch document and poison-DLQ arrival (30 minutes by default), then can request a separately approved native redrive. It is never CI proof and must not be run against shared queues.

## Projection fences and rebuild

Withdrawals write a content-free, external-versioned `projectionDeleted: true` document. Never physically delete or expire that fence. Rebuild from authoritative PostgreSQL into a fenced fresh target, include withdrawn source versions, catch up committed work, verify document IDs/versions/fences, then cut over. A target index is never a recovery source.
