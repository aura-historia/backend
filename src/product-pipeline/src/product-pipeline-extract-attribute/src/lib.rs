pub mod service;
pub mod types;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    batch::{Batch, dynamodb::handle_dynamodb_batch_write_put_product_output},
    dynamodb_stream::extract_from_dynamodb_stream,
    event_id::EventId,
    has_key::HasKey,
    product_id::ProductKey,
};
use lambda_runtime::LambdaEvent;
use product::{
    core::{
        product_event::{
            ProductEvent, ProductEventPayload,
            enrichment::{
                ExtractedAttributesProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
            },
            policy::{ProductPolicyEventPayload, ProhibitedContentProductPolicyEventPayload},
        },
        prohibited_content::{ProhibitedContent, ProhibitedContentReason},
    },
    dynamodb::{product_event_record::ProductEventRecord, repository::ProductDynamoDbRepository},
};
use service::ExtractionService;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{error, info, warn};

#[tracing::instrument(
    skip(extraction_service, product_repository, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    extraction_service: &(impl ExtractionService + Sync),
    product_repository: &(impl ProductDynamoDbRepository + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    // First pass: collect valid enrichment records that have a non-empty native title.
    let mut valid_records: Vec<(String, ProductEventRecord)> = Vec::new();

    for (message_id, event_record) in event_records {
        match event_record {
            ProductEventRecord::Enrichment(ref enrichment_record) => {
                let title = enrichment_record.native_title.as_deref().unwrap_or("");
                if title.is_empty() {
                    warn!(
                        messageId = message_id,
                        shopId = %enrichment_record.shop_id,
                        shopsProductId = %enrichment_record.shops_product_id,
                        eventId = %enrichment_record.event_id,
                        "Enrichment record has no native title, skipping attribute extraction."
                    );
                    continue;
                }
                valid_records.push((message_id, event_record));
            }
            other => {
                let key = other.key();
                error!(
                    messageId = message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    eventId = %other.event_id(),
                    "Unexpected non-Enrichment event record type in extract-attribute handler."
                );
            }
        }
    }

    if valid_records.is_empty() {
        let failures = failed_message_ids.len();
        info!(successful = 0, failures = failures, "Handler finished.");
        let mut sqs_batch_response = SqsBatchResponse::default();
        sqs_batch_response.batch_item_failures = failed_message_ids
            .into_iter()
            .map(mk_batch_item_failure)
            .collect();
        return Ok(sqs_batch_response);
    }

    // Sort by title length (shortest first) so batching packs efficiently.
    valid_records.sort_by_key(|(_, record)| match record {
        ProductEventRecord::Enrichment(r) => r.native_title.as_deref().map_or(0, |t| t.len()),
        _ => 0,
    });

    let texts: Vec<String> = valid_records
        .iter()
        .map(|(_, record)| match record {
            ProductEventRecord::Enrichment(r) => r.native_title.clone().unwrap_or_default(),
            _ => String::new(),
        })
        .collect();

    let extraction_results = extraction_service.extract(&texts).await;

    let mut enrichment_events: Vec<(String, ProductEventRecord)> = Vec::new();

    for ((message_id, event_record), maybe_attrs) in valid_records
        .into_iter()
        .zip(extraction_results.into_iter())
    {
        let enrichment_record = match event_record {
            ProductEventRecord::Enrichment(r) => r,
            _ => unreachable!("valid_records only contains Enrichment variants"),
        };

        let attrs = match maybe_attrs {
            Some(attrs) => attrs,
            None => {
                error!(
                    messageId = message_id,
                    shopId = %enrichment_record.shop_id,
                    shopsProductId = %enrichment_record.shops_product_id,
                    eventId = %enrichment_record.event_id,
                    "Extraction service returned no result for product — marking message as failed."
                );
                failed_message_ids.push(message_id);
                continue;
            }
        };

        let now = OffsetDateTime::now_utc();
        let product_id = enrichment_record.product_id;

        // Enrichment event for the extracted attributes.
        let attribute_event = ProductEvent {
            aggregate_id: product_id,
            event_id: EventId::new(),
            timestamp: now,
            payload: ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::ExtractedAttributes(
                    ExtractedAttributesProductEnrichmentEventPayload {
                        shop_id: enrichment_record.shop_id,
                        seller_id: enrichment_record.seller_id,
                        shops_product_id: enrichment_record.shops_product_id.clone(),
                        origin_year_min: attrs.y_min,
                        origin_year: attrs.y,
                        origin_year_max: attrs.y_max,
                        authenticity: attrs.auth.map(Into::into),
                        condition: attrs.cond.map(Into::into),
                        provenance: attrs.prov.map(Into::into),
                        restoration: attrs.rest.map(Into::into),
                    },
                ),
            ),
        };
        let attribute_record: ProductEventRecord = attribute_event.into();
        enrichment_events.push((message_id.clone(), attribute_record));

        // Policy event for prohibited-content decision (only when deterministic).
        let prohibited_content = match attrs.nazi {
            Some(true) => Some(ProhibitedContent::NaziGermany),
            Some(false) => Some(ProhibitedContent::None),
            None => None,
        };

        if let Some(decision) = prohibited_content {
            let policy_event = ProductEvent {
                aggregate_id: product_id,
                event_id: EventId::new(),
                timestamp: now,
                payload: ProductEventPayload::ProductPolicyEvent(
                    ProductPolicyEventPayload::ProhibitedContentDecision(
                        ProhibitedContentProductPolicyEventPayload {
                            shop_id: enrichment_record.shop_id,
                            seller_id: enrichment_record.seller_id,
                            shops_product_id: enrichment_record.shops_product_id,
                            decision,
                            reason: ProhibitedContentReason::ProductText,
                        },
                    ),
                ),
            };
            let policy_record: ProductEventRecord = policy_event.into();
            enrichment_events.push((message_id, policy_record));
        }
    }

    persist_events(
        product_repository,
        enrichment_events,
        &mut failed_message_ids,
    )
    .await;

    let failures = failed_message_ids.len();
    info!(
        successful = count - failures,
        failures = failures,
        "Handler finished.",
    );

    let mut sqs_batch_response = SqsBatchResponse::default();
    sqs_batch_response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(mk_batch_item_failure)
        .collect();
    Ok(sqs_batch_response)
}

fn mk_batch_item_failure(item_identifier: String) -> BatchItemFailure {
    let mut failure = BatchItemFailure::default();
    failure.item_identifier = item_identifier;
    failure
}

async fn persist_events(
    repository: &(impl ProductDynamoDbRepository + Sync),
    events: Vec<(String, ProductEventRecord)>,
    failed_message_ids: &mut Vec<String>,
) {
    for batch in Batch::chunked_from(events.into_iter()) {
        let batch: Batch<_, 25> = batch;
        let batch_message_ids = batch
            .iter()
            .map(|(message_id, record)| (record.key(), message_id.clone()))
            .collect::<HashMap<ProductKey, String>>();
        let batch = Batch::try_from_iter(batch.into_iter().map(|(_, record)| record))
            .expect("shouldn't fail re-building batch of same size from former batch");
        match repository.put_product_event_records(batch).await {
            Ok(output) => {
                let mut failures = Vec::new();
                handle_dynamodb_batch_write_put_product_output::<ProductEventRecord>(
                    output,
                    &mut failures,
                );
                for key in failures {
                    match batch_message_ids.get(&key) {
                        Some(message_id) => failed_message_ids.push(message_id.clone()),
                        None => {
                            error!(
                                productKey = %key,
                                "There exists no message_id for failed ProductEventRecord."
                            );
                        }
                    }
                }
            }
            Err(err) => {
                error!(error = ?err, "Failed entire event batch.");
                failed_message_ids.extend(batch_message_ids.into_values());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::event_id::EventId;
    use common::product_id::ProductId;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    };
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use std::time::SystemTime;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use crate::service::MockExtractionService;
    use crate::types::ExtractedAttributes;
    use common::event::Event;

    fn mk_event_bridge_payload(event_record: &impl serde::Serialize) -> String {
        let mut stream_record = StreamRecord::default();
        stream_record.approximate_creation_date_time = SystemTime::now().into();
        stream_record.new_image = serde_dynamo::to_item(event_record).unwrap();
        stream_record.size_bytes = 42;

        let mut event = EventRecord::default();
        event.aws_region = "eu-central-1".to_string();
        event.change = stream_record;
        event.event_id = Uuid::new_v4().to_string();
        event.event_name = "INSERT".to_string();

        let mut eb_event = EventBridgeEvent::<EventRecord>::default();
        eb_event.detail_type = "DynamoDBStreamRecord".to_string();
        eb_event.source = "test-table".to_string();
        eb_event.detail = event;

        serde_json::to_string(&eb_event).unwrap()
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

    fn mk_lambda_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = messages;
        LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        }
    }

    fn mk_enrichment_event_record() -> ProductEnrichmentEventRecord {
        let mut record: ProductEnrichmentEventRecord = Faker.fake();
        record.embedding = Some(vec![0.42f32; 768]);
        record.native_title = Some("Antique oak chair circa 1870".to_string());
        record
    }

    fn mk_domain_event_record() -> ProductDomainEventRecord {
        let payload: ProductCreatedDomainEventPayload = Faker.fake();
        let event = Event {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(payload),
        };
        event.into()
    }

    fn mk_extraction_service_returning(attrs: ExtractedAttributes) -> MockExtractionService {
        let mut mock = MockExtractionService::default();
        mock.expect_extract().returning(move |texts| {
            let count = texts.len();
            Box::pin(async move { vec![Some(attrs); count] })
        });
        mock
    }

    fn mk_write_repository() -> MockProductDynamoDbRepository {
        let mut mock = MockProductDynamoDbRepository::default();
        mock.expect_put_product_event_records().returning(|_| {
            Box::pin(async {
                Ok(
                    aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                        .build(),
                )
            })
        });
        mock
    }

    #[tokio::test]
    async fn should_return_no_failures_when_batch_is_empty() {
        let mock_service = MockExtractionService::default();
        let mock_repository = MockProductDynamoDbRepository::default();
        let event = mk_lambda_event(vec![]);

        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_single_product_extracted_successfully() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);

        let mock_service = mk_extraction_service_returning(ExtractedAttributes {
            y: Some(1870.into()),
            nazi: Some(false),
            ..Default::default()
        });
        let mock_repository = mk_write_repository();

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_extraction_fails_for_product() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);
        let message_id = "test-msg-extract-fail".to_string();

        let mut mock_service = MockExtractionService::default();
        mock_service.expect_extract().times(1).returning(|texts| {
            let count = texts.len();
            Box::pin(async move { vec![None; count] })
        });

        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_skip_when_enrichment_record_has_no_native_title() {
        let mut enrichment_record = mk_enrichment_event_record();
        enrichment_record.native_title = None;
        let event_record = ProductEventRecord::Enrichment(enrichment_record);

        let mock_service = MockExtractionService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_when_enrichment_record_has_empty_native_title() {
        let mut enrichment_record = mk_enrichment_event_record();
        enrichment_record.native_title = Some(String::new());
        let event_record = ProductEventRecord::Enrichment(enrichment_record);

        let mock_service = MockExtractionService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_non_enrichment_records() {
        let domain_record = mk_domain_event_record();
        let event_record = ProductEventRecord::Domain(domain_record);
        let message_id = "non-enrichment-msg".to_string();

        let mock_service = MockExtractionService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_multiple_products_extracted_successfully() {
        let record1 = mk_enrichment_event_record();
        let record2 = mk_enrichment_event_record();

        let mock_service = mk_extraction_service_returning(ExtractedAttributes {
            y: Some(1800.into()),
            nazi: Some(false),
            ..Default::default()
        });
        let mock_repository = mk_write_repository();

        let event = mk_lambda_event(vec![
            mk_sqs_message(&ProductEventRecord::Enrichment(record1)),
            mk_sqs_message(&ProductEventRecord::Enrichment(record2)),
        ]);
        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_not_create_policy_event_when_nazi_is_none() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);

        // nazi = None → no policy event → only one write per product (the enrichment)
        let mock_service = mk_extraction_service_returning(ExtractedAttributes {
            y: Some(1800.into()),
            nazi: None,
            ..Default::default()
        });

        let mut mock_repository = MockProductDynamoDbRepository::default();
        mock_repository
            .expect_put_product_event_records()
            .once()
            .withf(|batch| batch.len() == 1) // only the enrichment event
            .returning(|_| {
                Box::pin(async {
                    Ok(
                        aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                            .build(),
                    )
                })
            });

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_create_both_attribute_and_policy_events_when_nazi_is_some() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);

        let mock_service = mk_extraction_service_returning(ExtractedAttributes {
            y: Some(1940.into()),
            nazi: Some(true),
            ..Default::default()
        });

        let mut mock_repository = MockProductDynamoDbRepository::default();
        mock_repository
            .expect_put_product_event_records()
            .once()
            .withf(|batch| batch.len() == 2) // enrichment + policy
            .returning(|_| {
                Box::pin(async {
                    Ok(
                        aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                            .build(),
                    )
                })
            });

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(&mock_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }
}
