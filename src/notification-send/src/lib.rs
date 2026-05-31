use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    actor::{RequestContext, domain::Actor},
    dynamodb_stream::extract_from_dynamodb_stream,
};
use lambda_runtime::LambdaEvent;
use notification::{
    core::notification::Notification, dynamodb::notification_record::NotificationRecord,
    service::notification_service::NotificationService,
};
use tracing::{debug, error, info, warn};

#[tracing::instrument(skip(service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    service: &impl NotificationService,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let (notification_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<NotificationRecord>(event.payload.records);

    for (message_id, notification_record) in notification_records {
        let notification: Notification = match notification_record.try_into() {
            Ok(n) => n,
            Err(err) => {
                error!(messageId = message_id, error = %err, "Failed mapping NotificationRecord. Skipping.");
                continue;
            }
        };
        let user_id = notification.user_id;
        let origin_event_id = notification.origin_event_id;

        match service
            .send_externally(
                &RequestContext {
                    actor: Actor::System,
                },
                &user_id,
                &origin_event_id,
            )
            .await
        {
            Ok(_) => {
                debug!(
                    messageId = message_id,
                    userId = %user_id,
                    originEventId = %origin_event_id,
                    "Notification sent externally."
                );
            }
            Err(err) => {
                warn!(
                    error = %err,
                    messageId = message_id,
                    userId = %user_id,
                    originEventId = %origin_event_id,
                    "Failed sending notification externally."
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
    use notification::core::notification::Notification;
    use notification::dynamodb::notification_record::NotificationRecord;
    use notification::service::notification_service::{MockNotificationService, NotificationError};
    use std::time::SystemTime;
    use uuid::Uuid;

    fn mk_event_bridge_payload(notification_record: &impl serde::Serialize) -> String {
        let mut stream_record = StreamRecord::default();
        stream_record.approximate_creation_date_time = SystemTime::now().into();
        stream_record.new_image = serde_dynamo::to_item(notification_record).unwrap();
        stream_record.size_bytes = 42;

        let mut event_record = EventRecord::default();
        event_record.aws_region = "eu-central-1".to_string();
        event_record.change = stream_record;
        event_record.event_id = Uuid::new_v4().to_string();
        event_record.event_name = "INSERT".to_string();

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail_type = "DynamoDBStreamRecord".to_string();
        event.source = "test-table".to_string();
        event.detail = event_record;

        serde_json::to_string(&event).unwrap()
    }

    fn mk_sqs_message(record: &impl serde::Serialize) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Faker.fake());
        msg.body = Some(mk_event_bridge_payload(record));
        msg
    }

    fn mk_sqs_message_with_id(record: &impl serde::Serialize, message_id: String) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id);
        msg.body = Some(mk_event_bridge_payload(record));
        msg
    }

    fn mk_lambda_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = messages;
        LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        }
    }

    fn mk_notification_record() -> NotificationRecord {
        Faker.fake::<NotificationRecord>()
    }

    fn mk_notification_from_record(record: &NotificationRecord) -> Notification {
        record.clone().try_into().unwrap()
    }

    #[tokio::test]
    async fn should_return_no_failures_when_batch_is_empty() {
        let mock_service = MockNotificationService::default();
        let event = mk_lambda_event(vec![]);

        let result = handler(&mock_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_single_notification_sent_successfully() {
        let record = mk_notification_record();
        let notification = mk_notification_from_record(&record);
        let user_id = notification.user_id;
        let origin_event_id = notification.origin_event_id;

        let mut mock_service = MockNotificationService::default();
        let returned_notification: Notification =
            Faker.fake::<NotificationRecord>().try_into().unwrap();
        mock_service
            .expect_send_externally()
            .withf(move |_, uid, eid| *uid == user_id && *eid == origin_event_id)
            .times(1)
            .returning(move |_, _, _| {
                let n = returned_notification.clone();
                Box::pin(async move { Ok(n) })
            });

        let event = mk_lambda_event(vec![mk_sqs_message(&record)]);
        let result = handler(&mock_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_multiple_notifications_sent_successfully() {
        let record1 = mk_notification_record();
        let record2 = mk_notification_record();

        let mut mock_service = MockNotificationService::default();
        let returned: Notification = Faker.fake::<NotificationRecord>().try_into().unwrap();
        mock_service
            .expect_send_externally()
            .times(2)
            .returning(move |_, _, _| {
                let n = returned.clone();
                Box::pin(async move { Ok(n) })
            });

        let event = mk_lambda_event(vec![mk_sqs_message(&record1), mk_sqs_message(&record2)]);
        let result = handler(&mock_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_send_externally_fails() {
        let record = mk_notification_record();
        let notification = mk_notification_from_record(&record);
        let user_id = notification.user_id;
        let origin_event_id = notification.origin_event_id;
        let message_id = "test-message-id-1".to_string();

        let mut mock_service = MockNotificationService::default();
        mock_service
            .expect_send_externally()
            .withf(move |_, uid, eid| *uid == user_id && *eid == origin_event_id)
            .times(1)
            .returning(move |_, uid, eid| {
                let err = NotificationError::NotificationNotFound(*uid, *eid);
                Box::pin(async move { Err(err) })
            });

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(&mock_service, event).await.unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_partial_failures_when_some_succeed_and_some_fail() {
        let record_success = mk_notification_record();
        let record_fail = mk_notification_record();
        let notification_fail = mk_notification_from_record(&record_fail);
        let fail_user_id = notification_fail.user_id;
        let fail_origin_event_id = notification_fail.origin_event_id;
        let fail_message_id = "fail-message-id".to_string();
        let success_message_id = "success-message-id".to_string();

        let mut mock_service = MockNotificationService::default();
        let returned: Notification = Faker.fake::<NotificationRecord>().try_into().unwrap();

        mock_service
            .expect_send_externally()
            .withf(move |_, uid, eid| *uid == fail_user_id && *eid == fail_origin_event_id)
            .times(1)
            .returning(move |_, uid, eid| {
                let err = NotificationError::NotificationNotFound(*uid, *eid);
                Box::pin(async move { Err(err) })
            });

        mock_service
            .expect_send_externally()
            .withf(move |_, uid, eid| *uid != fail_user_id || *eid != fail_origin_event_id)
            .times(1)
            .returning(move |_, _, _| {
                let n = returned.clone();
                Box::pin(async move { Ok(n) })
            });

        let event = mk_lambda_event(vec![
            mk_sqs_message_with_id(&record_success, success_message_id),
            mk_sqs_message_with_id(&record_fail, fail_message_id.clone()),
        ]);
        let result = handler(&mock_service, event).await.unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(
            fail_message_id,
            result.batch_item_failures[0].item_identifier
        );
    }

    #[tokio::test]
    async fn should_skip_messages_with_empty_body() {
        let mock_service = MockNotificationService::default();

        let mut empty_msg = SqsMessage::default();
        empty_msg.message_id = Some("empty-body-msg".to_string());
        empty_msg.body = None;

        let event = mk_lambda_event(vec![empty_msg]);
        let result = handler(&mock_service, event).await.unwrap();

        // Empty body is skipped, not failed
        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_messages_with_invalid_json_body() {
        let mock_service = MockNotificationService::default();

        let mut invalid_msg = SqsMessage::default();
        invalid_msg.message_id = Some("invalid-json-msg".to_string());
        invalid_msg.body = Some("invalid json {".to_string());

        let event = mk_lambda_event(vec![invalid_msg]);
        let result = handler(&mock_service, event).await.unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(
            "invalid-json-msg",
            result.batch_item_failures[0].item_identifier
        );
    }

    #[tokio::test]
    async fn should_return_all_failures_when_all_notifications_fail() {
        let record1 = mk_notification_record();
        let record2 = mk_notification_record();
        let msg_id_1 = "msg-1".to_string();
        let msg_id_2 = "msg-2".to_string();

        let mut mock_service = MockNotificationService::default();
        mock_service
            .expect_send_externally()
            .times(2)
            .returning(|_, uid, eid| {
                let err = NotificationError::NotificationNotFound(*uid, *eid);
                Box::pin(async move { Err(err) })
            });

        let event = mk_lambda_event(vec![
            mk_sqs_message_with_id(&record1, msg_id_1.clone()),
            mk_sqs_message_with_id(&record2, msg_id_2.clone()),
        ]);
        let result = handler(&mock_service, event).await.unwrap();

        assert_eq!(2, result.batch_item_failures.len());
        let failed_ids: Vec<&str> = result
            .batch_item_failures
            .iter()
            .map(|f| f.item_identifier.as_str())
            .collect();
        assert!(failed_ids.contains(&msg_id_1.as_str()));
        assert!(failed_ids.contains(&msg_id_2.as_str()));
    }

    #[tokio::test]
    async fn should_handle_mixed_valid_invalid_and_empty_messages() {
        let record = mk_notification_record();
        let valid_msg_id = "valid-msg".to_string();

        let mut mock_service = MockNotificationService::default();
        let returned: Notification = Faker.fake::<NotificationRecord>().try_into().unwrap();
        mock_service
            .expect_send_externally()
            .times(1)
            .returning(move |_, _, _| {
                let n = returned.clone();
                Box::pin(async move { Ok(n) })
            });

        let mut empty_msg = SqsMessage::default();
        empty_msg.message_id = Some("empty-msg".to_string());
        empty_msg.body = None;

        let mut invalid_msg = SqsMessage::default();
        invalid_msg.message_id = Some("invalid-msg".to_string());
        invalid_msg.body = Some("not json".to_string());

        let event = mk_lambda_event(vec![
            mk_sqs_message_with_id(&record, valid_msg_id),
            empty_msg,
            invalid_msg,
        ]);
        let result = handler(&mock_service, event).await.unwrap();

        // Only the invalid JSON should be a failure; empty is skipped, valid succeeds
        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!("invalid-msg", result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[case(100)]
    #[trace]
    async fn should_return_no_failures_when_large_batch_all_succeed(#[case] batch_size: usize) {
        let records: Vec<NotificationRecord> =
            (0..batch_size).map(|_| mk_notification_record()).collect();

        let mut mock_service = MockNotificationService::default();
        let returned: Notification = Faker.fake::<NotificationRecord>().try_into().unwrap();
        mock_service
            .expect_send_externally()
            .times(batch_size)
            .returning(move |_, _, _| {
                let n = returned.clone();
                Box::pin(async move { Ok(n) })
            });

        let messages: Vec<SqsMessage> = records.iter().map(mk_sqs_message).collect();
        let event = mk_lambda_event(messages);
        let result = handler(&mock_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_notification_already_sent_externally() {
        // Tests the idempotency path: send_externally returns Ok even if already sent
        let record = mk_notification_record();

        let mut mock_service = MockNotificationService::default();
        // The service returns Ok for already-sent notifications (idempotent)
        let returned: Notification = Faker.fake::<NotificationRecord>().try_into().unwrap();
        mock_service
            .expect_send_externally()
            .times(1)
            .returning(move |_, _, _| {
                let n = returned.clone();
                Box::pin(async move { Ok(n) })
            });

        let event = mk_lambda_event(vec![mk_sqs_message(&record)]);
        let result = handler(&mock_service, event).await.unwrap();

        assert!(result.batch_item_failures.is_empty());
    }
}
