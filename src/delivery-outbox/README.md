# Delivery outbox

`delivery-outbox` carries committed inserts of the selected source tables into the
[job queue](../platform-postgres-queue/README.md) without logical replication.
[Event flow](../../docs/events/flow.md#outbox-routing) owns the
selected sources and routes, the [worker runbook](../../docs/durable-worker-runbook.md#delivery-outbox-recovery)
owns recovery, and the [migration](../../migrations/20261118090000_delivery_outbox.sql) owns
the schema.

## Capture

An `AFTER INSERT … FOR EACH ROW` trigger on every selected source appends one row to
`delivery_outbox` in the writer's own transaction: the source table, snapshot version 1 and a
JSONB snapshot of exactly the columns that source's route reads. A rolled-back write captures
nothing. Updates, deletes and other tables capture nothing. Committing a capture notifies the
channel `delivery_outbox`.

## Dispatch

`OutboxDispatcher::dispatch_batch` runs one transaction:

1. take the dispatch lock (`Busy` if another dispatch transaction holds it);
2. claim the oldest snapshots with `FOR UPDATE SKIP LOCKED`, in outbox ID order;
3. decode each with its source's strict [codec](src/snapshot.rs) and route it with
   `aura-historia-cdc-routing`, building and validating all of its jobs;
4. enqueue every job; a FIFO job keeps its ordering key as group key and its idempotency key
   as dedup key;
5. move each snapshot that cannot be decoded, routed or encoded into failure custody;
6. delete the claimed snapshots and commit.

Publication is never partial: a crash or error before commit leaves every snapshot of the batch
to be dispatched again with the same effect, and the jobs and the deletion commit together.
`dispatch_within` runs the same steps inside a caller's transaction.

`OutboxWakeups` listens for captures and redrives; the dispatcher also polls, since a
reconnecting listener can miss a notification. Every transaction that writes a selected source
therefore notifies on commit, and PostgreSQL serializes the commits of notifying transactions.

## Ordering

One dispatch transaction runs at a time, across all processes, and it enqueues in outbox ID
order. Consent writers serialize per recipient on the recipient lock before inserting an
intent, so a recipient's snapshots receive increasing outbox IDs in commit order, and the FIFO
group receives them in that order. No other source relies on order.

## Failure custody

A snapshot whose source, version, columns or values fail validation, or whose jobs cannot be
encoded, moves unchanged into `delivery_outbox_failures` in the dispatch transaction, with its
outbox ID, enqueue time and a bounded error class such as `invalid_snapshot`,
`unsupported_source` or `invalid_identity`. It never blocks later snapshots and is never
purged. `inspect` reports pending count, oldest pending age and custody count; `redrive` moves
selected or all custody rows back under their original outbox ID.

## Reprojection

`reproject_all` enqueues one projector job per existing listing, active or withdrawn, in keyset
pages by listing ID, each page in its own transaction. Each job names the listing's current
event, so the projector writes only current state and turns withdrawn listings into their
versioned tombstones. Jobs are deduplicated by event against jobs not yet claimed, so a rerun
skips listings whose earlier job is still waiting and enqueues every other listing again; the
projector completes those that are already current as stale. `ProjectionTarget::Live` enqueues
into the live projector scope. `ProjectionTarget::Generation` enqueues into that generation's
own scope, which the worker runtime consumes with a projector that writes only the generation's
index. An interrupted run resumes after the last listing
it reported.

## Tests

Integration tests under [`tests/`](tests/) run against real PostgreSQL: capture and dispatch of
every source, custody, a crash before commit, consent order, the dispatch lock, custody
operations, wake-ups and reprojection.
