use crate::core::command::UpdateUserSearchFilterMatchCommand;
use common::{
    actor::{domain::Actor, record::ActorRecord},
    dynamodb_update::DynamoDbUpdate,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserSearchFilterMatchRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<bool>,

    pub updated_by: ActorRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for UserSearchFilterMatchRecordUpdate {}

impl From<(UpdateUserSearchFilterMatchCommand, Actor)> for UserSearchFilterMatchRecordUpdate {
    fn from((command, actor): (UpdateUserSearchFilterMatchCommand, Actor)) -> Self {
        UserSearchFilterMatchRecordUpdate {
            feedback: command.feedback,
            updated_by: actor.into(),
            updated: OffsetDateTime::now_utc(),
        }
    }
}
