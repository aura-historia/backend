use crate::dynamodb_update::DynamoDbUpdate;
use crate::{
    notification_record::{NotificationRecord, mk_pk, mk_sk},
    notification_record_update::NotificationRecordUpdate,
};
use application::error::box_error;
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::{get_item::GetItemError, put_item::PutItemError, update_item::UpdateItemError},
    types::{AttributeValue, ReturnValue},
};
use domain_primitives::event_id::EventId;
use notification_core::notification::Notification;
use notification_service::ports::notification_repository::{
    NotificationRepository, NotificationRepositoryError,
};
use time::OffsetDateTime;
use tracing::error;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct NotificationDynamoDbRepository<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> NotificationDynamoDbRepository<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    async fn insert_record(
        &self,
        record: NotificationRecord,
    ) -> Result<(), SdkError<PutItemError>> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(
                serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?,
            ))
            .send()
            .await
            .map(|_| ())
    }

    async fn find_record_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Option<NotificationRecord>, SdkError<GetItemError>> {
        let record = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(origin_event_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, NotificationRecord>)
            .and_then(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(
                        userId = %user_id,
                        originEventId = %origin_event_id,
                        error = %err,
                        r#type = %std::any::type_name::<NotificationRecord>(),
                        "Failed deserializing NotificationRecord."
                    );
                    None
                }
            });

        Ok(record)
    }

    async fn update_record(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
        update: NotificationRecordUpdate,
    ) -> Result<Option<NotificationRecord>, SdkError<UpdateItemError>> {
        let update_expr = update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(origin_event_id)))
            .update_expression(update_expr.update_expr)
            .set_expression_attribute_names(Some(update_expr.expr_attr_names))
            .set_expression_attribute_values(Some(update_expr.expr_attr_values))
            .return_values(ReturnValue::AllNew)
            .send()
            .await
            .map(|output| output.attributes)
            .map(|attr_opt| {
                attr_opt
                    .map(serde_dynamo::from_item)
                    .and_then(|record_res| match record_res {
                        Ok(record) => Some(record),
                        Err(err) => {
                            error!(
                                userId = %user_id,
                                originEventId = %origin_event_id,
                                error = %err,
                                r#type = %std::any::type_name::<NotificationRecord>(),
                                "Failed deserializing NotificationRecord."
                            );
                            None
                        }
                    })
            })
    }
}

#[async_trait::async_trait]
impl NotificationRepository for NotificationDynamoDbRepository<'_> {
    async fn insert(
        &self,
        notification: &Notification,
    ) -> Result<Notification, NotificationRepositoryError> {
        self.insert_record(NotificationRecord::from_notification(notification))
            .await
            .map_err(|source| NotificationRepositoryError::OperationFailed {
                source: box_error(source),
            })?;
        Ok(notification.clone())
    }

    async fn find_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Option<Notification>, NotificationRepositoryError> {
        let record = self
            .find_record_by_origin_event_id(user_id, origin_event_id)
            .await
            .map_err(|source| NotificationRepositoryError::OperationFailed {
                source: box_error(source),
            })?;

        record
            .map(Notification::try_from)
            .transpose()
            .map_err(
                |source| NotificationRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                },
            )
    }

    async fn update(
        &self,
        notification: &Notification,
    ) -> Result<Notification, NotificationRepositoryError> {
        let update = NotificationRecordUpdate {
            seen: Some(notification.seen()),
            notification_type: notification.notification_type().map(Into::into),
            updated: OffsetDateTime::now_utc(),
        };
        let record = self
            .update_record(
                &notification.user_id(),
                &notification.origin_event_id(),
                update,
            )
            .await
            .map_err(|source| NotificationRepositoryError::OperationFailed {
                source: box_error(source),
            })?
            .ok_or_else(|| NotificationRepositoryError::OperationFailed {
                source: application::error::static_error("notification update returned no state"),
            })?;

        Notification::try_from(record).map_err(|source| {
            NotificationRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })
    }
}
