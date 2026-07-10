use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::batch::Batch;
use common::dynamodb_stream::extract_from_dynamodb_stream;
use common::opensearch::bulk_response::{BulkItemResult, BulkResponse};
use common::product_lifecycle::domain::ProductLifecycle;
use lambda_runtime::LambdaEvent;
use product::dynamodb::product_event_record::ProductEventRecord;
use product::opensearch::repository::ProductOpenSearchRepository;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepository;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepository;
use tracing::{info, warn};

#[tracing::instrument(
    skip(opensearch_repository, watchlist_repository, search_filter_repository, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    opensearch_repository: &impl ProductOpenSearchRepository,
    watchlist_repository: &impl WatchlistProductDynamoDbRepository,
    search_filter_repository: &impl UserSearchFilterDynamoDbRepository,
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
    log_bulk_failures(response, "Delete");
    Ok(())
}

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

fn log_bulk_failures(response: BulkResponse, operation: &str) {
    if !response.errors {
        return;
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
}
