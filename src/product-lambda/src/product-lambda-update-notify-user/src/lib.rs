pub mod service;

use crate::service::ProductEventWatchlistNotificationsService;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::NotificationService;
use product::core::product_event::ProductDomainEvent;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use tracing::{error, info};

#[tracing::instrument(skip(product_event_notification_service, notification_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    product_event_notification_service: &impl ProductEventWatchlistNotificationsService,
    notification_service: &impl NotificationService,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;

    for message in event.payload.records {
        let message_id = message
            .message_id
            .clone()
            .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");
        if let Some(product_event_record) = extract_sqs_event_bridge_dynamodb_record::<
            ProductDomainEventRecord,
        >(
            message, &mut failed_message_ids, &mut skipped_count
        ) {
            match ProductDomainEvent::try_from(product_event_record) {
                Ok(product_event) => {
                    let notification_cmds_res = product_event_notification_service
                        .determine_notification_commands(product_event)
                        .await;
                    match notification_cmds_res {
                        Ok(cmds) => {
                            let create_notifications_res =
                                notification_service.create_notifications(cmds).await;
                        }
                        Err(err) => {
                            error!(messageId = message_id, error = %err, "Failed creating MailPayloads.");
                            failed_message_ids.push(message_id);
                        }
                    }
                }
                Err(err) => {
                    error!(
                        error = %err,
                        fromType = %std::any::type_name::<ProductDomainEventRecord>(),
                        toType = %std::any::type_name::<ProductDomainEvent>(),
                        "Failed mapping types. Skipping event."
                    );
                    skipped_count += 1;
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
