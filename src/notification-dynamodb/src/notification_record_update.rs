use crate::dynamodb_update::DynamoDbUpdate;
use crate::notification_type_record::NotificationTypeRecord;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct NotificationRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<NotificationTypeRecord>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for NotificationRecordUpdate {}
