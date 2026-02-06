use crate::core::product_event::domain::LocalizedProductDomainEventPayloadView;
use crate::data::product_state_data::ProductStateData;
use common::{
    event::Event, event_id::EventId, price::data::PriceData, product_id::ProductId,
    shop_id::ShopId, shops_product_id::ShopsProductId,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductEventTypeData {
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
pub enum ProductEventPayloadData {
    Created(ProductCreatedEventPayloadData),
    StateListed(ProductEventStateChangedPayloadData),
    StateAvailable(ProductEventStateChangedPayloadData),
    StateReserved(ProductEventStateChangedPayloadData),
    StateSold(ProductEventStateChangedPayloadData),
    StateRemoved(ProductEventStateChangedPayloadData),
    StateUnknown(ProductEventStateChangedPayloadData),
    PriceDiscovered(ProductEventPriceDiscoveredPayloadData),
    PriceDropped(ProductEventPriceChangedPayloadData),
    PriceIncreased(ProductEventPriceChangedPayloadData),
    PriceRemoved(ProductEventPriceRemovedPayloadData),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventStateChangedPayloadData {
    pub old_state: ProductStateData,
    pub new_state: ProductStateData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventPriceDiscoveredPayloadData {
    pub new_price: PriceData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventPriceChangedPayloadData {
    pub old_price: PriceData,
    pub new_price: PriceData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventPriceRemovedPayloadData {
    pub old_price: PriceData,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductCreatedEventPayloadData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    pub state: ProductStateData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProductEventData {
    pub event_type: ProductEventTypeData,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub payload: ProductEventPayloadData,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl From<Event<ProductId, LocalizedProductDomainEventPayloadView>> for GetProductEventData {
    fn from(event: Event<ProductId, LocalizedProductDomainEventPayloadView>) -> Self {
        let (event_type, shop_id, shops_product_id, payload) = match event.payload {
            LocalizedProductDomainEventPayloadView::Created(payload) => (
                ProductEventTypeData::Created,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::Created(ProductCreatedEventPayloadData {
                    price: payload.price.map(PriceData::from),
                    state: payload.state.into(),
                }),
            ),
            LocalizedProductDomainEventPayloadView::StateListed(payload) => (
                ProductEventTypeData::StateListed,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::StateListed(ProductEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Listed,
                }),
            ),
            LocalizedProductDomainEventPayloadView::StateAvailable(payload) => (
                ProductEventTypeData::StateAvailable,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::StateAvailable(ProductEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Available,
                }),
            ),
            LocalizedProductDomainEventPayloadView::StateReserved(payload) => (
                ProductEventTypeData::StateReserved,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::StateReserved(ProductEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Reserved,
                }),
            ),
            LocalizedProductDomainEventPayloadView::StateSold(payload) => (
                ProductEventTypeData::StateSold,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::StateSold(ProductEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Sold,
                }),
            ),
            LocalizedProductDomainEventPayloadView::StateRemoved(payload) => (
                ProductEventTypeData::StateRemoved,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::StateRemoved(ProductEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Removed,
                }),
            ),
            LocalizedProductDomainEventPayloadView::StateUnknown(payload) => (
                ProductEventTypeData::StateUnknown,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::StateUnknown(ProductEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: ProductStateData::Unknown,
                }),
            ),
            LocalizedProductDomainEventPayloadView::PriceDiscovered(payload) => (
                ProductEventTypeData::PriceDiscovered,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::PriceDiscovered(ProductEventPriceDiscoveredPayloadData {
                    new_price: payload.price.into(),
                }),
            ),
            LocalizedProductDomainEventPayloadView::PriceDropped(payload) => (
                ProductEventTypeData::PriceDropped,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::PriceDropped(ProductEventPriceChangedPayloadData {
                    old_price: payload.old_price.into(),
                    new_price: payload.new_price.into(),
                }),
            ),
            LocalizedProductDomainEventPayloadView::PriceIncreased(payload) => (
                ProductEventTypeData::PriceIncreased,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::PriceIncreased(ProductEventPriceChangedPayloadData {
                    old_price: payload.old_price.into(),
                    new_price: payload.new_price.into(),
                }),
            ),
            LocalizedProductDomainEventPayloadView::PriceRemoved(payload) => (
                ProductEventTypeData::PriceRemoved,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::PriceRemoved(ProductEventPriceRemovedPayloadData {
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
    use crate::core::product_event::domain::{
        LocalizedProductCreatedDomainEventPayloadView, LocalizedProductDomainEventPayloadView,
        LocalizedProductPriceChangeDomainEventPayloadView,
        LocalizedProductPriceDiscoveryDomainEventPayloadView,
        LocalizedProductStateChangeDomainEventPayloadView,
    };
    use crate::data::{
        get_product_event_data::{
            GetProductEventData, ProductCreatedEventPayloadData, ProductEventPayloadData,
            ProductEventPriceChangedPayloadData, ProductEventPriceDiscoveredPayloadData,
            ProductEventStateChangedPayloadData, ProductEventTypeData,
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
    use fake::Fake;
    use rstest;
    use time::macros::utc_datetime;
    use url::Url;
    use uuid::Uuid;

    #[rstest::rstest]
    #[case::created(
        LocalizedProductDomainEventPayloadView::Created(LocalizedProductCreatedDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            shop_name: "baz".into(),
            title: Localized::new(common::language::domain::Language::De, "boop".into()),
            shop_type: fake::Faker.fake(),
            description: None,
            price: Some(Price::new(500u64.into(), Currency::Eur)),
            state: ProductState::Listed,
            url: Url::parse("https://foo.bar/boop").unwrap(),
            images: vec![],
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::Created,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::Created(ProductCreatedEventPayloadData { price: Some(PriceData::new(CurrencyData::Eur, 500u64)), state: ProductStateData::Listed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_listed(
        LocalizedProductDomainEventPayloadView::StateListed(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Available
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateListed,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateListed(ProductEventStateChangedPayloadData { old_state: ProductStateData::Available, new_state: ProductStateData::Listed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_available(
        LocalizedProductDomainEventPayloadView::StateAvailable(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Listed
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateAvailable,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateAvailable(ProductEventStateChangedPayloadData { old_state: ProductStateData::Listed, new_state: ProductStateData::Available }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_reserved(
        LocalizedProductDomainEventPayloadView::StateReserved(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Available
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateReserved,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateReserved(ProductEventStateChangedPayloadData { old_state: ProductStateData::Available, new_state: ProductStateData::Reserved }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_sold(
        LocalizedProductDomainEventPayloadView::StateSold(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Reserved
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateSold,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateSold(ProductEventStateChangedPayloadData { old_state: ProductStateData::Reserved, new_state: ProductStateData::Sold }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_removed(
        LocalizedProductDomainEventPayloadView::StateRemoved(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Sold
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateRemoved,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateRemoved(ProductEventStateChangedPayloadData { old_state: ProductStateData::Sold, new_state: ProductStateData::Removed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_unknown(
        LocalizedProductDomainEventPayloadView::StateUnknown(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Removed
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateUnknown,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateUnknown(ProductEventStateChangedPayloadData { old_state: ProductStateData::Removed, new_state: ProductStateData::Unknown }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_discovered(
        LocalizedProductDomainEventPayloadView::PriceDiscovered(LocalizedProductPriceDiscoveryDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            price: Price::new(500u64.into(), Currency::Eur),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::PriceDiscovered,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::PriceDiscovered(ProductEventPriceDiscoveredPayloadData {
                new_price: PriceData::new(CurrencyData::Eur, 500u64)
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_dropped(
        LocalizedProductDomainEventPayloadView::PriceDropped(LocalizedProductPriceChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            new_price: Price::new(500u64.into(), Currency::Eur),
            old_price: Price::new(700u64.into(), Currency::Eur),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::PriceDropped,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::PriceDropped(ProductEventPriceChangedPayloadData { old_price: PriceData::new(CurrencyData::Eur, 700u64), new_price: PriceData::new(CurrencyData::Eur, 500u64) }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_increased(
        LocalizedProductDomainEventPayloadView::PriceIncreased(LocalizedProductPriceChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            new_price: Price::new(777u64.into(), Currency::Eur),
            old_price: Price::new(500u64.into(), Currency::Eur),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::PriceIncreased,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::PriceIncreased(ProductEventPriceChangedPayloadData { old_price: PriceData::new(CurrencyData::Eur, 500u64), new_price: PriceData::new(CurrencyData::Eur, 777u64) }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[trace]
    fn should_from_event_localized_product_event_payload_for_get_product_event_data(
        #[case] payload_view: LocalizedProductDomainEventPayloadView,
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
