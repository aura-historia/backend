use crate::batch::Batch;
use crate::notification_record::NotificationRecord;
use application::error::box_error;
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::batch_write_item::{BatchWriteItemError, BatchWriteItemOutput},
};
use notification_core::notification::Notification;
use notification_service::ports::notification_batch_inserter::{
    NotificationBatchInsertError, NotificationBatchInserter,
};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct DynamoDbNotificationBatchInserter<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> DynamoDbNotificationBatchInserter<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    async fn insert_record_batch(
        &self,
        records: Batch<NotificationRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError>> {
        self.client
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(
                self.table.clone(),
                records.into_dynamodb_write_requests(),
            )])))
            .send()
            .await
    }
}

#[async_trait::async_trait]
impl NotificationBatchInserter for DynamoDbNotificationBatchInserter<'_> {
    async fn insert_many(
        &self,
        notifications: &[Notification],
    ) -> Result<Vec<Notification>, NotificationBatchInsertError> {
        if notifications.is_empty() {
            return Ok(Vec::new());
        }

        let records = notifications
            .iter()
            .map(NotificationRecord::from_notification)
            .collect::<Vec<_>>();
        for batch in Batch::<NotificationRecord, 25>::chunked_from(records) {
            self.insert_record_batch(batch).await.map_err(|source| {
                NotificationBatchInsertError::OperationFailed {
                    source: box_error(source),
                }
            })?;
        }

        Ok(notifications.to_vec())
    }
}
