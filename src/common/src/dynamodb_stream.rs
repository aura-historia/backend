use std::collections::HashMap;

use aws_lambda_events::eventbridge::EventBridgeEvent;
use serde::de::DeserializeOwned;
use tracing::{error, info, warn};

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
pub fn extract_sqs_event_bridge_dynamodb_record<T: DeserializeOwned>(
    message: impl Into<SqsMessage>,
    failed_message_ids: &mut Vec<String>,
    skipped_count: &mut usize,
) -> Option<T> {
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
                Ok(event_bridge_event) => match serde_dynamo::from_item::<_, T>(
                    event_bridge_event.detail.change.new_image,
                ) {
                    Ok(example_record) => Some(example_record),
                    Err(e) => {
                        error!(
                            error = %e,
                            type = %std::any::type_name::<T>(),
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

// TODO: Delete above fun and migrate all usages to below variant
pub type MessageId = String;
pub fn extract_from_dynamodb_stream<T: DeserializeOwned>(
    messages: Vec<impl Into<SqsMessage>>,
) -> (HashMap<MessageId, T>, Vec<MessageId>) {
    let count = messages.len();
    messages.into_iter().fold(
        (HashMap::with_capacity(count), vec![]),
        |(mut event_records, mut failed_message_ids), msg| {
            match extract_event_bridge_sqs_dynamodb_record::<T>(msg) {
                Ok((message_id, Some(event_record))) => {
                    event_records.insert(message_id, event_record);
                    (event_records, failed_message_ids)
                }
                Ok((_, None)) => (event_records, failed_message_ids),
                Err(message_id) => {
                    failed_message_ids.push(message_id);
                    (event_records, failed_message_ids)
                }
            }
        },
    )
}

pub fn extract_event_bridge_sqs_dynamodb_record<T: DeserializeOwned>(
    message: impl Into<SqsMessage>,
) -> Result<(MessageId, Option<T>), MessageId> {
    let message: SqsMessage = message.into();
    let message_id = message.message_id;

    match message.body {
        None => {
            warn!("Received empty body. Skipping message.");
            Ok((message_id, None))
        }
        Some(event_bridge_event_json) => {
            match serde_json::from_str::<EventBridgeEvent<aws_lambda_events::dynamodb::EventRecord>>(
                &event_bridge_event_json,
            ) {
                Ok(event_bridge_event) => match serde_dynamo::from_item::<_, T>(
                    event_bridge_event.detail.change.new_image,
                ) {
                    Ok(record) => Ok((message_id, Some(record))),
                    Err(e) => {
                        error!(
                            error = %e,
                            type = %std::any::type_name::<T>(),
                            payload = %event_bridge_event_json,
                            "Failed deserializing 'detail.new_image'."
                        );
                        Err(message_id)
                    }
                },
                Err(e) => {
                    error!(
                        error = %e,
                        type = %std::any::type_name::<EventBridgeEvent<aws_lambda_events::dynamodb::EventRecord>>(),
                        payload = %event_bridge_event_json,
                        "Failed deserializing."
                    );
                    Err(message_id)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::extract_sqs_event_bridge_dynamodb_record;
    use aws_lambda_events::{
        dynamodb::{EventRecord, StreamRecord},
        eventbridge::EventBridgeEvent,
        sqs::SqsMessage,
    };
    use fake::{Fake, Faker};
    use serde::{Deserialize, Serialize};
    use std::time::SystemTime;
    use uuid::Uuid;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, fake::Dummy)]
    struct ExampleRecord {
        pub pk: String,
        pub sk: String,
        pub foo: u32,
    }

    fn mk_sqs_message(message_id: &str, body: Option<String>) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id.to_string());
        msg.body = body;
        msg
    }

    fn mk_event_bridge_event(example_record: &ExampleRecord) -> EventBridgeEvent<EventRecord> {
        let mut stream_record = StreamRecord::default();
        stream_record.approximate_creation_date_time = SystemTime::now().into();
        stream_record.new_image = serde_dynamo::to_item(example_record).unwrap();
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
        let actual = extract_sqs_event_bridge_dynamodb_record::<ExampleRecord>(
            msg,
            &mut failed_message_ids,
            &mut skipped_count,
        );

        assert!(actual.is_none());
        assert_eq!(vec!["msg1".to_string()], failed_message_ids);
        assert_eq!(0, skipped_count);
    }

    #[test]
    fn should_skip_message_for_empty_message_body() {
        let msg = mk_sqs_message("msg2", None);

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_sqs_event_bridge_dynamodb_record::<ExampleRecord>(
            msg,
            &mut failed_message_ids,
            &mut skipped_count,
        );

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
        let actual = extract_sqs_event_bridge_dynamodb_record::<ExampleRecord>(
            msg,
            &mut failed_message_ids,
            &mut skipped_count,
        );

        assert!(actual.is_none());
        assert_eq!(vec!["test_msg".to_string()], failed_message_ids);
        assert_eq!(0, skipped_count);
    }

    #[test]
    fn should_succeed_extract_message_data_with_valid_data() {
        let expected = Faker.fake::<ExampleRecord>();
        let event = mk_event_bridge_event(&expected);
        let msg = mk_sqs_message("test_msg", Some(serde_json::to_string(&event).unwrap()));

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_sqs_event_bridge_dynamodb_record::<ExampleRecord>(
            msg,
            &mut failed_message_ids,
            &mut skipped_count,
        );

        assert!(actual.is_some());
        assert_eq!(expected, actual.unwrap());
        assert!(failed_message_ids.is_empty());
        assert_eq!(0, skipped_count);
    }
}
