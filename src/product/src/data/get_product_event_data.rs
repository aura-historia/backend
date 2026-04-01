use crate::core::product_event::domain::LocalizedProductDomainEventPayloadView;
use crate::data::product_state_data::ProductStateData;
use crate::data::{
    authenticity_data, condition_data, origin_year_data, product_image_data, provenance_data,
    restoration_data,
};
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
    EstimatePriceChanged,
    UrlChanged,
    ImagesChanged,
    AuctionTimeChanged,
    OriginYearChanged,
    AuthenticityChanged,
    ConditionChanged,
    ProvenanceChanged,
    RestorationChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProductEventPayloadData {
    Created(ProductCreatedEventPayloadData),
    StateChanged(ProductEventStateChangedPayloadData),
    PriceChanged(ProductEventPriceChangedPayloadData),
    EstimatePriceChanged(ProductEventEstimatePriceChangedPayloadData),
    UrlChanged(ProductEventUrlChangedPayloadData),
    ImagesChanged(ProductEventImagesChangedPayloadData),
    AuctionTimeChanged(ProductEventAuctionTimeChangedPayloadData),
    OriginYearChanged(ProductEventOriginYearChangedPayloadData),
    AuthenticityChanged(ProductEventAuthenticityChangedPayloadData),
    ConditionChanged(ProductEventConditionChangedPayloadData),
    ProvenanceChanged(ProductEventProvenanceChangedPayloadData),
    RestorationChanged(ProductEventRestorationChangedPayloadData),
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
pub struct ProductEventEstimatePriceChangedPayloadData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventUrlChangedPayloadData {
    pub url: url::Url,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventImagesChangedPayloadData {
    pub images: Vec<product_image_data::ProductImageData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventAuctionTimeChangedPayloadData {
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_end: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventOriginYearChangedPayloadData {
    pub origin_year: origin_year_data::OriginYearData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventAuthenticityChangedPayloadData {
    pub authenticity: authenticity_data::AuthenticityData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventConditionChangedPayloadData {
    pub condition: condition_data::ConditionData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventProvenanceChangedPayloadData {
    pub provenance: provenance_data::ProvenanceData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductEventRestorationChangedPayloadData {
    pub restoration: restoration_data::RestorationData,
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
            LocalizedProductDomainEventPayloadView::EstimatePriceChanged(payload) => (
                ProductEventTypeData::EstimatePriceChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::EstimatePriceChanged(
                    ProductEventEstimatePriceChangedPayloadData {
                        price_estimate_min: payload.price_estimate_min.map(PriceData::from),
                        price_estimate_max: payload.price_estimate_max.map(PriceData::from),
                    },
                ),
            ),
            LocalizedProductDomainEventPayloadView::UrlChanged(payload) => (
                ProductEventTypeData::UrlChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::UrlChanged(ProductEventUrlChangedPayloadData {
                    url: payload.url,
                }),
            ),
            LocalizedProductDomainEventPayloadView::ImagesChanged(payload) => (
                ProductEventTypeData::ImagesChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::ImagesChanged(ProductEventImagesChangedPayloadData {
                    images: payload
                        .images
                        .into_iter()
                        .map(product_image_data::ProductImageData::from)
                        .collect(),
                }),
            ),
            LocalizedProductDomainEventPayloadView::AuctionTimeChanged(payload) => (
                ProductEventTypeData::AuctionTimeChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::AuctionTimeChanged(
                    ProductEventAuctionTimeChangedPayloadData {
                        auction_start: payload.auction_start,
                        auction_end: payload.auction_end,
                    },
                ),
            ),
            LocalizedProductDomainEventPayloadView::OriginYearChanged(payload) => (
                ProductEventTypeData::OriginYearChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::OriginYearChanged(
                    ProductEventOriginYearChangedPayloadData {
                        origin_year: payload.origin_year.into(),
                    },
                ),
            ),
            LocalizedProductDomainEventPayloadView::AuthenticityChanged(payload) => (
                ProductEventTypeData::AuthenticityChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::AuthenticityChanged(
                    ProductEventAuthenticityChangedPayloadData {
                        authenticity: payload.authenticity.into(),
                    },
                ),
            ),
            LocalizedProductDomainEventPayloadView::ConditionChanged(payload) => (
                ProductEventTypeData::ConditionChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::ConditionChanged(
                    ProductEventConditionChangedPayloadData {
                        condition: payload.condition.into(),
                    },
                ),
            ),
            LocalizedProductDomainEventPayloadView::ProvenanceChanged(payload) => (
                ProductEventTypeData::ProvenanceChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::ProvenanceChanged(
                    ProductEventProvenanceChangedPayloadData {
                        provenance: payload.provenance.into(),
                    },
                ),
            ),
            LocalizedProductDomainEventPayloadView::RestorationChanged(payload) => (
                ProductEventTypeData::RestorationChanged,
                payload.shop_id,
                payload.shops_product_id,
                ProductEventPayloadData::RestorationChanged(
                    ProductEventRestorationChangedPayloadData {
                        restoration: payload.restoration.into(),
                    },
                ),
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
    use crate::core::authenticity::Authenticity;
    use crate::core::condition::Condition;
    use crate::core::origin_year::OriginYear;
    use crate::core::product_event::domain::{
        LocalizedProductAuctionTimeChangeDomainEventPayloadView,
        LocalizedProductAuthenticityChangeDomainEventPayloadView,
        LocalizedProductConditionChangeDomainEventPayloadView,
        LocalizedProductCreatedDomainEventPayloadView, LocalizedProductDomainEventPayloadView,
        LocalizedProductEstimatePriceChangeDomainEventPayloadView,
        LocalizedProductImagesChangeDomainEventPayloadView,
        LocalizedProductOriginYearChangeDomainEventPayloadView,
        LocalizedProductPriceChangeDomainEventPayloadView,
        LocalizedProductProvenanceChangeDomainEventPayloadView,
        LocalizedProductRestorationChangeDomainEventPayloadView,
        LocalizedProductStateChangeDomainEventPayloadView,
        LocalizedProductUrlChangeDomainEventPayloadView,
    };
    use crate::core::provenance::Provenance;
    use crate::core::restoration::Restoration;
    use crate::data::{
        authenticity_data::AuthenticityData,
        condition_data::ConditionData,
        get_product_event_data::{
            GetProductEventData, ProductCreatedEventPayloadData,
            ProductEventAuctionTimeChangedPayloadData, ProductEventAuthenticityChangedPayloadData,
            ProductEventConditionChangedPayloadData, ProductEventEstimatePriceChangedPayloadData,
            ProductEventImagesChangedPayloadData, ProductEventOriginYearChangedPayloadData,
            ProductEventPayloadData, ProductEventPriceChangedPayloadData,
            ProductEventProvenanceChangedPayloadData, ProductEventRestorationChangedPayloadData,
            ProductEventStateChangedPayloadData, ProductEventTypeData,
            ProductEventUrlChangedPayloadData,
        },
        origin_year_data::OriginYearData,
        product_state_data::ProductStateData,
        provenance_data::ProvenanceData,
        restoration_data::RestorationData,
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
    use std::collections::HashMap;
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
    #[case::estimate_price_changed(
        LocalizedProductDomainEventPayloadView::EstimatePriceChanged(LocalizedProductEstimatePriceChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            price_estimate_min: Some(Price::new(100u64.into(), Currency::Eur)),
            price_estimate_max: Some(Price::new(500u64.into(), Currency::Eur)),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::EstimatePriceChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::EstimatePriceChanged(ProductEventEstimatePriceChangedPayloadData {
                price_estimate_min: Some(PriceData::new(CurrencyData::Eur, 100u64)),
                price_estimate_max: Some(PriceData::new(CurrencyData::Eur, 500u64)),
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::url_changed(
        LocalizedProductDomainEventPayloadView::UrlChanged(LocalizedProductUrlChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            url: Url::parse("https://foo.bar/new-url").unwrap(),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::UrlChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::UrlChanged(ProductEventUrlChangedPayloadData {
                url: Url::parse("https://foo.bar/new-url").unwrap(),
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::images_changed(
        LocalizedProductDomainEventPayloadView::ImagesChanged(LocalizedProductImagesChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            images: vec![],
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::ImagesChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::ImagesChanged(ProductEventImagesChangedPayloadData {
                images: vec![],
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::auction_time_changed(
        LocalizedProductDomainEventPayloadView::AuctionTimeChanged(LocalizedProductAuctionTimeChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            auction_start: None,
            auction_end: None,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::AuctionTimeChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::AuctionTimeChanged(ProductEventAuctionTimeChangedPayloadData {
                auction_start: None,
                auction_end: None,
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::origin_year_changed(
        LocalizedProductDomainEventPayloadView::OriginYearChanged(LocalizedProductOriginYearChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            origin_year: OriginYear::ExactYear(common::year::Year::from(1900i32)),
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::OriginYearChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::OriginYearChanged(ProductEventOriginYearChangedPayloadData {
                origin_year: OriginYearData { min: None, year: Some(common::year::Year::from(1900i32)), max: None },
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::authenticity_changed(
        LocalizedProductDomainEventPayloadView::AuthenticityChanged(LocalizedProductAuthenticityChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            authenticity: Authenticity::Original,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::AuthenticityChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::AuthenticityChanged(ProductEventAuthenticityChangedPayloadData {
                authenticity: AuthenticityData::Original,
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::condition_changed(
        LocalizedProductDomainEventPayloadView::ConditionChanged(LocalizedProductConditionChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            condition: Condition::Excellent,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::ConditionChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::ConditionChanged(ProductEventConditionChangedPayloadData {
                condition: ConditionData::Excellent,
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::provenance_changed(
        LocalizedProductDomainEventPayloadView::ProvenanceChanged(LocalizedProductProvenanceChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            provenance: Provenance::Complete,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::ProvenanceChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::ProvenanceChanged(ProductEventProvenanceChangedPayloadData {
                provenance: ProvenanceData::Complete,
            }),
            timestamp: utc_datetime!(2025 - 05 - 05 2:22).into(),
        }
    )]
    #[case::restoration_changed(
        LocalizedProductDomainEventPayloadView::RestorationChanged(LocalizedProductRestorationChangeDomainEventPayloadView {
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            restoration: Restoration::Minor,
        }),
        GetProductEventData {
            event_type: ProductEventTypeData::RestorationChanged,
            product_id: Uuid::max().into(),
            event_id: Uuid::max().into(),
            shop_id: "569c809e-b9e0-48c0-8c52-ac37d82a0959".try_into().unwrap(),
            shops_product_id: "bar".into(),
            payload: ProductEventPayloadData::RestorationChanged(ProductEventRestorationChangedPayloadData {
                restoration: RestorationData::Minor,
            }),
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
