# Job queue consumer runtime

`platform-worker` runs the consumers of the [PostgreSQL job queue](../platform-postgres-queue/README.md)
in a long-running process. It owns claiming, concurrency, timeouts, panic containment, wake-ups
and shutdown; each scope's `JobHandler` owns decoding, the use case and which outcomes complete,
retry or park a job.

- **Claiming.** One loop per scope keeps at most `concurrency` jobs in flight and claims as many
  as it has free slots. It waits for a `LISTEN/NOTIFY` wake-up of its scope, a finished job or
  the poll interval; polling picks up retries, expired leases and unblocked FIFO groups.
- **Settlement.** The handler's `Disposition` settles the job: `Complete` acknowledges it,
  `Retry` releases it after the scope's retry delay and `Park` moves a redelivered job to
  custody. Handlers see a `Job` without its lease, so only the runtime settles.
- **Timeout.** Each attempt runs at most the consumer's job timeout, which must end before the
  scope's lease; a timed-out attempt is cancelled and retried with error class `timed_out`.
- **Panics.** Each attempt runs in its own task; a panic is retried with error class `panicked`.
- **Shutdown.** On shutdown a consumer stops claiming and lets in-flight jobs finish within the
  shutdown grace. It then cancels the rest and releases their leases, so another worker claims
  them at once without counting the interrupted attempt.
- **Failures.** A failed claim backs off for a second. A failed settlement is logged and the job
  is redelivered when its lease expires; a superseded lease is logged as stale.

[`tests/consumer.rs`](tests/consumer.rs) proves settlement, panic and timeout containment and
both shutdown paths against real PostgreSQL.
