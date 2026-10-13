use application::error::box_error;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationCommand, PendingSavedSearchNotificationReader,
    SavedSearchNotificationRecoveryError,
};
use sqlx::PgPool;
use user_core::user_id::UserId;

pub struct SqlxPendingSavedSearchNotificationReader {
    pool: PgPool,
}
impl SqlxPendingSavedSearchNotificationReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[async_trait::async_trait]
impl PendingSavedSearchNotificationReader for SqlxPendingSavedSearchNotificationReader {
    async fn defer(
        &self,
        command: &GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<(), SavedSearchNotificationRecoveryError> {
        sqlx::query("UPDATE search_filter_matches SET notification_recovery_after = now() + INTERVAL '15 minutes' WHERE user_id = $1 AND user_search_filter_id = $2 AND product_listing_id = $3 AND origin_event_id = $4 AND NOT notification_selection_complete")
            .bind(command.user_id.into_uuid()).bind(command.search_filter_id.into_uuid()).bind(command.product_listing_id.into_uuid()).bind(command.origin_event_id.into_uuid()).execute(&self.pool).await
            .map_err(|source| SavedSearchNotificationRecoveryError { source: box_error(source) })?;
        Ok(())
    }
    async fn find_pending(
        &self,
        limit: usize,
    ) -> Result<
        Vec<GenerateSearchFilterMatchNotificationCommand>,
        SavedSearchNotificationRecoveryError,
    > {
        let limit =
            i64::try_from(limit).map_err(|source| SavedSearchNotificationRecoveryError {
                source: box_error(source),
            })?;
        let rows = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid)>("SELECT user_id, user_search_filter_id, product_listing_id, origin_event_id FROM search_filter_matches WHERE matching_run_end IS NOT NULL AND NOT notification_selection_complete AND notification_recovery_after <= now() ORDER BY notification_recovery_after, created, origin_event_id, user_search_filter_id, product_listing_id LIMIT $1")
            .bind(limit).fetch_all(&self.pool).await.map_err(|source| SavedSearchNotificationRecoveryError { source: box_error(source) })?;
        rows.into_iter()
            .map(|(user, filter, listing, event)| {
                Ok(GenerateSearchFilterMatchNotificationCommand {
                    user_id: UserId::try_from(user).map_err(id_error)?,
                    search_filter_id: UserSearchFilterId::try_from(filter).map_err(id_error)?,
                    product_listing_id: ProductListingId::try_from(listing).map_err(id_error)?,
                    origin_event_id: EventId::try_from(event).map_err(id_error)?,
                })
            })
            .collect()
    }
}
fn id_error(
    source: domain_primitives::object_id::ObjectIdError,
) -> SavedSearchNotificationRecoveryError {
    SavedSearchNotificationRecoveryError {
        source: box_error(source),
    }
}
