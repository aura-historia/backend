use common::{error::boxed::box_error, postgres::SqlxTransaction};
use product_core::fx_rate_snapshot::FxRateSnapshot;
use product_service::ports::{
    FxRateSnapshotInsertOutcome, FxRateSnapshotRepository, FxRateSnapshotRepositoryError,
    FxRateSnapshotRepositoryFactory,
};
use sqlx::{PgConnection, Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxFxRateSnapshotRepositoryFactory;

struct SqlxFxRateSnapshotRepository<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("FX rate snapshot SQL insert failed")]
struct FxRateSnapshotInsertSqlxError(#[source] sqlx::Error);

impl SqlxFxRateSnapshotRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl FxRateSnapshotRepositoryFactory<SqlxTransaction> for SqlxFxRateSnapshotRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl FxRateSnapshotRepository + 'tx {
        SqlxFxRateSnapshotRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl FxRateSnapshotRepository for SqlxFxRateSnapshotRepository<'_> {
    async fn insert(
        &mut self,
        snapshot: &FxRateSnapshot,
        source_event_id: &str,
    ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
        let inserted = sqlx::query(
            r#"
            INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (source_event_id) DO NOTHING
            "#,
        )
        .bind(uuid::Uuid::from(snapshot.id()))
        .bind(snapshot.captured_at())
        .bind(snapshot.source().as_str())
        .bind(source_event_id)
        .execute(&mut *self.connection)
        .await
        .map_err(FxRateSnapshotInsertSqlxError)?;

        if inserted.rows_affected() == 0 {
            return Ok(FxRateSnapshotInsertOutcome::Duplicate);
        }

        if snapshot
            .conversions()
            .iter()
            .any(|conversion| conversion.rate() > i64::MAX as u64)
        {
            return Err(FxRateSnapshotRepositoryError::InsertFailed {
                source: common::error::boxed::static_error(
                    "FX rate conversion exceeds PostgreSQL bigint range",
                ),
            });
        }

        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO fx_rate_conversions (fx_rate_id, from_currency, to_currency, rate) ",
        );
        query.push_values(snapshot.conversions(), |mut row, conversion| {
            row.push_bind(uuid::Uuid::from(snapshot.id()))
                .push_bind(conversion.from_currency().as_str())
                .push_bind(conversion.to_currency().as_str())
                .push_bind(conversion.rate() as i64);
        });
        query
            .build()
            .execute(&mut *self.connection)
            .await
            .map_err(FxRateSnapshotInsertSqlxError)?;

        Ok(FxRateSnapshotInsertOutcome::Inserted)
    }
}

impl From<FxRateSnapshotInsertSqlxError> for FxRateSnapshotRepositoryError {
    fn from(source: FxRateSnapshotInsertSqlxError) -> Self {
        Self::InsertFailed {
            source: box_error(source),
        }
    }
}
