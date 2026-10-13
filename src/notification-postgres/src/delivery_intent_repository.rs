use crate::delivery_mapping::channel_to_persisted;
use application::error::box_error;
use notification_service::ports::{
    notification_creator::NotificationCreationError,
    notification_delivery_intent_repository::{
        NewNotificationDeliveryIntent, NotificationDeliveryIntentRepository,
        NotificationDeliveryIntentRepositoryFactory,
    },
};
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};

const INSERT_CHUNK_SIZE: usize = 500;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxNotificationDeliveryIntentRepositoryFactory;

struct SqlxNotificationDeliveryIntentRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxNotificationDeliveryIntentRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl NotificationDeliveryIntentRepositoryFactory<SqlxTransaction>
    for SqlxNotificationDeliveryIntentRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl NotificationDeliveryIntentRepository + 'tx {
        SqlxNotificationDeliveryIntentRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl NotificationDeliveryIntentRepository for SqlxNotificationDeliveryIntentRepository<'_> {
    async fn stage_saved_search(
        &mut self,
        notification_id: notification_core::notification_id::NotificationId,
        run_end: time::OffsetDateTime,
    ) -> Result<(), NotificationCreationError> {
        // Serialize population against finalization. Late members remain unassigned
        // and are selected by the next user/run group; finalized membership is immutable.
        sqlx::query("SELECT g.user_id FROM saved_search_digest_groups g JOIN notifications n ON n.user_id = g.user_id WHERE g.window_end = $1 AND n.notification_id = $2 FOR UPDATE OF g")
            .bind(run_end).bind(notification_id.into_uuid()).fetch_one(&mut *self.connection).await
            .map_err(|source| NotificationCreationError::CreateFailed { source: box_error(source) })?;
        sqlx::query(r#"INSERT INTO saved_search_digest_members(user_id, user_search_filter_id, product_listing_id, origin_event_id, notification_id, matching_run_end, snapshot)
            SELECT user_id, user_search_filter_id, product_listing_id, origin_event_id, notification_id, $2, to_jsonb(n)
            FROM notifications n WHERE notification_id = $1 AND kind = 'SEARCH_FILTER_MATCH'
            ON CONFLICT (user_id, user_search_filter_id, product_listing_id) DO NOTHING"#)
            .bind(notification_id.into_uuid()).bind(run_end).execute(&mut *self.connection).await
            .map_err(|source| NotificationCreationError::CreateFailed { source: box_error(source) })?;
        Ok(())
    }

    async fn insert_many(
        &mut self,
        deliveries: &[NewNotificationDeliveryIntent],
    ) -> Result<(), NotificationCreationError> {
        for deliveries in deliveries.chunks(INSERT_CHUNK_SIZE) {
            let mut query = QueryBuilder::<Postgres>::new(
                "INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key) ",
            );
            query.push_values(deliveries, |mut row, delivery| {
                row.push_bind(delivery.notification_delivery_id.into_uuid())
                    .push_bind(delivery.notification_id.into_uuid())
                    .push_bind(channel_to_persisted(delivery.plan.channel))
                    .push_bind(delivery.plan.target_key.as_str());
            });
            query
                .build()
                .execute(&mut *self.connection)
                .await
                .map_err(|source| NotificationCreationError::CreateFailed {
                    source: box_error(source),
                })?;
        }
        Ok(())
    }
}
