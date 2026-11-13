use crate::listing_source_partners::partner_listing_sources;
use crate::mapping::{NotificationRow, mapping_error};
use application::error::box_error;
use notification_core::notification::Notification;
use notification_service::ports::notification_list_reader::{
    NotificationListCursor, NotificationListItem, NotificationListPage, NotificationListReadError,
    NotificationListReader,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct SqlxNotificationListReader {
    pool: PgPool,
}

impl SqlxNotificationListReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct NotificationListRow {
    notification_id: Option<uuid::Uuid>,
    user_id: Option<uuid::Uuid>,
    kind: Option<String>,
    origin_event_id: Option<uuid::Uuid>,
    product_listing_id: Option<uuid::Uuid>,
    user_search_filter_id: Option<uuid::Uuid>,
    partnership_application_id: Option<uuid::Uuid>,
    payload_version: Option<i16>,
    payload: Option<serde_json::Value>,
    seen: Option<bool>,
    created: Option<OffsetDateTime>,
    updated: Option<OffsetDateTime>,
    show_unassessed_or_sensitive_content: bool,
}

impl NotificationListRow {
    fn notification_row(self) -> Result<Option<NotificationRow>, NotificationListReadError> {
        let Self {
            notification_id,
            user_id,
            kind,
            origin_event_id,
            product_listing_id,
            user_search_filter_id,
            partnership_application_id,
            payload_version,
            payload,
            seen,
            created,
            updated,
            show_unassessed_or_sensitive_content: _,
        } = self;
        let Some(notification_id) = notification_id else {
            return Ok(None);
        };
        let Some(user_id) = user_id else {
            return Err(missing_notification_column("user_id"));
        };
        let Some(kind) = kind else {
            return Err(missing_notification_column("kind"));
        };
        let Some(payload_version) = payload_version else {
            return Err(missing_notification_column("payload_version"));
        };
        let Some(payload) = payload else {
            return Err(missing_notification_column("payload"));
        };
        let Some(seen) = seen else {
            return Err(missing_notification_column("seen"));
        };
        let Some(created) = created else {
            return Err(missing_notification_column("created"));
        };
        let Some(updated) = updated else {
            return Err(missing_notification_column("updated"));
        };

        Ok(Some(NotificationRow {
            notification_id,
            user_id,
            kind,
            origin_event_id,
            product_listing_id,
            user_search_filter_id,
            partnership_application_id,
            payload_version,
            payload,
            seen,
            created,
            updated,
        }))
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
        let limit = limit.clamp(1, 100);
        let page_size =
            usize::try_from(limit).map_err(|source| NotificationListReadError::ReadFailed {
                source: box_error(source),
            })?;
        let rows = sqlx::query_as::<_, NotificationListRow>(
            "WITH notification_page AS (SELECT n.notification_id, n.user_id, n.kind, n.origin_event_id, n.product_listing_id, n.user_search_filter_id, n.partnership_application_id, n.payload_version, n.payload, n.seen, n.created, n.updated FROM notifications n WHERE n.user_id = $1 AND ($2::timestamptz IS NULL OR (n.created, n.notification_id) < ($2, $3)) ORDER BY n.created DESC, n.notification_id DESC LIMIT $4) SELECT p.notification_id, p.user_id, p.kind, p.origin_event_id, p.product_listing_id, p.user_search_filter_id, p.partnership_application_id, p.payload_version, p.payload, p.seen, p.created, p.updated, u.show_unassessed_or_sensitive_content FROM users u LEFT JOIN notification_page p ON TRUE WHERE u.user_id = $1 ORDER BY p.created DESC NULLS LAST, p.notification_id DESC NULLS LAST",
        )
        .bind(user_id.into_uuid())
        .bind(cursor.map(|cursor| cursor.created))
        .bind(cursor.map(|cursor| cursor.notification_id.into_uuid()))
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(|source| NotificationListReadError::ReadFailed {
            source: box_error(source),
        })?;

        let show_unassessed_or_sensitive_content = rows
            .first()
            .is_some_and(|row| row.show_unassessed_or_sensitive_content);
        let mut notifications = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(row) = row.notification_row()? else {
                continue;
            };
            let (created, updated, seen) = (row.created, row.updated, row.seen);
            let notification = Notification::try_from(row).map_err(|error| {
                NotificationListReadError::InvalidReadModel {
                    source: mapping_error(error),
                }
            })?;
            notifications.push((notification, seen, created, updated));
        }
        let partner_listing_sources = partner_listing_sources(
            &self.pool,
            notifications
                .iter()
                .filter_map(|(notification, ..)| notification.content().listing_source_id()),
        )
        .await?;
        let mut items = notifications
            .into_iter()
            .map(
                |(notification, seen, created, updated)| NotificationListItem {
                    notification_id: notification.notification_id(),
                    listing_source_partner: partner_listing_sources
                        .names_partner(notification.content()),
                    content: notification.content().clone(),
                    seen,
                    created,
                    updated,
                },
            )
            .collect::<Vec<_>>();

        let has_more = items.len() > page_size;
        if has_more {
            let _ = items.pop();
        }
        let next_cursor = if has_more {
            items.last().map(|item| NotificationListCursor {
                created: item.created,
                notification_id: item.notification_id,
            })
        } else {
            None
        };
        Ok(NotificationListPage {
            items,
            next_cursor,
            show_unassessed_or_sensitive_content,
        })
    }
}

fn missing_notification_column(column: &'static str) -> NotificationListReadError {
    NotificationListReadError::InvalidReadModel {
        source: box_error(std::io::Error::other(format!(
            "notification list row is missing {column}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing_source_partners::tests::{dissolve_partnership, seed_listing_source};
    use listing_source_core::ListingSourceId;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn seed_watchlist_notification(
        pool: &PgPool,
        user_id: UserId,
        listing_source_id: ListingSourceId,
    ) {
        sqlx::query("INSERT INTO notifications (notification_id, user_id, kind, origin_event_id, product_listing_id, payload_version, payload) VALUES ($1, $2, 'WATCHLIST_PRICE_CHANGED', $3, $4, 1, $5)")
            .bind(uuid::Uuid::now_v7())
            .bind(user_id.into_uuid())
            .bind(uuid::Uuid::now_v7())
            .bind(uuid::Uuid::now_v7())
            .bind(serde_json::json!({
                "type": "WATCHLIST",
                "snapshot": {
                    "listing_source_id": listing_source_id.into_uuid().to_string(),
                    "source_listing_id": "source-product",
                    "listing_source_slug_id": "test-source",
                    "product_listing_title_slug_id": "test-product-a1b2c3",
                    "listing_source_name": "Snapshot Source",
                    "title": null,
                    "image": null,
                    "url": "https://source.example/product",
                    "view_url": "https://source.example/product?view=1"
                },
                "change": { "type": "PRICE_CHANGE", "old_price": null, "new_price": null }
            }))
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("seed notification: {error}"));
    }

    async fn partner_flags(reader: &SqlxNotificationListReader, user_id: UserId) -> Vec<bool> {
        let page = reader
            .list_for_user(user_id, None, 100)
            .await
            .unwrap_or_else(|error| panic!("list notifications: {error}"));
        let mut flags = page
            .items
            .iter()
            .map(|item| item.listing_source_partner)
            .collect::<Vec<_>>();
        flags.sort_unstable();
        flags
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
    async fn should_resolve_listing_source_partner_on_read_until_the_partnership_ends() {
        let pool = get_postgres_client().await;
        let user_id = UserId::new();
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@example.test"))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("seed user: {error}"));
        let partner = seed_listing_source(&pool, Some("ACTIVE")).await;
        let other = seed_listing_source(&pool, None).await;
        seed_watchlist_notification(&pool, user_id, partner).await;
        seed_watchlist_notification(&pool, user_id, other).await;
        let reader = SqlxNotificationListReader::new(pool.clone());

        assert_eq!(vec![false, true], partner_flags(&reader, user_id).await);

        dissolve_partnership(&pool, partner).await;

        assert_eq!(vec![false, false], partner_flags(&reader, user_id).await);
    }
}
