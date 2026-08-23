use crate::products::product_data::ProductImageData;
use crate::values::{LocalizedTextData, PriceData};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use product_core::product::{ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation};
use product_core::product_id::ProductId;
use product_core::product_image::ProductImage;
use product_core::product_lifecycle::ProductLifecycle;
use product_core::product_state::ProductState;
use product_service::use_cases::{ProductEvent, ProductEventPayload, ProductEventType};
use serde::Serialize;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductEventData {
    event_type: ProductEventTypeData,
    product_id: ProductId,
    event_id: EventId,
    payload: ProductEventPayloadData,
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductEventTypeData {
    Created,
    StateChanged,
    AddressChanged,
    PriceChanged,
    UrlChanged,
    ImagesChanged,
    AuctionChanged,
    Deleted,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
enum ProductEventPayloadData {
    Created(ProductCreatedHistoryPayloadData),
    StateChanged(ProductStateChangedHistoryPayloadData),
    AddressChanged(ProductAddressChangedHistoryPayloadData),
    PriceChanged(ProductPriceChangedHistoryPayloadData),
    UrlChanged(ProductUrlChangedHistoryPayloadData),
    ImagesChanged(ProductImagesChangedHistoryPayloadData),
    AuctionChanged(ProductAuctionChangedHistoryPayloadData),
    Deleted(ProductDeletedHistoryPayloadData),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductCreatedHistoryPayloadData {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_address: Option<GeoAddressData>,
    pricing: ProductPricingData,
    #[serde(skip_serializing_if = "Option::is_none")]
    sale_valuation: Option<ProductSaleValuationData>,
    state: ProductStateData,
    url: Url,
    images: Vec<ProductImageData>,
    auction: ProductAuctionData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductStateChangedHistoryPayloadData {
    old_state: ProductStateData,
    new_state: ProductStateData,
    #[serde(skip_serializing_if = "Option::is_none")]
    sale_valuation: Option<ProductSaleValuationData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductAddressChangedHistoryPayloadData {
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_address: Option<GeoAddressData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductPriceChangedHistoryPayloadData {
    old_pricing: ProductPricingData,
    new_pricing: ProductPricingData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductUrlChangedHistoryPayloadData {
    old_url: Url,
    new_url: Url,
}

#[derive(Debug, Serialize)]
struct ProductImagesChangedHistoryPayloadData {
    images: Vec<ProductImageData>,
}

#[derive(Debug, Serialize)]
struct ProductAuctionChangedHistoryPayloadData {
    auction: ProductAuctionData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductDeletedHistoryPayloadData {
    old_lifecycle: ProductLifecycleData,
    new_lifecycle: ProductLifecycleData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductPricingData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<PriceData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductSaleValuationData {
    #[serde(with = "time::serde::rfc3339")]
    sold_at: OffsetDateTime,
    fx_rate_id: FxRateId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductAuctionData {
    #[serde(with = "time::serde::rfc3339::option")]
    start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    end: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductStateData {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductLifecycleData {
    Active,
    Deleted,
}

impl From<ProductEvent> for ProductEventData {
    fn from(event: ProductEvent) -> Self {
        Self {
            event_type: event.event_type.into(),
            product_id: event.product_id,
            event_id: event.event_id,
            payload: event.payload.into(),
            timestamp: event.timestamp,
        }
    }
}

impl From<ProductEventType> for ProductEventTypeData {
    fn from(value: ProductEventType) -> Self {
        match value {
            ProductEventType::Created => Self::Created,
            ProductEventType::StateChanged => Self::StateChanged,
            ProductEventType::AddressChanged => Self::AddressChanged,
            ProductEventType::PriceChanged => Self::PriceChanged,
            ProductEventType::UrlChanged => Self::UrlChanged,
            ProductEventType::ImagesChanged => Self::ImagesChanged,
            ProductEventType::AuctionChanged => Self::AuctionChanged,
            ProductEventType::Deleted => Self::Deleted,
        }
    }
}

impl From<ProductEventPayload> for ProductEventPayloadData {
    fn from(value: ProductEventPayload) -> Self {
        match value {
            ProductEventPayload::Created(value) => {
                Self::Created(ProductCreatedHistoryPayloadData {
                    title: value.title.map(Into::into),
                    description: value.description.map(Into::into),
                    structured_address: value.address.structured.map(Into::into),
                    geo_address: value.address.geo.map(Into::into),
                    pricing: value.pricing.into(),
                    sale_valuation: value.sale_valuation.map(Into::into),
                    state: value.state.into(),
                    url: value.url,
                    images: images(value.images),
                    auction: value.auction.into(),
                })
            }
            ProductEventPayload::StateChanged(value) => {
                Self::StateChanged(ProductStateChangedHistoryPayloadData {
                    old_state: value.old_state.into(),
                    new_state: value.new_state.into(),
                    sale_valuation: value.sale_valuation.map(Into::into),
                })
            }
            ProductEventPayload::AddressChanged(value) => {
                Self::AddressChanged(ProductAddressChangedHistoryPayloadData::from(value.address))
            }
            ProductEventPayload::PriceChanged(value) => {
                Self::PriceChanged(ProductPriceChangedHistoryPayloadData {
                    old_pricing: value.old_pricing.into(),
                    new_pricing: value.new_pricing.into(),
                })
            }
            ProductEventPayload::UrlChanged(value) => {
                Self::UrlChanged(ProductUrlChangedHistoryPayloadData {
                    old_url: value.old_url,
                    new_url: value.new_url,
                })
            }
            ProductEventPayload::ImagesChanged(value) => {
                Self::ImagesChanged(ProductImagesChangedHistoryPayloadData {
                    images: images(value.images),
                })
            }
            ProductEventPayload::AuctionChanged(value) => {
                Self::AuctionChanged(ProductAuctionChangedHistoryPayloadData {
                    auction: value.auction.into(),
                })
            }
            ProductEventPayload::Deleted(value) => {
                Self::Deleted(ProductDeletedHistoryPayloadData {
                    old_lifecycle: value.old_lifecycle.into(),
                    new_lifecycle: value.new_lifecycle.into(),
                })
            }
        }
    }
}

impl From<ProductAddress> for ProductAddressChangedHistoryPayloadData {
    fn from(address: ProductAddress) -> Self {
        Self {
            structured_address: address.structured.map(Into::into),
            geo_address: address.geo.map(Into::into),
        }
    }
}

impl From<ProductPricing> for ProductPricingData {
    fn from(pricing: ProductPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<ProductSaleValuation> for ProductSaleValuationData {
    fn from(valuation: ProductSaleValuation) -> Self {
        Self {
            sold_at: valuation.sold_at,
            fx_rate_id: valuation.fx_rate_id,
        }
    }
}

impl From<ProductAuction> for ProductAuctionData {
    fn from(auction: ProductAuction) -> Self {
        Self {
            start: auction.start,
            end: auction.end,
        }
    }
}

impl From<ProductState> for ProductStateData {
    fn from(state: ProductState) -> Self {
        match state {
            ProductState::Listed => Self::Listed,
            ProductState::Available => Self::Available,
            ProductState::Reserved => Self::Reserved,
            ProductState::Sold => Self::Sold,
            ProductState::Removed => Self::Removed,
            ProductState::Unknown => Self::Unknown,
        }
    }
}

impl From<ProductLifecycle> for ProductLifecycleData {
    fn from(lifecycle: ProductLifecycle) -> Self {
        match lifecycle {
            ProductLifecycle::Active => Self::Active,
            ProductLifecycle::Deleted => Self::Deleted,
        }
    }
}

fn images(images: impl IntoIterator<Item = ProductImage>) -> Vec<ProductImageData> {
    images.into_iter().map(ProductImageData::from).collect()
}
