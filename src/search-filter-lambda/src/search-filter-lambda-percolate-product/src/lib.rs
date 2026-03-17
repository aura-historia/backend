pub mod service;

use crate::service::ProductEventSearchFilterNotificationsService;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::NotificationService;
use product::core::product_event::ProductEvent;
use product::dynamodb::product_event_record::ProductEventRecord;
use tracing::{error, info, warn};

#[tracing::instrument(skip(product_event_notification_service, notification_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    product_event_notification_service: &impl ProductEventSearchFilterNotificationsService,
    notification_service: &impl NotificationService,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.");

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;

    for message in event.payload.records {
        let message_id = message
            .message_id
            .clone()
            .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");

        if let Some(product_event_record) = extract_sqs_event_bridge_dynamodb_record::<
            ProductEventRecord,
        >(
            message, &mut failed_message_ids, &mut skipped_count
        ) {
            match ProductEvent::try_from(product_event_record) {
                Ok(product_event) => {
                    let event_id = product_event.event_id;
                    let notification_cmds_res = product_event_notification_service
                        .determine_notification_commands(product_event)
                        .await;
                    match notification_cmds_res {
                        Ok(cmds) => {
                            if cmds.is_empty() {
                                continue;
                            }
                            let create_notifications_res = notification_service
                                .create_notifications(&event_id, cmds)
                                .await;
                            if !create_notifications_res.unprocessed.is_empty() {
                                warn!(
                                    messageId = message_id,
                                    unprocessed = create_notifications_res.unprocessed.len(),
                                    "Some CreateNotificationCommands were not processed. Marking message as failed to trigger retry."
                                );
                                failed_message_ids.push(message_id);
                            }
                        }
                        Err(err) => {
                            error!(messageId = message_id, error = %err, "Failed creating CreateNotificationCommands.");
                            failed_message_ids.push(message_id);
                        }
                    }
                }
                Err(err) => {
                    error!(
                        error = %err,
                        fromType = %std::any::type_name::<ProductEventRecord>(),
                        toType = %std::any::type_name::<ProductEvent>(),
                        "Failed mapping types. Skipping event."
                    );
                    skipped_count += 1;
                }
            }
        }
    }

    let failure_count = failed_message_ids.len();
    info!(
        successful = records_count - failure_count - skipped_count,
        failures = failure_count,
        skipped = skipped_count,
        "Handler finished."
    );

    let mut sqs_batch_response = SqsBatchResponse::default();
    sqs_batch_response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(|item_identifier| {
            let mut failure = BatchItemFailure::default();
            failure.item_identifier = item_identifier;
            failure
        })
        .collect();
    Ok(sqs_batch_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{
        MockProductEventSearchFilterNotificationsService,
        ProductEventSearchFilterNotificationsServiceError,
    };
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use lambda_runtime::Context;
    use notification::service::{
        command::CreateNotificationCommand,
        notification_service::{CreateNotificationsResult, MockNotificationService},
    };
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;

    fn mk_sqs_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
        let sqs_event = SqsEvent { records: messages };
        let context = Context::default();
        LambdaEvent::new(sqs_event, context)
    }

    fn mk_sqs_message(body: &str) -> SqsMessage {
        SqsMessage {
            message_id: Some("test-message-id".to_string()),
            body: Some(body.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn should_return_empty_batch_response_when_no_records() {
        let service = MockProductEventSearchFilterNotificationsService::default();
        let notification_service = MockNotificationService::default();
        let event = mk_sqs_event(vec![]);

        let result = handler(&service, &notification_service, event).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failed_batch_when_deserialization_fails() {
        let service = MockProductEventSearchFilterNotificationsService::default();
        let notification_service = MockNotificationService::default();
        let event = mk_sqs_event(vec![mk_sqs_message("{\"not\":\"a valid event\"}")]);

        let result = handler(&service, &notification_service, event).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.batch_item_failures.len(), 1);
    }

    #[tokio::test]
    async fn should_return_failed_batch_when_service_errors() {
        let mut service = MockProductEventSearchFilterNotificationsService::default();
        service
            .expect_determine_notification_commands()
            .return_once(|_| {
                Box::pin(async {
                    Err(
                        ProductEventSearchFilterNotificationsServiceError::GetProductError(
                            product::service::get_service::GetProductError::ProductNotFound(
                                fake::Faker.fake(),
                                fake::Faker.fake(),
                            ),
                        ),
                    )
                })
            });

        let notification_service = MockNotificationService::default();

        let domain_event_record: ProductDomainEventRecord = fake::Faker.fake();
        let event_bridge_body = mk_event_bridge_body(&domain_event_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&event_bridge_body)]);

        let result = handler(&service, &notification_service, event).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.batch_item_failures.len(), 1);
    }

    #[tokio::test]
    async fn should_succeed_when_notification_commands_empty() {
        let mut service = MockProductEventSearchFilterNotificationsService::default();
        service
            .expect_determine_notification_commands()
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let notification_service = MockNotificationService::default();

        let domain_event_record: ProductDomainEventRecord = fake::Faker.fake();
        let event_bridge_body = mk_event_bridge_body(&domain_event_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&event_bridge_body)]);

        let result = handler(&service, &notification_service, event).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_succeed_when_notifications_created() {
        let mut service = MockProductEventSearchFilterNotificationsService::default();
        let cmd: CreateNotificationCommand = fake::Faker.fake();
        service
            .expect_determine_notification_commands()
            .return_once(move |_| Box::pin(async move { Ok(vec![cmd]) }));

        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .return_once(|_, _| {
                Box::pin(async {
                    CreateNotificationsResult {
                        unprocessed: vec![],
                        processed: vec![],
                    }
                })
            });

        let domain_event_record: ProductDomainEventRecord = fake::Faker.fake();
        let event_bridge_body = mk_event_bridge_body(&domain_event_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&event_bridge_body)]);

        let result = handler(&service, &notification_service, event).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_when_message_body_empty() {
        let service = MockProductEventSearchFilterNotificationsService::default();
        let notification_service = MockNotificationService::default();
        let event = mk_sqs_event(vec![SqsMessage {
            message_id: Some("test-message-id".to_string()),
            body: None,
            ..Default::default()
        }]);

        let result = handler(&service, &notification_service, event).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    fn mk_event_bridge_body(record: &ProductDomainEventRecord) -> String {
        let new_image = serde_dynamo::to_item(record).unwrap();
        let new_image_json = serde_json::to_value(&new_image).unwrap();
        serde_json::json!({
            "version": "0",
            "id": "test-event-id",
            "source": "test-source",
            "account": "123456789012",
            "time": "2024-01-01T00:00:00Z",
            "region": "eu-central-1",
            "resources": [],
            "detail-type": "DynamoDBStreamRecord",
            "detail": {
                "eventID": "test-event-id",
                "eventName": "INSERT",
                "eventVersion": "1.1",
                "eventSource": "aws:dynamodb",
                "awsRegion": "eu-central-1",
                "dynamodb": {
                    "Keys": {},
                    "NewImage": new_image_json,
                    "SequenceNumber": "1",
                    "SizeBytes": 100,
                    "StreamViewType": "NEW_IMAGE"
                }
            }
        })
        .to_string()
    }
}
