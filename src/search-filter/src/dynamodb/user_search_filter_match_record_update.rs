use crate::service::command::UpdateUserSearchFilterMatchCommand;
use common::dynamodb_update::DynamoDbUpdate;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserSearchFilterMatchRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches_feedback: Option<bool>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for UserSearchFilterMatchRecordUpdate {}

impl From<UpdateUserSearchFilterMatchCommand> for UserSearchFilterMatchRecordUpdate {
    fn from(command: UpdateUserSearchFilterMatchCommand) -> Self {
        UserSearchFilterMatchRecordUpdate {
            matches_feedback: command.matches_feedback,
            updated: OffsetDateTime::now_utc(),
        }
    }
}
