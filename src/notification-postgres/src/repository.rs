use crate::mapping::{NotificationWriteValues, PAYLOAD_VERSION, mapping_error};
use common::{error::boxed::box_error, postgres::SqlxTransaction};
use notification_core::notification_delivery_id::NotificationDeliveryId;
use notification_service::ports::notification_creator::{
    NewNotification, NotificationCreationError, NotificationCreationOutcome, NotificationCreator,
    NotificationCreatorFactory,
};
use sqlx::{PgConnection, Postgres, QueryBuilder};
use std::collections::HashSet;

const INSERT_CHUNK_SIZE: usize = 500;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxNotificationCreatorFactory;

struct SqlxNotificationCreator<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxNotificationCreatorFactory {
    pub fn new() -> Self {
        Self
    }
}

impl NotificationCreatorFactory<SqlxTransaction> for SqlxNotificationCreatorFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl NotificationCreator + 'tx {
        SqlxNotificationCreator {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl NotificationCreator for SqlxNotificationCreator<'_> {
    async fn create_many(
        &mut self,
        notifications: &[NewNotification],
    ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
        if notifications.is_empty() {
            return Ok(Vec::new());
        }
        let values = notifications
            .iter()
            .map(|item| NotificationWriteValues::try_from(&item.notification))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| NotificationCreationError::CreateFailed {
                source: mapping_error(error),
            })?;
        let mut inserted = HashSet::new();
        for values in values.chunks(INSERT_CHUNK_SIZE) {
            inserted.extend(insert_notifications(self.connection, values).await?);
        }
        insert_email_deliveries(self.connection, notifications, &inserted).await?;
        Ok(notifications
            .iter()
            .map(|item| {
                let id = uuid::Uuid::from(item.notification.notification_id());
                if inserted.contains(&id) {
                    NotificationCreationOutcome::Inserted {
                        notification_id: item.notification.notification_id(),
                    }
                } else {
                    NotificationCreationOutcome::Duplicate
                }
            })
            .collect())
    }
}

async fn insert_notifications(
    connection: &mut PgConnection,
    values: &[NotificationWriteValues],
) -> Result<Vec<uuid::Uuid>, NotificationCreationError> {
    let mut inserted = Vec::new();
    for kind in [
        "WATCHLIST_PRICE_CHANGED",
        "WATCHLIST_STATE_CHANGED",
        "SEARCH_FILTER_MATCH",
        "PARTNER_APPLICATION_APPROVED",
        "PARTNER_APPLICATION_REJECTED",
    ] {
        let group = values
            .iter()
            .filter(|value| value.kind == kind)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO notifications (notification_id, user_id, kind, origin_event_id, product_id, user_search_filter_id, partner_shop_application_id, payload_version, payload, seen) ",
        );
        query.push_values(group, |mut row, value| {
            row.push_bind(value.notification_id)
                .push_bind(value.user_id)
                .push_bind(value.kind)
                .push_bind(value.origin_event_id)
                .push_bind(value.product_id)
                .push_bind(value.user_search_filter_id)
                .push_bind(value.partner_shop_application_id)
                .push_bind(PAYLOAD_VERSION)
                .push_bind(&value.payload)
                .push_bind(false);
        });
        match kind {
            "WATCHLIST_PRICE_CHANGED" | "WATCHLIST_STATE_CHANGED" => query.push(" ON CONFLICT (user_id, origin_event_id, kind) WHERE kind IN ('WATCHLIST_PRICE_CHANGED', 'WATCHLIST_STATE_CHANGED') DO NOTHING"),
            "SEARCH_FILTER_MATCH" => query.push(" ON CONFLICT (user_id, user_search_filter_id, product_id, origin_event_id) WHERE kind = 'SEARCH_FILTER_MATCH' DO NOTHING"),
            "PARTNER_APPLICATION_APPROVED" | "PARTNER_APPLICATION_REJECTED" => query.push(" ON CONFLICT (user_id, partner_shop_application_id) WHERE kind IN ('PARTNER_APPLICATION_APPROVED', 'PARTNER_APPLICATION_REJECTED') DO NOTHING"),
            _ => unreachable!(),
        };
        query.push(" RETURNING notification_id");
        let ids = query
            .build_query_scalar::<uuid::Uuid>()
            .fetch_all(&mut *connection)
            .await
            .map_err(|source| NotificationCreationError::CreateFailed {
                source: box_error(source),
            })?;
        inserted.extend(ids);
    }
    Ok(inserted)
}

async fn insert_email_deliveries(
    connection: &mut PgConnection,
    notifications: &[NewNotification],
    inserted: &HashSet<uuid::Uuid>,
) -> Result<(), NotificationCreationError> {
    let deliveries = notifications
        .iter()
        .filter_map(|item| {
            let notification_id = uuid::Uuid::from(item.notification.notification_id());
            (item.external_delivery_requested && inserted.contains(&notification_id)).then(|| {
                (
                    uuid::Uuid::from(NotificationDeliveryId::new()),
                    notification_id,
                )
            })
        })
        .collect::<Vec<_>>();
    for deliveries in deliveries.chunks(INSERT_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel) ",
        );
        query.push_values(deliveries, |mut row, (delivery_id, notification_id)| {
            row.push_bind(*delivery_id)
                .push_bind(*notification_id)
                .push_bind("EMAIL");
        });
        query
            .build()
            .execute(&mut *connection)
            .await
            .map_err(|source| NotificationCreationError::CreateFailed {
                source: box_error(source),
            })?;
    }
    Ok(())
}
