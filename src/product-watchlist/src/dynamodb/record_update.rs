use crate::dynamodb::watchlist_product_state_record::WatchlistProductStateRecord;
use crate::service::command::UpdateWatchlistProductCommand;
use common::dynamodb_update::DynamoDbUpdate;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct WatchlistProductRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<WatchlistProductStateRecord>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for WatchlistProductRecordUpdate {}

impl WatchlistProductRecordUpdate {
    pub fn from_cmd(cmd: UpdateWatchlistProductCommand) -> WatchlistProductRecordUpdate {
        WatchlistProductRecordUpdate {
            notifications: cmd.notifications,
            state: cmd.state.map(WatchlistProductStateRecord::from),
            updated: OffsetDateTime::now_utc(),
        }
    }
}
