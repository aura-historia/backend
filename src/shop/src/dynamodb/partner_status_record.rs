use crate::core::partner_status::ShopPartnerStatus;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopPartnerStatusRecord {
    Scraped,
    Partnered,
}

impl From<ShopPartnerStatus> for ShopPartnerStatusRecord {
    fn from(value: ShopPartnerStatus) -> Self {
        match value {
            ShopPartnerStatus::Scraped => Self::Scraped,
            ShopPartnerStatus::Partnered => Self::Partnered,
        }
    }
}

impl From<ShopPartnerStatusRecord> for ShopPartnerStatus {
    fn from(value: ShopPartnerStatusRecord) -> Self {
        match value {
            ShopPartnerStatusRecord::Scraped => Self::Scraped,
            ShopPartnerStatusRecord::Partnered => Self::Partnered,
        }
    }
}
