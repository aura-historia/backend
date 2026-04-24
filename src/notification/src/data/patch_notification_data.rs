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

#[cfg(test)]
mod tests {
    use super::PatchNotificationData;
    use serde_json::json;

    #[test]
    fn should_roundtrip_patch_notification_data_when_using_camel_case_fields() {
        let json = json!({
            "seen": true,
        });

        let data: PatchNotificationData = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }
}
