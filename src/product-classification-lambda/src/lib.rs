use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::batch::Batch;
use common::category_key::CategoryId;
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::product_id::{ProductId, ProductKey};
use lambda_runtime::LambdaEvent;
use product::core::product_event::enrichment::{
    ClassifyCategoryProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
};
use product::core::product_event::{ProductEvent, ProductEventPayload};
use product::dynamodb::repository::{ProductDynamoDbRepository, extract_product_id};
use product::dynamodb::{
    product_event_record::ProductEventRecord,
    product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord,
};
use product_classification::category::service::CategoryService;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{error, info};

#[tracing::instrument(skip(product_repository, category_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    product_repository: &impl ProductDynamoDbRepository,
    category_service: &impl CategoryService,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut updates = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ProductId, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(update) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        )
        .await
        {
            updates.push(update);
        }
    }

    let mut event_records: Vec<ProductEventRecord> = Vec::with_capacity(updates.len());
    for (product_id, product_key, embedding) in updates {
        if let Some(category_id) = determine_category_id(
            &product_id,
            &embedding,
            &mut failed_message_ids,
            &mut message_ids,
            category_service,
        )
        .await
        {
            let event = ProductEvent {
                aggregate_id: product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::ClassifyCategory(
                        ClassifyCategoryProductEnrichmentEventPayload {
                            shop_id: product_key.shop_id,
                            shops_product_id: product_key.shops_product_id,
                            category_id,
                        },
                    ),
                ),
            };
            match ProductEventRecord::try_from(event) {
                Ok(record) => {
                    event_records.push(record);
                }
                Err(err) => {
                    error!(
                        error = ?err,
                        productId = %product_id,
                        fromtype = %std::any::type_name::<ProductEvent>(),
                        totype = %std::any::type_name::<ProductEventRecord>(),
                        "Failed mapping"
                    );
                    fail_message(&product_id, &mut message_ids, &mut failed_message_ids);
                }
            };
        }
    }

    for batch in Batch::chunked_from(event_records.into_iter()) {
        let batch_product_ids = batch
            .iter()
            .map(|record| *record.product_id())
            .collect::<Vec<_>>();
        let batch_res = product_repository.put_product_event_records(batch).await;
        match batch_res {
            Ok(batch_output) => {
                batch_output
                    .unprocessed_items
                    .unwrap_or_default()
                    .into_iter()
                    .flat_map(|(_table, reqs)| reqs)
                    .map(|req| req.put_request.expect("shouldn't be any other request than 'PutRequest' because events are append-only").item)
                    .map(extract_product_id)
                    .filter_map(|result| match result {
                        Ok(event) => Some(event),
                        Err(err) => {
                            error!(error = %err, "Failed extracting ProductId.");
                            None
                        }
                    }).for_each(|product_id| fail_message(&product_id, &mut message_ids, &mut failed_message_ids));
            }
            Err(err) => {
                error!(
                    error = ?err,
                    productIds = ?batch_product_ids,
                    "Failed to put ProductEventRecords batch into repository."
                );
                for product_id in batch_product_ids {
                    fail_message(&product_id, &mut message_ids, &mut failed_message_ids);
                }
            }
        }
    }

    let failure_count = failed_message_ids.len();
    info!(
        successful = records_count - failure_count - skipped_count,
        failures = failure_count,
        skipped = skipped_count,
        "Handler finished.",
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

async fn determine_category_id(
    product_id: &ProductId,
    embedding: &[f32],
    failed_message_ids: &mut Vec<String>,
    message_ids: &mut HashMap<ProductId, String>,
    category_service: &impl CategoryService,
) -> Option<CategoryId> {
    let similar_res = category_service.find_similar(embedding, 1).await;
    match similar_res {
        Ok(similar) => match similar.first() {
            Some((category, score)) => {
                info!(
                    productId = %product_id,
                    categoryId = %category.category_id,
                    score = score,
                    "Successfully found category similar to products' text-embedding. Choosing highest score."
                );
                Some(category.category_id.clone())
            }
            None => {
                error!(
                    productId = %product_id,
                    "Failed finding categories similar to products' text-embedding because no similar category was found."
                );
                fail_message(product_id, message_ids, failed_message_ids);
                None
            }
        },
        Err(err) => {
            error!(
                error = ?err,
                productId = %product_id,
                "Failed finding categories similar to products' text-embedding.");
            fail_message(product_id, message_ids, failed_message_ids);
            None
        }
    }
}

fn fail_message(
    product_id: &ProductId,
    message_ids: &mut HashMap<ProductId, String>,
    failed_message_ids: &mut Vec<String>,
) {
    match message_ids.remove(product_id) {
        Some(message_id) => failed_message_ids.push(message_id),
        None => {
            error!(
                productId = %product_id,
                "There exists no message_id for failed ProductRecord."
            );
        }
    }
}

async fn extract_message_data(
    message: SqsMessage,
    failed_message_ids: &mut Vec<String>,
    skipped_count: &mut usize,
    message_ids: &mut HashMap<ProductId, String>,
) -> Option<(ProductId, ProductKey, Vec<f32>)> {
    let message_id = message
        .message_id
        .clone()
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");
    let product_event_record: ProductEventRecord =
        extract_sqs_event_bridge_dynamodb_record(message, failed_message_ids, skipped_count)?;
    let product_key = product_event_record.key();
    let product_id = *product_event_record.product_id();
    let embedding = match product_event_record {
        ProductEventRecord::Enrichment(event_record) => match event_record.event_type {
            ProductEnrichmentEventTypeRecord::EnrichmentEmbeddedText => {
                match event_record.text_embedding {
                    Some(embedding) => Some(embedding),
                    None => {
                        error!(
                            productId = %product_id,
                            "Failed to extract embedding although previously matched 'ProductEnrichmentEventTypeRecord::EnrichmentEmbeddedText'.
                             This is a logic-bug."
                        );
                        None
                    }
                }
            }
            other => {
                error!(
                    productId = %product_id,
                    expectedEventType = "ENRICHMENT_CLASSIFY_CATEGORY",
                    actualEventType = serde_json::to_string(&other).unwrap_or_else(|_| "Failed to serialize event type.".to_string()).replace("\"", ""),
                    "Failed to extract embedding because supplied payload is wrong."
                );
                None
            }
        },
        other => {
            error!(
                productId = %product_id,
                expectedEventType = "ENRICHMENT_CLASSIFY_CATEGORY",
                actualPayload = ?other,
                "Failed to extract product-id and embedding because supplied payload is wrong."
            );
            None
        }
    };
    message_ids.insert(product_id, message_id);
    Some((product_id, product_key, embedding?))
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::enrichment::{
        ClassifyCategoryProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
    };
    use product::core::product_event::{ProductEvent, ProductEventPayload};
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use product_classification::category::core::Category;
    use product_classification::category::service::MockCategoryService;
    use std::time::SystemTime;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn mk_event_bridge_payload(product_event_record: &ProductEventRecord) -> String {
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

    fn mk_sqs_message(product_event_record: &ProductEventRecord) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Faker.fake());
        msg.body = Some(mk_event_bridge_payload(product_event_record));
        msg
    }

    #[tokio::test]
    #[rstest::rstest]
    #[trace]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[case(47)]
    #[case(100)]
    #[case(150)]
    #[case(453)]
    #[case(900)]
    #[case(2874)]
    #[case(10874)]
    async fn should_handle_sqs_message(#[case] record_count: usize) {
        let records = fake::vec![ClassifyCategoryProductEnrichmentEventPayload; record_count]
            .into_iter()
            .map(|payload| ProductEvent {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::ClassifyCategory(payload),
                ),
            })
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|product_event_record| mk_sqs_message(&product_event_record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_put_product_event_records() // if policy event
            .returning(move |_| {
                Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
            });
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async { Ok(fake::vec![(Category, f64); 1]) }));

        let actual = handler(&product_repository, &category_service, lambda_event)
            .await
            .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }
}
