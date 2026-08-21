use crate::{
    notification_record::{NotificationRecord, mk_lsi2_sk_product_prefix, mk_pk},
    reader_common::deserialize_records,
};
use application::error::box_error;
use aws_sdk_dynamodb::{Client, types::AttributeValue};
use notification_core::notification::Notification;
use notification_service::ports::product_notifications_reader::{
    ProductNotificationReadItem, ProductNotificationsReadError, ProductNotificationsReader,
};
use product_core::product_id::ProductId;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct DynamoDbProductNotificationsReader<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> DynamoDbProductNotificationsReader<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

fn product_item_from_record(
    record: NotificationRecord,
) -> Result<ProductNotificationReadItem, ProductNotificationsReadError> {
    let notification = Notification::try_from(record).map_err(|source| {
        ProductNotificationsReadError::InvalidReadModel {
            source: box_error(source),
        }
    })?;
    Ok(ProductNotificationReadItem {
        user_id: notification.user_id(),
        origin_event_id: notification.origin_event_id(),
        notification_id: notification.notification_id(),
        notification_type: notification.notification_type(),
        notification_payload: notification.notification_payload().clone(),
        seen: notification.seen(),
        external: notification.external(),
    })
}

#[async_trait::async_trait]
impl ProductNotificationsReader for DynamoDbProductNotificationsReader<'_> {
    async fn list_by_product(
        &self,
        user_id: &UserId,
        product_id: &ProductId,
        limit: Option<i32>,
        newest_first: bool,
    ) -> Result<Vec<ProductNotificationReadItem>, ProductNotificationsReadError> {
        let (prefix, _) = mk_lsi2_sk_product_prefix(product_id);

        let mut query_builder = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("lsi2")
            .key_condition_expression("#pk = :pk_val AND begins_with(#lsi2_sk, :prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#lsi2_sk", "lsi2_sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(":prefix", AttributeValue::S(prefix))
            .scan_index_forward(!newest_first);

        if let Some(n) = limit {
            query_builder = query_builder.limit(n);
        }

        let items = query_builder
            .send()
            .await
            .map_err(|source| ProductNotificationsReadError::OperationFailed {
                source: box_error(source),
            })?
            .items
            .unwrap_or_default();

        deserialize_records(user_id, items)
            .into_iter()
            .map(product_item_from_record)
            .collect()
    }
}
