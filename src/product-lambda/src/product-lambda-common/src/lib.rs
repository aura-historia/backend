use aws_lambda_events::eventbridge::EventBridgeEvent;
use product::dynamodb::product_event_record::ProductEventRecord;
use tracing::{error, info};

#[derive(Debug, Clone)]
pub struct SqsMessage {
    pub message_id: String,
    pub body: Option<String>,
}

impl From<aws_lambda_events::sqs::SqsMessage> for SqsMessage {
    fn from(value: aws_lambda_events::sqs::SqsMessage) -> Self {
        SqsMessage {
            message_id: value.message_id.expect(
                "shouldn't receive an SQS-Message without 'message_id' because AWS sets it.",
            ),
            body: value.body,
        }
    }
}

impl From<aws_sdk_sqs::types::Message> for SqsMessage {
    fn from(value: aws_sdk_sqs::types::Message) -> Self {
        SqsMessage {
            message_id: value.message_id.expect(
                "shouldn't receive an SQS-Message without 'message_id' because AWS sets it.",
            ),
            body: value.body,
        }
    }
}

#[tracing::instrument(skip(message, failed_message_ids, skipped_count))]
pub fn extract_product_event_record(
    message: impl Into<SqsMessage>,
    failed_message_ids: &mut Vec<String>,
    skipped_count: &mut usize,
) -> Option<ProductEventRecord> {
    let message: SqsMessage = message.into();
    let message_id = message.message_id;

    match message.body {
        None => {
            info!("Received empty body. Skipping message.");
            *skipped_count += 1;
            None
        }
        Some(event_bridge_event_json) => {
            match serde_json::from_str::<EventBridgeEvent<aws_lambda_events::dynamodb::EventRecord>>(
                &event_bridge_event_json,
            ) {
                Ok(event_bridge_event) => match serde_dynamo::from_item::<_, ProductEventRecord>(
                    event_bridge_event.detail.change.new_image,
                ) {
                    Ok(product_event_record) => Some(product_event_record),
                    Err(e) => {
                        error!(
                            error = %e,
                            type = %std::any::type_name::<ProductEventRecord>(),
                            payload = %event_bridge_event_json,
                            "Failed deserializing 'detail.new_image'."
                        );
                        failed_message_ids.push(message_id);
                        None
                    }
                },
                Err(e) => {
                    error!(
                        error = %e,
                        type = %std::any::type_name::<EventBridgeEvent<aws_lambda_events::dynamodb::EventRecord>>(),
                        payload = %event_bridge_event_json,
                        "Failed deserializing."
                    );
                    failed_message_ids.push(message_id);
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::extract_product_event_record;
    use aws_lambda_events::{
        dynamodb::{EventRecord, StreamRecord},
        eventbridge::EventBridgeEvent,
        sqs::SqsMessage,
    };
    use fake::{Fake, Faker};
    use product::dynamodb::product_event_record::ProductEventRecord;
    use std::time::SystemTime;
    use uuid::Uuid;

    fn mk_sqs_message(message_id: &str, body: Option<String>) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id.to_string());
        msg.body = body;
        msg
    }

    fn mk_event_bridge_event(product_event_record: &ProductEventRecord) -> EventBridgeEvent<EventRecord> {
        let mut stream_record = StreamRecord::default();
        stream_record.approximate_creation_date_time = SystemTime::now().into();
        stream_record.new_image = serde_dynamo::to_item(product_event_record).unwrap();
        stream_record.size_bytes = 42;

        let mut event_record = EventRecord::default();
        event_record.aws_region = "eu-central-1".to_string();
        event_record.change = stream_record;
        event_record.event_id = Uuid::new_v4().to_string();
        event_record.event_name = "INSERT".to_string();

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail_type = "foo".to_string();
        event.source = "bar".to_string();
        event.detail = event_record;
        event
    }

    #[test]
    fn should_fail_when_invalid_json() {
        let msg = mk_sqs_message("msg1", Some("invalid json {".to_string()));

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_product_event_record(msg, &mut failed_message_ids, &mut skipped_count);

        assert!(actual.is_none());
        assert_eq!(vec!["msg1".to_string()], failed_message_ids);
        assert_eq!(0, skipped_count);
    }

    #[test]
    fn should_skip_message_for_empty_message_body() {
        let msg = mk_sqs_message("msg2", None);

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_product_event_record(msg, &mut failed_message_ids, &mut skipped_count);

        assert!(actual.is_none());
        assert!(failed_message_ids.is_empty());
        assert_eq!(1, skipped_count);
    }

    #[test]
    fn should_fail_when_valid_json_cannot_be_deserialized_to_target_type() {
        let invalid_conversion_json = r#"{"eventType":"Created","shopId":"test","shopsProductId":"test","timestamp":"2023-01-01T00:00:00Z","boop":{"item":null}}"#;
        let msg = mk_sqs_message("test_msg", Some(invalid_conversion_json.to_string()));

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_product_event_record(msg, &mut failed_message_ids, &mut skipped_count);

        assert!(actual.is_none());
        assert_eq!(vec!["test_msg".to_string()], failed_message_ids);
        assert_eq!(0, skipped_count);
    }

    #[test]
    fn should_succeed_extract_message_data_with_valid_data() {
        let expected = Faker.fake::<ProductEventRecord>();
        let event = mk_event_bridge_event(&expected);
        let msg = mk_sqs_message("test_msg", Some(serde_json::to_string(&event).unwrap()));

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_product_event_record(msg, &mut failed_message_ids, &mut skipped_count);

        assert!(actual.is_some());
        assert_eq!(expected, actual.unwrap());
        assert!(failed_message_ids.is_empty());
        assert_eq!(0, skipped_count);
    }
}
