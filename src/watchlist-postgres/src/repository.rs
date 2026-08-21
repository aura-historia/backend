use crate::mapping::{WatchlistRow, format_state};
use common::postgres::SqlxTransaction;
use common::product_id::ProductId;
use common::user_id::UserId;
use time::OffsetDateTime;
use watchlist_core::WatchlistProduct;
use watchlist_service::ports::{
    WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory,
};

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
    ) -> Result<Option<WatchlistProduct>, WatchlistRepositoryError> {
        let row = sqlx::query_as::<_, WatchlistRow>(
            "SELECT user_id, product_id, notifications, state, created, updated \
             FROM product_watchlist WHERE user_id = $1 AND product_id = $2",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(self.tx.connection())
        .await
        .map_err(|_| WatchlistRepositoryError::LookupFailed)?;

        row.map(WatchlistRow::into_domain).transpose()
    }

    async fn insert(
        &mut self,
        entry: &WatchlistProduct,
    ) -> Result<WatchlistProduct, WatchlistRepositoryError> {
        let now = OffsetDateTime::now_utc();
        sqlx::query(
            "INSERT INTO product_watchlist \
             (user_id, product_id, notifications, state, active_since, notifications_enabled_since, created, updated) \
             VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'ACTIVE' THEN $5 ELSE NULL END, CASE WHEN $3 THEN $5 ELSE NULL END, $5, $5)",
        )
        .bind(uuid::Uuid::from(entry.user_id()))
        .bind(uuid::Uuid::from(entry.product_id()))
        .bind(entry.notifications())
        .bind(format_state(entry.state()))
        .bind(now)
        .execute(self.tx.connection())
        .await
        .map_err(map_insert_error)?;
        Ok(entry.clone())
    }

    async fn update(
        &mut self,
        entry: &WatchlistProduct,
    ) -> Result<WatchlistProduct, WatchlistRepositoryError> {
        sqlx::query(
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
                 updated = $5 \
             WHERE user_id = $1 AND product_id = $2",
        )
        .bind(uuid::Uuid::from(entry.user_id()))
        .bind(uuid::Uuid::from(entry.product_id()))
        .bind(entry.notifications())
        .bind(format_state(entry.state()))
        .bind(OffsetDateTime::now_utc())
        .execute(self.tx.connection())
        .await
        .map_err(|_| WatchlistRepositoryError::UpdateFailed)?;
        Ok(entry.clone())
    }

    async fn delete(
        &mut self,
        user_id: UserId,
        product_id: ProductId,
    ) -> Result<(), WatchlistRepositoryError> {
        sqlx::query("DELETE FROM product_watchlist WHERE user_id = $1 AND product_id = $2")
            .bind(uuid::Uuid::from(user_id))
            .bind(uuid::Uuid::from(product_id))
            .execute(self.tx.connection())
            .await
            .map_err(|_| WatchlistRepositoryError::DeleteFailed)?;
        Ok(())
    }
}

fn map_insert_error(error: sqlx::Error) -> WatchlistRepositoryError {
    if let sqlx::Error::Database(db) = &error
        && db.is_unique_violation()
    {
        return WatchlistRepositoryError::AlreadyExists;
    }
    WatchlistRepositoryError::InsertFailed
}
