use application::error::box_error;
use aws_sdk_dynamodb::{Client, error::SdkError, operation::put_item::PutItemError};
use notification_core::notification::Notification;
use notification_service::ports::{
    NotificationWriteError, NotificationWriteOutcome, NotificationWriter,
};

use crate::notification_record::NotificationRecord;

#[derive(Debug, Clone)]
pub struct ConditionalDynamoDbNotificationWriter {
    client: Client,
    table: String,
}

impl ConditionalDynamoDbNotificationWriter {
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    async fn insert_if_absent(
        &self,
        record: NotificationRecord,
    ) -> Result<(), SdkError<PutItemError>> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(
                serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?,
            ))
            .condition_expression("attribute_not_exists(pk) AND attribute_not_exists(sk)")
            .send()
            .await
            .map(|_| ())
    }
}

#[async_trait::async_trait]
impl NotificationWriter for ConditionalDynamoDbNotificationWriter {
    async fn insert(
        &self,
        notification: &Notification,
    ) -> Result<NotificationWriteOutcome, NotificationWriteError> {
        match self
            .insert_if_absent(NotificationRecord::from_notification(notification))
            .await
        {
            Ok(()) => Ok(NotificationWriteOutcome::Inserted(notification.clone())),
            Err(SdkError::ServiceError(error))
                if error.err().is_conditional_check_failed_exception() =>
            {
                Ok(NotificationWriteOutcome::AlreadyExists)
            }
            Err(source) => Err(NotificationWriteError::WriteFailed {
                source: box_error(source),
            }),
        }
    }
}
