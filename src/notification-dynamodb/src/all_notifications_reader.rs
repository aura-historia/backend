use crate::{
    notification_record::{NotificationRecord, mk_pk},
    reader_common::{SK_LOWER_BOUND, SK_UPPER_BOUND, deserialize_records},
};
use application::error::box_error;
use aws_sdk_dynamodb::{Client, types::AttributeValue};
use notification_core::notification::Notification;
use notification_service::ports::all_notifications_reader::{
    AllNotificationsReadError, AllNotificationsReadItem, AllNotificationsReader,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct DynamoDbAllNotificationsReader<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> DynamoDbAllNotificationsReader<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

fn all_item_from_record(
    record: NotificationRecord,
) -> Result<AllNotificationsReadItem, AllNotificationsReadError> {
    let created = record.created;
    let updated = record.updated;
    let notification = Notification::try_from(record).map_err(|source| {
        AllNotificationsReadError::InvalidReadModel {
            source: box_error(source),
        }
    })?;
    Ok(AllNotificationsReadItem {
        user_id: notification.user_id(),
        origin_event_id: notification.origin_event_id(),
        notification_id: notification.notification_id(),
        notification_type: notification.notification_type(),
        notification_payload: notification.notification_payload().clone(),
        seen: notification.seen(),
        external: notification.external(),
        created,
        updated,
    })
}

#[async_trait::async_trait]
impl AllNotificationsReader for DynamoDbAllNotificationsReader<'_> {
    async fn list_all_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError> {
        let records = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND #sk BETWEEN :sk_lower AND :sk_upper")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(":sk_lower", AttributeValue::S(SK_LOWER_BOUND.to_string()))
            .expression_attribute_values(":sk_upper", AttributeValue::S(SK_UPPER_BOUND.to_string()))
            .into_paginator()
            .send()
            .try_collect()
            .await
            .map_err(|source| AllNotificationsReadError::OperationFailed {
                source: box_error(source),
            })?
            .into_iter()
            .flat_map(|query_output| query_output.items.unwrap_or_default())
            .collect();

        deserialize_records(user_id, records)
            .into_iter()
            .map(all_item_from_record)
            .collect()
    }
}
