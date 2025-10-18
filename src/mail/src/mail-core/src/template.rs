use common::language::data::LanguageData;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MailTemplateType {
    CreatedNotification,
    StateListedNotification,
    StateAvailableNotification,
    StateReservedNotification,
    StateSoldNotification,
    StateRemovedNotification,
    StateUnknownNotification,
    PriceDiscoveredNotification,
    PriceDroppedNotification,
    PriceIncreasedNotification,
    PriceRemovedNotification,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MailTemplate {
    pub template_type: MailTemplateType,
    pub language: LanguageData,
}

impl MailTemplate {
    pub fn as_str(&self) -> &'static str {
        todo!()
    }
}
