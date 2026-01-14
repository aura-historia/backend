use crate::core::shop_type::ShopType;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopTypeData {
    AuctionHouse,
    CommercialDealer,
    Marketplace,
}

impl From<ShopType> for ShopTypeData {
    fn from(value: ShopType) -> Self {
        match value {
            ShopType::AuctionHouse => ShopTypeData::AuctionHouse,
            ShopType::CommercialDealer => ShopTypeData::CommercialDealer,
            ShopType::Marketplace => ShopTypeData::Marketplace,
        }
    }
}

impl From<ShopTypeData> for ShopType {
    fn from(value: ShopTypeData) -> Self {
        match value {
            ShopTypeData::AuctionHouse => ShopType::AuctionHouse,
            ShopTypeData::CommercialDealer => ShopType::CommercialDealer,
            ShopTypeData::Marketplace => ShopType::Marketplace,
        }
    }
}
