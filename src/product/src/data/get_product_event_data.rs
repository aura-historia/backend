use crate::core::product_event::domain::LocalizedProductDomainEventPayloadView;
use crate::data::product_image_data;
use crate::data::product_state_data::ProductStateData;
use common::{
    event::Event, event_id::EventId, price::data::PriceData, product_id::ProductId,
    shop_id::ShopId, shops_product_id::ShopsProductId,
};
use indexmap::IndexSet;
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
    pub images: IndexSet<product_image_data::ProductImageData>,
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
pub struct GetProductEventData {
    pub event_type: ProductEventTypeData,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub payload: ProductEventPayloadData,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl From<Event<ProductId, LocalizedProductDomainEventPayloadView>> for GetProductEventData {
    fn from(event: Event<ProductId, LocalizedProductDomainEventPayloadView>) -> Self {
        let (event_type, shop_id, seller_id, shops_product_id, payload) = match event.payload {
            LocalizedProductDomainEventPayloadView::Created(payload) => (
                ProductEventTypeData::Created,
                payload.shop_id,
                payload.seller_id,
                payload.shops_product_id,
                ProductEventPayloadData::Created(ProductCreatedEventPayloadData {
                    price: payload.price.map(PriceData::from),
                    state: payload.state.into(),
                }),
            ),
            LocalizedProductDomainEventPayloadView::StateChanged(payload) => (
                ProductEventTypeData::StateChanged,
                payload.shop_id,
                payload.seller_id,
                payload.shops_product_id,
                ProductEventPayloadData::StateChanged(ProductEventStateChangedPayloadData {
                    old_state: payload.old_state.into(),
                    new_state: payload.new_state.into(),
                }),
            ),
            LocalizedProductDomainEventPayloadView::PriceChanged(payload) => (
                ProductEventTypeData::PriceChanged,
                payload.shop_id,
                payload.seller_id,
                payload.shops_product_id,
                ProductEventPayloadData::PriceChanged(ProductEventPriceChangedPayloadData {
                    old_price: payload.old_price.map(PriceData::from),
                    new_price: payload.new_price.map(PriceData::from),
                }),
            ),
            LocalizedProductDomainEventPayloadView::EstimatePriceChanged(payload) => (
                ProductEventTypeData::EstimatePriceChanged,
                payload.shop_id,
                payload.seller_id,
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
                payload.seller_id,
                payload.shops_product_id,
                ProductEventPayloadData::UrlChanged(ProductEventUrlChangedPayloadData {
                    url: payload.url,
                }),
            ),
            LocalizedProductDomainEventPayloadView::ImagesChanged(payload) => (
                ProductEventTypeData::ImagesChanged,
                payload.shop_id,
                payload.seller_id,
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
                payload.seller_id,
                payload.shops_product_id,
                ProductEventPayloadData::AuctionTimeChanged(
                    ProductEventAuctionTimeChangedPayloadData {
                        auction_start: payload.auction_start,
                        auction_end: payload.auction_end,
                    },
                ),
            ),
        };

        GetProductEventData {
            event_type,
            product_id: event.aggregate_id,
            event_id: event.event_id,
            shop_id,
            seller_id,
            shops_product_id,
            payload,
            timestamp: event.timestamp,
        }
    }
}
