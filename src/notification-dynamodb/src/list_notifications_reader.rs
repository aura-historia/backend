use crate::{
    notification_record::{NotificationRecord, mk_pk, mk_sk},
    reader_common::{SK_LOWER_BOUND, SK_UPPER_BOUND, deserialize_records},
};
use application::error::box_error;
use application::pagination::Cursor;
use aws_sdk_dynamodb::{Client, types::AttributeValue};
use domain_primitives::event_id::EventId;
use notification_core::notification::Notification;
use notification_service::ports::list_notifications_reader::{
    ListNotificationsReadError, ListNotificationsReader, NotificationListReadItem,
};
use std::collections::HashMap;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct DynamoDbListNotificationsReader<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> DynamoDbListNotificationsReader<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

fn list_item_from_record(
    record: NotificationRecord,
) -> Result<NotificationListReadItem, ListNotificationsReadError> {
    let created = record.created;
    let updated = record.updated;
    let notification = Notification::try_from(record).map_err(|source| {
        ListNotificationsReadError::InvalidReadModel {
            source: box_error(source),
        }
    })?;
    Ok(NotificationListReadItem {
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
impl ListNotificationsReader for DynamoDbListNotificationsReader<'_> {
    async fn list_by_user(
        &self,
        user_id: &UserId,
        cursor: &Cursor<EventId>,
        newest_first: bool,
    ) -> Result<Vec<NotificationListReadItem>, ListNotificationsReadError> {
        let exclusive_start_key: Option<HashMap<String, AttributeValue>> =
            cursor.search_after.map(|id| {
                [
                    ("pk".to_string(), AttributeValue::S(mk_pk(user_id))),
                    ("sk".to_string(), AttributeValue::S(mk_sk(&id))),
                ]
                .into()
            });

        let items = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND #sk BETWEEN :sk_lower AND :sk_upper")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(":sk_lower", AttributeValue::S(SK_LOWER_BOUND.to_string()))
            .expression_attribute_values(":sk_upper", AttributeValue::S(SK_UPPER_BOUND.to_string()))
            .set_exclusive_start_key(exclusive_start_key)
            .limit(cursor.size as i32)
            .scan_index_forward(!newest_first)
            .send()
            .await
            .map_err(|source| ListNotificationsReadError::OperationFailed {
                source: box_error(source),
            })?
            .items
            .unwrap_or_default();

        deserialize_records(user_id, items)
            .into_iter()
            .map(list_item_from_record)
            .collect()
    }

    async fn count_by_user(
        &self,
        user_id: &UserId,
        cursor: &Cursor<EventId>,
        newest_first: bool,
    ) -> Result<u64, ListNotificationsReadError> {
        let exclusive_start_key: Option<HashMap<String, AttributeValue>> =
            cursor.search_after.map(|id| {
                [
                    ("pk".to_string(), AttributeValue::S(mk_pk(user_id))),
                    ("sk".to_string(), AttributeValue::S(mk_sk(&id))),
                ]
                .into()
            });

        let count = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND #sk BETWEEN :sk_lower AND :sk_upper")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(":sk_lower", AttributeValue::S(SK_LOWER_BOUND.to_string()))
            .expression_attribute_values(":sk_upper", AttributeValue::S(SK_UPPER_BOUND.to_string()))
            .set_exclusive_start_key(exclusive_start_key)
            .scan_index_forward(!newest_first)
            .select(aws_sdk_dynamodb::types::Select::Count)
            .send()
            .await
            .map_err(|source| ListNotificationsReadError::OperationFailed {
                source: box_error(source),
            })?
            .count;

        Ok(count as u64)
    }
}
