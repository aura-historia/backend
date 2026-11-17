//! Enqueue, inside a caller's transaction or on its own.

use crate::{
    error::JobQueueError,
    job::{JobId, NewJob},
};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Enqueued(JobId),
    /// An unclaimed job of the same scope already carries the job's dedup key; nothing was
    /// inserted.
    Deduplicated,
}

const INSERT: &str = "INSERT INTO job_queue_jobs (scope, group_key, dedup_key, payload) \
     SELECT $1, $2, $3, $4";
const ON_CONFLICT: &str = "ON CONFLICT (scope, dedup_key) \
     WHERE dedup_key IS NOT NULL AND attempts = 0 \
     DO NOTHING \
     RETURNING job_id";

/// Inserts the job, or nothing when its dedup key is already taken. Pass a transaction's
/// connection to make the job commit or roll back with the caller's writes, or a pool to
/// enqueue on its own. Listening workers are notified when the insert commits.
///
/// A FIFO job first takes its group's transaction-scoped lock, so enqueues of one group commit
/// in the order of their sequence: a job never becomes visible behind a later job of its group
/// that a worker may already have attempted. The lock is held until the caller commits, so a
/// transaction that enqueues into several groups should do so in a stable order.
pub async fn enqueue<'c>(
    executor: impl PgExecutor<'c>,
    job: &NewJob,
) -> Result<EnqueueOutcome, JobQueueError> {
    let statement = match job.group_key() {
        None => format!("{INSERT} {ON_CONFLICT}"),
        Some(_) => format!(
            "{INSERT} FROM (SELECT pg_advisory_xact_lock( \
                 hashtextextended('job_queue:' || $1 || ':' || $2, 0))) AS serialized \
             {ON_CONFLICT}"
        ),
    };
    let inserted: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(statement))
        .bind(job.scope().as_str())
        .bind(job.group_key().map(|key| key.as_str()))
        .bind(job.dedup_key().map(|key| key.as_str()))
        .bind(job.payload())
        .fetch_optional(executor)
        .await?;
    Ok(match inserted {
        Some(id) => EnqueueOutcome::Enqueued(JobId::from_uuid(id)),
        None => EnqueueOutcome::Deduplicated,
    })
}
