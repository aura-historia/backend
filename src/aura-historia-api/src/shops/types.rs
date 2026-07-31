use serde::Serialize;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_type::ShopType;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ShopTypeData {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}

impl From<ShopType> for ShopTypeData {
    fn from(value: ShopType) -> Self {
        match value {
            ShopType::AuctionHouse => Self::AuctionHouse,
            ShopType::AuctionPlatform => Self::AuctionPlatform,
            ShopType::CommercialDealer => Self::CommercialDealer,
            ShopType::Marketplace => Self::Marketplace,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ShopPartnerStatusData {
    Scraped,
    Partnered,
}

impl From<ShopPartnerStatus> for ShopPartnerStatusData {
    fn from(value: ShopPartnerStatus) -> Self {
        match value {
            ShopPartnerStatus::Scraped => Self::Scraped,
            ShopPartnerStatus::Partnered => Self::Partnered,
        }
    }
}
