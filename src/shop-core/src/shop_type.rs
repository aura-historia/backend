#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum ShopType {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn should_keep_all_shop_type_variants_distinct() {
        let shop_types = HashSet::from([
            ShopType::AuctionHouse,
            ShopType::AuctionPlatform,
            ShopType::CommercialDealer,
            ShopType::Marketplace,
        ]);

        assert_eq!(4, shop_types.len());
    }
}
