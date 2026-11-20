//! Figures across every scope at once, for the host's health metrics.

use crate::{error::JobQueueError, scope::ScopeName};
use sqlx::PgConnection;
use std::time::Duration;

/// The queue as a whole: each scope's oldest waiting job and the size of failure custody.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueOverview {
    /// Time since each scope's oldest waiting job was enqueued, for scopes that have one. Waiting
    /// is what [`crate::JobQueue::inspect`] counts as depth: not under an unexpired lease.
    pub oldest_waiting: Vec<(ScopeName, Duration)>,
    /// Jobs in failure custody, across every scope.
    pub in_custody: u64,
}

/// Reads every scope, including scopes no catalogue names, such as isolated projection
/// generations. It scans the whole job table: a caller expecting a large backlog runs it in a
/// transaction with a statement bound of its own.
pub async fn overview(connection: &mut PgConnection) -> Result<QueueOverview, JobQueueError> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT scope, extract(epoch FROM now() - min(enqueued_at))::float8 \
         FROM job_queue_jobs \
         WHERE NOT coalesce(leased_until > now(), false) \
         GROUP BY scope",
    )
    .fetch_all(&mut *connection)
    .await?;
    let in_custody: i64 = sqlx::query_scalar("SELECT count(*) FROM job_queue_failures")
        .fetch_one(&mut *connection)
        .await?;
    let oldest_waiting = rows
        .into_iter()
        .map(|(scope, seconds)| {
            let scope = ScopeName::new(scope)
                .map_err(|_| JobQueueError::InvalidPersistedState("invalid job scope name"))?;
            let age = Duration::try_from_secs_f64(seconds.max(0.0)).unwrap_or_default();
            Ok((scope, age))
        })
        .collect::<Result<_, JobQueueError>>()?;
    Ok(QueueOverview {
        oldest_waiting,
        in_custody: u64::try_from(in_custody)
            .map_err(|_| JobQueueError::InvalidPersistedState("negative custody count"))?,
    })
}
