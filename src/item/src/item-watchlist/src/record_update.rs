use crate::{
    command::UpdateWatchlistItemCommand,
    record::{mk_gsi1_pk, mk_gsi1_sk},
};
use common::{item_id::ItemId, user_id::UserId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchlistItemRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_pk: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_sk: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl WatchlistItemRecordUpdate {
    pub fn from_cmd(
        cmd: UpdateWatchlistItemCommand,
        user_id: &UserId,
        item_id: &ItemId,
    ) -> WatchlistItemRecordUpdate {
        WatchlistItemRecordUpdate {
            gsi1_pk: if let Some(true) = cmd.notifications {
                Some(mk_gsi1_pk(item_id))
            } else {
                None
            },
            gsi1_sk: if let Some(true) = cmd.notifications {
                Some(mk_gsi1_sk(user_id))
            } else {
                None
            },
            notifications: cmd.notifications,
            updated: OffsetDateTime::now_utc(),
        }
    }
}
