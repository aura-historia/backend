use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchlistItemRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}
