use aws_lambda_events::sqs::SqsEvent;
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
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
) -> Result<(), lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let mut skipped_count = 0;
    let mut failed_message_ids = Vec::new();

    for msg in event.payload.records {
        let user_record = extract_sqs_event_bridge_dynamodb_record::<UserRecord>(
            msg,
            &mut failed_message_ids,
            &mut skipped_count,
        );

        match user_record {
            None => {
                if skipped_count > 0 {
                    info!("Skipped message (empty body or unrecognized format).");
                }
            }
            Some(record) => {
                let user: User = record.into();
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
                        return Err(format!(
                            "Failed syncing user {user_id} to Zoho Campaigns: {err}"
                        )
                        .into());
                    }
                }
            }
        }
    }

    let failures = failed_message_ids.len();
    info!(
        successful = count - failures - skipped_count,
        failures = failures,
        skipped = skipped_count,
        "Handler finished.",
    );

    if !failed_message_ids.is_empty() {
        return Err(format!("Failed processing {} messages.", failures).into());
    }

    Ok(())
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

    fn mk_sqs_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = messages;
        LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        }
    }

    #[tokio::test]
    async fn should_sync_user_when_valid_modify_event() {
        let user_record: UserRecord = Faker.fake();
        let body = mk_event_bridge_body(&user_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .once()
            .returning(|_| Box::pin(async { Ok(()) }));

        let result = handler(&zoho_service, event).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_return_error_when_zoho_subscribe_fails() {
        let user_record: UserRecord = Faker.fake();
        let body = mk_event_bridge_body(&user_record);
        let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service.expect_subscribe().once().returning(|_| {
            Box::pin(async {
                Err(ZohoCampaignsError::ApiRequestError(
                    "Network error".to_string(),
                ))
            })
        });

        let result = handler(&zoho_service, event).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn should_succeed_when_event_has_no_records() {
        let event = mk_sqs_event(vec![]);

        let zoho_service = MockZohoCampaignsService::default();

        let result = handler(&zoho_service, event).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_skip_message_with_empty_body() {
        let mut message = SqsMessage::default();
        message.message_id = Some("test-id".to_string());
        message.body = None;
        let event = mk_sqs_event(vec![message]);

        let zoho_service = MockZohoCampaignsService::default();

        let result = handler(&zoho_service, event).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_return_error_when_message_deserialization_fails() {
        let message = mk_sqs_message("{\"invalid\": \"json\"}");
        let event = mk_sqs_event(vec![message]);

        let zoho_service = MockZohoCampaignsService::default();

        let result = handler(&zoho_service, event).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn should_sync_multiple_users_when_batch_event() {
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

        let result = handler(&zoho_service, event).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_stop_on_first_zoho_failure_in_batch() {
        let user_record_1: UserRecord = Faker.fake();
        let user_record_2: UserRecord = Faker.fake();
        let body_1 = mk_event_bridge_body(&user_record_1);
        let body_2 = mk_event_bridge_body(&user_record_2);
        let event = mk_sqs_event(vec![mk_sqs_message(&body_1), mk_sqs_message(&body_2)]);

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service.expect_subscribe().once().returning(|_| {
            Box::pin(async {
                Err(ZohoCampaignsError::ApiResponseError {
                    status: "error".to_string(),
                    message: "Invalid list key.".to_string(),
                })
            })
        });

        let result = handler(&zoho_service, event).await;

        assert!(result.is_err());
    }
}
