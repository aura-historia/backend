use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::command::UpdateWatchlistItemCommand;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchlistItemRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<UpdateWatchlistItemCommand> for WatchlistItemRecordUpdate {
    fn from(cmd: UpdateWatchlistItemCommand) -> Self {
        WatchlistItemRecordUpdate {
            notifications: cmd.notifications,
            updated: OffsetDateTime::now_utc(),
        }
    }
}
