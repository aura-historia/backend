use crate::service::command::UpdateNotificationCommand;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchNotificationData {
    #[serde(default)]
    pub seen: Option<bool>,
}

impl From<PatchNotificationData> for UpdateNotificationCommand {
    fn from(data: PatchNotificationData) -> Self {
        Self { seen: data.seen }
    }
}
