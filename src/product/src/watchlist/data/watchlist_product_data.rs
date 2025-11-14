use crate::watchlist::core::watchlist_product::WatchlistProduct;
use common::{product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistProductData {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub product_id: ProductId,
    pub notifications: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<WatchlistProduct> for WatchlistProductData {
    fn from(domain: WatchlistProduct) -> Self {
        WatchlistProductData {
            shop_id: domain.shop_id,
            shops_product_id: domain.shops_product_id,
            product_id: domain.product_id,
            notifications: domain.notifications,
            created: domain.created,
            updated: domain.updated,
        }
    }
}
