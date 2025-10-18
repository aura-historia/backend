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

impl MailTemplateType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MailTemplateType::CreatedNotification => "created-notification",
            MailTemplateType::StateListedNotification => "state-listed-notification",
            MailTemplateType::StateAvailableNotification => "state-available-notification",
            MailTemplateType::StateReservedNotification => "state-reserved-notification",
            MailTemplateType::StateSoldNotification => "state-sold-notification",
            MailTemplateType::StateRemovedNotification => "state-removed-notification",
            MailTemplateType::StateUnknownNotification => "state-unknown-notification",
            MailTemplateType::PriceDiscoveredNotification => "price-discovered-notification",
            MailTemplateType::PriceDroppedNotification => "price-dropped-notification",
            MailTemplateType::PriceIncreasedNotification => "price-increased-notification",
            MailTemplateType::PriceRemovedNotification => "price-removed-notification",
        }
    }
}

impl MailTemplate {
    pub fn as_str(&self) -> String {
        format!("{}_{}", self.template_type.as_str(), self.language.as_str())
    }
}
