use common::has_key::HasKey;
use common::product_id::ProductKey;
use common::product_lifecycle::domain::ProductLifecycle;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub enum ProductLifecycleEventPayload {
    Deleted(ProductDeletedLifecycleEventPayload),
}

impl ProductLifecycleEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductLifecycleEventPayload::Deleted(_) => "LIFECYCLE_DELETED",
        }
    }
}

impl HasKey for ProductLifecycleEventPayload {
    type Key = ProductKey;

    fn key(&self) -> ProductKey {
        match self {
            ProductLifecycleEventPayload::Deleted(payload) => {
                ProductKey::new(payload.shop_id, payload.shops_product_id.clone())
            }
        }
    }
}

pub trait ProductCommonLifecycleEventPayload {
    fn shop_id(&self) -> &ShopId;
    fn shops_product_id(&self) -> &ShopsProductId;
    fn seller_id(&self) -> &ShopId;
}

impl ProductCommonLifecycleEventPayload for ProductLifecycleEventPayload {
    fn shop_id(&self) -> &ShopId {
        match self {
            ProductLifecycleEventPayload::Deleted(payload) => &payload.shop_id,
        }
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        match self {
            ProductLifecycleEventPayload::Deleted(payload) => &payload.shops_product_id,
        }
    }

    fn seller_id(&self) -> &ShopId {
        match self {
            ProductLifecycleEventPayload::Deleted(payload) => &payload.seller_id,
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProductDeletedLifecycleEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_lifecycle: ProductLifecycle,
    pub new_lifecycle: ProductLifecycle,
}
