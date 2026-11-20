//! Failure custody and the operator functions over the outbox: inspection and redrive.

use crate::error::OutboxError;
use sqlx::PgPool;
use std::time::Duration;

/// The figures the outbox metrics publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutboxStats {
    /// Snapshots waiting for dispatch.
    pub pending: u64,
    /// Age of the oldest waiting snapshot, if any.
    pub oldest_pending_age: Option<Duration>,
    /// Snapshots in failure custody.
    pub custody: u64,
}

pub async fn inspect(connection: &mut sqlx::PgConnection) -> Result<OutboxStats, OutboxError> {
    let (pending, oldest_seconds, custody): (i64, Option<f64>, i64) = sqlx::query_as(
        "SELECT \
             (SELECT count(*) FROM delivery_outbox), \
             (SELECT extract(epoch FROM now() - enqueued_at)::float8 \
                FROM delivery_outbox ORDER BY outbox_id LIMIT 1), \
             (SELECT count(*) FROM delivery_outbox_failures)",
    )
    .fetch_one(&mut *connection)
    .await?;
    Ok(OutboxStats {
        pending: u64::try_from(pending).unwrap_or_default(),
        oldest_pending_age: oldest_seconds.map(|seconds| Duration::from_secs_f64(seconds.max(0.0))),
        custody: u64::try_from(custody).unwrap_or_default(),
    })
}

/// Which custody rows to move back into the outbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedriveSelection {
    All,
    Rows(Vec<i64>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RedriveReport {
    /// Outbox IDs moved back, in outbox order.
    pub redriven: Vec<i64>,
    /// Selected IDs that were not in custody.
    pub not_in_custody: Vec<i64>,
}

/// Moves custody rows back into the outbox unchanged and under their original outbox ID, so
/// they dispatch ahead of every snapshot captured after them that is still waiting. Snapshots
/// of the same consent recipient that were already dispatched have run before them; the
/// consent worker rereads its intent, so a redriven older intent cannot override a newer one.
pub async fn redrive(
    pool: &PgPool,
    selection: &RedriveSelection,
) -> Result<RedriveReport, OutboxError> {
    let selected: Option<Vec<i64>> = match selection {
        RedriveSelection::All => None,
        RedriveSelection::Rows(ids) => Some(ids.clone()),
    };
    let mut redriven: Vec<i64> = sqlx::query_scalar(
        "WITH moved AS ( \
             DELETE FROM delivery_outbox_failures \
             WHERE $1::bigint[] IS NULL OR outbox_id = ANY($1) \
             RETURNING outbox_id, source_table, snapshot_version, snapshot, enqueued_at) \
         INSERT INTO delivery_outbox (outbox_id, source_table, snapshot_version, snapshot, enqueued_at) \
         SELECT outbox_id, source_table, snapshot_version, snapshot, enqueued_at FROM moved \
         RETURNING outbox_id",
    )
    .bind(&selected)
    .fetch_all(pool)
    .await?;
    redriven.sort_unstable();
    let mut not_in_custody: Vec<i64> = selected
        .unwrap_or_default()
        .into_iter()
        .filter(|id| redriven.binary_search(id).is_err())
        .collect();
    not_in_custody.sort_unstable();
    not_in_custody.dedup();
    Ok(RedriveReport {
        redriven,
        not_in_custody,
    })
}
