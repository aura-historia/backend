use aws_lambda_events::sqs::SqsEvent;
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use lambda_runtime::LambdaEvent;
use tracing::{error, info};
use user::{dynamodb::user_record::UserRecord, opensearch::repository::UserOpenSearchRepository};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl UserOpenSearchRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<(), lambda_runtime::Error> {
    let mut event = event;
    let msg = event.payload.records.remove(0);
    let mut skipped_count = 0;
    let mut failed_message_ids = Vec::new();
    let user_record_res = extract_sqs_event_bridge_dynamodb_record::<UserRecord>(
        msg,
        &mut failed_message_ids,
        &mut skipped_count,
    );
    match user_record_res {
        None => {
            let msg = "Failed extracting UserRecord from Lambda-Event.";
            error!(msg);
            Err(msg.into())
        }
        Some(user_record) => {
            let user_id = user_record.user_id;
            let index_res = repository.index_user_document(user_record.into()).await;
            match index_res {
                Ok(response) => {
                    info!(userId = %user_id, result = response.result, "Indexed UserRecord");
                    Ok(())
                }
                Err(err) => {
                    let msg = "Failed indexing UserRecord";
                    error!(error = %err, userId = %user_id, msg);
                    Err(msg.into())
                }
            }
        }
    }
}
