//! Failure custody: jobs that left the queue unfinished, kept unchanged until an operator
//! redrives them.

use crate::{
    error::JobQueueError,
    job::{ErrorClass, JobId},
    scope::ScopeName,
};
use sqlx::PgConnection;
use uuid::Uuid;

/// Why a job is in failure custody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustodyReason {
    /// Every allowed attempt was claimed without a confirmed completion.
    RetryBudgetExhausted,
    /// Its consumer found, on a redelivery, that it can never succeed as delivered.
    Parked,
}

impl CustodyReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RetryBudgetExhausted => "RETRY_BUDGET_EXHAUSTED",
            Self::Parked => "PARKED",
        }
    }
}

/// Moves one job, unchanged, from the queue into custody. Returns false when the job was no
/// longer queued. Runs inside the caller's transaction, which holds the job's row lock.
pub(crate) async fn move_to_custody(
    connection: &mut PgConnection,
    scope: &ScopeName,
    job_id: JobId,
    attempts: i32,
    reason: CustodyReason,
    error_class: &ErrorClass,
) -> Result<bool, JobQueueError> {
    let moved = sqlx::query(
        "WITH moved AS ( \
             DELETE FROM job_queue_jobs WHERE job_id = $1 AND scope = $2 RETURNING * \
         ) \
         INSERT INTO job_queue_failures ( \
             job_id, sequence, scope, group_key, dedup_key, payload, enqueued_at, attempts, \
             reason, last_error_class, last_claimed_at \
         ) \
         SELECT job_id, sequence, scope, group_key, dedup_key, payload, enqueued_at, attempts, \
                $3, $4, last_claimed_at \
         FROM moved",
    )
    .bind(job_id.as_uuid())
    .bind(scope.as_str())
    .bind(reason.as_str())
    .bind(error_class.as_str())
    .execute(connection)
    .await?
    .rows_affected()
        == 1;
    if moved {
        log_custody_transfer(scope, job_id, attempts, reason, error_class.as_str());
    }
    Ok(moved)
}

pub(crate) fn log_custody_transfer(
    scope: &ScopeName,
    job_id: JobId,
    attempts: i32,
    reason: CustodyReason,
    error_class: &str,
) {
    tracing::warn!(
        job_scope = scope.as_str(),
        job_id = %job_id,
        attempts,
        custody_reason = reason.as_str(),
        error_class,
        "Job moved to failure custody"
    );
}

/// Which custody jobs of a scope to redrive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedriveSelection {
    All,
    Jobs(Vec<JobId>),
}

/// The outcome of a redrive, in original enqueue order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RedriveReport {
    /// Back in the queue with their identity, order, grouping and payload, and a fresh retry
    /// budget.
    pub redriven: Vec<JobId>,
    /// Still in custody because the queue already holds the job ID, or an unclaimed job with
    /// the same dedup key.
    pub conflicting: Vec<JobId>,
    /// Selected IDs that are not in this scope's custody.
    pub not_in_custody: Vec<JobId>,
}

/// Moves the selected custody jobs back into the queue in one transaction. A job keeps its ID,
/// enqueue sequence and time, so a FIFO group releases it before every later job of the group.
pub(crate) async fn redrive(
    connection: &mut PgConnection,
    scope: &ScopeName,
    selection: &RedriveSelection,
) -> Result<RedriveReport, JobQueueError> {
    let selected_ids: Option<Vec<Uuid>> = match selection {
        RedriveSelection::All => None,
        RedriveSelection::Jobs(ids) => Some(ids.iter().map(JobId::as_uuid).collect()),
    };
    let rows: Vec<(Uuid, bool)> = sqlx::query_as(
        "WITH selected AS ( \
             SELECT * FROM job_queue_failures \
             WHERE scope = $1 AND ($2::uuid[] IS NULL OR job_id = ANY($2)) \
             FOR UPDATE \
         ), inserted AS ( \
             INSERT INTO job_queue_jobs ( \
                 job_id, sequence, scope, group_key, dedup_key, payload, enqueued_at \
             ) \
             OVERRIDING SYSTEM VALUE \
             SELECT job_id, sequence, scope, group_key, dedup_key, payload, enqueued_at \
             FROM selected ORDER BY sequence \
             ON CONFLICT DO NOTHING \
             RETURNING job_id \
         ), removed AS ( \
             DELETE FROM job_queue_failures AS failures USING inserted \
             WHERE failures.job_id = inserted.job_id \
             RETURNING failures.job_id \
         ) \
         SELECT selected.job_id, removed.job_id IS NOT NULL \
         FROM selected LEFT JOIN removed USING (job_id) \
         ORDER BY selected.sequence",
    )
    .bind(scope.as_str())
    .bind(selected_ids.as_deref())
    .fetch_all(connection)
    .await?;

    let mut report = RedriveReport::default();
    for (id, redriven) in rows {
        let id = JobId::from_uuid(id);
        if redriven {
            report.redriven.push(id);
        } else {
            report.conflicting.push(id);
        }
    }
    if let RedriveSelection::Jobs(ids) = selection {
        report.not_in_custody = ids
            .iter()
            .filter(|id| !report.redriven.contains(id) && !report.conflicting.contains(id))
            .copied()
            .collect();
    }
    tracing::warn!(
        job_scope = scope.as_str(),
        redriven = report.redriven.len(),
        conflicting = report.conflicting.len(),
        not_in_custody = report.not_in_custody.len(),
        "Failure custody redriven"
    );
    Ok(report)
}
