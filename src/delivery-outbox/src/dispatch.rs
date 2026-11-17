//! The dispatcher: claims captured snapshots in outbox order, routes them into the job queue
//! and deletes them, all in one transaction, so publication is never partial.

use crate::{error::OutboxError, snapshot};
use aura_historia_cdc_routing::route_change;
use aura_historia_jobs::{DomainJob, wire};
use platform_postgres_queue::{DedupKey, EnqueueOutcome, GroupKey, NewJob, enqueue};
use platform_worker_queue_contract::job_queue::worker_job_scope;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use std::num::NonZeroU16;
use time::OffsetDateTime;
use tracing::{debug, warn};

/// One dispatch transaction runs at a time across every process, which keeps jobs in outbox
/// order: within one consent recipient, writers serialize on the recipient lock, so its
/// snapshots are captured and committed in outbox ID order.
const DISPATCH_LOCK: &str =
    "SELECT pg_try_advisory_xact_lock(hashtextextended('delivery_outbox:dispatch', 0))";

/// Labels for snapshots whose jobs could not be built.
const INVALID_JOB: &str = "invalid_job";

#[derive(Clone)]
pub struct OutboxDispatcher {
    pool: PgPool,
    batch_size: NonZeroU16,
}

/// What one dispatch transaction did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch {
    /// Another dispatch transaction holds the dispatch lock; nothing was claimed.
    Busy,
    Dispatched(DispatchReport),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DispatchReport {
    /// Snapshots claimed and removed from the outbox.
    pub claimed: usize,
    /// Jobs inserted into the job queue.
    pub enqueued: usize,
    /// Jobs whose dedup key an unclaimed job already carried.
    pub deduplicated: usize,
    /// Snapshots moved to failure custody instead.
    pub custody: usize,
}

#[derive(sqlx::FromRow)]
struct OutboxRow {
    outbox_id: i64,
    source_table: String,
    snapshot_version: i16,
    snapshot: Value,
    enqueued_at: OffsetDateTime,
}

impl OutboxDispatcher {
    pub fn new(pool: PgPool, batch_size: NonZeroU16) -> Self {
        Self { pool, batch_size }
    }

    /// Dispatches one batch in its own transaction and commits it.
    pub async fn dispatch_batch(&self) -> Result<Dispatch, OutboxError> {
        let mut transaction = self.pool.begin().await?;
        let dispatch = self.dispatch_within(&mut transaction).await?;
        transaction.commit().await?;
        Ok(dispatch)
    }

    /// Dispatches one batch inside the caller's transaction:
    ///
    /// 1. claim the oldest snapshots with `SKIP LOCKED`, in outbox ID order;
    /// 2. decode each with its source's strict codec and route it;
    /// 3. enqueue every job of the batch;
    /// 4. move snapshots that cannot be decoded, routed or encoded into failure custody;
    /// 5. delete the claimed snapshots.
    ///
    /// Nothing is published until the caller commits; a rollback leaves every snapshot to be
    /// dispatched again with the same effect.
    pub async fn dispatch_within(
        &self,
        connection: &mut PgConnection,
    ) -> Result<Dispatch, OutboxError> {
        let locked: bool = sqlx::query_scalar(DISPATCH_LOCK)
            .fetch_one(&mut *connection)
            .await?;
        if !locked {
            return Ok(Dispatch::Busy);
        }
        let rows: Vec<OutboxRow> = sqlx::query_as(
            "SELECT outbox_id, source_table, snapshot_version, snapshot, enqueued_at \
             FROM delivery_outbox ORDER BY outbox_id LIMIT $1 FOR UPDATE SKIP LOCKED",
        )
        .bind(i64::from(self.batch_size.get()))
        .fetch_all(&mut *connection)
        .await?;

        let mut report = DispatchReport {
            claimed: rows.len(),
            ..DispatchReport::default()
        };
        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            claimed.push(row.outbox_id);
            match plan(&row) {
                Ok(jobs) => {
                    for job in &jobs {
                        match enqueue(&mut *connection, job).await? {
                            EnqueueOutcome::Enqueued(_) => report.enqueued += 1,
                            EnqueueOutcome::Deduplicated => report.deduplicated += 1,
                        }
                    }
                }
                Err(error_class) => {
                    move_to_custody(connection, &row, error_class).await?;
                    report.custody += 1;
                }
            }
        }
        sqlx::query("DELETE FROM delivery_outbox WHERE outbox_id = ANY($1)")
            .bind(&claimed)
            .execute(&mut *connection)
            .await?;
        if report.claimed > 0 {
            debug!(
                claimed = report.claimed,
                enqueued = report.enqueued,
                deduplicated = report.deduplicated,
                custody = report.custody,
                "Dispatched delivery outbox batch"
            );
        }
        Ok(Dispatch::Dispatched(report))
    }
}

/// Validates and encodes every job of one snapshot before any of them is enqueued. The error
/// is the snapshot's bounded custody label.
fn plan(row: &OutboxRow) -> Result<Vec<NewJob>, &'static str> {
    let change = snapshot::decode(
        &row.source_table,
        row.snapshot_version,
        row.snapshot.clone(),
    )
    .map_err(snapshot::SnapshotError::class)?;
    let jobs = route_change(&change).map_err(|error| error.class())?;
    jobs.iter().map(new_job).collect()
}

/// The job queue form of a routed job: a FIFO job keeps its ordering key as its group and its
/// idempotency key as its dedup key, as the SQS FIFO group and deduplication IDs did.
fn new_job(job: &DomainJob) -> Result<NewJob, &'static str> {
    let prepared = wire::prepare(job).map_err(|_| INVALID_JOB)?;
    let scope = worker_job_scope(prepared.scope());
    let (group_key, dedup_key) = match prepared.fifo_message_attributes() {
        Some(fifo) => (
            Some(GroupKey::new(fifo.message_group_id()).map_err(|_| INVALID_JOB)?),
            Some(DedupKey::new(fifo.message_deduplication_id()).map_err(|_| INVALID_JOB)?),
        ),
        None => (None, None),
    };
    let new_job = NewJob::new(&scope, group_key, prepared.body()).map_err(|_| INVALID_JOB)?;
    Ok(match dedup_key {
        Some(key) => new_job.with_dedup_key(key),
        None => new_job,
    })
}

async fn move_to_custody(
    connection: &mut PgConnection,
    row: &OutboxRow,
    error_class: &'static str,
) -> Result<(), OutboxError> {
    sqlx::query(
        "INSERT INTO delivery_outbox_failures \
         (outbox_id, source_table, snapshot_version, snapshot, enqueued_at, error_class) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(row.outbox_id)
    .bind(&row.source_table)
    .bind(row.snapshot_version)
    .bind(&row.snapshot)
    .bind(row.enqueued_at)
    .bind(error_class)
    .execute(&mut *connection)
    .await?;
    warn!(
        outbox_id = row.outbox_id,
        source_table = %row.source_table,
        error_class,
        "Delivery outbox snapshot moved to failure custody"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_historia_jobs::WorkerQueue;
    use serde_json::json;

    fn row(source_table: &str, snapshot: Value) -> OutboxRow {
        OutboxRow {
            outbox_id: 1,
            source_table: source_table.to_owned(),
            snapshot_version: snapshot::SNAPSHOT_VERSION,
            snapshot,
            enqueued_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn uuid() -> String {
        uuid::Uuid::now_v7().to_string()
    }

    #[test]
    fn should_group_and_deduplicate_consent_jobs_like_their_fifo_queue() {
        let key = "ab".repeat(32);
        let intent = uuid();

        let jobs = plan(&row(
            "marketing_email_consent_sync_intents",
            json!({"intent_id": intent, "recipient_key": key}),
        ))
        .unwrap();

        assert_eq!(1, jobs.len());
        assert_eq!("marketing-consent-sync", jobs[0].scope().as_str());
        assert_eq!(
            Some(format!("marketing-email:{key}").as_str()),
            jobs[0].group_key().map(GroupKey::as_str)
        );
        assert!(
            jobs[0]
                .dedup_key()
                .is_some_and(|dedup| dedup.as_str().starts_with("marketing-consent:"))
        );
    }

    #[test]
    fn should_enqueue_standard_jobs_without_group_or_dedup_key() {
        let jobs = plan(&row(
            "notification_deliveries",
            json!({"notification_delivery_id": uuid()}),
        ))
        .unwrap();

        assert_eq!(1, jobs.len());
        assert_eq!("notification-delivery", jobs[0].scope().as_str());
        assert_eq!((None, None), (jobs[0].group_key(), jobs[0].dedup_key()));
    }

    #[test]
    fn should_label_each_failure_by_the_step_that_rejected_it() {
        assert_eq!(
            Err("unsupported_source"),
            plan(&row("users", json!({"user_id": uuid()})))
        );
        assert_eq!(
            Err("invalid_snapshot"),
            plan(&row("notification_deliveries", json!({"id": uuid()})))
        );
        assert_eq!(
            Err("invalid_identity"),
            plan(&row(
                "notification_deliveries",
                json!({"notification_delivery_id": "not-a-uuid"})
            ))
        );
        let mut versioned = row(
            "notification_deliveries",
            json!({"notification_delivery_id": uuid()}),
        );
        versioned.snapshot_version = 2;
        assert_eq!(Err("unsupported_snapshot_version"), plan(&versioned));
    }

    #[test]
    fn should_refuse_a_job_its_scope_cannot_carry() {
        let change = snapshot::decode(
            "notification_deliveries",
            snapshot::SNAPSHOT_VERSION,
            json!({"notification_delivery_id": uuid()}),
        )
        .unwrap();
        let job = route_change(&change).unwrap().remove(0);
        let misrouted = DomainJob {
            target_queue: WorkerQueue::MarketingConsentSync,
            ..job.clone()
        };

        assert!(new_job(&job).is_ok());
        assert_eq!(Err(INVALID_JOB), new_job(&misrouted));
    }
}
