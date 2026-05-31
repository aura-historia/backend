use crate::dynamodb::notification_type_record::NotificationTypeRecord;
use common::{actor::record::ActorRecord, dynamodb_update::DynamoDbUpdate};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct NotificationRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<NotificationTypeRecord>,

    pub updated_by: ActorRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for NotificationRecordUpdate {}
