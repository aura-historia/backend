pub mod service;

use crate::service::ProductEventSearchFilterNotificationsService;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::NotificationService;
use product::core::product_event::{ProductDomainEvent, ProductEventPayload};
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
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
            ProductDomainEventRecord,
        >(
            message, &mut failed_message_ids, &mut skipped_count
        ) {
            match ProductDomainEvent::try_from(product_event_record) {
                Ok(domain_event) => {
                    let event_id = domain_event.event_id;
                    let product_event = domain_event.map_payload(ProductEventPayload::from);
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
                        fromType = %std::any::type_name::<ProductDomainEventRecord>(),
                        toType = %std::any::type_name::<ProductDomainEvent>(),
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
    use fake::{Fake, Faker};
    use lambda_runtime::Context;
    use notification::service::{
        command::CreateNotificationCommand,
        notification_service::{CreateNotificationsResult, MockNotificationService},
    };
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;

    fn mk_sqs_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = messages;
        let context = Context::default();
        LambdaEvent::new(sqs_event, context)
    }

    fn mk_sqs_message(body: &str) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some("test-message-id".to_string());
        msg.body = Some(body.to_string());
        msg
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
                                Faker.fake(),
                                Faker.fake(),
                            ),
                        ),
                    )
                })
            });

        let notification_service = MockNotificationService::default();

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
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

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
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
        let cmd: CreateNotificationCommand = Faker.fake();
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

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
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
        let mut msg = SqsMessage::default();
        msg.message_id = Some("test-message-id".to_string());
        msg.body = None;
        let event = mk_sqs_event(vec![msg]);

        let result = handler(&service, &notification_service, event).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    fn mk_event_bridge_body(record: &ProductDomainEventRecord) -> String {
        use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
        use aws_lambda_events::eventbridge::EventBridgeEvent;

        let new_image = serde_dynamo::to_item(record.clone()).unwrap();

        let mut stream_record = StreamRecord::default();
        stream_record.new_image = new_image;

        let mut event_record = EventRecord::default();
        event_record.event_name = "INSERT".to_string();
        event_record.change = stream_record;

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail_type = "DynamoDBStreamRecord".to_string();
        event.source = "test-source".to_string();
        event.detail = event_record;

        serde_json::to_string(&event).unwrap()
    }
}
