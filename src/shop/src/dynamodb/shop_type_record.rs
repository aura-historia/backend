use crate::core::shop_type::ShopType;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopTypeRecord {
    AuctionHouse,
    CommercialDealer,
    Marketplace,
}

impl From<ShopTypeRecord> for ShopType {
    fn from(record: ShopTypeRecord) -> Self {
        match record {
            ShopTypeRecord::AuctionHouse => ShopType::AuctionHouse,
            ShopTypeRecord::CommercialDealer => ShopType::CommercialDealer,
            ShopTypeRecord::Marketplace => ShopType::Marketplace,
        }
    }
}

impl From<ShopType> for ShopTypeRecord {
    fn from(value: ShopType) -> Self {
        match value {
            ShopType::AuctionHouse => ShopTypeRecord::AuctionHouse,
            ShopType::CommercialDealer => ShopTypeRecord::CommercialDealer,
            ShopType::Marketplace => ShopTypeRecord::Marketplace,
        }
    }
}
