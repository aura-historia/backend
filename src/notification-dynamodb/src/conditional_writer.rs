use aws_sdk_dynamodb::{Client, error::SdkError, operation::put_item::PutItemError};
use common::error::boxed::box_error;
use notification_core::notification::Notification;
use notification_service::ports::{
    NotificationWriter, notification_repository::NotificationRepositoryError,
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
    ) -> Result<Notification, NotificationRepositoryError> {
        match self
            .insert_if_absent(NotificationRecord::from_notification(notification))
            .await
        {
            Ok(()) => Ok(notification.clone()),
            Err(SdkError::ServiceError(error))
                if error.err().is_conditional_check_failed_exception() =>
            {
                Ok(notification.clone())
            }
            Err(source) => Err(NotificationRepositoryError::OperationFailed {
                source: box_error(source),
            }),
        }
    }
}
