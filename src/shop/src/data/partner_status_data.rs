use crate::core::partner_status::ShopPartnerStatus;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopPartnerStatusData {
    Scraped,
    Partnered,
}
impl From<ShopPartnerStatus> for ShopPartnerStatusData {
    fn from(status: ShopPartnerStatus) -> Self {
        match status {
            ShopPartnerStatus::Scraped => ShopPartnerStatusData::Scraped,
            ShopPartnerStatus::Partnered => ShopPartnerStatusData::Partnered,
        }
    }
}

impl From<ShopPartnerStatusData> for ShopPartnerStatus {
    fn from(document: ShopPartnerStatusData) -> Self {
        match document {
            ShopPartnerStatusData::Scraped => ShopPartnerStatus::Scraped,
            ShopPartnerStatusData::Partnered => ShopPartnerStatus::Partnered,
        }
    }
}
