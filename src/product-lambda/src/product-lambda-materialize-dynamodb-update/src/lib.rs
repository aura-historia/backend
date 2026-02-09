use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::category_key::CategoryId;
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use common::has_key::HasKey;
use common::product_id::ProductKey;
use lambda_runtime::LambdaEvent;
use once_cell::sync::OnceCell;
use product::dynamodb::product_update_record::ProductRecordUpdate;
use product::dynamodb::repository::ProductDynamoDbRepository;
use product::dynamodb::{
    product_event_record::ProductEventRecord,
    product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord,
};
use product_classification::category::dynamodb_repository::CategoryDynamoDbRepository;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

#[tracing::instrument(skip(product_repository, category_repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    product_repository: &impl ProductDynamoDbRepository,
    category_repository: &impl CategoryDynamoDbRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut updates = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ProductKey, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(update) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
            product_repository,
            category_repository,
        )
        .await
        {
            updates.push(update);
        }
    }

    for (key, update) in updates {
        let update_res = product_repository
            .update_product_record(&key.shop_id, &key.shops_product_id, update)
            .await;
        if let Err(err) = update_res {
            error!(error = ?err, productKey = %key, "Failed update.");
            match message_ids.remove(&key) {
                Some(message_id) => failed_message_ids.push(message_id),
                None => {
                    error!(
                        productKey = %key,
                        "There exists no message_id for failed ProductRecord."
                    );
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

struct CategoryNames {
    category_name_de: String,
    category_name_en: String,
    category_name_fr: String,
    category_name_es: String,
}
static CATEGORY_CACHE: OnceCell<Arc<RwLock<HashMap<CategoryId, CategoryNames>>>> = OnceCell::new();

async fn extract_message_data(
    message: SqsMessage,
    failed_message_ids: &mut Vec<String>,
    skipped_count: &mut usize,
    message_ids: &mut HashMap<ProductKey, String>,
    product_repository: &impl ProductDynamoDbRepository,
    category_repository: &impl CategoryDynamoDbRepository,
) -> Option<(ProductKey, ProductRecordUpdate)> {
    let message_id = message
        .message_id
        .clone()
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");
    let product_event_record: ProductEventRecord =
        extract_sqs_event_bridge_dynamodb_record(message, failed_message_ids, skipped_count)?;
    let key = product_event_record.key();
    let update_record = match product_event_record {
        ProductEventRecord::Domain(event_record) => Some(ProductRecordUpdate::from(event_record)),
        ProductEventRecord::Enrichment(event_record) => match event_record.event_type {
            ProductEnrichmentEventTypeRecord::EnrichmentClassifyCategory => {
                let category_cache_rw =
                    CATEGORY_CACHE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())));
                match event_record.category_id {
                    Some(ref category_id) => {
                        let category_id = category_id.clone();
                        let mut update_record = ProductRecordUpdate::from(event_record);
                        {
                            let category_cache_r = category_cache_rw.read().await;
                            if let Some(resolved) = category_cache_r.get(&category_id) {
                                update_record.category_name_de =
                                    Some(resolved.category_name_de.clone());
                                update_record.category_name_en =
                                    Some(resolved.category_name_en.clone());
                                update_record.category_name_fr =
                                    Some(resolved.category_name_fr.clone());
                                update_record.category_name_es =
                                    Some(resolved.category_name_es.clone());
                            }
                        }
                        if update_record.category_name_de.is_none() {
                            {
                                let mut category_cache_w = category_cache_rw.write().await;
                                let category_res =
                                    category_repository.get_category_record(&category_id).await;
                                match category_res {
                                    Ok(Some(category_record)) => {
                                        let category_names = CategoryNames {
                                            category_name_de: category_record
                                                .display_name_de
                                                .clone(),
                                            category_name_en: category_record
                                                .display_name_en
                                                .clone(),
                                            category_name_fr: category_record
                                                .display_name_fr
                                                .clone(),
                                            category_name_es: category_record
                                                .display_name_es
                                                .clone(),
                                        };
                                        update_record.category_name_de =
                                            Some(category_names.category_name_de.clone());
                                        update_record.category_name_en =
                                            Some(category_names.category_name_en.clone());
                                        update_record.category_name_fr =
                                            Some(category_names.category_name_fr.clone());
                                        update_record.category_name_es =
                                            Some(category_names.category_name_es.clone());

                                        category_cache_w
                                            .insert(category_id.clone(), category_names);
                                    }
                                    Ok(None) => {
                                        error!(
                                            categoryId = %category_id                                            ,
                                            "Failed to find category name for category_id because no CategoryRecord exists for this category_id.",
                                        );
                                    }
                                    Err(err) => {
                                        error!(
                                            error = ?err,
                                            categoryId = %category_id                                            ,
                                            "Failed to find category name for category_id.",
                                        );
                                    }
                                }
                            }
                        }
                        Some(update_record)
                    }
                    None => {
                        error!(
                            "Failed to resolve category name because category_id is None.
                             This is a logic error.
                             EnrichmentClassifyCategory event should always contain category_id."
                        );
                        Some(ProductRecordUpdate::from(event_record))
                    }
                }
            }
            _ => Some(ProductRecordUpdate::from(event_record)),
        },
        ProductEventRecord::Policy(event_record) => {
            let record_res = product_repository
                .get_product_record(&event_record.shop_id, &event_record.shops_product_id)
                .await;
            match record_res {
                Ok(Some(record)) => {
                    let mut update_record = ProductRecordUpdate::default();
                    let prohibited_images = record
                        .images
                        .into_iter()
                        .map(|mut image| {
                            image.prohibited_content = event_record.prohibited_content_decision;
                            image
                        })
                        .collect();
                    update_record.images = Some(prohibited_images);
                    Some(update_record)
                }
                Ok(None) => {
                    error!(
                        shopId = %event_record.shop_id,
                        shopsProductId = %event_record.shops_product_id,
                        "ProductRecord doesn't exist. This is a logic error. Impossible to apply policy to non-existent product."
                    );
                    failed_message_ids.push(message_id.clone());
                    None
                }
                Err(err) => {
                    error!(
                        error = ?err,
                        shopId = %event_record.shop_id,
                        "Failed getting ProductRecord"
                    );
                    failed_message_ids.push(message_id.clone());
                    None
                }
            }
        }
    };
    message_ids.insert(key.clone(), message_id);
    Some((key, update_record?))
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::update_item::UpdateItemOutput;
    use common::has_key::HasKey;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::ProductEvent;
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use product_classification::category::dynamodb_repository::MockCategoryDynamoDbRepository;
    use std::time::SystemTime;
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

    fn mk_sqs_message_with_id(
        product_event_record: &ProductEventRecord,
        message_id: String,
    ) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id);
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
        let records = fake::vec![ProductEvent; record_count]
            .into_iter()
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
            .expect_get_product_record() // if policy event
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        product_repository
            .expect_update_product_record()
            .returning(move |_, _, _| {
                Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
            });
        let mut category_repository = MockCategoryDynamoDbRepository::default();
        category_repository
            .expect_get_category_record()
            .returning(|_| Box::pin(async { Ok(Some(Faker.fake())) }));

        let actual = handler(&product_repository, &category_repository, lambda_event)
            .await
            .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[trace]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(7, 10)]
    #[case(24, 25)]
    #[case(0, 47)]
    #[case(98, 100)]
    #[case(1, 150)]
    #[case(0, 453)]
    #[case(0, 900)]
    #[case(2874, 2874)]
    #[case(874, 10874)]
    async fn should_respond_with_partial_failures(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let events = fake::vec![ProductEvent; record_count];
        let expected_failed_events = events
            .clone()
            .into_iter()
            .take(failure_count)
            .collect::<Vec<_>>();
        let mut expected_failed_message_ids = Vec::with_capacity(failure_count);
        let records = events
            .into_iter()
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| {
                let message_id = Uuid::new_v4().to_string();
                if expected_failed_events
                    .iter()
                    .any(|event| event.payload.key() == event_record.key())
                {
                    expected_failed_message_ids.push(message_id.clone());
                }
                mk_sqs_message_with_id(&event_record, message_id)
            })
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_get_product_record() // if policy event
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        product_repository.expect_update_product_record().returning(
            move |shop_id, shops_product_id, _| {
                if expected_failed_events.iter().any(|event| {
                    &event.payload.key().shop_id == shop_id
                        && &event.payload.key().shops_product_id == shops_product_id
                }) {
                    Box::pin(
                        async move { Err(SdkError::construction_failure("Something went wrong.")) },
                    )
                } else {
                    Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
                }
            },
        );
        let mut category_repository = MockCategoryDynamoDbRepository::default();
        category_repository
            .expect_get_category_record()
            .returning(|_| Box::pin(async { Ok(Some(Faker.fake())) }));

        expected_failed_message_ids.sort();
        let mut actual_failed_message_ids =
            handler(&product_repository, &category_repository, lambda_event)
                .await
                .unwrap()
                .batch_item_failures
                .into_iter()
                .map(|failure| failure.item_identifier)
                .collect::<Vec<_>>();
        actual_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }
}
