//! Kinesis envelope decoding shared by live invocations and failure-archive replay.

use aws_lambda_events::kinesis::KinesisEvent;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(try_from = "Value")]
pub struct KinesisInvocation(KinesisEvent);

impl KinesisInvocation {
    pub fn into_event(self) -> KinesisEvent {
        self.0
    }
}

impl TryFrom<Value> for KinesisInvocation {
    type Error = serde_json::Error;

    fn try_from(mut value: Value) -> Result<Self, Self::Error> {
        if let Some(records) = value.get_mut("Records").and_then(Value::as_array_mut) {
            for record in records {
                if let Some(timestamp @ Value::Number(_)) =
                    record.pointer_mut("/kinesis/approximateArrivalTimestamp")
                {
                    // The AWS timestamp decoder's untagged float rejects JSON numbers when
                    // another release binary enables serde_json/arbitrary_precision. Its
                    // supported string representation follows the same timestamp validation.
                    *timestamp = Value::String(timestamp.to_string());
                }
            }
        }
        serde_json::from_value(value).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(timestamp: &str) -> String {
        format!(
            r#"{{"Records":[{{"awsRegion":"eu-central-1","eventID":"shardId-000000000000:100","eventName":"aws:kinesis:record","eventSource":"aws:kinesis","eventSourceARN":"arn:aws:kinesis:eu-central-1:123456789012:stream/test","eventVersion":"1.0","kinesis":{{"approximateArrivalTimestamp":{timestamp},"data":"eyJwcm9iZSI6dHJ1ZX0=","partitionKey":"test","sequenceNumber":"100","kinesisSchemaVersion":"1.0"}}}}]}}"#
        )
    }

    #[test]
    fn accepts_fractional_aws_timestamps_with_arbitrary_precision() {
        let raw = invocation("1793011239.981");
        let event = serde_json::from_str::<KinesisInvocation>(&raw)
            .unwrap()
            .into_event();
        let record = &event.records[0];
        assert_eq!(
            1793011239981,
            record
                .kinesis
                .approximate_arrival_timestamp
                .timestamp_millis()
        );
        assert_eq!("100", record.kinesis.sequence_number);
        assert_eq!("test", record.kinesis.partition_key);
        assert_eq!(br#"{"probe":true}"#, record.kinesis.data.as_slice());
        assert_eq!(Some("aws:kinesis"), record.event_source.as_deref());
    }

    #[test]
    fn retains_integer_and_string_timestamp_support() {
        for timestamp in ["1793011239", r#""1793011239.981""#] {
            let event = serde_json::from_str::<KinesisInvocation>(&invocation(timestamp))
                .unwrap()
                .into_event();
            assert_eq!(
                1793011239,
                event.records[0]
                    .kinesis
                    .approximate_arrival_timestamp
                    .timestamp()
            );
        }
    }

    #[test]
    fn rejects_malformed_timestamps_and_record_data() {
        for timestamp in ["null", "true", "{}", "[]", r#""invalid""#, "-1", "1e100"] {
            assert!(serde_json::from_str::<KinesisInvocation>(&invocation(timestamp)).is_err());
        }
        let invalid_base64 =
            invocation("1793011239.981").replace("eyJwcm9iZSI6dHJ1ZX0=", "!invalid!");
        assert!(serde_json::from_str::<KinesisInvocation>(&invalid_base64).is_err());
        assert!(serde_json::from_str::<KinesisInvocation>(r#"{"Records":null}"#).is_err());
    }
}
