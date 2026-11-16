//! Per-user address suggestion quota counters. One row per user, holding only the current
//! minute and UTC-day windows; nothing about the queries themselves is stored.
use application::error::{BoxError, box_error};
use geo_service::suggestions::{
    SuggestionUsage, SuggestionUsageRepository, SuggestionUsageRepositoryFactory,
};
use platform_postgres::SqlxTransaction;
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxSuggestionUsageRepositoryFactory;

impl SuggestionUsageRepositoryFactory<SqlxTransaction> for SqlxSuggestionUsageRepositoryFactory {
    fn usage<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl SuggestionUsageRepository + 'tx {
        UsageRepository {
            connection: tx.connection(),
        }
    }
}

struct UsageRepository<'a> {
    connection: &'a mut PgConnection,
}

#[derive(FromRow)]
struct UsageRow {
    minute_start: OffsetDateTime,
    minute_uses: i32,
    day_start: OffsetDateTime,
    day_uses: i32,
}

impl TryFrom<UsageRow> for SuggestionUsage {
    type Error = BoxError;
    fn try_from(row: UsageRow) -> Result<Self, Self::Error> {
        Ok(Self {
            minute_start: row.minute_start,
            minute_uses: u32::try_from(row.minute_uses).map_err(box_error)?,
            day_start: row.day_start,
            day_uses: u32::try_from(row.day_uses).map_err(box_error)?,
        })
    }
}

#[async_trait::async_trait]
impl SuggestionUsageRepository for UsageRepository<'_> {
    async fn lock(&mut self, user: Uuid) -> Result<Option<SuggestionUsage>, BoxError> {
        // An empty row first, so concurrent first requests serialize on the same row lock
        // instead of overwriting each other's count. Its epoch windows count as unused.
        sqlx::query(
            "INSERT INTO address_suggestion_usage(user_id,minute_start,minute_uses,day_start,day_uses)
             VALUES ($1,'epoch',0,'epoch',0) ON CONFLICT (user_id) DO NOTHING",
        )
        .bind(user)
        .execute(&mut *self.connection)
        .await
        .map_err(box_error)?;
        let row: UsageRow = sqlx::query_as(
            "SELECT minute_start,minute_uses,day_start,day_uses FROM address_suggestion_usage
             WHERE user_id=$1 FOR UPDATE",
        )
        .bind(user)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(box_error)?;
        row.try_into().map(Some)
    }

    async fn save(&mut self, user: Uuid, usage: SuggestionUsage) -> Result<(), BoxError> {
        let count = |uses: u32| i32::try_from(uses).map_err(box_error);
        sqlx::query(
            "UPDATE address_suggestion_usage SET minute_start=$2,minute_uses=$3,day_start=$4,day_uses=$5
             WHERE user_id=$1",
        )
        .bind(user)
        .bind(usage.minute_start)
        .bind(count(usage.minute_uses)?)
        .bind(usage.day_start)
        .bind(count(usage.day_uses)?)
        .execute(&mut *self.connection)
        .await
        .map_err(box_error)?;
        Ok(())
    }
}
