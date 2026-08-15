use common::{
    currency::domain::Currency,
    error::boxed::{box_error, static_error},
    fx_rate_id::FxRateId,
};
use fxrate_core::{FxRateGeneration, FxRateQuote, FxRateSnapshot, FxRateSource};
use fxrate_service::ports::{FxRateSnapshotReadError, FxRateSnapshotReader};
use sqlx::PgPool;
use std::collections::HashMap;
use strum::IntoEnumIterator;
use time::OffsetDateTime;

#[derive(Clone)]
pub struct SqlxFxRateSnapshotReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct SnapshotRow {
    fx_rate_id: uuid::Uuid,
    generation: i64,
    captured_at: OffsetDateTime,
    source: String,
}

#[derive(Debug, sqlx::FromRow)]
struct QuoteRow {
    fx_rate_id: uuid::Uuid,
    currency: String,
    units_per_eur: i64,
}

impl SqlxFxRateSnapshotReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn read(
        &self,
        snapshots: Vec<SnapshotRow>,
    ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotReadError> {
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }
        let ids = snapshots
            .iter()
            .map(|snapshot| snapshot.fx_rate_id)
            .collect::<Vec<_>>();
        let quotes = sqlx::query_as::<_, QuoteRow>(
            "SELECT fx_rate_id, currency, units_per_eur FROM fx_rate_quotes WHERE fx_rate_id = ANY($1)",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|source| FxRateSnapshotReadError::ReadFailed {
            source: box_error(source),
        })?;
        map_snapshots(snapshots, quotes)
    }
}

#[async_trait::async_trait]
impl FxRateSnapshotReader for SqlxFxRateSnapshotReader {
    async fn latest(&self) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        let row = sqlx::query_as::<_, SnapshotRow>(
            "SELECT fx_rate_id, generation, captured_at, source FROM fx_rates ORDER BY captured_at DESC, generation DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| FxRateSnapshotReadError::ReadFailed {
            source: box_error(source),
        })?;
        let snapshots = self.read(row.into_iter().collect()).await?;
        Ok(snapshots.into_iter().next())
    }

    async fn latest_at_or_before(
        &self,
        timestamp: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        let row = sqlx::query_as::<_, SnapshotRow>(
            "SELECT fx_rate_id, generation, captured_at, source FROM fx_rates WHERE captured_at <= $1 ORDER BY captured_at DESC, generation DESC LIMIT 1",
        )
        .bind(timestamp)
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| FxRateSnapshotReadError::ReadFailed {
            source: box_error(source),
        })?;
        let snapshots = self.read(row.into_iter().collect()).await?;
        Ok(snapshots.into_iter().next())
    }

    async fn find_by_id(
        &self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        let row = sqlx::query_as::<_, SnapshotRow>(
            "SELECT fx_rate_id, generation, captured_at, source FROM fx_rates WHERE fx_rate_id = $1",
        )
        .bind(uuid::Uuid::from(id))
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| FxRateSnapshotReadError::ReadFailed {
            source: box_error(source),
        })?;
        let snapshots = self.read(row.into_iter().collect()).await?;
        Ok(snapshots.into_iter().next())
    }

    async fn find_by_ids(
        &self,
        ids: &[FxRateId],
    ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotReadError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = ids
            .iter()
            .copied()
            .map(uuid::Uuid::from)
            .collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, SnapshotRow>(
            "SELECT fx_rate_id, generation, captured_at, source FROM fx_rates WHERE fx_rate_id = ANY($1) ORDER BY generation ASC",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|source| FxRateSnapshotReadError::ReadFailed {
            source: box_error(source),
        })?;
        self.read(rows).await
    }
}

fn map_snapshots(
    snapshots: Vec<SnapshotRow>,
    quotes: Vec<QuoteRow>,
) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotReadError> {
    let mut quotes_by_snapshot = HashMap::<uuid::Uuid, Vec<FxRateQuote>>::new();
    for quote in quotes {
        let currency = Currency::iter()
            .find(|currency| currency.as_str() == quote.currency)
            .ok_or_else(|| FxRateSnapshotReadError::InvalidPersistedSnapshot {
                source: static_error("persisted FX quote currency is unsupported"),
            })?;
        let units_per_eur = u64::try_from(quote.units_per_eur).map_err(|_| {
            FxRateSnapshotReadError::InvalidPersistedSnapshot {
                source: static_error("persisted FX quote is negative"),
            }
        })?;
        quotes_by_snapshot
            .entry(quote.fx_rate_id)
            .or_default()
            .push(FxRateQuote::new(currency, units_per_eur));
    }

    snapshots
        .into_iter()
        .map(|snapshot| {
            let generation = FxRateGeneration::try_from(snapshot.generation).map_err(|source| {
                FxRateSnapshotReadError::InvalidPersistedSnapshot {
                    source: box_error(source),
                }
            })?;
            let source = FxRateSource::try_from_persisted(&snapshot.source).map_err(|source| {
                FxRateSnapshotReadError::InvalidPersistedSnapshot {
                    source: box_error(source),
                }
            })?;
            FxRateSnapshot::rehydrate(
                FxRateId::from(snapshot.fx_rate_id),
                generation,
                snapshot.captured_at,
                source,
                quotes_by_snapshot
                    .remove(&snapshot.fx_rate_id)
                    .unwrap_or_default(),
            )
            .map_err(|source| FxRateSnapshotReadError::InvalidPersistedSnapshot {
                source: box_error(source),
            })
        })
        .collect()
}
