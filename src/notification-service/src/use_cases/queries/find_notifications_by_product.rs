use crate::ports::product_notifications_reader::{
    ProductNotificationsReadError, ProductNotificationsReader,
};
use common::user_id::UserId;
use notification_core::{
    notification::NotificationPayload, notification_id::NotificationId,
    notification_type::NotificationType,
};
use product_core::product_id::ProductId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FindNotificationsByProductRequest {
    pub user_id: UserId,
    pub product_id: ProductId,
    pub limit: Option<i32>,
    pub newest_first: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductNotificationSummary {
    pub notification_id: NotificationId,
    pub notification_type: Option<NotificationType>,
    pub payload: NotificationPayload,
    pub seen: bool,
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FindNotificationsByProductResult {
    pub items: Vec<ProductNotificationSummary>,
}

#[derive(Debug, thiserror::Error)]
pub enum FindNotificationsByProductError {
    #[error("product notification read failed")]
    ReadFailed(#[source] ProductNotificationsReadError),
}

#[async_trait::async_trait]
pub trait FindNotificationsByProductUseCase: Send + Sync {
    async fn execute(
        &self,
        request: FindNotificationsByProductRequest,
    ) -> Result<FindNotificationsByProductResult, FindNotificationsByProductError>;
}

pub struct FindNotificationsByProductHandler<R> {
    reader: R,
}

impl<R> FindNotificationsByProductHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> FindNotificationsByProductUseCase for FindNotificationsByProductHandler<R>
where
    R: ProductNotificationsReader,
{
    async fn execute(
        &self,
        request: FindNotificationsByProductRequest,
    ) -> Result<FindNotificationsByProductResult, FindNotificationsByProductError> {
        let rows = self
            .reader
            .list_by_product(
                &request.user_id,
                &request.product_id,
                request.limit,
                request.newest_first,
            )
            .await
            .map_err(FindNotificationsByProductError::ReadFailed)?;
        let items = rows
            .into_iter()
            .map(|row| ProductNotificationSummary {
                notification_id: row.notification_id,
                notification_type: row.notification_type,
                payload: row.notification_payload,
                seen: row.seen,
                external: row.external,
            })
            .collect();
        Ok(FindNotificationsByProductResult { items })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::product_notifications_reader::ProductNotificationReadItem;
    use common::{partner_shop_application_id::PartnerShopApplicationId, shop_name::ShopName};
    use notification_core::{
        notification::NotificationPartnerApplicationPayload, notification_id::NotificationId,
    };

    #[derive(Clone)]
    struct FakeReader(Vec<ProductNotificationReadItem>);

    fn item() -> ProductNotificationReadItem {
        ProductNotificationReadItem {
            user_id: UserId::new(),
            origin_event_id: common::event_id::EventId::new(),
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::PartnerApplication {
                shop_name: ShopName::from("test shop"),
                image: None,
                partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                    partner_application_id: PartnerShopApplicationId::new(),
                },
            },
            seen: false,
            external: true,
        }
    }

    #[async_trait::async_trait]
    impl ProductNotificationsReader for FakeReader {
        async fn list_by_product(
            &self,
            _user_id: &UserId,
            _product_id: &ProductId,
            _limit: Option<i32>,
            _newest_first: bool,
        ) -> Result<Vec<ProductNotificationReadItem>, ProductNotificationsReadError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn should_find_notifications_by_product_with_dedicated_view() {
        let request = FindNotificationsByProductRequest {
            user_id: UserId::new(),
            product_id: ProductId::new(),
            limit: Some(1),
            newest_first: true,
        };

        let result = FindNotificationsByProductHandler::new(FakeReader(vec![item()]))
            .execute(request)
            .await
            .expect("product read should succeed");

        assert_eq!(1, result.items.len());
    }
}
