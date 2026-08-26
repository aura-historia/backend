use crate::shop_listing_id::ShopListingId;
use shop_core::shop_id::ShopId;

domain_primitives::uuid_v4_newtype!(ProductListingId);

impl From<ProductListingId> for uuid::Uuid {
    fn from(id: ProductListingId) -> Self {
        id.0
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct ProductListingKey {
    pub shop_id: ShopId,
    pub shop_listing_id: ShopListingId,
}

impl ProductListingKey {
    pub fn new(shop_id: ShopId, shop_listing_id: ShopListingId) -> Self {
        Self {
            shop_id,
            shop_listing_id,
        }
    }
}
