use application::error::box_error;
use platform_postgres::SqlxTransaction;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    WatchlistNotificationRecipient, WatchlistNotificationRecipientReadError,
    WatchlistNotificationRecipientReader, WatchlistNotificationRecipientReaderFactory,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxWatchlistNotificationRecipientReaderFactory;

struct SqlxWatchlistNotificationRecipientReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

#[derive(Debug, thiserror::Error)]
#[error("watchlist notification recipient SQL query failed")]
struct WatchlistNotificationRecipientQueryError(#[source] sqlx::Error);

impl From<WatchlistNotificationRecipientQueryError> for WatchlistNotificationRecipientReadError {
    fn from(source: WatchlistNotificationRecipientQueryError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl WatchlistNotificationRecipientReaderFactory<SqlxTransaction>
    for SqlxWatchlistNotificationRecipientReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl WatchlistNotificationRecipientReader + 'tx {
        SqlxWatchlistNotificationRecipientReader { tx }
    }
}

#[async_trait::async_trait]
impl WatchlistNotificationRecipientReader for SqlxWatchlistNotificationRecipientReader<'_> {
    async fn find_eligible_for_product_at(
        &mut self,
        product_id: ProductListingId,
        event_time: OffsetDateTime,
    ) -> Result<Vec<WatchlistNotificationRecipient>, WatchlistNotificationRecipientReadError> {
        let rows = sqlx::query_as::<_, (uuid::Uuid, bool)>(
            "SELECT user_id, notifications AND notifications_enabled_since <= $2 AS external_delivery_requested \
             FROM product_watchlist \
             WHERE product_id = $1 \
               AND state = 'ACTIVE' \
               AND active_since <= $2 \
             ORDER BY user_id ASC",
        )
        .bind(uuid::Uuid::from(product_id))
        .bind(event_time)
        .fetch_all(self.tx.connection())
        .await
        .map_err(WatchlistNotificationRecipientQueryError)?;

        Ok(rows
            .into_iter()
            .map(
                |(user_id, external_delivery_requested)| WatchlistNotificationRecipient {
                    user_id: user_id.into(),
                    external_delivery_requested,
                },
            )
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: WatchlistNotificationRecipientReadError =
            WatchlistNotificationRecipientQueryError(sqlx::Error::RowNotFound).into();

        let WatchlistNotificationRecipientReadError::QueryFailed { source } = error;
        let query_error = source
            .downcast_ref::<WatchlistNotificationRecipientQueryError>()
            .unwrap_or_else(|| panic!("expected recipient query error"));
        assert!(std::error::Error::source(query_error).is_some());
    }
}
