use crate::batch::Batch;
use crate::notification_record::{mk_pk, mk_sk};
use application::error::box_error;
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::{
        batch_write_item::{BatchWriteItemError, BatchWriteItemOutput},
        delete_item::DeleteItemError,
    },
    types::{AttributeValue, DeleteRequest, WriteRequest},
};
use domain_primitives::event_id::EventId;
use notification_service::ports::notification_deleter::{
    NotificationDeleteError, NotificationDeleter,
};
use std::collections::HashMap;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct DynamoDbNotificationDeleter<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> DynamoDbNotificationDeleter<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    async fn delete_record_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<(), SdkError<DeleteItemError>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(origin_event_id)))
            .send()
            .await
            .map(|_| ())
    }

    async fn delete_record_batch_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_ids: &Batch<EventId, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError>> {
        let write_requests: Vec<WriteRequest> = origin_event_ids
            .iter()
            .map(|id| {
                let mut key = HashMap::new();
                key.insert("pk".to_string(), AttributeValue::S(mk_pk(user_id)));
                key.insert("sk".to_string(), AttributeValue::S(mk_sk(id)));
                DeleteRequest::builder()
                    .set_key(Some(key))
                    .build()
                    .map(|delete_request| {
                        WriteRequest::builder()
                            .delete_request(delete_request)
                            .build()
                    })
            })
            .collect::<Result<_, _>>()
            .map_err(SdkError::construction_failure)?;

        self.client
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(self.table.clone(), write_requests)])))
            .send()
            .await
    }
}

#[async_trait::async_trait]
impl NotificationDeleter for DynamoDbNotificationDeleter<'_> {
    async fn delete_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<(), NotificationDeleteError> {
        self.delete_record_by_origin_event_id(user_id, origin_event_id)
            .await
            .map_err(|source| NotificationDeleteError::OperationFailed {
                source: box_error(source),
            })
    }

    async fn delete_many_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_ids: &[EventId],
    ) -> Result<(), NotificationDeleteError> {
        if origin_event_ids.is_empty() {
            return Ok(());
        }

        for batch in Batch::<EventId, 25>::chunked_from(origin_event_ids.iter().copied()) {
            self.delete_record_batch_by_origin_event_id(user_id, &batch)
                .await
                .map_err(|source| NotificationDeleteError::OperationFailed {
                    source: box_error(source),
                })?;
        }

        Ok(())
    }
}
