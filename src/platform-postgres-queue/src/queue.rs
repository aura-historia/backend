//! A worker's and an operator's view of one scope: claim, settle, inspect and redrive.

use crate::{
    custody::{self, CustodyReason, RedriveReport, RedriveSelection},
    error::JobQueueError,
    job::{ClaimedJob, ErrorClass, GroupKey, JobId, JobLease},
    scope::{JobScope, QueueOrdering, micros},
};
use sqlx::{PgConnection, PgPool};
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

/// How a worker settles an attempt. These are the dispositions of the SQS FIFO consumers,
/// applied to every scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Confirmed complete, including a deterministic business rejection: the job is deleted.
    Complete,
    /// Not confirmed complete: release the job for another attempt after the scope's retry
    /// delay, or move it to custody once its retry budget is spent.
    Retry(ErrorClass),
    /// Can never succeed as delivered. A first delivery is released like [`Disposition::Retry`],
    /// which absorbs a consumer deploy that lags its producer; a redelivered job moves to
    /// custody at once, so it stops holding its FIFO group.
    Park(ErrorClass),
}

/// What a settlement did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settlement {
    Acknowledged,
    Released,
    MovedToCustody(CustodyReason),
    /// The lease is no longer the job's current lease: the job was settled already, or its
    /// lease expired and another claim replaced it. Nothing changed.
    StaleLease,
}

/// Per-scope figures for operators and metrics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeStats {
    /// Jobs waiting for a claim: claimable now, in retry backoff or behind their FIFO group.
    pub depth: u64,
    /// Jobs under an unexpired lease.
    pub leased: u64,
    /// Time since the oldest waiting job was enqueued.
    pub oldest_waiting_age: Option<Duration>,
    /// Jobs in failure custody.
    pub in_custody: u64,
}

type ClaimRow = (
    Uuid,
    Option<Uuid>,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<OffsetDateTime>,
);

/// One scope of the queue.
#[derive(Clone, Debug)]
pub struct JobQueue {
    pool: PgPool,
    scope: JobScope,
}

impl JobQueue {
    pub fn new(pool: PgPool, scope: JobScope) -> Self {
        Self { pool, scope }
    }

    pub fn scope(&self) -> &JobScope {
        &self.scope
    }

    /// Leases up to `limit` claimable jobs, oldest first, and counts an attempt for each.
    /// Concurrent claimers never lease the same job, and a FIFO claim returns at most each
    /// group's oldest job. A job whose last allowed attempt expired unsettled moves to custody
    /// instead of being leased again.
    pub async fn claim(&self, limit: u16) -> Result<Vec<ClaimedJob>, JobQueueError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        loop {
            let mut transaction = self.pool.begin().await?;
            self.prepare_claim(&mut transaction).await?;
            let rows = self.claim_once(&mut transaction, limit).await?;
            transaction.commit().await?;
            let mut claimed = Vec::with_capacity(rows.len());
            let mut moved_to_custody = false;
            for row in rows {
                match row {
                    (id, None, None, None, None, None) => {
                        moved_to_custody = true;
                        custody::log_custody_transfer(
                            self.scope.name(),
                            JobId::from_uuid(id),
                            i32::try_from(self.scope.max_attempts()).unwrap_or(i32::MAX),
                            CustodyReason::RetryBudgetExhausted,
                            ErrorClass::LEASE_EXPIRED,
                        );
                    }
                    row => claimed.push(claimed_job(row)?),
                }
            }
            // Moving an exhausted FIFO head unblocks its group only for the next claim.
            if !claimed.is_empty() || !moved_to_custody {
                return Ok(claimed);
            }
        }
    }

    /// Pins the claim to its ordered index scan, which stops after the batch; a sort or bitmap
    /// plan chosen on stale statistics would read every queued job of the scope. A FIFO claim
    /// also waits for the scope's earlier claims, so its snapshot includes their leases and no
    /// group is leased twice.
    async fn prepare_claim(&self, connection: &mut PgConnection) -> Result<(), JobQueueError> {
        const PIN_PLAN: &str = "SELECT set_config('enable_seqscan', 'off', true), \
             set_config('enable_bitmapscan', 'off', true), \
             set_config('enable_sort', 'off', true)";
        match self.scope.ordering() {
            QueueOrdering::Standard => {
                sqlx::query(PIN_PLAN).execute(connection).await?;
            }
            QueueOrdering::Fifo => {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "{PIN_PLAN}, pg_advisory_xact_lock(hashtextextended($1, 0))"
                )))
                .bind(self.scope.name().notify_channel())
                .execute(connection)
                .await?;
            }
        }
        Ok(())
    }

    /// Selects and locks candidates, moves exhausted ones to custody and leases the rest, in
    /// one statement. Custody rows come back with every column but the ID null.
    async fn claim_once(
        &self,
        connection: &mut PgConnection,
        limit: u16,
    ) -> Result<Vec<ClaimRow>, JobQueueError> {
        let candidates = match self.scope.ordering() {
            // A job whose group key does not match its scope's ordering is never claimed; it
            // stays visible to inspection. Enqueue never writes one.
            QueueOrdering::Standard => {
                "SELECT job_id, attempts FROM job_queue_jobs \
                 WHERE scope = $1 AND group_key IS NULL AND available_at <= now() \
                 ORDER BY available_at \
                 LIMIT $2 \
                 FOR UPDATE SKIP LOCKED"
            }
            // Only a group's oldest job, and only while no job of the group is leased: a
            // redriven job can be older than its group's leased head.
            QueueOrdering::Fifo => {
                "SELECT job_id, attempts FROM job_queue_jobs AS job \
                 WHERE scope = $1 AND group_key IS NOT NULL AND available_at <= now() \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM job_queue_jobs AS earlier \
                       WHERE earlier.scope = job.scope AND earlier.group_key = job.group_key \
                         AND earlier.sequence < job.sequence \
                   ) \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM job_queue_jobs AS leased \
                       WHERE leased.scope = job.scope AND leased.group_key = job.group_key \
                         AND leased.lease_token IS NOT NULL AND leased.leased_until > now() \
                   ) \
                 ORDER BY sequence \
                 LIMIT $2 \
                 FOR UPDATE SKIP LOCKED"
            }
        };
        let statement = format!(
            "WITH candidates AS ({candidates}), \
             exhausted AS ( \
                 DELETE FROM job_queue_jobs AS jobs USING candidates \
                 WHERE jobs.job_id = candidates.job_id AND candidates.attempts >= $3 \
                 RETURNING jobs.* \
             ), custody AS ( \
                 INSERT INTO job_queue_failures ( \
                     job_id, sequence, scope, group_key, dedup_key, payload, enqueued_at, \
                     attempts, reason, last_error_class, last_claimed_at \
                 ) \
                 SELECT job_id, sequence, scope, group_key, dedup_key, payload, enqueued_at, \
                        attempts, '{exhausted}', '{lease_expired}', last_claimed_at \
                 FROM exhausted \
                 RETURNING job_id \
             ), leased AS ( \
                 UPDATE job_queue_jobs AS jobs \
                 SET attempts = jobs.attempts + 1, \
                     lease_token = gen_random_uuid(), \
                     leased_until = now() + $4::bigint * interval '1 microsecond', \
                     available_at = now() + $4::bigint * interval '1 microsecond', \
                     last_claimed_at = now() \
                 FROM candidates \
                 WHERE jobs.job_id = candidates.job_id AND candidates.attempts < $3 \
                 RETURNING jobs.job_id, jobs.lease_token, jobs.attempts, jobs.group_key, \
                           jobs.payload, jobs.enqueued_at \
             ) \
             SELECT job_id, lease_token, attempts, group_key, payload, enqueued_at FROM leased \
             UNION ALL \
             SELECT job_id, NULL::uuid, NULL::integer, NULL::text, NULL::text, \
                    NULL::timestamptz \
             FROM custody",
            exhausted = CustodyReason::RetryBudgetExhausted.as_str(),
            lease_expired = ErrorClass::LEASE_EXPIRED,
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(statement))
            .bind(self.scope.name().as_str())
            .bind(i64::from(limit))
            .bind(i32::try_from(self.scope.max_attempts()).unwrap_or(i32::MAX))
            .bind(micros(self.scope.lease()))
            .fetch_all(connection)
            .await?)
    }

    /// Settles a claimed job. Only the current lease can settle it; a lost reply is safe to
    /// repeat because a repeated settlement finds the lease stale.
    pub async fn settle(
        &self,
        lease: &JobLease,
        disposition: Disposition,
    ) -> Result<Settlement, JobQueueError> {
        let error_class = match disposition {
            Disposition::Complete => return self.acknowledge(lease).await,
            Disposition::Retry(ref class) | Disposition::Park(ref class) => class,
        };
        let mut transaction = self.pool.begin().await?;
        let attempts: Option<i32> = sqlx::query_scalar(
            "SELECT attempts FROM job_queue_jobs \
             WHERE job_id = $1 AND scope = $2 AND lease_token = $3 \
             FOR UPDATE",
        )
        .bind(lease.job_id().as_uuid())
        .bind(self.scope.name().as_str())
        .bind(lease.token())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(attempts) = attempts else {
            return Ok(Settlement::StaleLease);
        };
        let attempt = u32::try_from(attempts)
            .map_err(|_| JobQueueError::InvalidPersistedState("negative job attempt count"))?;
        let custody_reason = if matches!(disposition, Disposition::Park(_)) && attempt >= 2 {
            Some(CustodyReason::Parked)
        } else if attempt >= self.scope.max_attempts() {
            Some(CustodyReason::RetryBudgetExhausted)
        } else {
            None
        };
        let settlement = match custody_reason {
            Some(reason) => {
                custody::move_to_custody(
                    &mut transaction,
                    self.scope.name(),
                    lease.job_id(),
                    attempts,
                    reason,
                    error_class,
                )
                .await?;
                Settlement::MovedToCustody(reason)
            }
            None => {
                sqlx::query(
                    "UPDATE job_queue_jobs \
                     SET lease_token = NULL, leased_until = NULL, last_error_class = $2, \
                         available_at = now() + $3::bigint * interval '1 microsecond' \
                     WHERE job_id = $1",
                )
                .bind(lease.job_id().as_uuid())
                .bind(error_class.as_str())
                .bind(micros(self.scope.retry_delay()))
                .execute(&mut *transaction)
                .await?;
                Settlement::Released
            }
        };
        transaction.commit().await?;
        Ok(settlement)
    }

    async fn acknowledge(&self, lease: &JobLease) -> Result<Settlement, JobQueueError> {
        let deleted = sqlx::query(
            "DELETE FROM job_queue_jobs WHERE job_id = $1 AND scope = $2 AND lease_token = $3",
        )
        .bind(lease.job_id().as_uuid())
        .bind(self.scope.name().as_str())
        .bind(lease.token())
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(if deleted == 1 {
            Settlement::Acknowledged
        } else {
            Settlement::StaleLease
        })
    }

    /// Depth, leases, oldest waiting age and custody size of this scope.
    pub async fn inspect(&self) -> Result<ScopeStats, JobQueueError> {
        let (depth, leased, oldest_age_seconds, in_custody): (i64, i64, Option<f64>, i64) =
            sqlx::query_as(
                "SELECT \
                     count(*) FILTER (WHERE NOT coalesce(leased_until > now(), false)), \
                     count(*) FILTER (WHERE leased_until > now()), \
                     extract(epoch FROM now() - min(enqueued_at) \
                         FILTER (WHERE NOT coalesce(leased_until > now(), false)))::float8, \
                     (SELECT count(*) FROM job_queue_failures WHERE scope = $1) \
                 FROM job_queue_jobs WHERE scope = $1",
            )
            .bind(self.scope.name().as_str())
            .fetch_one(&self.pool)
            .await?;
        let count = |value: i64| {
            u64::try_from(value)
                .map_err(|_| JobQueueError::InvalidPersistedState("negative job count"))
        };
        Ok(ScopeStats {
            depth: count(depth)?,
            leased: count(leased)?,
            oldest_waiting_age: oldest_age_seconds
                .map(|seconds| Duration::try_from_secs_f64(seconds.max(0.0)).unwrap_or_default()),
            in_custody: count(in_custody)?,
        })
    }

    /// Moves selected or all custody jobs of this scope back into the queue, preserving their
    /// identity and order. Custody is never deleted any other way.
    pub async fn redrive(
        &self,
        selection: &RedriveSelection,
    ) -> Result<RedriveReport, JobQueueError> {
        let mut transaction = self.pool.begin().await?;
        let report = custody::redrive(&mut transaction, self.scope.name(), selection).await?;
        transaction.commit().await?;
        Ok(report)
    }
}

fn claimed_job(row: ClaimRow) -> Result<ClaimedJob, JobQueueError> {
    let (id, Some(token), Some(attempts), group_key, Some(payload), Some(enqueued_at)) = row else {
        return Err(JobQueueError::InvalidPersistedState(
            "incomplete claimed job",
        ));
    };
    let attempt = u32::try_from(attempts)
        .ok()
        .filter(|attempt| *attempt >= 1)
        .ok_or(JobQueueError::InvalidPersistedState(
            "invalid job attempt count",
        ))?;
    let group_key = group_key
        .map(GroupKey::new)
        .transpose()
        .map_err(|_| JobQueueError::InvalidPersistedState("invalid job group key"))?;
    Ok(ClaimedJob {
        lease: JobLease::new(JobId::from_uuid(id), token),
        attempt,
        group_key,
        payload,
        enqueued_at,
    })
}
