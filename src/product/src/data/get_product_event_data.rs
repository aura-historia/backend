use crate::core::product_event::LocalizedItemEventPayloadView;
use crate::data::product_state_data::ProductStateData;
use common::{
    event::Event, event_id::EventId, price::data::PriceData, product_id::ProductId,
    shop_id::ShopId, shops_product_id::ShopsProductId,
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
    pub old_state: ProductStateData,
    pub new_state: ProductStateData,
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

    pub state: ProductStateData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProductEventData {
    pub event_type: ItemEventTypeData,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub payload: ItemEventPayloadData,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl From<Event<ProductId, LocalizedItemEventPayloadView>> for GetProductEventData {
    fn from(event: Event<ProductId, LocalizedItemEventPayloadView>) -> Self {
        let (event_type, shop_id, shops_product_id, payload) = match event.payload {
            LocalizedItemEventPayloadView::Created(payload) => (
                ItemEventTypeData::Created,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::Created(ItemCreatedEventPayloadData {
                    price: payload.price.map(PriceData::from),
                    state: payload.state.into(),
                }),
            ),
            LocalizedItemEventPayloadView::StateListed(payload) => (
                ItemEventTypeData::StateListed,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::StateListed(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Listed,
                }),
            ),
            LocalizedItemEventPayloadView::StateAvailable(payload) => (
                ItemEventTypeData::StateAvailable,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::StateAvailable(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Available,
                }),
            ),
            LocalizedItemEventPayloadView::StateReserved(payload) => (
                ItemEventTypeData::StateReserved,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::StateReserved(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Reserved,
                }),
            ),
            LocalizedItemEventPayloadView::StateSold(payload) => (
                ItemEventTypeData::StateSold,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::StateSold(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Sold,
                }),
            ),
            LocalizedItemEventPayloadView::StateRemoved(payload) => (
                ItemEventTypeData::StateRemoved,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::StateRemoved(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Removed,
                }),
            ),
            LocalizedItemEventPayloadView::StateUnknown(payload) => (
                ItemEventTypeData::StateUnknown,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::StateUnknown(ItemEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Unknown,
                }),
            ),
            LocalizedItemEventPayloadView::PriceDiscovered(payload) => (
                ItemEventTypeData::PriceDiscovered,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::PriceDiscovered(ItemEventPriceDiscoveredPayloadData {
                    new_price: payload.price.into(),
                }),
            ),
            LocalizedItemEventPayloadView::PriceDropped(payload) => (
                ItemEventTypeData::PriceDropped,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::PriceDropped(ItemEventPriceChangedPayloadData {
                    old_price: payload.old_price.into(),
                    new_price: payload.new_price.into(),
                }),
            ),
            LocalizedItemEventPayloadView::PriceIncreased(payload) => (
                ItemEventTypeData::PriceIncreased,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::PriceIncreased(ItemEventPriceChangedPayloadData {
                    old_price: payload.old_price.into(),
                    new_price: payload.new_price.into(),
                }),
            ),
            LocalizedItemEventPayloadView::PriceRemoved(payload) => (
                ItemEventTypeData::PriceRemoved,
                payload.shop_id,
                payload.shops_product_id,
                ItemEventPayloadData::PriceRemoved(ItemEventPriceRemovedPayloadData {
                    old_price: payload.old_price.into(),
                }),
            ),
        };

        GetProductEventData {
            event_type,
            product_id: event.aggregate_id,
            event_id: event.event_id,
            shop_id,
            shops_product_id,
            payload,
            timestamp: event.timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::product_event::{
        LocalizedItemCreatedEventPayloadView, LocalizedItemEventPayloadView,
        LocalizedItemPriceChangeEventPayloadView, LocalizedItemPriceDiscoveryEventPayloadView,
        LocalizedItemStateChangeEventPayloadView,
    };
    use crate::data::{
        get_product_event_data::{
            GetProductEventData, ItemCreatedEventPayloadData, ItemEventPayloadData,
            ItemEventPriceChangedPayloadData, ItemEventPriceDiscoveredPayloadData,
            ItemEventStateChangedPayloadData, ItemEventTypeData,
        },
        product_state_data::ProductStateData,
    };
    use common::{
        currency::{data::CurrencyData, domain::Currency},
        event::Event,
        localized::Localized,
        price::{data::PriceData, domain::Price},
        product_state::domain::ProductState,
    };
    use time::macros::utc_datetime;
    use url::Url;
    use uuid::Uuid;

    #[rstest::rstest]
    #[case::created(
        LocalizedItemEventPayloadView::Created(LocalizedItemCreatedEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            shop_name: "baz".into(),
            title: Localized::new(common::language::domain::Language::De, "boop".into()),
            description: None,
            price: Some(Price::new(500u64.into(), Currency::Eur)),
            state: ProductState::Listed,
            url: Url::parse("https://foo.bar/boop").unwrap(),
            images: vec![],
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::Created,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::Created(ItemCreatedEventPayloadData { price: Some(PriceData::new(CurrencyData::Eur, 500u64)), state: ProductStateData::Listed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_listed(
        LocalizedItemEventPayloadView::StateListed(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Available
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::StateListed,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::StateListed(ItemEventStateChangedPayloadData { old_state: ProductStateData::Available, new_state: ProductStateData::Listed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_available(
        LocalizedItemEventPayloadView::StateAvailable(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Listed
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::StateAvailable,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::StateAvailable(ItemEventStateChangedPayloadData { old_state: ProductStateData::Listed, new_state: ProductStateData::Available }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_reserved(
        LocalizedItemEventPayloadView::StateReserved(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Available
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::StateReserved,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::StateReserved(ItemEventStateChangedPayloadData { old_state: ProductStateData::Available, new_state: ProductStateData::Reserved }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_sold(
        LocalizedItemEventPayloadView::StateSold(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Reserved
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::StateSold,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::StateSold(ItemEventStateChangedPayloadData { old_state: ProductStateData::Reserved, new_state: ProductStateData::Sold }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_removed(
        LocalizedItemEventPayloadView::StateRemoved(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Sold
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::StateRemoved,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::StateRemoved(ItemEventStateChangedPayloadData { old_state: ProductStateData::Sold, new_state: ProductStateData::Removed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_unknown(
        LocalizedItemEventPayloadView::StateUnknown(LocalizedItemStateChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Removed
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::StateUnknown,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::StateUnknown(ItemEventStateChangedPayloadData { old_state: ProductStateData::Removed, new_state: ProductStateData::Unknown }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_discovered(
        LocalizedItemEventPayloadView::PriceDiscovered(LocalizedItemPriceDiscoveryEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            price: Price::new(500u64.into(), Currency::Eur),
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::PriceDiscovered,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::PriceDiscovered(ItemEventPriceDiscoveredPayloadData {
                new_price: PriceData::new(CurrencyData::Eur, 500u64)
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_dropped(
        LocalizedItemEventPayloadView::PriceDropped(LocalizedItemPriceChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            new_price: Price::new(500u64.into(), Currency::Eur),
            old_price: Price::new(700u64.into(), Currency::Eur),
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::PriceDropped,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::PriceDropped(ItemEventPriceChangedPayloadData { old_price: PriceData::new(CurrencyData::Eur, 700u64), new_price: PriceData::new(CurrencyData::Eur, 500u64) }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_increased(
        LocalizedItemEventPayloadView::PriceIncreased(LocalizedItemPriceChangeEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            new_price: Price::new(777u64.into(), Currency::Eur),
            old_price: Price::new(500u64.into(), Currency::Eur),
        }),
        GetProductEventData {
            event_type: ItemEventTypeData::PriceIncreased,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ItemEventPayloadData::PriceIncreased(ItemEventPriceChangedPayloadData { old_price: PriceData::new(CurrencyData::Eur, 500u64), new_price: PriceData::new(CurrencyData::Eur, 777u64) }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    fn should_from_event_localized_item_event_payload_for_get_product_event_data(
        #[case] payload_view: LocalizedItemEventPayloadView,
        #[case] expected: GetProductEventData,
    ) {
        let event = Event {
            aggregate_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
            payload: payload_view,
        };

        let actual: GetProductEventData = event.into();

        assert_eq!(expected, actual);
    }
}
