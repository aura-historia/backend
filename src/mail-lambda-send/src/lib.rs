use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use lambda_runtime::LambdaEvent;
use mail_core::{payload::MailPayload, send_service::SendMailService};
use tracing::{error, info, warn};

#[tracing::instrument(skip(service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    service: &impl SendMailService,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let messages_count = event.payload.records.len();
    info!(total = messages_count, "Handler invoked.",);

    let mut skipped_count = 0;
    let mut failed_message_ids = Vec::new();

    for msg in event.payload.records {
        let message_id = msg
            .message_id
            .expect("shouldn't fail extracting SQS-Message-Id because AWS sets it.");
        match msg.body {
            None => {
                warn!(
                    messageId = message_id,
                    "Skipping SQS-Message because its body is empty."
                );
                skipped_count += 1;
            }
            Some(body) => match serde_json::from_str::<MailPayload>(&body) {
                Ok(payload) => {
                    let send_mail_res = service.send_mail(payload).await;
                    if let Err(err) = send_mail_res {
                        error!(error = %err, "Failed sending mail.");
                        failed_message_ids.push(message_id);
                    }
                }
                Err(err) => {
                    error!(
                        error = %err,
                        type = %std::any::type_name::<MailPayload>(),
                        payload = body,
                        "Failed deserializing SQS-Message. Skipping message."
                    );
                    skipped_count += 1;
                }
            },
        }
    }

    let failure_count = failed_message_ids.len();
    info!(
        successful = messages_count - failure_count - skipped_count,
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
