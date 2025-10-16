use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MailTemplate {
    StateAvailableNotification,
    PriceDiscoverdNotification,
    PriceIncreasedNotification,
    PriceDroppedNotification,
}

impl MailTemplate {
    pub fn as_str(&self) -> &'static str {
        match self {
            MailTemplate::StateAvailableNotification => "TODO",
            MailTemplate::PriceDiscoverdNotification => "TODO",
            MailTemplate::PriceIncreasedNotification => "TODO",
            MailTemplate::PriceDroppedNotification => "TODO",
        }
    }
}
