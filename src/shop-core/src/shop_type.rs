#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, strum_macros::EnumIter)]
pub enum ShopType {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}

impl ShopType {
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|shop_type| shop_type.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuctionHouse => "AUCTION_HOUSE",
            Self::AuctionPlatform => "AUCTION_PLATFORM",
            Self::CommercialDealer => "COMMERCIAL_DEALER",
            Self::Marketplace => "MARKETPLACE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_keep_all_shop_type_identifiers_unique() {
        let shop_types = ShopType::iter()
            .map(ShopType::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(ShopType::iter().count(), shop_types.len());
    }

    #[test]
    fn should_use_canonical_shop_type_identifiers() {
        assert_eq!("AUCTION_HOUSE", ShopType::AuctionHouse.as_str());
        assert_eq!("AUCTION_PLATFORM", ShopType::AuctionPlatform.as_str());
        assert_eq!("COMMERCIAL_DEALER", ShopType::CommercialDealer.as_str());
        assert_eq!("MARKETPLACE", ShopType::Marketplace.as_str());
        assert_eq!(
            Some(ShopType::Marketplace),
            ShopType::from_code("MARKETPLACE")
        );
        assert_eq!(None, ShopType::from_code("marketplace"));
    }
}
