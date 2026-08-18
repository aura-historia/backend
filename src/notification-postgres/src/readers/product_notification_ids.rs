use common::{
    error::boxed::box_error, notification_id::NotificationId, product_id::ProductId,
    user_id::UserId,
};
use notification_service::ports::product_notification_ids_reader::{
    ProductNotificationIdsReadError, ProductNotificationIdsReader,
};
use sqlx::PgPool;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SqlxProductNotificationIdsReader {
    pool: PgPool,
}
impl SqlxProductNotificationIdsReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ProductNotificationIdRow {
    product_id: uuid::Uuid,
    notification_id: uuid::Uuid,
}

#[async_trait::async_trait]
impl ProductNotificationIdsReader for SqlxProductNotificationIdsReader {
    async fn unseen_ids_for_products(
        &self,
        user_id: UserId,
        product_ids: &[ProductId],
    ) -> Result<HashMap<ProductId, Vec<NotificationId>>, ProductNotificationIdsReadError> {
        if product_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let product_ids = product_ids
            .iter()
            .copied()
            .map(uuid::Uuid::from)
            .collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, ProductNotificationIdRow>(
            "SELECT product_id, notification_id FROM notifications WHERE user_id = $1 AND product_id = ANY($2) AND seen = false ORDER BY product_id, created DESC, notification_id DESC",
        ).bind(uuid::Uuid::from(user_id)).bind(product_ids).fetch_all(&self.pool).await
            .map_err(|source| ProductNotificationIdsReadError::ReadFailed { source: box_error(source) })?;
        Ok(rows.into_iter().fold(HashMap::new(), |mut ids, row| {
            ids.entry(ProductId::from(row.product_id))
                .or_insert_with(Vec::new)
                .push(NotificationId::from(row.notification_id));
            ids
        }))
    }
}
