use crate::mapping::{NotificationRow, mapping_error};
use common::{error::boxed::box_error, notification_id::NotificationId, user_id::UserId};
use notification_core::notification::Notification;
use notification_service::ports::notification_list_reader::{
    NotificationListCursor, NotificationListItem, NotificationListPage, NotificationListReadError,
    NotificationListReader,
};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct SqlxNotificationListReader {
    pool: PgPool,
}

impl SqlxNotificationListReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl NotificationListReader for SqlxNotificationListReader {
    async fn list_for_user(
        &self,
        user_id: UserId,
        cursor: Option<NotificationListCursor>,
        limit: u32,
    ) -> Result<NotificationListPage, NotificationListReadError> {
        let rows = sqlx::query_as::<_, NotificationRow>(
            "SELECT notification_id, user_id, kind, origin_event_id, product_id, user_search_filter_id, partner_shop_application_id, payload_version, payload, seen, created, updated FROM notifications WHERE user_id = $1 AND ($2::timestamptz IS NULL OR (created, notification_id) < ($2, $3)) ORDER BY created DESC, notification_id DESC LIMIT $4",
        ).bind(uuid::Uuid::from(user_id))
            .bind(cursor.map(|cursor| cursor.created))
            .bind(cursor.map(|cursor| uuid::Uuid::from(cursor.notification_id)))
            .bind(i64::from(limit.clamp(1, 100)))
            .fetch_all(&self.pool).await
            .map_err(|source| NotificationListReadError::ReadFailed { source: box_error(source) })?;
        let items = rows
            .into_iter()
            .map(|row| {
                let notification_id = NotificationId::from(row.notification_id);
                let created = row.created;
                let updated = row.updated;
                let seen = row.seen;
                let content = Notification::try_from(row)
                    .map_err(|error| NotificationListReadError::InvalidReadModel {
                        source: mapping_error(error),
                    })?
                    .content()
                    .clone();
                Ok(NotificationListItem {
                    notification_id,
                    content,
                    seen,
                    created,
                    updated,
                })
            })
            .collect::<Result<Vec<_>, NotificationListReadError>>()?;
        let next_cursor = items.last().map(|item| NotificationListCursor {
            created: item.created,
            notification_id: item.notification_id,
        });
        Ok(NotificationListPage { items, next_cursor })
    }
}
