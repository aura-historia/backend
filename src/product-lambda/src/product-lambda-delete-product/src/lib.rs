use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::batch::Batch;
use common::dynamodb_stream::extract_from_dynamodb_stream;
use common::opensearch::bulk_response::{BulkItemResult, BulkResponse};
use common::product_lifecycle::domain::ProductLifecycle;
use lambda_runtime::LambdaEvent;
use product::dynamodb::{
    product_event_record::ProductEventRecord, repository::ProductDynamoDbRepository,
};
use product::opensearch::repository::ProductOpenSearchRepository;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepository;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepository;
use tracing::{info, warn};

#[tracing::instrument(
    skip(
        opensearch_repository,
        watchlist_repository,
        search_filter_repository,
        product_repository,
        event
    ),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    opensearch_repository: &impl ProductOpenSearchRepository,
    watchlist_repository: &impl WatchlistProductDynamoDbRepository,
    search_filter_repository: &impl UserSearchFilterDynamoDbRepository,
    product_repository: &impl ProductDynamoDbRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count, "Start deleting products...");
    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    for (message_id, event_record) in event_records {
        let ProductEventRecord::Lifecycle(record) = event_record else {
            continue;
        };
        if ProductLifecycle::from(record.new_lifecycle) != ProductLifecycle::Deleted {
            continue;
        }

        let product_id = record.product_id;
        let shop_id = record.shop_id;
        let shops_product_id = record.shops_product_id;

        let mut failed = false;
        if let Err(err) = delete_opensearch_product(opensearch_repository, product_id).await {
            warn!(error = ?err, %product_id, "Failed deleting product document.");
            failed = true;
        }
        if let Err(err) = delete_watchlist_records(watchlist_repository, &product_id).await {
            warn!(error = ?err, %product_id, "Failed deleting watchlist records.");
            failed = true;
        }
        if let Err(err) =
            delete_search_filter_match_records(search_filter_repository, &product_id).await
        {
            warn!(error = ?err, %product_id, "Failed deleting search-filter match records.");
            failed = true;
        }
        if !failed
            && let Err(err) =
                delete_product_dynamodb_records(product_repository, &shop_id, &shops_product_id)
                    .await
        {
            warn!(error = ?err, %product_id, "Failed deleting product DynamoDB records.");
            failed = true;
        }
        if failed {
            failed_message_ids.push(message_id);
        }
    }

    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(|item_identifier| {
            let mut failure = BatchItemFailure::default();
            failure.item_identifier = item_identifier;
            failure
        })
        .collect();
    Ok(response)
}

async fn delete_opensearch_product(
    repository: &impl ProductOpenSearchRepository,
    product_id: common::product_id::ProductId,
) -> Result<(), opensearch::Error> {
    let response = repository
        .delete_product_documents(vec![product_id])
        .await?;
    if log_bulk_failures(response, "Delete") {
        return Err(serde_json::Error::io(std::io::Error::other(
            "OpenSearch bulk delete product failed",
        ))
        .into());
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn delete_watchlist_records(
    repository: &impl WatchlistProductDynamoDbRepository,
    product_id: &common::product_id::ProductId,
) -> Result<
    (),
    aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemError,
    >,
> {
    let records = repository
        .query_user_ids_watching_product(product_id)
        .await
        .map_err(|err| {
            aws_sdk_dynamodb::error::SdkError::construction_failure(format!("{err:?}"))
        })?;
    let keys = records
        .into_iter()
        .map(|record| (record.user_id, record.shop_id, record.shops_product_id));
    for batch in Batch::<_, 25>::chunked_from(keys) {
        repository.delete_watchlist_records(batch).await?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn delete_search_filter_match_records(
    repository: &impl UserSearchFilterDynamoDbRepository,
    product_id: &common::product_id::ProductId,
) -> Result<
    (),
    aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemError,
        aws_sdk_dynamodb::config::http::HttpResponse,
    >,
> {
    let keys = repository
        .query_user_search_filter_match_keys_for_product_id(product_id)
        .await
        .map_err(|err| {
            aws_sdk_dynamodb::error::SdkError::construction_failure(format!("{err:?}"))
        })?;
    for batch in Batch::<_, 25>::chunked_from(keys.into_iter()) {
        repository
            .delete_user_search_filter_match_records(batch)
            .await?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn delete_product_dynamodb_records(
    repository: &impl ProductDynamoDbRepository,
    shop_id: &common::shop_id::ShopId,
    shops_product_id: &common::shops_product_id::ShopsProductId,
) -> Result<
    (),
    aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemError,
        aws_sdk_dynamodb::config::http::HttpResponse,
    >,
> {
    let keys = repository
        .query_product_record_and_event_record_keys(shop_id, shops_product_id)
        .await
        .map_err(|err| {
            aws_sdk_dynamodb::error::SdkError::construction_failure(format!("{err:?}"))
        })?;
    for batch in Batch::<_, 25>::chunked_from(keys.into_iter()) {
        repository
            .delete_product_record_and_event_records(batch)
            .await?;
    }
    Ok(())
}

fn log_bulk_failures(response: BulkResponse, operation: &str) -> bool {
    if !response.errors {
        return false;
    }
    for item in response.items {
        let result = match item {
            BulkItemResult::Create { create } => create,
            BulkItemResult::Update { update } => update,
            BulkItemResult::Delete { delete } => delete,
        };
        if result.is_err() {
            warn!(
                index = result.index,
                productId = result.id,
                status = result.status,
                error = ?result.error,
                operation,
                "Failed product operation in OpenSearch."
            );
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::{
        dynamodb::{EventRecord, StreamRecord},
        eventbridge::EventBridgeEvent,
        sqs::SqsMessage,
    };
    use aws_sdk_dynamodb::{
        error::SdkError,
        operation::batch_write_item::{BatchWriteItemError, BatchWriteItemOutput},
    };
    use common::{
        product_id::ProductId, product_lifecycle::record::ProductLifecycleRecord, shop_id::ShopId,
        shops_product_id::ShopsProductId, user_search_filter_id::UserSearchFilterId,
    };
    use fake::{Fake, Faker};
    use lambda_runtime::Context;
    use product::{
        dynamodb::{
            product_event_record::{
                ProductEventRecord, domain::ProductDomainEventRecord,
                lifecycle::ProductLifecycleEventRecord,
            },
            product_event_type_record::{
                domain::ProductDomainEventTypeRecord, lifecycle::ProductLifecycleEventTypeRecord,
            },
            repository::MockProductDynamoDbRepository,
        },
        opensearch::repository::MockProductOpenSearchRepository,
    };
    use product_watchlist::dynamodb::{
        record::WatchlistProductRecord, repository::MockWatchlistProductDynamoDbRepository,
    };
    use search_filter::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;

    fn sqs_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
        let mut event = SqsEvent::default();
        event.records = messages;
        LambdaEvent::new(event, Context::default())
    }

    fn sqs_message(message_id: &str, body: Option<String>) -> SqsMessage {
        let mut message = SqsMessage::default();
        message.message_id = Some(message_id.to_owned());
        message.body = body;
        message
    }

    fn event_bridge_body(record: ProductEventRecord) -> String {
        let mut stream_record = StreamRecord::default();
        stream_record.new_image = serde_dynamo::to_item(record)
            .expect("product event record should serialize to dynamodb item");

        let mut event_record = EventRecord::default();
        event_record.event_name = "INSERT".to_owned();
        event_record.change = stream_record;

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail = event_record;
        serde_json::to_string(&event).expect("event should serialize")
    }

    fn lifecycle_record(
        product_id: ProductId,
        new_lifecycle: ProductLifecycleRecord,
    ) -> ProductLifecycleEventRecord {
        let mut record = Faker.fake::<ProductLifecycleEventRecord>();
        record.product_id = product_id;
        record.event_type = ProductLifecycleEventTypeRecord::LifecycleDeleted;
        record.new_lifecycle = new_lifecycle;
        record.old_lifecycle = ProductLifecycleRecord::Active;
        record.timestamp = OffsetDateTime::now_utc();
        record
    }

    fn deleted_product_message(message_id: &str, product_id: ProductId) -> SqsMessage {
        let record = lifecycle_record(product_id, ProductLifecycleRecord::Deleted);
        sqs_message(
            message_id,
            Some(event_bridge_body(ProductEventRecord::Lifecycle(record))),
        )
    }

    fn domain_product_message(message_id: &str) -> SqsMessage {
        let mut record = Faker.fake::<ProductDomainEventRecord>();
        record.event_type = ProductDomainEventTypeRecord::DomainStateChanged;
        record.timestamp = OffsetDateTime::now_utc();
        sqs_message(
            message_id,
            Some(event_bridge_body(ProductEventRecord::Domain(record))),
        )
    }

    fn active_lifecycle_message(message_id: &str, product_id: ProductId) -> SqsMessage {
        let record = lifecycle_record(product_id, ProductLifecycleRecord::Active);
        sqs_message(
            message_id,
            Some(event_bridge_body(ProductEventRecord::Lifecycle(record))),
        )
    }

    fn bulk_response(product_id: ProductId, errors: bool) -> BulkResponse {
        let item = if errors {
            json!({
                "delete": {
                    "_index": "products",
                    "_id": product_id.to_string(),
                    "status": 500,
                    "error": {
                        "type": "internal_error",
                        "reason": "boom"
                    }
                }
            })
        } else {
            json!({
                "delete": {
                    "_index": "products",
                    "_id": product_id.to_string(),
                    "_version": 2,
                    "status": 200
                }
            })
        };
        serde_json::from_value(json!({
            "took": 1,
            "errors": errors,
            "items": [item]
        }))
        .expect("bulk response should deserialize")
    }

    fn empty_bulk_response() -> BulkResponse {
        serde_json::from_value(json!({
            "took": 1,
            "errors": false,
            "items": []
        }))
        .expect("bulk response should deserialize")
    }

    fn watchlist_records(product_id: ProductId, count: usize) -> Vec<WatchlistProductRecord> {
        (0..count)
            .map(|_| {
                let mut record = Faker.fake::<WatchlistProductRecord>();
                record.product_id = product_id;
                record
            })
            .collect()
    }

    fn search_filter_keys(
        count: usize,
    ) -> Vec<(
        common::user_id::UserId,
        UserSearchFilterId,
        ShopId,
        ShopsProductId,
    )> {
        (0..count)
            .map(|_| (Faker.fake(), Faker.fake(), Faker.fake(), Faker.fake()))
            .collect()
    }

    fn product_dynamodb_keys(count: usize) -> Vec<(String, String)> {
        (0..count)
            .map(|idx| {
                (
                    "product#shop_id#shop#shops_product_id#product".to_owned(),
                    format!("product#{idx}"),
                )
            })
            .collect()
    }

    fn batch_failure_ids(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect()
    }

    fn successful_batch_write() -> BatchWriteItemOutput {
        BatchWriteItemOutput::builder().build()
    }

    #[tokio::test]
    async fn should_delete_product_and_cleanup_user_resources_when_deleted_lifecycle_event_received()
     {
        let product_id = ProductId::new();
        let watchlist_records = watchlist_records(product_id, 26);
        let search_filter_keys = search_filter_keys(26);
        let product_dynamodb_keys = product_dynamodb_keys(26);
        let watchlist_batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let search_filter_batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let product_dynamodb_batch_sizes = Arc::new(Mutex::new(Vec::new()));

        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .times(1)
            .withf(move |product_ids| product_ids.as_slice() == [product_id])
            .return_once(move |_| Box::pin(async move { Ok(bulk_response(product_id, false)) }));

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .times(1)
            .withf(move |actual_product_id| *actual_product_id == product_id)
            .return_once(move |_| Box::pin(async move { Ok(watchlist_records) }));
        let watchlist_batch_sizes_clone = Arc::clone(&watchlist_batch_sizes);
        watchlist_repository
            .expect_delete_watchlist_records()
            .times(2)
            .returning(move |batch| {
                watchlist_batch_sizes_clone
                    .lock()
                    .expect("batch sizes lock should not be poisoned")
                    .push(batch.len());
                Box::pin(async { Ok(successful_batch_write()) })
            });

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .times(1)
            .withf(move |actual_product_id| *actual_product_id == product_id)
            .return_once(move |_| Box::pin(async move { Ok(search_filter_keys) }));
        let search_filter_batch_sizes_clone = Arc::clone(&search_filter_batch_sizes);
        search_filter_repository
            .expect_delete_user_search_filter_match_records()
            .times(2)
            .returning(move |batch| {
                search_filter_batch_sizes_clone
                    .lock()
                    .expect("batch sizes lock should not be poisoned")
                    .push(batch.len());
                Box::pin(async { Ok(successful_batch_write()) })
            });

        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_query_product_record_and_event_record_keys()
            .times(1)
            .return_once(move |_, _| Box::pin(async move { Ok(product_dynamodb_keys) }));
        let product_dynamodb_batch_sizes_clone = Arc::clone(&product_dynamodb_batch_sizes);
        product_repository
            .expect_delete_product_record_and_event_records()
            .times(2)
            .returning(move |batch| {
                product_dynamodb_batch_sizes_clone
                    .lock()
                    .expect("batch sizes lock should not be poisoned")
                    .push(batch.len());
                Box::pin(async { Ok(successful_batch_write()) })
            });

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message("msg-delete", product_id)]),
        )
        .await
        .expect("handler should respond");

        assert!(response.batch_item_failures.is_empty());
        assert_eq!(
            vec![25, 1],
            *watchlist_batch_sizes
                .lock()
                .expect("batch sizes lock should not be poisoned")
        );
        assert_eq!(
            vec![25, 1],
            *search_filter_batch_sizes
                .lock()
                .expect("batch sizes lock should not be poisoned")
        );
        assert_eq!(
            vec![25, 1],
            *product_dynamodb_batch_sizes
                .lock()
                .expect("batch sizes lock should not be poisoned")
        );
    }

    #[tokio::test]
    async fn should_return_partial_batch_failures_when_one_deleted_product_fails() {
        let successful_product_id = ProductId::new();
        let failed_product_id = ProductId::new();

        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .times(2)
            .returning(|product_ids| {
                let product_id = product_ids[0];
                Box::pin(async move { Ok(bulk_response(product_id, false)) })
            });

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .times(2)
            .returning(move |product_id| {
                let failed = *product_id == failed_product_id;
                Box::pin(async move {
                    if failed {
                        Err(SdkError::construction_failure("watchlist query failed"))
                    } else {
                        Ok(Vec::new())
                    }
                })
            });

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .times(2)
            .returning(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_query_product_record_and_event_record_keys()
            .times(1)
            .return_once(|_, _| Box::pin(async { Ok(product_dynamodb_keys(1)) }));
        product_repository
            .expect_delete_product_record_and_event_records()
            .times(1)
            .return_once(|_| Box::pin(async { Ok(successful_batch_write()) }));

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![
                deleted_product_message("msg-ok", successful_product_id),
                deleted_product_message("msg-fail", failed_product_id),
            ]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-fail"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_skip_records_when_not_deleted_lifecycle_event() {
        let product_id = ProductId::new();
        let opensearch_repository = MockProductOpenSearchRepository::default();
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let product_repository = MockProductDynamoDbRepository::default();

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![
                domain_product_message("msg-domain"),
                active_lifecycle_message("msg-active", product_id),
                sqs_message("msg-empty", None),
            ]),
        )
        .await
        .expect("handler should respond");

        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_message_body_is_invalid() {
        let opensearch_repository = MockProductOpenSearchRepository::default();
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let product_repository = MockProductDynamoDbRepository::default();

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![sqs_message(
                "msg-invalid",
                Some("not-json".to_owned()),
            )]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-invalid"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_opensearch_delete_request_fails() {
        let product_id = ProductId::new();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .times(1)
            .return_once(|_| {
                Box::pin(async {
                    Err(opensearch::Error::from(serde_json::Error::io(
                        std::io::Error::other("boom"),
                    )))
                })
            });

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));
        let product_repository = MockProductDynamoDbRepository::default();

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message("msg-os", product_id)]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-os"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_opensearch_bulk_delete_item_fails() {
        let product_id = ProductId::new();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(bulk_response(product_id, true)) }));

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));
        let product_repository = MockProductDynamoDbRepository::default();

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message("msg-bulk", product_id)]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-bulk"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_watchlist_query_fails() {
        let product_id = ProductId::new();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .return_once(move |_| Box::pin(async move { Ok(empty_bulk_response()) }));

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .return_once(|_| {
                Box::pin(async { Err(SdkError::construction_failure("query failed")) })
            });

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));
        let product_repository = MockProductDynamoDbRepository::default();

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message("msg-watch-query", product_id)]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-watch-query"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_watchlist_delete_fails() {
        let product_id = ProductId::new();
        let watchlist_records = watchlist_records(product_id, 1);
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .return_once(move |_| Box::pin(async move { Ok(empty_bulk_response()) }));

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .return_once(move |_| Box::pin(async move { Ok(watchlist_records) }));
        watchlist_repository
            .expect_delete_watchlist_records()
            .return_once(|_| {
                Box::pin(async {
                    Err(SdkError::<BatchWriteItemError>::construction_failure(
                        "delete failed",
                    ))
                })
            });

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));
        let product_repository = MockProductDynamoDbRepository::default();

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message(
                "msg-watch-delete",
                product_id,
            )]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-watch-delete"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_search_filter_query_fails() {
        let product_id = ProductId::new();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .return_once(move |_| Box::pin(async move { Ok(empty_bulk_response()) }));

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .return_once(|_| {
                Box::pin(async { Err(SdkError::construction_failure("query failed")) })
            });
        let product_repository = MockProductDynamoDbRepository::default();

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message(
                "msg-search-query",
                product_id,
            )]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-search-query"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_product_dynamodb_query_fails() {
        let product_id = ProductId::new();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .return_once(move |_| Box::pin(async move { Ok(empty_bulk_response()) }));

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_query_product_record_and_event_record_keys()
            .return_once(|_, _| {
                Box::pin(async { Err(SdkError::construction_failure("query failed")) })
            });

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message(
                "msg-product-query",
                product_id,
            )]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-product-query"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_product_dynamodb_delete_fails() {
        let product_id = ProductId::new();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .return_once(move |_| Box::pin(async move { Ok(empty_bulk_response()) }));

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_query_product_record_and_event_record_keys()
            .return_once(|_, _| Box::pin(async { Ok(product_dynamodb_keys(1)) }));
        product_repository
            .expect_delete_product_record_and_event_records()
            .return_once(|_| {
                Box::pin(async {
                    Err(SdkError::<
                        BatchWriteItemError,
                        aws_sdk_dynamodb::config::http::HttpResponse,
                    >::construction_failure("delete failed"))
                })
            });

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message(
                "msg-product-delete",
                product_id,
            )]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-product-delete"], batch_failure_ids(response));
    }

    #[tokio::test]
    async fn should_return_batch_failure_when_search_filter_delete_fails() {
        let product_id = ProductId::new();
        let search_filter_keys = search_filter_keys(1);
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_delete_product_documents()
            .return_once(move |_| Box::pin(async move { Ok(empty_bulk_response()) }));

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_keys_for_product_id()
            .return_once(move |_| Box::pin(async move { Ok(search_filter_keys) }));
        search_filter_repository
            .expect_delete_user_search_filter_match_records()
            .return_once(|_| {
                Box::pin(async {
                    Err(SdkError::<
                        BatchWriteItemError,
                        aws_sdk_dynamodb::config::http::HttpResponse,
                    >::construction_failure("delete failed"))
                })
            });
        let product_repository = MockProductDynamoDbRepository::default();

        let response = handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            &product_repository,
            sqs_event(vec![deleted_product_message(
                "msg-search-delete",
                product_id,
            )]),
        )
        .await
        .expect("handler should respond");

        assert_eq!(vec!["msg-search-delete"], batch_failure_ids(response));
    }
}
