pub mod service;

use crate::service::ProductEventSearchFilterNotificationsService;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use common::user_search_filter_name::UserSearchFilterName;
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::NotificationService;
use product::core::product_event::{ProductDomainEvent, ProductEventPayload};
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use search_filter::core::search_filter_product_match::SearchFilterProductMatch;
use search_filter::service::user_search_filter_service::UserSearchFilterService;
use time::OffsetDateTime;
use tracing::{error, info, warn};

#[tracing::instrument(skip(product_event_notification_service, notification_service, search_filter_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    product_event_notification_service: &impl ProductEventSearchFilterNotificationsService,
    notification_service: &impl NotificationService,
    search_filter_service: &impl UserSearchFilterService,
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
                        Ok(cmds_with_reasons) => {
                            if cmds_with_reasons.is_empty() {
                                continue;
                            }

                            // Collect product match info (including enhanced_match_reason) before creating notifications
                            let match_info: Vec<_> = cmds_with_reasons
                                .iter()
                                .filter_map(|cmd_with_reason| {
                                    if let notification::core::notification::NotificationPayload::SearchFilter {
                                        product_id,
                                        shop_id,
                                        shops_product_id,
                                        search_filter_payload,
                                        ..
                                    } = &cmd_with_reason.command.notification_payload
                                    {
                                        Some((
                                            cmd_with_reason.command.user_id,
                                            search_filter_payload.user_search_filter_id,
                                            search_filter_payload.user_search_filter_name.clone(),
                                            *shop_id,
                                            shops_product_id.clone(),
                                            *product_id,
                                            cmd_with_reason.enhanced_match_reason.clone(),
                                        ))
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            let cmds = cmds_with_reasons.into_iter().map(|c| c.command).collect();

                            let create_notifications_res = notification_service
                                .create_notifications(&event_id, cmds)
                                .await;

                            // Create search-filter product-matches for every successfully created notification
                            let now = OffsetDateTime::now_utc();
                            let product_matches: Vec<SearchFilterProductMatch> =
                                create_notifications_res
                                    .processed
                                    .iter()
                                    .filter_map(|notification| {
                                        match_info
                                            .iter()
                                            .find(|(user_id, _, _, _, _, _, _)| {
                                                *user_id == notification.user_id
                                            })
                                            .map(
                                                |(
                                                    user_id,
                                                    search_filter_id,
                                                    search_filter_name,
                                                    shop_id,
                                                    shops_product_id,
                                                    product_id,
                                                    enhanced_match_reason,
                                                )| {
                                                    SearchFilterProductMatch {
                                                        user_id: *user_id,
                                                        user_search_filter_id: *search_filter_id,
                                                        user_search_filter_name: Some(
                                                            UserSearchFilterName::from(
                                                                search_filter_name.as_ref(),
                                                            ),
                                                        ),
                                                        shop_id: *shop_id,
                                                        shops_product_id: shops_product_id.clone(),
                                                        product_id: *product_id,
                                                        origin_event_id: event_id,
                                                        enhanced_match_reason:
                                                            enhanced_match_reason.clone(),
                                                        created: now,
                                                        updated: now,
                                                    }
                                                },
                                            )
                                    })
                                    .collect();

                            if !product_matches.is_empty() {
                                let match_result = search_filter_service
                                    .create_search_filter_product_matches(product_matches)
                                    .await;
                                match match_result {
                                    Ok(result) if !result.unprocessed.is_empty() => {
                                        warn!(
                                            messageId = message_id,
                                            unprocessed = result.unprocessed.len(),
                                            "Some SearchFilterProductMatches were not persisted. Marking message as failed."
                                        );
                                        failed_message_ids.push(message_id.clone());
                                    }
                                    Err(err) => {
                                        warn!(
                                            messageId = message_id,
                                            error = %err,
                                            "Failed creating SearchFilterProductMatches. Marking message as failed."
                                        );
                                        failed_message_ids.push(message_id.clone());
                                    }
                                    _ => {}
                                }
                            }

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
        MockProductEventSearchFilterNotificationsService, NotificationCommandWithMatchReason,
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
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;

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
        let search_filter_service = MockUserSearchFilterService::default();
        let event = mk_sqs_event(vec![]);

        let result = handler(
            &service,
            &notification_service,
            &search_filter_service,
            event,
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failed_batch_when_deserialization_fails() {
        let service = MockProductEventSearchFilterNotificationsService::default();
        let notification_service = MockNotificationService::default();
        let search_filter_service = MockUserSearchFilterService::default();
        let event = mk_sqs_event(vec![mk_sqs_message("{\"not\":\"a valid event\"}")]);

        let result = handler(
            &service,
            &notification_service,
            &search_filter_service,
            event,
        )
        .await;

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
        let search_filter_service = MockUserSearchFilterService::default();

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
        let event_bridge_body = mk_event_bridge_body(&domain_event_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&event_bridge_body)]);

        let result = handler(
            &service,
            &notification_service,
            &search_filter_service,
            event,
        )
        .await;

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
        let search_filter_service = MockUserSearchFilterService::default();

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
        let event_bridge_body = mk_event_bridge_body(&domain_event_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&event_bridge_body)]);

        let result = handler(
            &service,
            &notification_service,
            &search_filter_service,
            event,
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_succeed_when_notifications_created() {
        let mut service = MockProductEventSearchFilterNotificationsService::default();
        let cmd: CreateNotificationCommand = Faker.fake();
        let cmd_with_reason = NotificationCommandWithMatchReason {
            command: cmd,
            enhanced_match_reason: None,
        };
        service
            .expect_determine_notification_commands()
            .return_once(move |_| Box::pin(async move { Ok(vec![cmd_with_reason]) }));

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

        let search_filter_service = MockUserSearchFilterService::default();

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
        let event_bridge_body = mk_event_bridge_body(&domain_event_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&event_bridge_body)]);

        let result = handler(
            &service,
            &notification_service,
            &search_filter_service,
            event,
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_when_message_body_empty() {
        let service = MockProductEventSearchFilterNotificationsService::default();
        let notification_service = MockNotificationService::default();
        let search_filter_service = MockUserSearchFilterService::default();
        let mut msg = SqsMessage::default();
        msg.message_id = Some("test-message-id".to_string());
        msg.body = None;
        let event = mk_sqs_event(vec![msg]);

        let result = handler(
            &service,
            &notification_service,
            &search_filter_service,
            event,
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.batch_item_failures.is_empty());
    }

    fn mk_event_bridge_body(record: &ProductDomainEventRecord) -> String {
        use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
        use aws_lambda_events::eventbridge::EventBridgeEvent;

        let new_image = serde_dynamo::to_item(record).unwrap();

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
