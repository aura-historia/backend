pub mod service;

use crate::service::ProductEventMailPayloadService;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use lambda_runtime::LambdaEvent;
use mail_core::{payload::MailPayload, queue_service::QueueMailService};
use product::core::product_event::ProductEvent;
use product::dynamodb::product_event_record::ProductEventRecord;
use product_lambda_common::extract_product_event_record;
use tracing::{error, info};

#[tracing::instrument(skip(queue_mail_service, product_event_mail_payload_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    queue_mail_service: &impl QueueMailService,
    product_event_mail_payload_service: &impl ProductEventMailPayloadService,
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
        if let Some(product_event_record) =
            extract_product_event_record(message, &mut failed_message_ids, &mut skipped_count)
        {
            match ProductEvent::try_from(product_event_record) {
                Ok(product_event) => {
                    let mail_payloads_res = product_event_mail_payload_service
                        .create_mail_payloads(product_event)
                        .await;
                    match mail_payloads_res {
                        Ok(mail_payloads) => {
                            handle_mail_payloads(queue_mail_service, mail_payloads).await
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
                        fromType = %std::any::type_name::<ProductEventRecord>(),
                        toType = %std::any::type_name::<ProductEvent>(),
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

async fn handle_mail_payloads(
    queue_mail_service: &impl QueueMailService,
    mail_payloads: Vec<MailPayload>,
) {
    if mail_payloads.is_empty() {
        return;
    }

    const MAX_RETRIES: u32 = 5;
    const BASE_DELAY_MS: u64 = 50;

    let mut mail_payloads = mail_payloads;
    let mut retry_count = 0;
    loop {
        let failed = queue_mail_service.queue_mails(mail_payloads).await;
        if failed.is_empty() {
            return;
        }
        if retry_count >= MAX_RETRIES {
            error!(
                mailCount = failed.len(),
                "Failed queuing emails after '{MAX_RETRIES}' retries."
            );
            return;
        }

        retry_count += 1;
        let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

        mail_payloads = failed;
    }
}

#[cfg(test)]
mod tests {
    use super::handler;
    use crate::service::{MockProductEventMailPayloadService, ProductEventMailPayloadServiceError};
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use mail_core::payload::MailPayload;
    use mail_core::queue_service::MockQueueMailService;
    use product::core::product_event::ProductEvent;
    use product::dynamodb::product_event_record::ProductEventRecord;
    use std::ops::SubAssign;
    use std::sync::Arc;
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
        event_record.event_name = "UPDATE".to_string();

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
        let mut product_event_mail_payload_service = MockProductEventMailPayloadService::default();
        product_event_mail_payload_service
            .expect_create_mail_payloads()
            .returning(|_| Box::pin(async move { Ok(fake::vec![MailPayload; 42]) }));
        let mut queue_mail_service = MockQueueMailService::default();
        queue_mail_service
            .expect_queue_mails()
            .returning(|_| Box::pin(async move { vec![] }));

        let actual = handler(
            &queue_mail_service,
            &product_event_mail_payload_service,
            lambda_event,
        )
        .await
        .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
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
        use std::sync::Mutex;

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

        let mut product_event_mail_payload_service = MockProductEventMailPayloadService::default();
        let remaining_failures: Arc<Mutex<usize>> = Arc::new(Mutex::new(failure_count));
        product_event_mail_payload_service
            .expect_create_mail_payloads()
            .returning(move |_| {
                let mut remaining = remaining_failures.lock().unwrap();
                if remaining.eq(&0) {
                    Box::pin(async move { Ok(fake::vec![MailPayload; 187]) })
                } else {
                    remaining.sub_assign(1);
                    Box::pin(async move {
                        Err(ProductEventMailPayloadServiceError::GetProductError(
                            product::service::get_service::GetProductError::SdkGetProductError(
                                SdkError::construction_failure("something went wrong"),
                            ),
                        ))
                    })
                }
            });
        let mut queue_mail_service = MockQueueMailService::default();
        queue_mail_service
            .expect_queue_mails()
            .returning(|_| Box::pin(async move { vec![] }));

        let actual = handler(
            &queue_mail_service,
            &product_event_mail_payload_service,
            lambda_event,
        )
        .await
        .unwrap();
        assert_eq!(failure_count, actual.batch_item_failures.len());
    }
}
