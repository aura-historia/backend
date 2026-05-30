use crate::service::command::UpdateWatchlistProductCommand;
use common::actor::domain::Actor;
use common::{
    actor::record::ActorRecord, dynamodb_update::DynamoDbUpdate,
    resource_state::record::ResourceStateRecord,
};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct WatchlistProductRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ResourceStateRecord>,

    pub updated_by: ActorRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for WatchlistProductRecordUpdate {}

impl WatchlistProductRecordUpdate {
    pub fn from_cmd(cmd: UpdateWatchlistProductCommand, actor: Actor) -> WatchlistProductRecordUpdate {
        WatchlistProductRecordUpdate {
            notifications: cmd.notifications,
            state: cmd.state.map(ResourceStateRecord::from),
            updated_by: actor.into(),
            updated: OffsetDateTime::now_utc(),
        }
    }
}
