use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchlistItemRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
}
