use crate::item_state_data::ItemStateData;
use common::{
    event_id::EventId, item_id::ItemId, price::data::PriceData, shop_id::ShopId,
    shops_item_id::ShopsItemId,
};
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemEventTypeData {
    Created,
    StateListed,
    StateAvailable,
    StateReserved,
    StateSold,
    StateRemoved,
    PriceDiscovered,
    PriceDropped,
    PriceIncreased,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ItemEventPayloadData {
    Created,
    StateListed(ItemStateData),
    StateAvailable(ItemStateData),
    StateReserved(ItemStateData),
    StateSold(ItemStateData),
    StateRemoved(ItemStateData),
    PriceDiscovered(PriceData),
    PriceDropped(PriceData),
    PriceIncreased(PriceData),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetItemEventData {
    pub event_type: ItemEventTypeData,

    pub item_id: ItemId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<ItemEventPayloadData>,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}
