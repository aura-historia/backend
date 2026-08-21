use crate::shops_product_id::ShopsProductId;
use shop_core::shop_id::ShopId;

domain_primitives::uuid_v4_newtype!(ProductId);

impl From<ProductId> for uuid::Uuid {
    fn from(id: ProductId) -> Self {
        id.0
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct ProductKey {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
}

impl ProductKey {
    pub fn new(shop_id: ShopId, shops_product_id: ShopsProductId) -> Self {
        Self {
            shop_id,
            shops_product_id,
        }
    }
}
