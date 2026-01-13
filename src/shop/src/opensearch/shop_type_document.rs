use crate::{core::shop_type::ShopType, dynamodb::shop_type_record::ShopTypeRecord};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize, strum_macros::EnumCount,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopTypeDocument {
    AuctionHouse,
    CommercialDealer,
    Marketplace,
}

impl ShopTypeDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShopTypeDocument::AuctionHouse => "AUCTION_HOUSE",
            ShopTypeDocument::CommercialDealer => "COMMERCIAL_DEALER",
            ShopTypeDocument::Marketplace => "MARKETPLACE",
        }
    }
}

impl From<ShopTypeRecord> for ShopTypeDocument {
    fn from(record: ShopTypeRecord) -> Self {
        match record {
            ShopTypeRecord::AuctionHouse => ShopTypeDocument::AuctionHouse,
            ShopTypeRecord::CommercialDealer => ShopTypeDocument::CommercialDealer,
            ShopTypeRecord::Marketplace => ShopTypeDocument::Marketplace,
        }
    }
}

impl From<ShopTypeDocument> for ShopType {
    fn from(doc: ShopTypeDocument) -> Self {
        match doc {
            ShopTypeDocument::AuctionHouse => ShopType::AuctionHouse,
            ShopTypeDocument::CommercialDealer => ShopType::CommercialDealer,
            ShopTypeDocument::Marketplace => ShopType::Marketplace,
        }
    }
}

impl From<ShopType> for ShopTypeDocument {
    fn from(value: ShopType) -> Self {
        match value {
            ShopType::AuctionHouse => ShopTypeDocument::AuctionHouse,
            ShopType::CommercialDealer => ShopTypeDocument::CommercialDealer,
            ShopType::Marketplace => ShopTypeDocument::Marketplace,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ShopTypeDocument;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(ShopTypeDocument::AuctionHouse, "\"AUCTION_HOUSE\"")]
    #[case(ShopTypeDocument::CommercialDealer, "\"COMMERCIAL_DEALER\"")]
    #[case(ShopTypeDocument::Marketplace, "\"MARKETPLACE\"")]
    fn should_serialize_shop_type_document_in_screaming_snake_case(
        #[case] shop_type: ShopTypeDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&shop_type).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"AUCTION_HOUSE\"", ShopTypeDocument::AuctionHouse)]
    #[case("\"COMMERCIAL_DEALER\"", ShopTypeDocument::CommercialDealer)]
    #[case("\"MARKETPLACE\"", ShopTypeDocument::Marketplace)]
    fn should_deserialize_shop_type_document_in_screaming_snake_case(
        #[case] shop_type: &str,
        #[case] expected: ShopTypeDocument,
    ) {
        let actual = serde_json::from_str::<ShopTypeDocument>(shop_type).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case(ShopTypeDocument::AuctionHouse)]
    #[case(ShopTypeDocument::CommercialDealer)]
    #[case(ShopTypeDocument::Marketplace)]
    fn should_as_str_match_serialized(#[case] shop_type: ShopTypeDocument) {
        let serialized = serde_json::to_string::<ShopTypeDocument>(&shop_type)
            .unwrap()
            .replace("\"", "");
        let as_str = shop_type.as_str();
        assert_eq!(serialized, as_str);
    }
}
