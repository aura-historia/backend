use aws_lambda_events::sqs::SqsEvent;
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use lambda_runtime::LambdaEvent;
use shop::{dynamodb::shop_record::ShopRecord, opensearch::repository::ShopOpenSearchRepository};
use tracing::{error, info};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl ShopOpenSearchRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<(), lambda_runtime::Error> {
    let mut event = event;
    let msg = event.payload.records.remove(0);
    let mut skipped_count = 0;
    let mut failed_message_ids = Vec::new();
    let shop_record_res = extract_sqs_event_bridge_dynamodb_record::<ShopRecord>(
        msg,
        &mut failed_message_ids,
        &mut skipped_count,
    );
    match shop_record_res {
        None => {
            let msg = "Failed extracting ShopRecord from Lambda-Event.";
            error!(msg);
            Err(msg.into())
        }
        Some(shop_record) => {
            let shop_id = shop_record.shop_id;
            let index_res = repository.create_shop_document(shop_record.into()).await;
            match index_res {
                Ok(response) => {
                    info!(
                        shopId = %shop_id,
                        result = response.result,
                        "Indexed ShopRecord"
                    );
                    Ok(())
                }
                Err(err) => {
                    let msg = "Failed indexing ShopRecord from Lambda-Event.";
                    error!(error = %err, shopId = %shop_id, msg);
                    Err(msg.into())
                }
            }
        }
    }
}
