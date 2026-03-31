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
    StateChanged,
    PriceChanged,
    DetailChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProductEventPayloadData {
    Created(ProductCreatedEventPayloadData),
    StateChanged(ProductEventStateChangedPayloadData),
    PriceChanged(ProductEventPriceChangedPayloadData),
    DetailChanged(ProductEventDetailChangedPayloadData),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventStateChangedPayloadData {
    pub old_state: ProductStateData,
    pub new_state: ProductStateData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventPriceChangedPayloadData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price: Option<PriceData>,
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
pub struct ProductEventDetailChangedPayloadData {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
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
            LocalizedProductDomainEventPayloadView::StateChanged(payload) => (
                ProductEventTypeData::StateChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::StateChanged(ProductEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: payload.new_state.into(),
                }),
            ),
            LocalizedProductDomainEventPayloadView::PriceChanged(payload) => (
                ProductEventTypeData::PriceChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::PriceChanged(ProductEventPriceChangedPayloadData {
                    old_price: payload.old_price.map(PriceData::from),
                    new_price: payload.new_price.map(PriceData::from),
                }),
            ),
            LocalizedProductDomainEventPayloadView::DetailChanged(payload) => {
                let shop_id = payload.shop_id;
                let shops_product_id = payload.shops_product_id;
                (
                    ProductEventTypeData::DetailChanged,
                    shop_id,
                    shops_product_id.clone(),
                    ProductEventPayloadData::DetailChanged(ProductEventDetailChangedPayloadData {
                        shop_id,
                        shops_product_id,
                    }),
                )
            }
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
        LocalizedProductStateChangeDomainEventPayloadView,
    };
    use crate::data::{
        get_product_event_data::{
            GetProductEventData, ProductCreatedEventPayloadData, ProductEventPayloadData,
            ProductEventPriceChangedPayloadData, ProductEventStateChangedPayloadData,
            ProductEventTypeData,
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
    #[case::state_changed_to_listed(
        LocalizedProductDomainEventPayloadView::StateChanged(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Available,
            new_state: ProductState::Listed,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateChanged(ProductEventStateChangedPayloadData { old_state: ProductStateData::Available, new_state: ProductStateData::Listed }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_changed_to_available(
        LocalizedProductDomainEventPayloadView::StateChanged(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Listed,
            new_state: ProductState::Available,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateChanged(ProductEventStateChangedPayloadData { old_state: ProductStateData::Listed, new_state: ProductStateData::Available }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::state_changed_to_sold(
        LocalizedProductDomainEventPayloadView::StateChanged(LocalizedProductStateChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_state: ProductState::Reserved,
            new_state: ProductState::Sold,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::StateChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::StateChanged(ProductEventStateChangedPayloadData { old_state: ProductStateData::Reserved, new_state: ProductStateData::Sold }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_discovered(
        LocalizedProductDomainEventPayloadView::PriceChanged(LocalizedProductPriceChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_price: None,
            new_price: Some(Price::new(500u64.into(), Currency::Eur)),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::PriceChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::PriceChanged(ProductEventPriceChangedPayloadData {
                old_price: None,
                new_price: Some(PriceData::new(CurrencyData::Eur, 500u64)),
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_dropped(
        LocalizedProductDomainEventPayloadView::PriceChanged(LocalizedProductPriceChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_price: Some(Price::new(700u64.into(), Currency::Eur)),
            new_price: Some(Price::new(500u64.into(), Currency::Eur)),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::PriceChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::PriceChanged(ProductEventPriceChangedPayloadData { old_price: Some(PriceData::new(CurrencyData::Eur, 700u64)), new_price: Some(PriceData::new(CurrencyData::Eur, 500u64)) }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_increased(
        LocalizedProductDomainEventPayloadView::PriceChanged(LocalizedProductPriceChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_price: Some(Price::new(500u64.into(), Currency::Eur)),
            new_price: Some(Price::new(777u64.into(), Currency::Eur)),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::PriceChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::PriceChanged(ProductEventPriceChangedPayloadData { old_price: Some(PriceData::new(CurrencyData::Eur, 500u64)), new_price: Some(PriceData::new(CurrencyData::Eur, 777u64)) }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::price_removed(
        LocalizedProductDomainEventPayloadView::PriceChanged(LocalizedProductPriceChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            old_price: Some(Price::new(500u64.into(), Currency::Eur)),
            new_price: None,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::PriceChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::PriceChanged(ProductEventPriceChangedPayloadData { old_price: Some(PriceData::new(CurrencyData::Eur, 500u64)), new_price: None }),
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
