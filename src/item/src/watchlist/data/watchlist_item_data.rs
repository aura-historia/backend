use crate::watchlist::core::watchlist_item::WatchlistItem;
use common::{item_id::ItemId, shop_id::ShopId, shops_item_id::ShopsItemId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistItemData {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub item_id: ItemId,
    pub notifications: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<WatchlistItem> for WatchlistItemData {
    fn from(domain: WatchlistItem) -> Self {
        WatchlistItemData {
            shop_id: domain.shop_id,
            shops_item_id: domain.shops_item_id,
            item_id: domain.item_id,
            notifications: domain.notifications,
            created: domain.created,
            updated: domain.updated,
        }
    }
}
