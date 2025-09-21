use crate::item_state_data::ItemStateData;
use common::{
    event::Event, event_id::EventId, item_id::ItemId, price::data::PriceData, shop_id::ShopId,
    shops_item_id::ShopsItemId,
};
use item_core::item_event::LocalizedItemEventPayloadView;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl ItemEventPayloadData {
    fn is_empty(&self) -> bool {
        matches!(&self, ItemEventPayloadData::Created)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetItemEventData {
    pub event_type: ItemEventTypeData,

    pub item_id: ItemId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    #[serde(skip_serializing_if = "ItemEventPayloadData::is_empty")]
    pub payload: ItemEventPayloadData,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl From<Event<ItemId, LocalizedItemEventPayloadView>> for GetItemEventData {
    fn from(event: Event<ItemId, LocalizedItemEventPayloadView>) -> Self {
        let (event_type, shop_id, shops_item_id, payload) = match event.payload {
            LocalizedItemEventPayloadView::Created(payload) => (
                ItemEventTypeData::Created,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::Created,
            ),
            LocalizedItemEventPayloadView::StateListed(payload) => (
                ItemEventTypeData::StateListed,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateListed(ItemStateData::Listed),
            ),
            LocalizedItemEventPayloadView::StateAvailable(payload) => (
                ItemEventTypeData::StateAvailable,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateAvailable(ItemStateData::Available),
            ),
            LocalizedItemEventPayloadView::StateReserved(payload) => (
                ItemEventTypeData::StateReserved,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateReserved(ItemStateData::Reserved),
            ),
            LocalizedItemEventPayloadView::StateSold(payload) => (
                ItemEventTypeData::StateSold,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateSold(ItemStateData::Sold),
            ),
            LocalizedItemEventPayloadView::StateRemoved(payload) => (
                ItemEventTypeData::StateRemoved,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateRemoved(ItemStateData::Removed),
            ),
            LocalizedItemEventPayloadView::PriceDiscovered(payload) => (
                ItemEventTypeData::PriceDiscovered,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::PriceDiscovered(payload.price.into()),
            ),
            LocalizedItemEventPayloadView::PriceDropped(payload) => (
                ItemEventTypeData::PriceDropped,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::PriceDropped(payload.price.into()),
            ),
            LocalizedItemEventPayloadView::PriceIncreased(payload) => (
                ItemEventTypeData::PriceIncreased,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::PriceIncreased(payload.price.into()),
            ),
        };

        GetItemEventData {
            event_type,
            item_id: event.aggregate_id,
            event_id: event.event_id,
            shop_id,
            shops_item_id,
            payload,
            timestamp: event.timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        get_item_event_data::{GetItemEventData, ItemEventPayloadData, ItemEventTypeData},
        item_state_data::ItemStateData,
    };
    use common::{
        currency::{data::CurrencyData, domain::Currency},
        event::Event,
        item_state::domain::ItemState,
        localized::Localized,
        price::{data::PriceData, domain::Price},
    };
    use item_core::{
        hash::ItemHash,
        item_event::{
            LocalizedItemCreatedEventPayloadView, LocalizedItemEventPayloadView,
            LocalizedItemPriceChangeEventPayloadView, LocalizedItemStateChangeEventPayloadView,
        },
    };
    use time::macros::utc_datetime;
    use url::Url;
    use uuid::Uuid;

    #[rstest::rstest]
    #[case::created(
        LocalizedItemEventPayloadView::Created(LocalizedItemCreatedEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            shop_name: "baz".into(),
            title: Localized::new(common::language::domain::Language::De, "boop".into()),
            description: None,
            price: Some(Price::new(500u64.into(), Currency::Eur)),
            state: ItemState::Listed,
            url: Url::parse("https://foo.bar/boop").unwrap(),
            images: vec![],
            hash: ItemHash::new(&Some(Price::new(500u64.into(), Currency::Eur)), &ItemState::Listed),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::Created,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::Created,
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_listed(
        LocalizedItemEventPayloadView::StateListed(LocalizedItemStateChangeEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            hash: ItemHash::new(&None, &ItemState::Listed),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateListed,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateListed(ItemStateData::Listed),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_available(
        LocalizedItemEventPayloadView::StateAvailable(LocalizedItemStateChangeEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            hash: ItemHash::new(&None, &ItemState::Available),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateAvailable,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateAvailable(ItemStateData::Available),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_reserved(
        LocalizedItemEventPayloadView::StateReserved(LocalizedItemStateChangeEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            hash: ItemHash::new(&None, &ItemState::Reserved),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateReserved,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateReserved(ItemStateData::Reserved),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_sold(
        LocalizedItemEventPayloadView::StateSold(LocalizedItemStateChangeEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            hash: ItemHash::new(&None, &ItemState::Sold),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateSold,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateSold(ItemStateData::Sold),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_removed(
        LocalizedItemEventPayloadView::StateRemoved(LocalizedItemStateChangeEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            hash: ItemHash::new(&None, &ItemState::Removed),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateRemoved,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateRemoved(ItemStateData::Removed),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_discovered(
        LocalizedItemEventPayloadView::PriceDiscovered(LocalizedItemPriceChangeEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            price: Price::new(500u64.into(), Currency::Eur),
            hash: ItemHash::new(&Some(Price::new(500u64.into(), Currency::Eur)), &ItemState::Available),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::PriceDiscovered,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::PriceDiscovered(PriceData::new(CurrencyData::Eur, 500u64)),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_dropped(
        LocalizedItemEventPayloadView::PriceDropped(LocalizedItemPriceChangeEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            price: Price::new(500u64.into(), Currency::Eur),
            hash: ItemHash::new(&Some(Price::new(500u64.into(), Currency::Eur)), &ItemState::Available),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::PriceDropped,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::PriceDropped(PriceData::new(CurrencyData::Eur, 500u64)),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_increased(
        LocalizedItemEventPayloadView::PriceIncreased(LocalizedItemPriceChangeEventPayloadView {
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            price: Price::new(500u64.into(), Currency::Eur),
            hash: ItemHash::new(&Some(Price::new(500u64.into(), Currency::Eur)), &ItemState::Available),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::PriceIncreased,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "foo".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::PriceIncreased(PriceData::new(CurrencyData::Eur, 500u64)),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    fn should_from_event_localized_item_event_payload_for_get_item_event_data(
        #[case] payload_view: LocalizedItemEventPayloadView,
        #[case] expected: GetItemEventData,
    ) {
        let event = Event {
            aggregate_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
            payload: payload_view,
        };

        let actual: GetItemEventData = event.into();

        assert_eq!(expected, actual);
    }
}
