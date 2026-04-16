pub mod service;

use crate::service::ProductMatcherService;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::NotificationService;
use product::core::product_event::{ProductDomainEvent, ProductEvent};
use product::core::product_event::{ProductEnrichmentEvent, ProductEventPayload};
use product::dynamodb::product_event_record::ProductEventRecord;
use search_filter::service::user_search_filter_service::UserSearchFilterService;
use tracing::{error, info, warn};

#[tracing::instrument(skip(product_matcher_service, notification_service, search_filter_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    product_matcher_service: &impl ProductMatcherService,
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
            ProductEventRecord,
        >(
            message, &mut failed_message_ids, &mut skipped_count
        ) {
            let product_event: ProductEvent = match product_event_record {
                ProductEventRecord::Domain(record) => match ProductDomainEvent::try_from(record) {
                    Ok(e) => e.map_payload(ProductEventPayload::from),
                    Err(err) => {
                        error!(
                            error = %err,
                            fromType = %std::any::type_name::<product::dynamodb::product_event_record::domain::ProductDomainEventRecord>(),
                            toType = %std::any::type_name::<ProductDomainEvent>(),
                            "Failed mapping types. Skipping event."
                        );
                        skipped_count += 1;
                        continue;
                    }
                },
                ProductEventRecord::Enrichment(record) => {
                    match ProductEnrichmentEvent::try_from(record) {
                        Ok(e) => e.map_payload(ProductEventPayload::from),
                        Err(err) => {
                            error!(
                                error = %err,
                                fromType = %std::any::type_name::<product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord>(),
                                toType = %std::any::type_name::<ProductEnrichmentEvent>(),
                                "Failed mapping types. Skipping event."
                            );
                            skipped_count += 1;
                            continue;
                        }
                    }
                }
                ProductEventRecord::Policy(_) => {
                    skipped_count += 1;
                    continue;
                }
            };

            let event_id = product_event.event_id;
            let matcher_result = product_matcher_service
                .process_product_event(product_event)
                .await;
            match matcher_result {
                Ok(result) => {
                    if result.matches.is_empty() && result.notification_commands.is_empty() {
                        continue;
                    }

                    // First: persist all eligible matches
                    if !result.matches.is_empty() {
                        let match_result = search_filter_service
                            .create_search_filter_product_matches(result.matches)
                            .await;
                        match match_result {
                            Ok(res) if !res.unprocessed.is_empty() => {
                                warn!(
                                    messageId = message_id,
                                    unprocessed = res.unprocessed.len(),
                                    "Some SearchFilterProductMatches were not persisted. Marking message as failed."
                                );
                                failed_message_ids.push(message_id.clone());
                                continue;
                            }
                            Err(err) => {
                                warn!(
                                    messageId = message_id,
                                    error = %err,
                                    "Failed creating SearchFilterProductMatches. Marking message as failed."
                                );
                                failed_message_ids.push(message_id.clone());
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // Then: create notifications for quota-eligible users
                    if !result.notification_commands.is_empty() {
                        let create_notifications_res = notification_service
                            .create_notifications(&event_id, result.notification_commands)
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
                }
                Err(err) => {
                    error!(messageId = message_id, error = %err, "Failed processing product event.");
                    failed_message_ids.push(message_id);
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
        MockProductMatcherService, ProductMatcherResult, ProductMatcherServiceError,
    };
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use fake::{Fake, Faker};
    use lambda_runtime::Context;
    use notification::service::{
        command::CreateNotificationCommand,
        notification_service::{CreateNotificationsResult, MockNotificationService},
    };
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
    use product::dynamodb::product_event_record::policy::ProductPolicyEventRecord;
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
        let service = MockProductMatcherService::default();
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
        let service = MockProductMatcherService::default();
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
        let mut service = MockProductMatcherService::default();
        service.expect_process_product_event().return_once(|_| {
            Box::pin(async {
                Err(ProductMatcherServiceError::GetProductError(
                    product::service::get_service::GetProductError::ProductNotFound(
                        Faker.fake(),
                        Faker.fake(),
                    ),
                ))
            })
        });

        let notification_service = MockNotificationService::default();
        let search_filter_service = MockUserSearchFilterService::default();

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
        let event_bridge_body = mk_event_bridge_body_domain(&domain_event_record);
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
    async fn should_succeed_when_result_is_empty() {
        let mut service = MockProductMatcherService::default();
        service.expect_process_product_event().return_once(|_| {
            Box::pin(async {
                Ok(ProductMatcherResult {
                    matches: vec![],
                    notification_commands: vec![],
                })
            })
        });

        let notification_service = MockNotificationService::default();
        let search_filter_service = MockUserSearchFilterService::default();

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
        let event_bridge_body = mk_event_bridge_body_domain(&domain_event_record);
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
    async fn should_succeed_when_matches_and_notifications_created() {
        let cmd: CreateNotificationCommand = Faker.fake();
        let product_match: search_filter::core::search_filter_product_match::SearchFilterProductMatch = Faker.fake();

        let mut service = MockProductMatcherService::default();
        service
            .expect_process_product_event()
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(ProductMatcherResult {
                        matches: vec![product_match],
                        notification_commands: vec![cmd],
                    })
                })
            });

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

        let mut search_filter_service = MockUserSearchFilterService::default();
        search_filter_service
            .expect_create_search_filter_product_matches()
            .return_once(|_| {
                Box::pin(async {
                    Ok(search_filter::service::user_search_filter_service::CreateSearchFilterProductMatchesResult {
                        processed: vec![],
                        unprocessed: vec![],
                    })
                })
            });

        let domain_event_record: ProductDomainEventRecord = Faker.fake();
        let event_bridge_body = mk_event_bridge_body_domain(&domain_event_record);
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
        let service = MockProductMatcherService::default();
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

    #[tokio::test]
    async fn should_process_enrichment_event_without_failure() {
        let mut service = MockProductMatcherService::default();
        service.expect_process_product_event().return_once(|_| {
            Box::pin(async {
                Ok(ProductMatcherResult {
                    matches: vec![],
                    notification_commands: vec![],
                })
            })
        });

        let notification_service = MockNotificationService::default();
        let search_filter_service = MockUserSearchFilterService::default();

        let enrichment_record: ProductEnrichmentEventRecord = Faker.fake();
        let event_bridge_body = mk_event_bridge_body_enrichment(&enrichment_record);
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
        assert!(
            response.batch_item_failures.is_empty(),
            "Enrichment events must be processed without failure"
        );
    }

    #[tokio::test]
    async fn should_skip_without_failure_when_policy_event_received() {
        let service = MockProductMatcherService::default();
        let notification_service = MockNotificationService::default();
        let search_filter_service = MockUserSearchFilterService::default();

        let policy_record: ProductPolicyEventRecord = Faker.fake();
        let event_bridge_body = mk_event_bridge_body_policy(&policy_record);
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
        assert!(
            response.batch_item_failures.is_empty(),
            "Policy events must be skipped, not failed"
        );
    }

    fn mk_event_bridge_body_domain(record: &ProductDomainEventRecord) -> String {
        mk_event_bridge_body(record)
    }

    fn mk_event_bridge_body_enrichment(record: &ProductEnrichmentEventRecord) -> String {
        mk_event_bridge_body(record)
    }

    fn mk_event_bridge_body_policy(record: &ProductPolicyEventRecord) -> String {
        mk_event_bridge_body(record)
    }

    fn mk_event_bridge_body<T: serde::Serialize>(record: &T) -> String {
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
