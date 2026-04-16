use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::dynamodb_stream::extract_from_dynamodb_stream;
use lambda_runtime::LambdaEvent;
use tracing::{error, info};
use user::{
    core::user::User, dynamodb::user_record::UserRecord,
    service::zoho_campaigns_service::ZohoCampaignsService,
};

#[tracing::instrument(skip(zoho_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    zoho_service: &(impl ZohoCampaignsService + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let (user_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<UserRecord>(event.payload.records);

    for (message_id, user_record) in user_records {
        let user: User = user_record.into();
        let user_id = user.user_id;

        match zoho_service.subscribe(&user).await {
            Ok(()) => {
                info!(userId = %user_id, "Synced user to Zoho Campaigns.");
            }
            Err(err) => {
                error!(
                    userId = %user_id,
                    error = %err,
                    "Failed syncing user to Zoho Campaigns."
                );
                failed_message_ids.push(message_id);
            }
        }
    }

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

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use user::dynamodb::user_record::UserRecord;
    use user::service::zoho_campaigns_service::{MockZohoCampaignsService, ZohoCampaignsError};

    fn mk_event_bridge_body(record: &UserRecord) -> String {
        let new_image = serde_dynamo::to_item(record.clone()).unwrap();

        let mut stream_record = StreamRecord::default();
        stream_record.new_image = new_image;

        let mut event_record = EventRecord::default();
        event_record.event_name = "MODIFY".to_string();
        event_record.change = stream_record;

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail_type = "DynamoDBStreamRecord".to_string();
        event.source = "table_1".to_string();
        event.detail = event_record;

        serde_json::to_string(&event).unwrap()
    }

    fn mk_sqs_message(body: &str) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(uuid::Uuid::new_v4().to_string());
        msg.body = Some(body.to_string());
        msg
    }

    fn mk_sqs_message_with_id(body: &str, message_id: String) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id);
        msg.body = Some(body.to_string());
        msg
    }

    fn mk_sqs_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = messages;
        LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        }
    }

    #[tokio::test]
    async fn should_return_no_failures_when_valid_modify_event() {
        let user_record: UserRecord = Faker.fake();
        let body = mk_event_bridge_body(&user_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .once()
            .returning(|_| Box::pin(async { Ok(()) }));

        let result = handler(&zoho_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_zoho_subscribe_fails() {
        let user_record: UserRecord = Faker.fake();
        let body = mk_event_bridge_body(&user_record);
        let message_id = "test-message-id".to_string();
        let event = mk_sqs_event(vec![mk_sqs_message_with_id(&body, message_id.clone())]);

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service.expect_subscribe().once().returning(|_| {
            Box::pin(async {
                Err(ZohoCampaignsError::ApiRequestError(
                    "Network error".to_string(),
                ))
            })
        });

        let result = handler(&zoho_service, event).await.unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_no_failures_when_event_has_no_records() {
        let event = mk_sqs_event(vec![]);

        let zoho_service = MockZohoCampaignsService::default();

        let result = handler(&zoho_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_message_with_empty_body() {
        let mut message = SqsMessage::default();
        message.message_id = Some("test-id".to_string());
        message.body = None;
        let event = mk_sqs_event(vec![message]);

        let zoho_service = MockZohoCampaignsService::default();

        let result = handler(&zoho_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_message_deserialization_fails() {
        let message_id = "bad-message-id".to_string();
        let message = mk_sqs_message_with_id("{\"invalid\": \"json\"}", message_id.clone());
        let event = mk_sqs_event(vec![message]);

        let zoho_service = MockZohoCampaignsService::default();

        let result = handler(&zoho_service, event).await.unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_no_failures_when_batch_succeeds() {
        let user_record_1: UserRecord = Faker.fake();
        let user_record_2: UserRecord = Faker.fake();
        let body_1 = mk_event_bridge_body(&user_record_1);
        let body_2 = mk_event_bridge_body(&user_record_2);
        let event = mk_sqs_event(vec![mk_sqs_message(&body_1), mk_sqs_message(&body_2)]);

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .times(2)
            .returning(|_| Box::pin(async { Ok(()) }));

        let result = handler(&zoho_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_partial_failure_when_one_zoho_call_fails_in_batch() {
        let user_record_1: UserRecord = Faker.fake();
        let user_record_2: UserRecord = Faker.fake();
        let body_1 = mk_event_bridge_body(&user_record_1);
        let body_2 = mk_event_bridge_body(&user_record_2);
        let msg_id_1 = "msg-1".to_string();
        let msg_id_2 = "msg-2".to_string();
        let event = mk_sqs_event(vec![
            mk_sqs_message_with_id(&body_1, msg_id_1.clone()),
            mk_sqs_message_with_id(&body_2, msg_id_2.clone()),
        ]);

        let mut zoho_service = MockZohoCampaignsService::default();
        let mut call_count = 0u32;
        zoho_service
            .expect_subscribe()
            .times(2)
            .returning(move |_| {
                call_count += 1;
                if call_count == 1 {
                    Box::pin(async { Ok(()) })
                } else {
                    Box::pin(async {
                        Err(ZohoCampaignsError::ApiResponseError {
                            status: "error".to_string(),
                            message: "Invalid list key.".to_string(),
                        })
                    })
                }
            });

        let result = handler(&zoho_service, event).await.unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        let failed_id = &result.batch_item_failures[0].item_identifier;
        assert!(
            *failed_id == msg_id_1 || *failed_id == msg_id_2,
            "Expected one of the message IDs to be reported as failed, got: {failed_id}"
        );
    }
}
