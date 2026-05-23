use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::dynamodb_stream::extract_from_dynamodb_stream;
use common::opensearch::bulk_response::{BulkItemResult, BulkResponse};
use common::product_id::ProductId;
use lambda_runtime::LambdaEvent;
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
use product::dynamodb::repository::ProductDynamoDbRepository;
use product::opensearch::product_document::ProductDocument;
use product::opensearch::product_image_document::ProductImageDocument;
use product::opensearch::product_update_document::ProductUpdateDocument;
use product::opensearch::repository::ProductOpenSearchRepository;
use std::collections::{HashMap, hash_map::Entry};
use tracing::{error, info, warn};

#[tracing::instrument(
    skip(
        opensearch_repository,
        product_dynamodb_repository,
        event
    ),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    opensearch_repository: &impl ProductOpenSearchRepository,
    product_dynamodb_repository: &impl ProductDynamoDbRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");
    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    let mut creates: HashMap<String, ProductDocument> = HashMap::new();
    let mut updates: HashMap<ProductId, (Vec<String>, ProductUpdateDocument)> = HashMap::new();

    for (message_id, event_record) in event_records {
        match event_record {
            ProductEventRecord::Domain(domain_record) => {
                handle_domain_event(
                    &message_id,
                    domain_record,
                    &mut creates,
                    &mut updates,
                    &mut failed_message_ids,
                );
            }
            ProductEventRecord::Enrichment(enrichment_record) => {
                let product_id = enrichment_record.product_id;
                let update = build_enrichment_update(enrichment_record);
                merge_update(message_id, product_id, update, &mut updates);
            }
            ProductEventRecord::Policy(policy_record) => {
                let product_id = policy_record.product_id;
                let record_res = product_dynamodb_repository
                    .get_product_record(&policy_record.shop_id, &policy_record.shops_product_id)
                    .await;
                match record_res {
                    Ok(Some(record)) => {
                        let mut update_document = ProductUpdateDocument::default();
                        let prohibited_images = record
                            .images
                            .into_iter()
                            .map(|mut image| {
                                image.prohibited_content =
                                    policy_record.prohibited_content_decision;
                                image
                            })
                            .map(ProductImageDocument::from)
                            .collect();
                        update_document.images = Some(prohibited_images);
                        merge_update(message_id, product_id, update_document, &mut updates);
                    }
                    Ok(None) => {
                        error!(
                            shopId = %policy_record.shop_id,
                            shopsProductId = %policy_record.shops_product_id,
                            "ProductRecord doesn't exist. This is a logic error. Impossible to apply policy to non-existent product."
                        );
                        failed_message_ids.push(message_id);
                    }
                    Err(err) => {
                        warn!(
                            error = ?err,
                            shopId = %policy_record.shop_id,
                            "Failed getting ProductRecord."
                        );
                        failed_message_ids.push(message_id);
                    }
                }
            }
        }
    }

    persist_creates(opensearch_repository, creates, &mut failed_message_ids).await;
    persist_updates(opensearch_repository, updates, &mut failed_message_ids).await;

    let failures = failed_message_ids.len();
    info!(
        successful = count - failures,
        failures = failures,
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

fn handle_domain_event(
    message_id: &str,
    domain_record: ProductDomainEventRecord,
    creates: &mut HashMap<String, ProductDocument>,
    updates: &mut HashMap<ProductId, (Vec<String>, ProductUpdateDocument)>,
    failed_message_ids: &mut Vec<String>,
) {
    if domain_record.event_type == ProductDomainEventTypeRecord::DomainCreated {
        match ProductDocument::try_from(domain_record) {
            Ok(document) => {
                creates.insert(message_id.to_string(), document);
            }
            Err(err) => {
                warn!(
                    error = %err,
                    fromType = %std::any::type_name::<ProductDomainEventRecord>(),
                    toType = %std::any::type_name::<ProductDocument>(),
                    "Failed mapping types."
                );
                failed_message_ids.push(message_id.to_string());
            }
        }
    } else {
        let product_id = domain_record.product_id;
        let update = ProductUpdateDocument::from(domain_record);
        merge_update(message_id.to_string(), product_id, update, updates);
    }
}

fn build_enrichment_update(
    event_record: product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord,
) -> ProductUpdateDocument {
    ProductUpdateDocument::from(event_record)
}

async fn persist_creates(
    repository: &impl ProductOpenSearchRepository,
    creates: HashMap<String, ProductDocument>,
    failed_message_ids: &mut Vec<String>,
) {
    if creates.is_empty() {
        return;
    }
    let mut message_ids: HashMap<ProductId, Vec<String>> = creates
        .iter()
        .map(|(msg_id, doc)| (doc._id(), vec![msg_id.clone()]))
        .collect();
    let documents: Vec<ProductDocument> = creates.into_values().collect();
    let result = repository.create_product_documents(documents).await;
    match result {
        Ok(response) => {
            handle_bulk_response(response, failed_message_ids, &mut message_ids, "Create");
        }
        Err(err) => {
            warn!(error = ?err, "Failed entire create batch.");
            failed_message_ids.extend(message_ids.into_values().flatten());
        }
    }
}

async fn persist_updates(
    repository: &impl ProductOpenSearchRepository,
    updates: HashMap<ProductId, (Vec<String>, ProductUpdateDocument)>,
    failed_message_ids: &mut Vec<String>,
) {
    if updates.is_empty() {
        return;
    }
    let mut message_ids: HashMap<ProductId, Vec<String>> = updates
        .iter()
        .map(|(product_id, (message_ids, _))| (*product_id, message_ids.clone()))
        .collect();
    let update_documents: HashMap<ProductId, ProductUpdateDocument> = updates
        .into_iter()
        .map(|(product_id, (_, update))| (product_id, update))
        .collect();
    let result = repository.update_product_documents(update_documents).await;
    match result {
        Ok(response) => {
            handle_bulk_response(response, failed_message_ids, &mut message_ids, "Update");
        }
        Err(err) => {
            warn!(error = ?err, "Failed entire update batch.");
            failed_message_ids.extend(message_ids.into_values().flatten());
        }
    }
}

fn handle_bulk_response(
    response: BulkResponse,
    failed_message_ids: &mut Vec<String>,
    message_ids: &mut HashMap<ProductId, Vec<String>>,
    operation: &str,
) {
    if response.errors {
        let failures = response.items.into_iter().filter_map(|bulk_item_result| {
            let op_result = match bulk_item_result {
                BulkItemResult::Create { create } => create,
                BulkItemResult::Update { update } => update,
            };
            Some(op_result).filter(|r| r.is_err())
        });

        for failure in failures {
            warn!(
                index = failure.index,
                productId = failure.id,
                status = failure.status,
                error = ?failure.error,
                operation = operation,
                "Failed product operation in OpenSearch."
            );
            match ProductId::try_from(failure.id.as_str()) {
                Ok(product_id) => match message_ids.remove(&product_id) {
                    Some(message_id_group) => {
                        failed_message_ids.extend(message_id_group);
                    }
                    None => {
                        error!(
                            index = failure.index,
                            productId = failure.id,
                            "Failed re-mapping product-id to message-id. Cannot retry."
                        );
                    }
                },
                Err(err) => {
                    error!(
                        index = failure.index,
                        productId = failure.id,
                        error = %err,
                        payload = ?failure,
                        "Failed parsing '_id' from OpenSearch-Response as 'ProductId'. Cannot retry."
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::event::Event;
    use common::opensearch::bulk_response::BulkItemResult;
    use common::opensearch::bulk_response::BulkOpResult;
    use common::opensearch::bulk_response::{BulkError, BulkResponse};
    use common::product_id::ProductId;
    use fake::Fake;
    use fake::Faker;
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::ProductEvent;
    use product::core::product_event::ProductEventPayload;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    };
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
    use product::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use product::opensearch::repository::MockProductOpenSearchRepository;
    use std::collections::HashMap;
    use std::time::SystemTime;
    use time::OffsetDateTime;
    use uuid::Uuid;

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

    fn mk_default_dynamodb_repository() -> MockProductDynamoDbRepository {
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_get_product_record()
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        repository
    }

    // ---- Tests for DOMAIN_CREATED events (creates) ----

    #[tokio::test]
    #[rstest::rstest]
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
    #[trace]
    async fn should_return_no_failures_when_all_created_events_processed(
        #[case] record_count: usize,
    ) {
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_create_product_documents()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: false,
                        items: vec![],
                    })
                })
            });

        let records = fake::vec![ProductCreatedDomainEventPayload; record_count]
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
            .map(|event_record| mk_sqs_message(&event_record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let dynamodb_repository = mk_default_dynamodb_repository();
        let actual = handler(&opensearch_repository, &dynamodb_repository, lambda_event)
            .await
            .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(9, 10)]
    #[case(0, 25)]
    #[case(47, 47)]
    #[case(100, 100)]
    #[case(0, 150)]
    #[case(234, 453)]
    #[case(773, 900)]
    #[case(299, 2874)]
    #[case(77, 10874)]
    #[trace]
    async fn should_return_partial_failures_when_opensearch_partial_create_failure(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let mut message_ids = HashMap::with_capacity(record_count);
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
            .map(|event_record| {
                let uuid = Uuid::new_v4().to_string();
                message_ids.insert(event_record.product_id, uuid.clone());
                mk_sqs_message_with_id(&event_record, uuid)
            })
            .collect();
        let expected_failures: Vec<_> = message_ids.keys().take(failure_count).cloned().collect();
        let expected_failures_clone = expected_failures.clone();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_create_product_documents()
            .return_once(move |batch| {
                let failures: Vec<_> = batch
                    .iter()
                    .filter(|&product_document| {
                        expected_failures_clone.contains(&product_document.product_id)
                    })
                    .map(|unprocessed_doc| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: unprocessed_doc.product_id.to_string(),
                            version: Some(2),
                            status: 409,
                            error: Some(BulkError {
                                error_type: "boop".to_string(),
                                reason: "[items][3]: version conflict, document already exists"
                                    .to_string(),
                                index_uuid: Some(Uuid::new_v4().to_string()),
                                shard: Some("shard-1".to_string()),
                                index: Some(index),
                                extra: Default::default(),
                            }),
                        }
                    })
                    .map(|create| BulkItemResult::Create { create })
                    .collect();

                let successes: Vec<_> = batch
                    .into_iter()
                    .filter(|product_document| {
                        !expected_failures_clone.contains(&product_document.product_id)
                    })
                    .map(|unprocessed_doc| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: unprocessed_doc.product_id.to_string(),
                            version: Some(2),
                            status: 201,
                            error: None,
                        }
                    })
                    .map(|create| BulkItemResult::Create { create })
                    .collect();
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: true,
                        items: [successes, failures].concat(),
                    })
                })
            });
        let dynamodb_repository = mk_default_dynamodb_repository();
        let mut actual_failed_message_ids =
            handler(&opensearch_repository, &dynamodb_repository, lambda_event)
                .await
                .unwrap()
                .batch_item_failures
                .into_iter()
                .map(|failure| failure.item_identifier)
                .collect::<Vec<_>>();
        actual_failed_message_ids.sort();
        let mut expected_failed_message_ids = expected_failures
            .into_iter()
            .map(|product_id| message_ids.remove(&product_id))
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }

    // ---- Tests for update events ----

    #[tokio::test]
    #[rstest::rstest]
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
    #[trace]
    async fn should_return_no_failures_when_all_update_events_processed(
        #[case] record_count: usize,
    ) {
        let mut dynamodb_repository = mk_default_dynamodb_repository();
        dynamodb_repository
            .expect_get_product_record()
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_create_product_documents()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: false,
                        items: vec![],
                    })
                })
            });
        opensearch_repository
            .expect_update_product_documents()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: false,
                        items: vec![],
                    })
                })
            });
        let records = fake::vec![ProductEventPayload; record_count]
            .into_iter()
            .map(|event_payload| Event {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload,
            })
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| mk_sqs_message(&event_record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let actual = handler(&opensearch_repository, &dynamodb_repository, lambda_event)
            .await
            .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(9, 10)]
    #[case(0, 25)]
    #[case(47, 47)]
    #[case(100, 100)]
    #[case(0, 150)]
    #[case(234, 453)]
    #[case(773, 900)]
    #[case(299, 2874)]
    #[case(77, 10874)]
    #[trace]
    async fn should_return_partial_failures_when_opensearch_partial_update_failure(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let mut message_ids = HashMap::with_capacity(record_count);
        let records: Vec<SqsMessage> = fake::vec![ProductEvent; record_count]
            .into_iter()
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| {
                let uuid = Uuid::new_v4().to_string();
                message_ids.insert(*event_record.product_id(), uuid.clone());
                mk_sqs_message_with_id(&event_record, uuid)
            })
            .collect();
        let expected_failures: Vec<_> = message_ids.keys().take(failure_count).cloned().collect();
        let expected_failures_for_create = expected_failures.clone();
        let expected_failures_clone = expected_failures.clone();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut dynamodb_repository = mk_default_dynamodb_repository();
        dynamodb_repository
            .expect_get_product_record()
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_create_product_documents()
            .return_once(move |batch| {
                let failures: Vec<_> = batch
                    .iter()
                    .filter(|doc| expected_failures_for_create.contains(&doc.product_id))
                    .map(|doc| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: doc.product_id.to_string(),
                            version: Some(2),
                            status: 409,
                            error: Some(BulkError {
                                error_type: "boop".to_string(),
                                reason: "version conflict".to_string(),
                                index_uuid: Some(Uuid::new_v4().to_string()),
                                shard: Some("shard-1".to_string()),
                                index: Some(index),
                                extra: Default::default(),
                            }),
                        }
                    })
                    .map(|create| BulkItemResult::Create { create })
                    .collect();
                let successes: Vec<_> = batch
                    .into_iter()
                    .filter(|doc| !expected_failures_for_create.contains(&doc.product_id))
                    .map(|doc| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: doc.product_id.to_string(),
                            version: Some(2),
                            status: 201,
                            error: None,
                        }
                    })
                    .map(|create| BulkItemResult::Create { create })
                    .collect();
                let has_errors = !failures.is_empty();
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: has_errors,
                        items: [successes, failures].concat(),
                    })
                })
            });
        opensearch_repository
            .expect_update_product_documents()
            .return_once(move |batch| {
                let failures: Vec<_> = batch
                    .iter()
                    .filter(|(product_id, _)| expected_failures_clone.contains(product_id))
                    .map(|(product_id, _)| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: product_id.to_string(),
                            version: Some(2),
                            status: 409,
                            error: Some(BulkError {
                                error_type: "boop".to_string(),
                                reason: "[items][3]: version conflict, document doesn't exist"
                                    .to_string(),
                                index_uuid: Some(Uuid::new_v4().to_string()),
                                shard: Some("shard-1".to_string()),
                                index: Some(index),
                                extra: Default::default(),
                            }),
                        }
                    })
                    .map(|update| BulkItemResult::Update { update })
                    .collect();

                let successes: Vec<_> = batch
                    .iter()
                    .filter(|(product_id, _)| !expected_failures_clone.contains(product_id))
                    .map(|(product_id, _)| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: product_id.to_string(),
                            version: Some(2),
                            status: 200,
                            error: None,
                        }
                    })
                    .map(|update| BulkItemResult::Update { update })
                    .collect();

                let has_errors = !failures.is_empty();
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: has_errors,
                        items: [successes, failures].concat(),
                    })
                })
            });
        let mut actual_failed_message_ids =
            handler(&opensearch_repository, &dynamodb_repository, lambda_event)
                .await
                .unwrap()
                .batch_item_failures
                .into_iter()
                .map(|failure| failure.item_identifier)
                .collect::<Vec<_>>();
        actual_failed_message_ids.sort();
        let mut expected_failed_message_ids = expected_failures
            .into_iter()
            .map(|product_id| message_ids.remove(&product_id))
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }

    // ---- Tests for mixed events ----

    #[tokio::test]
    async fn should_return_no_failures_when_mixed_create_and_update_events_all_succeed() {
        let create_records: Vec<SqsMessage> = fake::vec![ProductCreatedDomainEventPayload; 5]
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
        let update_records: Vec<SqsMessage> = fake::vec![ProductEvent; 5]
            .into_iter()
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|record| mk_sqs_message(&record))
            .collect();

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = [create_records, update_records].concat();
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut dynamodb_repository = mk_default_dynamodb_repository();
        dynamodb_repository
            .expect_get_product_record()
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_create_product_documents()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: false,
                        items: vec![],
                    })
                })
            });
        opensearch_repository
            .expect_update_product_documents()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: false,
                        items: vec![],
                    })
                })
            });
        let actual = handler(&opensearch_repository, &dynamodb_repository, lambda_event)
            .await
            .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_merge_updates_for_same_product_before_persisting() {
        let product_id = Faker.fake::<ProductId>();
        let message_id_en = Uuid::new_v4().to_string();
        let message_id_fr = Uuid::new_v4().to_string();

        let mut en_record = Faker.fake::<ProductEnrichmentEventRecord>();
        en_record.product_id = product_id;
        en_record.event_type = ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle;
        en_record.target_language = Some(common::language::record::LanguageRecord::En);
        en_record.target = Some("English title".to_string());

        let mut fr_record = Faker.fake::<ProductEnrichmentEventRecord>();
        fr_record.product_id = product_id;
        fr_record.event_type = ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle;
        fr_record.target_language = Some(common::language::record::LanguageRecord::Fr);
        fr_record.target = Some("Titre français".to_string());

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![
            mk_sqs_message_with_id(&en_record, message_id_en),
            mk_sqs_message_with_id(&fr_record, message_id_fr),
        ];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_update_product_documents()
            .return_once(move |batch| {
                assert_eq!(1, batch.len());
                let update = batch.get(&product_id).unwrap();
                assert_eq!(Some("English title"), update.title_en.as_deref());
                assert_eq!(Some("Titre français"), update.title_fr.as_deref());
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: false,
                        items: vec![],
                    })
                })
            });

        let dynamodb_repository = MockProductDynamoDbRepository::default();
        let actual = handler(&opensearch_repository, &dynamodb_repository, lambda_event)
            .await
            .unwrap();

        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_all_messages_when_merged_update_for_same_product_fails() {
        let product_id = Faker.fake::<ProductId>();
        let message_id_en = Uuid::new_v4().to_string();
        let message_id_fr = Uuid::new_v4().to_string();

        let mut en_record = Faker.fake::<ProductEnrichmentEventRecord>();
        en_record.product_id = product_id;
        en_record.event_type = ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle;
        en_record.target_language = Some(common::language::record::LanguageRecord::En);
        en_record.target = Some("English title".to_string());

        let mut fr_record = Faker.fake::<ProductEnrichmentEventRecord>();
        fr_record.product_id = product_id;
        fr_record.event_type = ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle;
        fr_record.target_language = Some(common::language::record::LanguageRecord::Fr);
        fr_record.target = Some("Titre français".to_string());

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![
            mk_sqs_message_with_id(&en_record, message_id_en.clone()),
            mk_sqs_message_with_id(&fr_record, message_id_fr.clone()),
        ];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_update_product_documents()
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: true,
                        items: vec![BulkItemResult::Update {
                            update: BulkOpResult {
                                index: "products".to_string(),
                                id: product_id.to_string(),
                                version: Some(2),
                                status: 409,
                                error: Some(BulkError {
                                    error_type: "version_conflict_engine_exception".to_string(),
                                    reason: "version conflict".to_string(),
                                    index_uuid: Some(Uuid::new_v4().to_string()),
                                    shard: Some("0".to_string()),
                                    index: Some("products".to_string()),
                                    extra: Default::default(),
                                }),
                            },
                        }],
                    })
                })
            });

        let dynamodb_repository = MockProductDynamoDbRepository::default();
        let mut actual_failed_message_ids =
            handler(&opensearch_repository, &dynamodb_repository, lambda_event)
                .await
                .unwrap()
                .batch_item_failures
                .into_iter()
                .map(|failure| failure.item_identifier)
                .collect::<Vec<_>>();
        actual_failed_message_ids.sort();

        let mut expected_failed_message_ids = vec![message_id_en, message_id_fr];
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }

    #[tokio::test]
    async fn should_return_no_failures_when_empty_batch() {
        let sqs_event = SqsEvent::default();
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let opensearch_repository = MockProductOpenSearchRepository::default();
        let dynamodb_repository = MockProductDynamoDbRepository::default();
        let actual = handler(&opensearch_repository, &dynamodb_repository, lambda_event)
            .await
            .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_messages_with_empty_body() {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Uuid::new_v4().to_string());
        msg.body = None;

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let opensearch_repository = MockProductOpenSearchRepository::default();
        let dynamodb_repository = MockProductDynamoDbRepository::default();
        let actual = handler(&opensearch_repository, &dynamodb_repository, lambda_event)
            .await
            .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_messages_with_invalid_json_body() {
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
        let opensearch_repository = MockProductOpenSearchRepository::default();
        let dynamodb_repository = MockProductDynamoDbRepository::default();
        let actual = handler(&opensearch_repository, &dynamodb_repository, lambda_event)
            .await
            .unwrap();
        assert_eq!(1, actual.batch_item_failures.len());
        assert_eq!(message_id, actual.batch_item_failures[0].item_identifier);
    }
}
fn merge_update(
    message_id: String,
    product_id: ProductId,
    update: ProductUpdateDocument,
    updates: &mut HashMap<ProductId, (Vec<String>, ProductUpdateDocument)>,
) {
    match updates.entry(product_id) {
        Entry::Occupied(mut entry) => {
            let (message_ids, current) = entry.get_mut();
            message_ids.push(message_id);
            merge_product_update_document(current, update);
        }
        Entry::Vacant(entry) => {
            entry.insert((vec![message_id], update));
        }
    }
}

fn merge_product_update_document(
    current: &mut ProductUpdateDocument,
    update: ProductUpdateDocument,
) {
    current.updated = update.updated;
    if let Some(event_id) = update.event_id {
        current.event_id = Some(event_id);
    }
    if let Some(price_eur) = update.price_eur {
        current.price_eur = Some(price_eur);
    }
    if let Some(price_usd) = update.price_usd {
        current.price_usd = Some(price_usd);
    }
    if let Some(price_gbp) = update.price_gbp {
        current.price_gbp = Some(price_gbp);
    }
    if let Some(price_aud) = update.price_aud {
        current.price_aud = Some(price_aud);
    }
    if let Some(price_cad) = update.price_cad {
        current.price_cad = Some(price_cad);
    }
    if let Some(price_nzd) = update.price_nzd {
        current.price_nzd = Some(price_nzd);
    }
    if let Some(price_cny) = update.price_cny {
        current.price_cny = Some(price_cny);
    }
    if let Some(price_brl) = update.price_brl {
        current.price_brl = Some(price_brl);
    }
    if let Some(price_pln) = update.price_pln {
        current.price_pln = Some(price_pln);
    }
    if let Some(price_try) = update.price_try {
        current.price_try = Some(price_try);
    }
    if let Some(price_jpy) = update.price_jpy {
        current.price_jpy = Some(price_jpy);
    }
    if let Some(price_czk) = update.price_czk {
        current.price_czk = Some(price_czk);
    }
    if let Some(price_rub) = update.price_rub {
        current.price_rub = Some(price_rub);
    }
    if let Some(price_aed) = update.price_aed {
        current.price_aed = Some(price_aed);
    }
    if let Some(price_sar) = update.price_sar {
        current.price_sar = Some(price_sar);
    }
    if let Some(price_hkd) = update.price_hkd {
        current.price_hkd = Some(price_hkd);
    }
    if let Some(price_sgd) = update.price_sgd {
        current.price_sgd = Some(price_sgd);
    }
    if let Some(price_chf) = update.price_chf {
        current.price_chf = Some(price_chf);
    }
    if let Some(state) = update.state {
        current.state = Some(state);
    }
    if let Some(title_de) = update.title_de {
        current.title_de = Some(title_de);
    }
    if let Some(title_en) = update.title_en {
        current.title_en = Some(title_en);
    }
    if let Some(title_fr) = update.title_fr {
        current.title_fr = Some(title_fr);
    }
    if let Some(title_es) = update.title_es {
        current.title_es = Some(title_es);
    }
    if let Some(title_it) = update.title_it {
        current.title_it = Some(title_it);
    }
    if let Some(images) = update.images {
        current.images = Some(images);
    }
    if let Some(price_estimate_min_eur) = update.price_estimate_min_eur {
        current.price_estimate_min_eur = Some(price_estimate_min_eur);
    }
    if let Some(price_estimate_min_usd) = update.price_estimate_min_usd {
        current.price_estimate_min_usd = Some(price_estimate_min_usd);
    }
    if let Some(price_estimate_min_gbp) = update.price_estimate_min_gbp {
        current.price_estimate_min_gbp = Some(price_estimate_min_gbp);
    }
    if let Some(price_estimate_min_aud) = update.price_estimate_min_aud {
        current.price_estimate_min_aud = Some(price_estimate_min_aud);
    }
    if let Some(price_estimate_min_cad) = update.price_estimate_min_cad {
        current.price_estimate_min_cad = Some(price_estimate_min_cad);
    }
    if let Some(price_estimate_min_nzd) = update.price_estimate_min_nzd {
        current.price_estimate_min_nzd = Some(price_estimate_min_nzd);
    }
    if let Some(price_estimate_min_cny) = update.price_estimate_min_cny {
        current.price_estimate_min_cny = Some(price_estimate_min_cny);
    }
    if let Some(price_estimate_min_brl) = update.price_estimate_min_brl {
        current.price_estimate_min_brl = Some(price_estimate_min_brl);
    }
    if let Some(price_estimate_min_pln) = update.price_estimate_min_pln {
        current.price_estimate_min_pln = Some(price_estimate_min_pln);
    }
    if let Some(price_estimate_min_try) = update.price_estimate_min_try {
        current.price_estimate_min_try = Some(price_estimate_min_try);
    }
    if let Some(price_estimate_min_jpy) = update.price_estimate_min_jpy {
        current.price_estimate_min_jpy = Some(price_estimate_min_jpy);
    }
    if let Some(price_estimate_min_czk) = update.price_estimate_min_czk {
        current.price_estimate_min_czk = Some(price_estimate_min_czk);
    }
    if let Some(price_estimate_min_rub) = update.price_estimate_min_rub {
        current.price_estimate_min_rub = Some(price_estimate_min_rub);
    }
    if let Some(price_estimate_min_aed) = update.price_estimate_min_aed {
        current.price_estimate_min_aed = Some(price_estimate_min_aed);
    }
    if let Some(price_estimate_min_sar) = update.price_estimate_min_sar {
        current.price_estimate_min_sar = Some(price_estimate_min_sar);
    }
    if let Some(price_estimate_min_hkd) = update.price_estimate_min_hkd {
        current.price_estimate_min_hkd = Some(price_estimate_min_hkd);
    }
    if let Some(price_estimate_min_sgd) = update.price_estimate_min_sgd {
        current.price_estimate_min_sgd = Some(price_estimate_min_sgd);
    }
    if let Some(price_estimate_min_chf) = update.price_estimate_min_chf {
        current.price_estimate_min_chf = Some(price_estimate_min_chf);
    }
    if let Some(price_estimate_max_eur) = update.price_estimate_max_eur {
        current.price_estimate_max_eur = Some(price_estimate_max_eur);
    }
    if let Some(price_estimate_max_usd) = update.price_estimate_max_usd {
        current.price_estimate_max_usd = Some(price_estimate_max_usd);
    }
    if let Some(price_estimate_max_gbp) = update.price_estimate_max_gbp {
        current.price_estimate_max_gbp = Some(price_estimate_max_gbp);
    }
    if let Some(price_estimate_max_aud) = update.price_estimate_max_aud {
        current.price_estimate_max_aud = Some(price_estimate_max_aud);
    }
    if let Some(price_estimate_max_cad) = update.price_estimate_max_cad {
        current.price_estimate_max_cad = Some(price_estimate_max_cad);
    }
    if let Some(price_estimate_max_nzd) = update.price_estimate_max_nzd {
        current.price_estimate_max_nzd = Some(price_estimate_max_nzd);
    }
    if let Some(price_estimate_max_cny) = update.price_estimate_max_cny {
        current.price_estimate_max_cny = Some(price_estimate_max_cny);
    }
    if let Some(price_estimate_max_brl) = update.price_estimate_max_brl {
        current.price_estimate_max_brl = Some(price_estimate_max_brl);
    }
    if let Some(price_estimate_max_pln) = update.price_estimate_max_pln {
        current.price_estimate_max_pln = Some(price_estimate_max_pln);
    }
    if let Some(price_estimate_max_try) = update.price_estimate_max_try {
        current.price_estimate_max_try = Some(price_estimate_max_try);
    }
    if let Some(price_estimate_max_jpy) = update.price_estimate_max_jpy {
        current.price_estimate_max_jpy = Some(price_estimate_max_jpy);
    }
    if let Some(price_estimate_max_czk) = update.price_estimate_max_czk {
        current.price_estimate_max_czk = Some(price_estimate_max_czk);
    }
    if let Some(price_estimate_max_rub) = update.price_estimate_max_rub {
        current.price_estimate_max_rub = Some(price_estimate_max_rub);
    }
    if let Some(price_estimate_max_aed) = update.price_estimate_max_aed {
        current.price_estimate_max_aed = Some(price_estimate_max_aed);
    }
    if let Some(price_estimate_max_sar) = update.price_estimate_max_sar {
        current.price_estimate_max_sar = Some(price_estimate_max_sar);
    }
    if let Some(price_estimate_max_hkd) = update.price_estimate_max_hkd {
        current.price_estimate_max_hkd = Some(price_estimate_max_hkd);
    }
    if let Some(price_estimate_max_sgd) = update.price_estimate_max_sgd {
        current.price_estimate_max_sgd = Some(price_estimate_max_sgd);
    }
    if let Some(price_estimate_max_chf) = update.price_estimate_max_chf {
        current.price_estimate_max_chf = Some(price_estimate_max_chf);
    }
    if let Some(url) = update.url {
        current.url = Some(url);
    }
    if let Some(auction_start) = update.auction_start {
        current.auction_start = Some(auction_start);
    }
    if let Some(auction_end) = update.auction_end {
        current.auction_end = Some(auction_end);
    }
    if let Some(embedding) = update.embedding {
        current.embedding = Some(embedding);
    }
}
