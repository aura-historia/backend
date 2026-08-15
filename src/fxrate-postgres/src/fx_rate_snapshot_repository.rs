use common::{error::boxed::box_error, postgres::SqlxTransaction};
use fxrate_core::{FxRateGeneration, NewFxRateSnapshot};
use fxrate_service::ports::{
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
        snapshot: &NewFxRateSnapshot,
        source_event_id: &str,
    ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
        let generation = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (source_event_id) DO NOTHING
            RETURNING generation
            "#,
        )
        .bind(uuid::Uuid::from(snapshot.id()))
        .bind(snapshot.captured_at())
        .bind(snapshot.source().as_str())
        .bind(source_event_id)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(FxRateSnapshotInsertSqlxError)?;

        let Some(generation) = generation else {
            return Ok(FxRateSnapshotInsertOutcome::Duplicate);
        };
        let generation = FxRateGeneration::try_from(generation).map_err(|source| {
            FxRateSnapshotRepositoryError::InsertFailed {
                source: box_error(source),
            }
        })?;

        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) ",
        );
        query.push_values(snapshot.quotes(), |mut row, quote| {
            row.push_bind(uuid::Uuid::from(snapshot.id()))
                .push_bind(quote.currency().as_str())
                .push_bind(quote.units_per_eur() as i64);
        });
        query
            .build()
            .execute(&mut *self.connection)
            .await
            .map_err(FxRateSnapshotInsertSqlxError)?;

        Ok(FxRateSnapshotInsertOutcome::Inserted(
            snapshot.clone().into_persisted(generation),
        ))
    }
}

impl From<FxRateSnapshotInsertSqlxError> for FxRateSnapshotRepositoryError {
    fn from(source: FxRateSnapshotInsertSqlxError) -> Self {
        Self::InsertFailed {
            source: box_error(source),
        }
    }
}
