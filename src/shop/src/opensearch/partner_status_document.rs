use crate::core::partner_status::ShopPartnerStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopPartnerStatusDocument {
    Scraped,
    Partnered,
}

impl ShopPartnerStatusDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShopPartnerStatusDocument::Scraped => "SCRAPED",
            ShopPartnerStatusDocument::Partnered => "PARTNERED",
        }
    }
}

impl From<ShopPartnerStatus> for ShopPartnerStatusDocument {
    fn from(status: ShopPartnerStatus) -> Self {
        match status {
            ShopPartnerStatus::Scraped => ShopPartnerStatusDocument::Scraped,
            ShopPartnerStatus::Partnered => ShopPartnerStatusDocument::Partnered,
        }
    }
}

impl From<ShopPartnerStatusDocument> for ShopPartnerStatus {
    fn from(document: ShopPartnerStatusDocument) -> Self {
        match document {
            ShopPartnerStatusDocument::Scraped => ShopPartnerStatus::Scraped,
            ShopPartnerStatusDocument::Partnered => ShopPartnerStatus::Partnered,
        }
    }
}
