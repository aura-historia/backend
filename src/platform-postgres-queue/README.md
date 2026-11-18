# PostgreSQL job queue

`platform-postgres-queue` is the durable job queue that replaces the SQS worker queues, the
ProductListing ingestion and marketing-consent FIFO queues and their dead-letter queues. It
keeps the delivery and custody rules of [architecture §12.2–12.4](../../docs/arch.md#122-routing-acknowledgment-and-durable-custody);
the [worker runbook](../../docs/durable-worker-runbook.md) owns operator procedures. The
[migration](../../migrations/20261117130000_job_queue.sql) owns the schema.

## Scopes

A scope is one logical queue with a name, an ordering (standard or FIFO), a lease, a retry
budget and a retry delay. [`platform-worker-queue-contract::job_queue`](../platform-worker-queue-contract/src/job_queue.rs)
catalogues the production scopes: each keeps its SQS queue's name, ordering, visibility timeout
as the lease and `maxReceiveCount` as the attempt budget. A failed SQS attempt became visible
again when its visibility timeout passed, so the retry delay equals the lease.

## Delivery

Delivery is at least once. A claim leases a batch of claimable jobs with
`FOR UPDATE SKIP LOCKED`, counts an attempt and issues a new lease token for each job, so
concurrent claimers never lease the same job. Only the current lease settles a job:

| Disposition | Effect |
| --- | --- |
| Complete (also a deterministic business rejection) | The job is deleted. |
| Retry | The job is released after the scope's retry delay. With its budget spent it moves to custody. |
| Park (can never succeed as delivered) | A first delivery is released like a retry, unless its budget is spent; a redelivered job moves to custody at once. |

A worker that stops before a job finishes releases it: the job is claimable again at once and
the interrupted attempt is not counted, except that a claimed job keeps at least one counted
attempt so it never re-enters deduplication. Repeated releases therefore never spend its budget.

A settlement with a superseded token, or a repeated one, reports a stale lease and changes
nothing. A job whose lease expires unsettled becomes claimable again; if that was its last
allowed attempt, the next claim moves it to custody instead. Handlers must still tolerate
duplicates: a lost acknowledgment or an expired lease redelivers work that may have completed.

Claims pin their plans to ordered index scans, because statistics of a table that swings between
empty and a large backlog are routinely stale.

## FIFO scopes

Every job of a FIFO scope carries a group key. Within a group only the oldest job is claimable,
and only while no job of the group is leased; groups do not block each other. Claims of one FIFO
scope are serialized, so no group is ever leased twice. Enqueues of one group are serialized
too: each holds its group's transaction lock until the enqueueing transaction commits, so a
group's jobs become visible in enqueue order and a later job never runs ahead of an earlier one
that was still uncommitted. A transaction that enqueues into several groups should take them in
a stable order.

A FIFO claim walks the scope in enqueue order while holding the scope's claim lock, so its cost
grows with the jobs queued behind blocked heads. Keep groups narrow, as the per-listing and
per-user keys are; a few wide groups with a deep backlog slow every claim of their scope.

## Deduplication

An optional dedup key is unique per scope among jobs no worker has claimed yet; enqueue is
insert-or-nothing. A claimed job leaves that set, so a retry never collides with a newer copy.

## Enqueue and wake-up

`enqueue` accepts a transaction's connection, making the job commit or roll back with the
caller's writes, or a pool. Job bodies are 1 byte to 1 MiB. The `product-listing-ingestion` scope
is filled only through the `SECURITY DEFINER` function
`job_queue_enqueue_product_listing_ingestion` ([migration](../../migrations/20261117140000_product_listing_ingestion_admission.sql)),
which takes the same group lock, so a producer role needs no table privilege
([admission](../../docs/product-listing.md#asynchronous-ingestion-submission)). A statement trigger notifies the channel `job_queue:<scope>` once
per committing transaction. PostgreSQL serializes the commits of transactions that notify, which
bounds enqueue throughput across all scopes. `JobWakeups` listens on a worker's scopes; workers also poll, since
retries, expired leases and unblocked FIFO groups become claimable without a notification and a
reconnecting listener can miss one.

## Failure custody and operator functions

A job whose retry budget is spent, or that its consumer parked, moves unchanged (ID, enqueue
sequence and time, group, dedup key, payload) into `job_queue_failures` in the same transaction,
with its attempt count, custody reason, last error class and timestamps. Error classes are
bounded labels, never error text or payload content. Custody is never purged.

- `inspect` reports a scope's waiting depth, leased count, age of the oldest waiting job and
  custody count, reading only that scope's rows. It is the source the queue metrics publish.
- `redrive` moves selected or all custody jobs of a scope back with their identity and order
  and a fresh retry budget. A redriven FIFO job runs before every later job of its group. A job
  whose ID or dedup key is already queued stays in custody and is reported as conflicting.

## Churn

Every claim, retry and acknowledgment rewrites or deletes a queue row. The migration sets
per-table autovacuum thresholds in rows instead of a share of the table, without cost-based
throttling, so a mostly empty queue does not accumulate dead tuples.

## Tests

Integration tests under [`tests/`](tests/) run against real PostgreSQL. The ignored
[throughput measurement](tests/throughput.rs) reports enqueue and claim-plus-acknowledge rates.
