pub mod service;
pub mod types;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    batch::{Batch, dynamodb::handle_dynamodb_batch_write_put_product_output},
    dynamodb_stream::extract_from_dynamodb_stream,
    event_id::EventId,
    has_key::HasKey,
    product_id::{ProductId, ProductKey},
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
    service::get_service::GetProductService,
};
use service::ExtractionService;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{error, info, warn};

#[tracing::instrument(
    skip(extraction_service, get_product_service, product_repository, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    extraction_service: &(impl ExtractionService + Sync),
    get_product_service: &(impl GetProductService + Sync),
    product_repository: &(impl ProductDynamoDbRepository + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    // First pass: collect valid enrichment records (ENRICHMENT_CLASSIFY_CATEGORY events).
    // message_id → enrichment record (deduplicated: last record wins for duplicate keys).
    let mut key_to_record: HashMap<ProductKey, (String, ProductEventRecord)> = HashMap::new();

    for (message_id, event_record) in event_records {
        match event_record {
            ProductEventRecord::Enrichment(ref enrichment_record) => {
                let key = ProductKey::new(
                    enrichment_record.shop_id,
                    enrichment_record.shops_product_id.clone(),
                );
                key_to_record.insert(key, (message_id, event_record));
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

    if key_to_record.is_empty() {
        let failures = failed_message_ids.len();
        info!(successful = 0, failures = failures, "Handler finished.");
        let mut sqs_batch_response = SqsBatchResponse::default();
        sqs_batch_response.batch_item_failures = failed_message_ids
            .into_iter()
            .map(mk_batch_item_failure)
            .collect();
        return Ok(sqs_batch_response);
    }

    // Batch-load materialized products from DynamoDB to obtain native_title / native_description.
    let keys: Vec<ProductKey> = key_to_record.keys().cloned().collect();
    let products = match get_product_service.find_products(keys).await {
        Ok(ps) => ps,
        Err(err) => {
            error!(error = ?err, "Failed batch-loading products from DynamoDB — marking all messages as failed.");
            failed_message_ids.extend(key_to_record.into_values().map(|(msg_id, _)| msg_id));
            let failures = failed_message_ids.len();
            info!(successful = 0, failures = failures, "Handler finished.");
            let mut sqs_batch_response = SqsBatchResponse::default();
            sqs_batch_response.batch_item_failures = failed_message_ids
                .into_iter()
                .map(mk_batch_item_failure)
                .collect();
            return Ok(sqs_batch_response);
        }
    };

    // Build text inputs for each found product; skip any product with an empty native title.
    // Carry: (message_id, product_id, shop_id, seller_id, shops_product_id, text)
    struct ProductInput {
        message_id: String,
        product_id: ProductId,
        key: ProductKey,
        seller_id: common::shop_id::ShopId,
        text: String,
    }

    let mut valid_inputs: Vec<ProductInput> = Vec::new();

    for product in products {
        let key = ProductKey::new(product.shop_id, product.shops_product_id.clone());
        let (message_id, _) = match key_to_record.remove(&key) {
            Some(v) => v,
            None => {
                warn!(
                    shopId = %product.shop_id,
                    shopsProductId = %product.shops_product_id,
                    "Loaded product has no corresponding SQS message — skipping."
                );
                continue;
            }
        };

        let title_trimmed = product.native_title.payload.as_ref().trim();
        if title_trimmed.is_empty() {
            warn!(
                messageId = message_id,
                shopId = %product.shop_id,
                shopsProductId = %product.shops_product_id,
                "Product has empty native title — skipping attribute extraction."
            );
            continue;
        }
        let title = title_trimmed.to_string();

        // Concatenate native title and description with a single space so Gemini
        // receives the fullest possible context for attribute extraction.
        let text = match &product.native_description {
            Some(desc) => {
                let d = desc.payload.as_ref().trim();
                if d.is_empty() {
                    title
                } else {
                    format!("{title} {d}")
                }
            }
            None => title,
        };

        valid_inputs.push(ProductInput {
            message_id,
            product_id: product.product_id,
            seller_id: product.seller_id,
            key,
            text,
        });
    }

    // Products not found in DynamoDB — mark the corresponding messages as failed (retry).
    for (message_id, record) in key_to_record.values() {
        let key = record.key();
        error!(
            messageId = message_id,
            shopId = %key.shop_id,
            shopsProductId = %key.shops_product_id,
            "Materialized product not found in DynamoDB — marking message as failed for retry."
        );
        failed_message_ids.push(message_id.clone());
    }

    if valid_inputs.is_empty() {
        let failures = failed_message_ids.len();
        info!(successful = 0, failures = failures, "Handler finished.");
        let mut sqs_batch_response = SqsBatchResponse::default();
        sqs_batch_response.batch_item_failures = failed_message_ids
            .into_iter()
            .map(mk_batch_item_failure)
            .collect();
        return Ok(sqs_batch_response);
    }

    // Sort by text length (shortest first) so batching packs efficiently.
    valid_inputs.sort_by_key(|i| i.text.len());

    let texts: Vec<String> = valid_inputs.iter().map(|i| i.text.clone()).collect();
    let extraction_results = extraction_service.extract(&texts).await;

    let mut enrichment_events: Vec<(String, ProductEventRecord)> = Vec::new();

    for (input, maybe_attrs) in valid_inputs.into_iter().zip(extraction_results.into_iter()) {
        let attrs = match maybe_attrs {
            Some(attrs) => attrs,
            None => {
                error!(
                    messageId = input.message_id,
                    shopId = %input.key.shop_id,
                    shopsProductId = %input.key.shops_product_id,
                    "Extraction service returned no result for product — marking message as failed."
                );
                failed_message_ids.push(input.message_id);
                continue;
            }
        };

        let now = OffsetDateTime::now_utc();

        // Enrichment event for the extracted attributes.
        let attribute_event = ProductEvent {
            aggregate_id: input.product_id,
            event_id: EventId::new(),
            timestamp: now,
            payload: ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::ExtractedAttributes(
                    ExtractedAttributesProductEnrichmentEventPayload {
                        shop_id: input.key.shop_id,
                        seller_id: input.seller_id,
                        shops_product_id: input.key.shops_product_id.clone(),
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
        enrichment_events.push((input.message_id.clone(), attribute_record));

        // Policy event for prohibited-content decision (only when deterministic).
        let prohibited_content = match attrs.nazi {
            Some(true) => Some(ProhibitedContent::NaziGermany),
            Some(false) => Some(ProhibitedContent::None),
            None => None,
        };

        if let Some(decision) = prohibited_content {
            let policy_event = ProductEvent {
                aggregate_id: input.product_id,
                event_id: EventId::new(),
                timestamp: now,
                payload: ProductEventPayload::ProductPolicyEvent(
                    ProductPolicyEventPayload::ProhibitedContentDecision(
                        ProhibitedContentProductPolicyEventPayload {
                            shop_id: input.key.shop_id,
                            seller_id: input.seller_id,
                            shops_product_id: input.key.shops_product_id,
                            decision,
                            reason: ProhibitedContentReason::ProductText,
                        },
                    ),
                ),
            };
            let policy_record: ProductEventRecord = policy_event.into();
            enrichment_events.push((input.message_id, policy_record));
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
    use common::category_key::CategoryId;
    use common::event_id::EventId;
    use common::product_id::ProductId;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product::Product;
    use product::core::product_event::enrichment::{
        ClassifiedCategoryProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
    };
    use product::core::product_event::{ProductEvent, ProductEventPayload};
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use product::service::get_service::{GetProductError, MockGetProductService};
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

    /// Build a ClassifiedCategory enrichment event record (the new trigger event type).
    fn mk_classify_event_record() -> ProductEnrichmentEventRecord {
        let category_id: CategoryId = "furniture".into();
        let payload: ClassifiedCategoryProductEnrichmentEventPayload = Faker.fake();
        let payload = ClassifiedCategoryProductEnrichmentEventPayload {
            category_id,
            ..payload
        };
        let event = ProductEvent {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::ClassifiedCategory(payload),
            ),
        };
        match ProductEventRecord::from(event) {
            ProductEventRecord::Enrichment(r) => r,
            _ => unreachable!(),
        }
    }

    /// Build a Product with a given native title (and optional description) for use in mock get_product_service.
    fn mk_product_with_title(record: &ProductEnrichmentEventRecord, title: &str) -> Product {
        mk_product_with_title_and_description(record, title, None)
    }

    /// Build a Product with native title and description for use in mock get_product_service.
    fn mk_product_with_title_and_description(
        record: &ProductEnrichmentEventRecord,
        title: &str,
        description: Option<&str>,
    ) -> Product {
        let mut product: Product = Faker.fake();
        product.shop_id = record.shop_id;
        product.shops_product_id = record.shops_product_id.clone();
        product.seller_id = record.seller_id;
        product.product_id = record.product_id;
        product.native_title = common::localized::Localized::new(
            common::language::domain::Language::En,
            product::core::title::Title::from(title),
        );
        product.native_description = description.map(|d| {
            common::localized::Localized::new(
                common::language::domain::Language::En,
                product::core::description::Description::from(d),
            )
        });
        product
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

    fn mk_product_service_returning(products: Vec<Product>) -> MockGetProductService {
        let mut mock = MockGetProductService::default();
        mock.expect_find_products().returning(move |_| {
            let ps = products.clone();
            Box::pin(async move { Ok(ps) })
        });
        mock
    }

    fn mk_product_service_failing() -> MockGetProductService {
        let mut mock = MockGetProductService::default();
        mock.expect_find_products()
            .returning(|_| Box::pin(async { Err(GetProductError::UnprocessedAfterMaxRetries(3)) }));
        mock
    }

    #[tokio::test]
    async fn should_return_no_failures_when_batch_is_empty() {
        let mock_service = MockExtractionService::default();
        let mock_product_service = MockGetProductService::default();
        let mock_repository = MockProductDynamoDbRepository::default();
        let event = mk_lambda_event(vec![]);

        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_single_product_extracted_successfully() {
        let classify_record = mk_classify_event_record();
        let product = mk_product_with_title(&classify_record, "Antique oak chair circa 1870");
        let event_record = ProductEventRecord::Enrichment(classify_record);

        let mock_service = mk_extraction_service_returning(ExtractedAttributes {
            y: Some(1870.into()),
            nazi: Some(false),
            ..Default::default()
        });
        let mock_product_service = mk_product_service_returning(vec![product]);
        let mock_repository = mk_write_repository();

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_extraction_fails_for_product() {
        let classify_record = mk_classify_event_record();
        let product = mk_product_with_title(&classify_record, "Antique oak chair circa 1870");
        let event_record = ProductEventRecord::Enrichment(classify_record);
        let message_id = "test-msg-extract-fail".to_string();

        let mut mock_service = MockExtractionService::default();
        mock_service.expect_extract().times(1).returning(|texts| {
            let count = texts.len();
            Box::pin(async move { vec![None; count] })
        });

        let mock_product_service = mk_product_service_returning(vec![product]);
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_skip_when_product_has_empty_native_title() {
        let classify_record = mk_classify_event_record();
        let product = mk_product_with_title(&classify_record, "");
        let event_record = ProductEventRecord::Enrichment(classify_record);

        let mock_service = MockExtractionService::default();
        let mock_product_service = mk_product_service_returning(vec![product]);
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_all_messages_when_product_service_errors() {
        let classify_record = mk_classify_event_record();
        let event_record = ProductEventRecord::Enrichment(classify_record);
        let message_id = "msg-product-service-fail".to_string();

        let mock_service = MockExtractionService::default();
        let mock_product_service = mk_product_service_failing();
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_fail_when_product_not_found_in_dynamodb() {
        let classify_record = mk_classify_event_record();
        let event_record = ProductEventRecord::Enrichment(classify_record);
        let message_id = "msg-product-not-found".to_string();

        let mock_service = MockExtractionService::default();
        // Return empty list — product not found
        let mock_product_service = mk_product_service_returning(vec![]);
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_skip_non_enrichment_records() {
        use product::core::product_event::domain::{
            ProductCreatedDomainEventPayload, ProductDomainEventPayload,
        };
        use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;

        let payload: ProductCreatedDomainEventPayload = Faker.fake();
        let event = Event {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(payload),
        };
        let domain_record: ProductDomainEventRecord = event.into();
        let event_record = ProductEventRecord::Domain(domain_record);
        let message_id = "non-enrichment-msg".to_string();

        let mock_service = MockExtractionService::default();
        let mock_product_service = MockGetProductService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_multiple_products_extracted_successfully() {
        let record1 = mk_classify_event_record();
        let record2 = mk_classify_event_record();
        let product1 = mk_product_with_title(&record1, "Victorian silver candlestick");
        let product2 = mk_product_with_title(&record2, "Antique mahogany desk circa 1820");

        let mock_service = mk_extraction_service_returning(ExtractedAttributes {
            y: Some(1800.into()),
            nazi: Some(false),
            ..Default::default()
        });
        let mock_product_service = mk_product_service_returning(vec![product1, product2]);
        let mock_repository = mk_write_repository();

        let event = mk_lambda_event(vec![
            mk_sqs_message(&ProductEventRecord::Enrichment(record1)),
            mk_sqs_message(&ProductEventRecord::Enrichment(record2)),
        ]);
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_not_create_policy_event_when_nazi_is_none() {
        let classify_record = mk_classify_event_record();
        let product = mk_product_with_title(&classify_record, "Victorian urn");
        let event_record = ProductEventRecord::Enrichment(classify_record);

        // nazi = None → no policy event → only one write per product (the enrichment)
        let mock_service = mk_extraction_service_returning(ExtractedAttributes {
            y: Some(1800.into()),
            nazi: None,
            ..Default::default()
        });
        let mock_product_service = mk_product_service_returning(vec![product]);

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
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_create_both_attribute_and_policy_events_when_nazi_is_some() {
        let classify_record = mk_classify_event_record();
        let product = mk_product_with_title(&classify_record, "SS officer uniform 1940");
        let event_record = ProductEventRecord::Enrichment(classify_record);

        let mock_service = mk_extraction_service_returning(ExtractedAttributes {
            y: Some(1940.into()),
            nazi: Some(true),
            ..Default::default()
        });
        let mock_product_service = mk_product_service_returning(vec![product]);

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
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_pass_title_and_description_concatenated_to_extraction_service() {
        let classify_record = mk_classify_event_record();
        let product = mk_product_with_title_and_description(
            &classify_record,
            "Victorian writing desk",
            Some("Solid mahogany, circa 1870, excellent condition."),
        );
        let event_record = ProductEventRecord::Enrichment(classify_record);

        let expected_text =
            "Victorian writing desk Solid mahogany, circa 1870, excellent condition.".to_string();

        let mut mock_service = MockExtractionService::default();
        mock_service
            .expect_extract()
            .once()
            .withf(move |texts| texts == [expected_text.clone()])
            .returning(|texts| {
                let count = texts.len();
                Box::pin(async move {
                    vec![
                        Some(ExtractedAttributes {
                            y: Some(1870.into()),
                            nazi: Some(false),
                            ..Default::default()
                        });
                        count
                    ]
                })
            });

        let mock_product_service = mk_product_service_returning(vec![product]);
        let mock_repository = mk_write_repository();

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(
            &mock_service,
            &mock_product_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }
}
