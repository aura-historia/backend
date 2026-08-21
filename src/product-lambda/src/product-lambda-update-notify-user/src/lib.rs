pub mod service;

use crate::service::ProductEventWatchlistNotificationsService;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    actor::{RequestContext, domain::Actor},
    dynamodb_stream::extract_sqs_event_bridge_dynamodb_record,
};
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::NotificationService;
use product::core::product_event::ProductDomainEvent;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use tracing::{error, info, warn};

#[tracing::instrument(skip(product_event_notification_service, notification_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    product_event_notification_service: &impl ProductEventWatchlistNotificationsService,
    notification_service: &impl NotificationService,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(
        total = records_count,
        "Start sending notifications for watchlist-updates...",
    );

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
                Ok(product_event) => {
                    let event_id = product_event.event_id;
                    let notification_cmds_res = product_event_notification_service
                        .determine_notification_commands(product_event)
                        .await;
                    match notification_cmds_res {
                        Ok(cmds) => {
                            let create_notifications_res = notification_service
                                .create_notifications(
                                    &RequestContext {
                                        actor: Actor::System,
                                    },
                                    &event_id,
                                    cmds,
                                )
                                .await;
                            // strictly fail on partial failure
                            // this is fine as all downstream components dedup on origin_event_id
                            // this enforces that all targets actually receive the notification for the event
                            if !create_notifications_res.unprocessed.is_empty() {
                                warn!(
                                    messageId = message_id,
                                    unprocessed = create_notifications_res.unprocessed.len(),
                                    "Some CreateNotificationCommands were not processed. Marking message as failed to trigger retry for the entire batch.",
                                );
                                failed_message_ids.push(message_id);
                            }
                        }
                        Err(err) => {
                            warn!(messageId = message_id, error = %err, "Failed creating CreateNotificationCommands.");
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
        "Finished sending notifications for watchlist-updates.",
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
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::event::Event;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use notification::service::command::CreateNotificationCommand;
    use notification::service::notification_service::{
        CreateNotificationsResult, MockNotificationService, NotificationError,
    };
    use product::core::product_event::ProductDomainEvent;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    };
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use crate::service::MockProductEventWatchlistNotificationsService;

    // ---- Helper functions ----

    fn mk_event_bridge_payload(product_event_record: &impl serde::Serialize) -> String {
        let mut stream_record = StreamRecord::default();
        stream_record.approximate_creation_date_time = SystemTime::now().into();
        stream_record.new_image = serde_dynamo::to_item(product_event_record).unwrap();
        stream_record.size_bytes = 42;

        let mut event_record = EventRecord::default();
        event_record.aws_region = "eu-central-1".to_string();
        event_record.change = stream_record;
        event_record.event_id = Uuid::new_v4().to_string();
        event_record.event_name = "INSERT".to_string();

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail_type = "foo".to_string();
        event.source = "bar".to_string();
        event.detail = event_record;

        serde_json::to_string(&event).unwrap()
    }

    fn mk_sqs_message(record: &impl serde::Serialize) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Faker.fake());
        msg.body = Some(mk_event_bridge_payload(record));
        msg
    }

    fn mk_sqs_message_with_id(record: &impl serde::Serialize, message_id: String) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id);
        msg.body = Some(mk_event_bridge_payload(record));
        msg
    }

    fn mk_domain_event_record() -> ProductDomainEventRecord {
        let event: ProductDomainEvent = Event {
            aggregate_id: Faker.fake(),
            event_id: Faker.fake(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(
                Faker.fake::<ProductCreatedDomainEventPayload>(),
            ),
        };
        ProductDomainEventRecord::from(event)
    }

    fn mk_watchlist_service_error() -> crate::service::ProductEventWatchlistNotificationsServiceError
    {
        use crate::service::ProductEventWatchlistNotificationsServiceError;
        use product_watchlist::service::product_watchlist_service::WatchProductError;
        ProductEventWatchlistNotificationsServiceError::WatchProductError(
            WatchProductError::UnprocessedAfterMaxRetries(3),
        )
    }

    // ---- Tests ----

    #[tokio::test]
    async fn should_return_no_failures_when_batch_is_empty() {
        let sqs_event = SqsEvent::default();
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let notification_svc = MockProductEventWatchlistNotificationsService::default();
        let notification_service = MockNotificationService::default();

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_all_messages_succeed_with_no_commands() {
        let records: Vec<SqsMessage> = (0..3)
            .map(|_| mk_sqs_message(&mk_domain_event_record()))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        // create_notifications is always called, even with an empty cmds vec
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .returning(|_, _, _| {
                Box::pin(async {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed: vec![],
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_all_messages_succeed_with_processed_commands() {
        let records: Vec<SqsMessage> = (0..3)
            .map(|_| mk_sqs_message(&mk_domain_event_record()))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| Box::pin(async { Ok(vec![Faker.fake::<CreateNotificationCommand>()]) }));
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .returning(|_, _, _| {
                Box::pin(async {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed: vec![],
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_determine_notification_commands_fails() {
        let message_id = Uuid::new_v4().to_string();
        let record = mk_domain_event_record();
        let msg = mk_sqs_message_with_id(&record, message_id.clone());
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| Box::pin(async { Err(mk_watchlist_service_error()) }));
        let notification_service = MockNotificationService::default();

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failure_when_create_notifications_has_unprocessed_items() {
        let message_id = Uuid::new_v4().to_string();
        let record = mk_domain_event_record();
        let msg = mk_sqs_message_with_id(&record, message_id.clone());
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| {
                Box::pin(async {
                    Ok(vec![
                        Faker.fake::<CreateNotificationCommand>(),
                        Faker.fake::<CreateNotificationCommand>(),
                    ])
                })
            });
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .returning(|_, _, cmds| {
                let unprocessed = cmds
                    .into_iter()
                    .map(|cmd| {
                        (
                            cmd,
                            NotificationError::SdkPutItemError(Box::new(
                                aws_sdk_dynamodb::error::SdkError::construction_failure(
                                    "test error",
                                ),
                            )),
                        )
                    })
                    .collect();
                Box::pin(async move {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed,
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_succeed_when_create_notifications_returns_all_processed() {
        let record = mk_domain_event_record();
        let msg = mk_sqs_message(&record);
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| Box::pin(async { Ok(vec![Faker.fake::<CreateNotificationCommand>()]) }));
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .returning(|_, _, _| {
                Box::pin(async {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed: vec![],
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_message_when_body_is_empty() {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Uuid::new_v4().to_string());
        msg.body = None;

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let notification_svc = MockProductEventWatchlistNotificationsService::default();
        let notification_service = MockNotificationService::default();

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        // A message with no body is silently skipped, not failed
        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_message_when_body_is_invalid_json() {
        let message_id = Uuid::new_v4().to_string();
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id.clone());
        msg.body = Some("invalid json {".to_string());

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let notification_svc = MockProductEventWatchlistNotificationsService::default();
        let notification_service = MockNotificationService::default();

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_fail_message_when_body_is_empty_json_object() {
        let message_id = Uuid::new_v4().to_string();
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id.clone());
        // Valid JSON, but not a valid EventBridge event wrapping a DynamoDB stream record
        msg.body = Some("{}".to_string());

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let notification_svc = MockProductEventWatchlistNotificationsService::default();
        let notification_service = MockNotificationService::default();

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failures_for_all_messages_when_all_fail_determine_notification_commands()
    {
        let message_id_1 = Uuid::new_v4().to_string();
        let message_id_2 = Uuid::new_v4().to_string();
        let message_id_3 = Uuid::new_v4().to_string();

        let records = vec![
            mk_sqs_message_with_id(&mk_domain_event_record(), message_id_1.clone()),
            mk_sqs_message_with_id(&mk_domain_event_record(), message_id_2.clone()),
            mk_sqs_message_with_id(&mk_domain_event_record(), message_id_3.clone()),
        ];
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| Box::pin(async { Err(mk_watchlist_service_error()) }));
        let notification_service = MockNotificationService::default();

        let mut actual_failed_ids: Vec<String> =
            handler(&notification_svc, &notification_service, lambda_event)
                .await
                .unwrap()
                .batch_item_failures
                .into_iter()
                .map(|f| f.item_identifier)
                .collect();
        actual_failed_ids.sort();

        let mut expected_failed_ids = vec![message_id_1, message_id_2, message_id_3];
        expected_failed_ids.sort();

        assert_eq!(expected_failed_ids, actual_failed_ids);
    }

    #[tokio::test]
    async fn should_return_only_failed_message_ids_when_some_succeed_and_some_fail() {
        let succeeding_id_1 = Uuid::new_v4().to_string();
        let succeeding_id_2 = Uuid::new_v4().to_string();
        let failing_id = Uuid::new_v4().to_string();

        let records = vec![
            mk_sqs_message_with_id(&mk_domain_event_record(), succeeding_id_1.clone()),
            mk_sqs_message_with_id(&mk_domain_event_record(), succeeding_id_2.clone()),
            mk_sqs_message_with_id(&mk_domain_event_record(), failing_id.clone()),
        ];
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        // The first two calls succeed (return Ok with empty cmds), the third fails.
        // determine_notification_commands is called for all three messages;
        // create_notifications is also called for the two that succeed (even with empty cmds).
        let call_count = Arc::new(Mutex::new(0u32));
        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(move |_| {
                let call_count = call_count.clone();
                Box::pin(async move {
                    let mut count = call_count.lock().unwrap();
                    *count += 1;
                    let current = *count;
                    drop(count);
                    if current <= 2 {
                        Ok(vec![])
                    } else {
                        Err(mk_watchlist_service_error())
                    }
                })
            });
        let mut notification_service = MockNotificationService::default();
        // Called twice for the two succeeding messages
        notification_service
            .expect_create_notifications()
            .returning(|_, _, _| {
                Box::pin(async {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed: vec![],
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(failing_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failure_when_only_some_notification_commands_are_unprocessed() {
        let message_id = Uuid::new_v4().to_string();
        let record = mk_domain_event_record();
        let msg = mk_sqs_message_with_id(&record, message_id.clone());
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| {
                Box::pin(async {
                    Ok(vec![
                        Faker.fake::<CreateNotificationCommand>(),
                        Faker.fake::<CreateNotificationCommand>(),
                        Faker.fake::<CreateNotificationCommand>(),
                    ])
                })
            });
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .returning(|_, _, mut cmds| {
                // Only one of three commands fails — the message is still failed (strict mode)
                let unprocessed_cmd = cmds.pop().unwrap();
                Box::pin(async move {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed: vec![(
                            unprocessed_cmd,
                            NotificationError::SdkPutItemError(Box::new(
                                aws_sdk_dynamodb::error::SdkError::construction_failure(
                                    "partial failure",
                                ),
                            )),
                        )],
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        // Strict failure: any unprocessed → whole message fails
        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_no_failures_when_determine_notification_commands_returns_empty_vec() {
        let record = mk_domain_event_record();
        let msg = mk_sqs_message(&record);
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .times(1)
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        // create_notifications is always called, even for an empty cmds vec — the handler does
        // not short-circuit. An empty unprocessed result means success.
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .times(1)
            .returning(|_, _, _| {
                Box::pin(async {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed: vec![],
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_process_single_message_with_single_command_successfully() {
        let record = mk_domain_event_record();
        let msg = mk_sqs_message(&record);
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .times(1)
            .returning(|_| Box::pin(async { Ok(vec![Faker.fake::<CreateNotificationCommand>()]) }));
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .times(1)
            .returning(|_, _, _| {
                Box::pin(async {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed: vec![],
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_mixed_results_when_some_have_unprocessed_and_some_succeed() {
        let failing_id = Uuid::new_v4().to_string();
        let succeeding_id = Uuid::new_v4().to_string();

        let records = vec![
            mk_sqs_message_with_id(&mk_domain_event_record(), failing_id.clone()),
            mk_sqs_message_with_id(&mk_domain_event_record(), succeeding_id.clone()),
        ];
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| Box::pin(async { Ok(vec![Faker.fake::<CreateNotificationCommand>()]) }));

        // First call: all unprocessed (failing_id message). Second call: all processed (succeeding_id).
        let call_count = Arc::new(Mutex::new(0u32));
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .returning(move |_, _, cmds| {
                let call_count = call_count.clone();
                let mut count = call_count.lock().unwrap();
                *count += 1;
                let current = *count;
                drop(count);

                let unprocessed = if current == 1 {
                    cmds.into_iter()
                        .map(|cmd| {
                            (
                                cmd,
                                NotificationError::SdkPutItemError(Box::new(
                                    aws_sdk_dynamodb::error::SdkError::construction_failure(
                                        "first call fails",
                                    ),
                                )),
                            )
                        })
                        .collect()
                } else {
                    vec![]
                };
                Box::pin(async move {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed,
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(failing_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[trace]
    async fn should_return_no_failures_for_large_batch_when_all_messages_succeed(
        #[case] record_count: usize,
    ) {
        let records: Vec<SqsMessage> = fake::vec![ProductCreatedDomainEventPayload; record_count]
            .into_iter()
            .map(ProductDomainEventPayload::Created)
            .map(|event_payload| Event {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload,
            })
            .map(ProductDomainEventRecord::try_from)
            .map(Result::unwrap)
            .map(|record| mk_sqs_message(&record))
            .collect();

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        // create_notifications is always called even with empty cmds
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .returning(|_, _, _| {
                Box::pin(async {
                    CreateNotificationsResult {
                        processed: vec![],
                        unprocessed: vec![],
                    }
                })
            });

        let result = handler(&notification_svc, &notification_service, lambda_event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[trace]
    async fn should_return_all_failures_for_large_batch_when_all_messages_fail(
        #[case] record_count: usize,
    ) {
        let mut expected_message_ids: Vec<String> = Vec::with_capacity(record_count);
        let records: Vec<SqsMessage> = fake::vec![ProductCreatedDomainEventPayload; record_count]
            .into_iter()
            .map(ProductDomainEventPayload::Created)
            .map(|event_payload| Event {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload,
            })
            .map(ProductDomainEventRecord::try_from)
            .map(Result::unwrap)
            .map(|record| {
                let message_id = Uuid::new_v4().to_string();
                expected_message_ids.push(message_id.clone());
                mk_sqs_message_with_id(&record, message_id)
            })
            .collect();

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut notification_svc = MockProductEventWatchlistNotificationsService::default();
        notification_svc
            .expect_determine_notification_commands()
            .returning(|_| Box::pin(async { Err(mk_watchlist_service_error()) }));
        let notification_service = MockNotificationService::default();

        let mut actual_failed_ids: Vec<String> =
            handler(&notification_svc, &notification_service, lambda_event)
                .await
                .unwrap()
                .batch_item_failures
                .into_iter()
                .map(|f| f.item_identifier)
                .collect();
        actual_failed_ids.sort();
        expected_message_ids.sort();

        assert_eq!(expected_message_ids, actual_failed_ids);
    }
}
