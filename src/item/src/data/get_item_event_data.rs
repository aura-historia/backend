use crate::core::item_event::LocalizedItemEventPayloadView;
use crate::data::item_state_data::ItemStateData;
use common::{
    event::Event, event_id::EventId, item_id::ItemId, price::data::PriceData, shop_id::ShopId,
    shops_item_id::ShopsItemId,
};
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
    StateUnknown,
    PriceDiscovered,
    PriceDropped,
    PriceIncreased,
    PriceRemoved,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ItemEventPayloadData {
    Created(ItemCreatedEventPayloadData),
    StateListed(ItemEventStateChangedPayloadData),
    StateAvailable(ItemEventStateChangedPayloadData),
    StateReserved(ItemEventStateChangedPayloadData),
    StateSold(ItemEventStateChangedPayloadData),
    StateRemoved(ItemEventStateChangedPayloadData),
    StateUnknown(ItemEventStateChangedPayloadData),
    PriceDiscovered(ItemEventPriceDiscoveredPayloadData),
    PriceDropped(ItemEventPriceChangedPayloadData),
    PriceIncreased(ItemEventPriceChangedPayloadData),
    PriceRemoved(ItemEventPriceRemovedPayloadData),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemEventStateChangedPayloadData {
    pub old_state: ItemStateData,
    pub new_state: ItemStateData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemEventPriceDiscoveredPayloadData {
    pub new_price: PriceData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemEventPriceChangedPayloadData {
    pub old_price: PriceData,
    pub new_price: PriceData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemEventPriceRemovedPayloadData {
    pub old_price: PriceData,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemCreatedEventPayloadData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    pub state: ItemStateData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetItemEventData {
    pub event_type: ItemEventTypeData,
    pub item_id: ItemId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
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
                ItemEventPayloadData::Created(ItemCreatedEventPayloadData {
                    price: payload.price.map(PriceData::from),
                    state: payload.state.into(),
                }),
            ),
            LocalizedItemEventPayloadView::StateListed(payload) => (
                ItemEventTypeData::StateListed,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateListed(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ItemStateData::Listed,
                }),
            ),
            LocalizedItemEventPayloadView::StateAvailable(payload) => (
                ItemEventTypeData::StateAvailable,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateAvailable(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ItemStateData::Available,
                }),
            ),
            LocalizedItemEventPayloadView::StateReserved(payload) => (
                ItemEventTypeData::StateReserved,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateReserved(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ItemStateData::Reserved,
                }),
            ),
            LocalizedItemEventPayloadView::StateSold(payload) => (
                ItemEventTypeData::StateSold,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateSold(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ItemStateData::Sold,
                }),
            ),
            LocalizedItemEventPayloadView::StateRemoved(payload) => (
                ItemEventTypeData::StateRemoved,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateRemoved(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ItemStateData::Removed,
                }),
            ),
            LocalizedItemEventPayloadView::StateUnknown(payload) => (
                ItemEventTypeData::StateUnknown,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::StateUnknown(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ItemStateData::Unknown,
                }),
            ),
            LocalizedItemEventPayloadView::PriceDiscovered(payload) => (
                ItemEventTypeData::PriceDiscovered,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::PriceDiscovered(ItemEventPriceDiscoveredPayloadData {
                    new_price: payload.price.into(),
                }),
            ),
            LocalizedItemEventPayloadView::PriceDropped(payload) => (
                ItemEventTypeData::PriceDropped,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::PriceDropped(ItemEventPriceChangedPayloadData {
                    old_price: payload.old_price.into(),
                    new_price: payload.new_price.into(),
                }),
            ),
            LocalizedItemEventPayloadView::PriceIncreased(payload) => (
                ItemEventTypeData::PriceIncreased,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::PriceIncreased(ItemEventPriceChangedPayloadData {
                    old_price: payload.old_price.into(),
                    new_price: payload.new_price.into(),
                }),
            ),
            LocalizedItemEventPayloadView::PriceRemoved(payload) => (
                ItemEventTypeData::PriceRemoved,
                payload.shop_id,
                payload.shops_item_id,
                ItemEventPayloadData::PriceRemoved(ItemEventPriceRemovedPayloadData {
                    old_price: payload.old_price.into(),
                }),
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
    use crate::core::item_event::{
        LocalizedItemCreatedEventPayloadView, LocalizedItemEventPayloadView,
        LocalizedItemPriceChangeEventPayloadView, LocalizedItemPriceDiscoveryEventPayloadView,
        LocalizedItemStateChangeEventPayloadView,
    };
    use crate::data::{
        get_item_event_data::{
            GetItemEventData, ItemCreatedEventPayloadData, ItemEventPayloadData,
            ItemEventPriceChangedPayloadData, ItemEventPriceDiscoveredPayloadData,
            ItemEventStateChangedPayloadData, ItemEventTypeData,
        },
        item_state_data::ItemStateData,
    };
    use common::{
        currency::{data::CurrencyData, domain::Currency},
        event::Event,
        item_state::domain::ItemState,
        localized::Localized,
        price::{data::PriceData, domain::Price},
    };
    use time::macros::utc_datetime;
    use url::Url;
    use uuid::Uuid;

    #[rstest::rstest]
    #[case::created(
        LocalizedItemEventPayloadView::Created(LocalizedItemCreatedEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            shop_name: "baz".into(),
            title: Localized::new(common::language::domain::Language::De, "boop".into()),
            description: None,
            price: Some(Price::new(500u64.into(), Currency::Eur)),
            state: ItemState::Listed,
            url: Url::parse("https://foo.bar/boop").unwrap(),
            images: vec![],
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::Created,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::Created(ItemCreatedEventPayloadData { price: Some(PriceData::new(CurrencyData::Eur, 500u64)), state: ItemStateData::Listed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_listed(
        LocalizedItemEventPayloadView::StateListed(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            old_state: ItemState::Available
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateListed,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateListed(ItemEventStateChangedPayloadData { old_state: ItemStateData::Available, new_state: ItemStateData::Listed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_available(
        LocalizedItemEventPayloadView::StateAvailable(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            old_state: ItemState::Listed
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateAvailable,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateAvailable(ItemEventStateChangedPayloadData { old_state: ItemStateData::Listed, new_state: ItemStateData::Available }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_reserved(
        LocalizedItemEventPayloadView::StateReserved(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            old_state: ItemState::Available
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateReserved,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateReserved(ItemEventStateChangedPayloadData { old_state: ItemStateData::Available, new_state: ItemStateData::Reserved }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_sold(
        LocalizedItemEventPayloadView::StateSold(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            old_state: ItemState::Reserved
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateSold,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateSold(ItemEventStateChangedPayloadData { old_state: ItemStateData::Reserved, new_state: ItemStateData::Sold }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_removed(
        LocalizedItemEventPayloadView::StateRemoved(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            old_state: ItemState::Sold
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateRemoved,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateRemoved(ItemEventStateChangedPayloadData { old_state: ItemStateData::Sold, new_state: ItemStateData::Removed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_unknown(
        LocalizedItemEventPayloadView::StateUnknown(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            old_state: ItemState::Removed
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::StateUnknown,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::StateUnknown(ItemEventStateChangedPayloadData { old_state: ItemStateData::Removed, new_state: ItemStateData::Unknown }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_discovered(
        LocalizedItemEventPayloadView::PriceDiscovered(LocalizedItemPriceDiscoveryEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            price: Price::new(500u64.into(), Currency::Eur),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::PriceDiscovered,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::PriceDiscovered(ItemEventPriceDiscoveredPayloadData {
                new_price: PriceData::new(CurrencyData::Eur, 500u64)
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_dropped(
        LocalizedItemEventPayloadView::PriceDropped(LocalizedItemPriceChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            new_price: Price::new(500u64.into(), Currency::Eur),
            old_price: Price::new(700u64.into(), Currency::Eur),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::PriceDropped,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::PriceDropped(ItemEventPriceChangedPayloadData { old_price: PriceData::new(CurrencyData::Eur, 700u64), new_price: PriceData::new(CurrencyData::Eur, 500u64) }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_increased(
        LocalizedItemEventPayloadView::PriceIncreased(LocalizedItemPriceChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            new_price: Price::new(777u64.into(), Currency::Eur),
            old_price: Price::new(500u64.into(), Currency::Eur),
        }),
        GetItemEventData {
            event_type: ItemEventTypeData::PriceIncreased,
            item_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_item_id: "bar".into(),
            payload: ItemEventPayloadData::PriceIncreased(ItemEventPriceChangedPayloadData { old_price: PriceData::new(CurrencyData::Eur, 500u64), new_price: PriceData::new(CurrencyData::Eur, 777u64) }),
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
