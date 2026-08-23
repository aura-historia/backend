use serde::{Deserialize, Serialize};
use shop_core::shop_type::ShopType;

#[derive(
    Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize, strum_macros::EnumCount,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ShopTypeDocument {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}

impl ShopTypeDocument {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::AuctionHouse => "AUCTION_HOUSE",
            Self::AuctionPlatform => "AUCTION_PLATFORM",
            Self::CommercialDealer => "COMMERCIAL_DEALER",
            Self::Marketplace => "MARKETPLACE",
        }
    }
}

impl From<ShopType> for ShopTypeDocument {
    fn from(value: ShopType) -> Self {
        match value {
            ShopType::AuctionHouse => Self::AuctionHouse,
            ShopType::AuctionPlatform => Self::AuctionPlatform,
            ShopType::CommercialDealer => Self::CommercialDealer,
            ShopType::Marketplace => Self::Marketplace,
        }
    }
}

impl From<ShopTypeDocument> for ShopType {
    fn from(value: ShopTypeDocument) -> Self {
        match value {
            ShopTypeDocument::AuctionHouse => Self::AuctionHouse,
            ShopTypeDocument::AuctionPlatform => Self::AuctionPlatform,
            ShopTypeDocument::CommercialDealer => Self::CommercialDealer,
            ShopTypeDocument::Marketplace => Self::Marketplace,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(ShopTypeDocument::AuctionHouse, "\"AUCTION_HOUSE\"")]
    #[case(ShopTypeDocument::AuctionPlatform, "\"AUCTION_PLATFORM\"")]
    #[case(ShopTypeDocument::CommercialDealer, "\"COMMERCIAL_DEALER\"")]
    #[case(ShopTypeDocument::Marketplace, "\"MARKETPLACE\"")]
    fn should_serialize_shop_type_document_in_screaming_snake_case(
        #[case] shop_type: ShopTypeDocument,
        #[case] expected: &'static str,
    ) -> Result<(), serde_json::Error> {
        assert_eq!(expected, serde_json::to_string(&shop_type)?);
        assert_eq!(expected.replace('"', ""), shop_type.as_str());
        Ok(())
    }

    #[rstest::rstest]
    #[case(ShopType::AuctionHouse, ShopTypeDocument::AuctionHouse)]
    #[case(ShopType::AuctionPlatform, ShopTypeDocument::AuctionPlatform)]
    #[case(ShopType::CommercialDealer, ShopTypeDocument::CommercialDealer)]
    #[case(ShopType::Marketplace, ShopTypeDocument::Marketplace)]
    fn should_roundtrip_shop_type(#[case] domain: ShopType, #[case] document: ShopTypeDocument) {
        assert_eq!(document, ShopTypeDocument::from(domain));
        assert_eq!(domain, ShopType::from(document));
    }
}
