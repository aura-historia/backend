use crate::mapping::{WatchlistRepositoryRow, format_state};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use product_core::product_id::ProductId;
use time::OffsetDateTime;
use user_core::user_id::UserId;
use watchlist_core::WatchlistProduct;
use watchlist_service::ports::{
    VersionedWatchlistProduct, WatchlistRepository, WatchlistRepositoryError,
    WatchlistRepositoryFactory, WatchlistStorageVersion,
};

const REPOSITORY_COLUMNS: &str = "user_id, product_id, notifications, state, version";

#[derive(Debug, Clone, Default)]
pub struct SqlxWatchlistRepositoryFactory;

struct SqlxWatchlistRepository<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl WatchlistRepositoryFactory<SqlxTransaction> for SqlxWatchlistRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl WatchlistRepository + 'tx {
        SqlxWatchlistRepository { tx }
    }
}

#[async_trait::async_trait]
impl WatchlistRepository for SqlxWatchlistRepository<'_> {
    async fn find_by_user_and_product(
        &mut self,
        user_id: UserId,
        product_id: ProductId,
    ) -> Result<Option<VersionedWatchlistProduct>, WatchlistRepositoryError> {
        let sql = format!(
            "SELECT {REPOSITORY_COLUMNS} FROM product_watchlist WHERE user_id = $1 AND product_id = $2"
        );
        let row = sqlx::query_as::<_, WatchlistRepositoryRow>(&sql)
            .bind(uuid::Uuid::from(user_id))
            .bind(uuid::Uuid::from(product_id))
            .fetch_optional(self.tx.connection())
            .await
            .map_err(|source| WatchlistRepositoryError::LookupFailed {
                source: box_error(source),
            })?;

        row.map(VersionedWatchlistProduct::try_from).transpose()
    }

    async fn insert(
        &mut self,
        entry: &WatchlistProduct,
    ) -> Result<VersionedWatchlistProduct, WatchlistRepositoryError> {
        let now = OffsetDateTime::now_utc();
        let sql = format!(
            "INSERT INTO product_watchlist \
             (user_id, product_id, notifications, state, active_since, notifications_enabled_since, created, updated) \
             VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'ACTIVE' THEN $5 ELSE NULL END, CASE WHEN $3 THEN $5 ELSE NULL END, $5, $5) \
             RETURNING {REPOSITORY_COLUMNS}"
        );
        let row = sqlx::query_as::<_, WatchlistRepositoryRow>(&sql)
            .bind(uuid::Uuid::from(entry.user_id()))
            .bind(uuid::Uuid::from(entry.product_id()))
            .bind(entry.notifications())
            .bind(format_state(entry.state()))
            .bind(now)
            .fetch_one(self.tx.connection())
            .await
            .map_err(map_insert_error)?;

        VersionedWatchlistProduct::try_from(row)
    }

    async fn update(
        &mut self,
        entry: &WatchlistProduct,
        expected_version: WatchlistStorageVersion,
    ) -> Result<VersionedWatchlistProduct, WatchlistRepositoryError> {
        let expected_version = version_to_i64(expected_version)?;
        let now = OffsetDateTime::now_utc();
        let sql = format!(
            "UPDATE product_watchlist SET \
                 notifications = $3, \
                 state = $4, \
                 active_since = CASE \
                     WHEN state <> 'ACTIVE' AND $4 = 'ACTIVE' THEN $5 \
                     WHEN $4 <> 'ACTIVE' THEN NULL \
                     ELSE active_since \
                 END, \
                 notifications_enabled_since = CASE \
                     WHEN notifications = false AND $3 = true THEN $5 \
                     WHEN $3 = false THEN NULL \
                     ELSE notifications_enabled_since \
                 END, \
                 version = version + 1, \
                 updated = $5 \
             WHERE user_id = $1 AND product_id = $2 AND version = $6 \
             RETURNING {REPOSITORY_COLUMNS}"
        );
        let row = sqlx::query_as::<_, WatchlistRepositoryRow>(&sql)
            .bind(uuid::Uuid::from(entry.user_id()))
            .bind(uuid::Uuid::from(entry.product_id()))
            .bind(entry.notifications())
            .bind(format_state(entry.state()))
            .bind(now)
            .bind(expected_version)
            .fetch_optional(self.tx.connection())
            .await
            .map_err(|source| WatchlistRepositoryError::UpdateFailed {
                source: box_error(source),
            })?
            .ok_or(WatchlistRepositoryError::ConcurrencyConflict)?;

        VersionedWatchlistProduct::try_from(row)
    }

    async fn delete(
        &mut self,
        user_id: UserId,
        product_id: ProductId,
        expected_version: WatchlistStorageVersion,
    ) -> Result<(), WatchlistRepositoryError> {
        let expected_version = version_to_i64(expected_version)?;
        let result = sqlx::query(
            "DELETE FROM product_watchlist WHERE user_id = $1 AND product_id = $2 AND version = $3",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(product_id))
        .bind(expected_version)
        .execute(self.tx.connection())
        .await
        .map_err(|source| WatchlistRepositoryError::DeleteFailed {
            source: box_error(source),
        })?;
        if result.rows_affected() == 0 {
            return Err(WatchlistRepositoryError::ConcurrencyConflict);
        }
        Ok(())
    }
}

fn version_to_i64(version: WatchlistStorageVersion) -> Result<i64, WatchlistRepositoryError> {
    i64::try_from(version.into_inner()).map_err(|_| WatchlistRepositoryError::InvalidPersistedState)
}

fn map_insert_error(error: sqlx::Error) -> WatchlistRepositoryError {
    if let sqlx::Error::Database(db) = &error
        && db.is_unique_violation()
    {
        return WatchlistRepositoryError::AlreadyExists;
    }
    WatchlistRepositoryError::InsertFailed {
        source: box_error(error),
    }
}
