use geo::data::continent_data::ContinentData;
use serde::{Deserialize, Serialize};
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_type::ShopType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl From<ShopTypeData> for ShopType {
    fn from(value: ShopTypeData) -> Self {
        match value {
            ShopTypeData::AuctionHouse => Self::AuctionHouse,
            ShopTypeData::AuctionPlatform => Self::AuctionPlatform,
            ShopTypeData::CommercialDealer => Self::CommercialDealer,
            ShopTypeData::Marketplace => Self::Marketplace,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl From<ShopPartnerStatusData> for ShopPartnerStatus {
    fn from(value: ShopPartnerStatusData) -> Self {
        match value {
            ShopPartnerStatusData::Scraped => ShopPartnerStatus::Scraped,
            ShopPartnerStatusData::Partnered => ShopPartnerStatus::Partnered,
        }
    }
}

pub(crate) type ShopContinentData = ContinentData;
