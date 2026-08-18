use common::{
    error::boxed::BoxError, notification_id::NotificationId, product_id::ProductId, user_id::UserId,
};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ProductNotificationIdsReadError {
    #[error("unseen product notification ID read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductNotificationIdsReader: Send + Sync {
    async fn unseen_ids_for_products(
        &self,
        user_id: UserId,
        product_ids: &[ProductId],
    ) -> Result<HashMap<ProductId, Vec<NotificationId>>, ProductNotificationIdsReadError>;
}
