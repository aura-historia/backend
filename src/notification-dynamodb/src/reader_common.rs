use crate::notification_record::NotificationRecord;
use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;
use tracing::error;
use user_core::user_id::UserId;

pub(crate) const SK_LOWER_BOUND: &str = "user#notification#origin_event_id#";
pub(crate) const SK_UPPER_BOUND: &str = "user#notification#origin_event_id#\u{ffff}";

pub(crate) fn deserialize_records(
    user_id: &UserId,
    records: Vec<HashMap<String, AttributeValue>>,
) -> Vec<NotificationRecord> {
    records
        .into_iter()
        .map(serde_dynamo::from_item::<_, NotificationRecord>)
        .filter_map(|res| match res {
            Ok(record) => Some(record),
            Err(err) => {
                error!(
                    userId = %user_id,
                    error = %err,
                    r#type = %std::any::type_name::<NotificationRecord>(),
                    "Failed deserializing."
                );
                None
            }
        })
        .collect()
}
